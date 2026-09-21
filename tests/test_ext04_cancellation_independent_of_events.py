"""EXT-01 finding #4 regression: a downstream ``session/cancel`` must be
sent the moment the host cancels, never only as a side effect of the next
engine event arriving.

Before this fix, ``EngineSessionManagerAdapter.prompt()`` only checked
``cancel_event.is_set()`` (and only then called ``manager.cancel()``)
*inside* the ``for event in manager.prompt_turn(...)`` loop -- i.e. only
when that generator actually yielded another event. An engine that accepts
the prompt and then goes silent (transport still open, no further event
ever produced) parked that generator forever, so a host Cancel while parked
there set ``cancel_event`` but never actually sent anything downstream.

This test drives ``EngineSessionManagerAdapter.prompt()`` directly against
a fake ``EngineSessionManager``-shaped ``manager`` whose ``prompt_turn()``
yields nothing until released, proving the downstream ``cancel()`` call
happens with *zero* events ever observed.
"""
from __future__ import annotations

import threading
import time

from cortex_runtime.delegation.live_adapters import EngineSessionManagerAdapter

_ASSERT_TIMEOUT = 2.0
_ASSERT_POLL = 0.02


def _wait_until(predicate, timeout=_ASSERT_TIMEOUT):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if predicate():
            return True
        time.sleep(_ASSERT_POLL)
    return predicate()


class _SilentManager:
    """A fake ``EngineSessionManager``: accepts the prompt, then yields
    nothing at all until the test explicitly releases it -- the fake-engine
    scenario the finding names ("accepts the prompt and then goes silent
    with the transport still open")."""

    def __init__(self):
        self.cancel_calls: list[str] = []
        self._release_gate = threading.Event()

    def prompt_turn(self, session_ref, message, *, prompt_blocks=None):
        self._release_gate.wait()
        return
        yield  # pragma: no cover -- unreachable; makes this a generator function

    def cancel(self, session_ref):
        self.cancel_calls.append(session_ref)

    def release(self):
        self._release_gate.set()


def _make_adapter(manager) -> EngineSessionManagerAdapter:
    adapter = EngineSessionManagerAdapter.__new__(EngineSessionManagerAdapter)
    adapter._lock = threading.Lock()
    adapter._store = None
    adapter._session_to_attempt = {"sess_1": "attempt_1"}
    adapter._attempt_task = {"attempt_1": "task_1"}
    adapter._managers = {"attempt_1": manager}
    return adapter


def test_cancel_is_sent_downstream_without_any_further_engine_event():
    manager = _SilentManager()
    adapter = _make_adapter(manager)
    cancel_event = threading.Event()

    result: dict = {}

    def _run():
        result["outcome"] = adapter.prompt("sess_1", "do the thing", cancel_event)

    prompt_thread = threading.Thread(target=_run, daemon=True)
    prompt_thread.start()
    try:
        # Give prompt() a moment to actually enter manager.prompt_turn()
        # and park there -- the fake manager never yields anything, so
        # there is no event for the pre-fix per-event check to ever run
        # against.
        time.sleep(0.05)
        assert manager.cancel_calls == [], "sanity: no cancel should be sent before cancel_event is ever set"

        cancel_event.set()

        # THE POINT OF THIS TEST: the downstream cancel must arrive
        # promptly even though the fake engine never emits another event.
        assert _wait_until(lambda: manager.cancel_calls == ["sess_1"]), (
            f"downstream session/cancel was never sent independently of the event "
            f"stream (cancel_calls={manager.cancel_calls!r})"
        )
    finally:
        manager.release()
        prompt_thread.join(timeout=_ASSERT_TIMEOUT)
    assert not prompt_thread.is_alive()
    assert "outcome" in result


def test_capacity_is_not_released_on_an_unconfirmed_stop_only_on_a_confirmed_one(tmp_path):
    """EXT-01 #4: this product distinguishes a *confirmed* process exit
    (``delegation.capacity.release_confirmed`` -- frees the slot) from an
    *unconfirmed* one (``mark_ambiguous`` -- still counts against the
    limit); a cancelled turn must never let a still-uncertain stop free
    capacity, and must never invent a third state. This drives that exact
    existing distinction (``EngineSessionManagerAdapter.terminate_owned``,
    the one place a cancelled attempt's owned process is actually
    terminated -- see ``OrchestratingRuntime._cancel_assignment``) for both
    outcomes of a process that was asked to stop after a cancel."""
    import uuid

    from cortex_runtime.bootstrap import bootstrap_task
    from cortex_runtime.delegation import capacity as delegation_capacity
    from cortex_runtime.portable.identity import SourceEvent
    from cortex_runtime.store import Store

    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    task_id = bootstrap_task(store, SourceEvent(origin="cli", event_id=str(uuid.uuid4())), "Build.", "owner-1").task_id

    class _FakeProcess:
        def __init__(self, exited: bool):
            self._exited = exited

        def wait_exited(self, timeout):
            return self._exited

    class _FakeManager:
        def __init__(self, exited: bool):
            self._process = _FakeProcess(exited)

        def shutdown(self, grace=2.0):
            pass

    def _terminate_owned_confirmed_or_ambiguous(exited: bool):
        connection_id = f"conn-{uuid.uuid4()}"
        attempt_id = f"attempt-{uuid.uuid4()}"
        reservation_id = delegation_capacity.reserve(store, connection_id, task_id, attempt_id, "worker")
        assert reservation_id is not None

        adapter = EngineSessionManagerAdapter.__new__(EngineSessionManagerAdapter)
        adapter._lock = threading.Lock()
        adapter._store = store
        adapter._managers = {attempt_id: _FakeManager(exited)}
        adapter._sessions = {attempt_id: f"sess-{attempt_id}"}
        adapter._reservations = {attempt_id: reservation_id}
        adapter._session_to_attempt = {f"sess-{attempt_id}": attempt_id}
        adapter._session_meta = {}
        adapter._pending_generation = {}
        adapter._session_configured_revision = {}
        adapter._activity_cond = threading.Condition()

        adapter.terminate_owned(attempt_id)

        with store.connection() as db:
            row = db.execute(
                "SELECT state FROM capacity_reservations WHERE reservation_id=?", (reservation_id,),
            ).fetchone()
        return row["state"]

    # An unconfirmed stop (the owned process's exit was never actually
    # observed) must stay "ambiguous" -- never silently freed.
    assert _terminate_owned_confirmed_or_ambiguous(exited=False) == "ambiguous"

    # A confirmed stop (the owned process's exit *was* observed) is the
    # only case that actually releases the slot.
    assert _terminate_owned_confirmed_or_ambiguous(exited=True) == "released"
