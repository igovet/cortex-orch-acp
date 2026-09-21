"""P1 EXT-01 #7 regression: ``_process_new_events`` must never register a
worker future's completion callback while still holding ``self._lock``.

``concurrent.futures.Future.add_done_callback`` invokes the callback
*immediately, on the calling thread* when the future is already done. The
old code called it from inside ``with self._lock:``, and
``_on_worker_done`` acquires ``self._cond`` (built on that same
non-reentrant ``threading.Lock``) -- so an already-completed future
deadlocked the thread calling ``_process_new_events``.

Offline, fakes only: no real engine, no network, no paid call.
"""
from __future__ import annotations

import threading
import uuid
from concurrent.futures import Future

import pytest

from cortex_runtime.acp.orchestrating_runtime import OrchestratingRuntime
from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.delegation import events as delegation_events
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.store import Store

from orchestration_fakes import FakeConfigSnapshotSource, FakeSessionRuntime, FakeWorkspace


def _task(tmp_path):
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    result = bootstrap_task(store, SourceEvent(origin="cli", event_id=str(uuid.uuid4())), "Build a page.", "owner-1")
    return store, result.task_id


def _make_runtime(store):
    engine = FakeSessionRuntime(store, router=lambda *a, **k: None)
    return OrchestratingRuntime(
        store, engine, FakeWorkspace(), FakeConfigSnapshotSource(),
        allow_unqualified_delivery=True, wait_timeout_seconds=5.0,
    )


class ImmediateExecutor:
    """Test double: runs ``fn`` synchronously and returns an already-done
    ``Future`` -- exactly the "future completes before its callback is
    registered" shape the finding describes, without depending on real
    thread-pool scheduling timing to provoke it."""

    def __init__(self):
        self.submitted = 0

    def submit(self, fn, *args, **kwargs):
        self.submitted += 1
        future = Future()
        try:
            future.set_result(fn(*args, **kwargs))
        except BaseException as exc:  # pragma: no cover - defensive
            future.set_exception(exc)
        return future


def _run_with_timeout(fn, *, timeout=5.0):
    result = {}

    def target():
        try:
            fn()
        except BaseException as exc:  # pragma: no cover - surfaced via result
            result["error"] = exc

    thread = threading.Thread(target=target, daemon=True)
    thread.start()
    thread.join(timeout=timeout)
    return thread, result


def test_already_completed_delegation_future_does_not_hang(tmp_path):
    store, task_id = _task(tmp_path)
    runtime = _make_runtime(store)
    channel, _session_ref = runtime._ensure_coordinator(task_id)

    # Replace the real pool with one that finishes synchronously (future is
    # already done the instant submit() returns), and stub the actual
    # worker-provisioning effect so this test stays fully offline.
    runtime._executor = ImmediateExecutor()
    provisioned = []
    runtime._provision_and_prompt = lambda t, c, payload, cancel_event: provisioned.append(payload)

    from runtime_scenarios import reserve_assignment
    reserve_assignment(store,channel,dict(title="A",brief="do A",role="backend-dev",acceptance="A done",request_key="req1"))

    thread, result = _run_with_timeout(
        lambda: runtime._process_new_events(task_id, channel, threading.Event())
    )

    assert not thread.is_alive(), "deadlocked: _process_new_events never returned"
    assert "error" not in result, result.get("error")
    assert len(provisioned)==1 and provisioned[0]["title"]=="A"
    # The callback already ran (synchronously, since the future was done
    # before add_done_callback was ever called) -- nothing left "running".
    assert not runtime._running.get(task_id)




def test_future_completing_later_still_accounts_correctly(tmp_path):
    """Ordinary path (future resolves on a real worker thread, after
    add_done_callback was registered): _running gains then loses the
    future, and _wait_for_progress's condition variable observes it."""
    store, task_id = _task(tmp_path)
    runtime = _make_runtime(store)
    channel, _session_ref = runtime._ensure_coordinator(task_id)

    release = threading.Event()
    started = threading.Event()

    def slow_provision(t, c, payload, cancel_event):
        started.set()
        release.wait(timeout=5.0)
        return None

    runtime._provision_and_prompt = slow_provision

    from runtime_scenarios import reserve_assignment
    reserve_assignment(store,channel,dict(title="A",brief="do A",role="backend-dev",acceptance="A done",request_key="req1"))

    runtime._process_new_events(task_id, channel, threading.Event())
    assert started.wait(timeout=5.0)
    # Still running: the real worker thread is blocked on `release`.
    assert runtime._running.get(task_id)

    release.set()

    import time as _time

    ok = False
    end = _time.monotonic() + 5.0
    while _time.monotonic() < end:
        if not runtime._running.get(task_id):
            ok = True
            break
        _time.sleep(0.02)
    assert ok, "future never drained from _running after completion"

    runtime._executor.shutdown(wait=True)
