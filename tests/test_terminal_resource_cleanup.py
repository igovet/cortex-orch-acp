"""No-paid regressions for terminal resource ownership, not paid host behavior."""
from __future__ import annotations

import os
import signal
import sys
import threading
import time

import pytest

from cortex_runtime.delegation.live_adapters import _jail_for as real_jail_for
from cortex_runtime.engine.process import EngineProcess
from cortex_runtime.orchestration.state import TaskMachine
from cortex_runtime.portable.engine_contract import StopReason

from engine_fakes import isolated_environ, make_connection_record, new_connection_id
from test_transport_disconnect_recovery import (
    SESSION, _finishing_router, _runtime, _task,
)

pytestmark = pytest.mark.usefixtures("unjailed_gateway_command")


def test_completed_prompt_releases_coordinator_without_manual_stop(tmp_path):
    store, task = _task(tmp_path)
    runtime, engine = _runtime(store, _finishing_router())
    try:
        assert runtime.start_prompt(task, SESSION, "go", lambda _: True, threading.Event()) == StopReason.END_TURN
        assert TaskMachine(store).get_state(task) == "COMPLETED"
        assert not engine._owned
        assert task not in runtime._coordinator_sessions
    finally:
        runtime.shutdown()


def test_unexpected_prompt_exception_stops_owned_work_without_cancelling_task(tmp_path, monkeypatch):
    store, task = _task(tmp_path)
    runtime, engine = _runtime(store, _finishing_router())
    def explode(*args, **kwargs):
        TaskMachine(store).create(task, "go")
        runtime._ensure_coordinator(task)
        raise RuntimeError("private diagnostic text")
    monkeypatch.setattr(runtime, "_run", explode)
    try:
        with pytest.raises(RuntimeError, match="private diagnostic"):
            runtime.start_prompt(task, SESSION, "go", lambda _: True, threading.Event())
        assert not engine._owned
        assert task not in runtime._coordinator_sessions
        assert TaskMachine(store).get_state(task) not in {"COMPLETED", "FAILED", "CANCELLED"}
    finally:
        runtime.shutdown()


def test_waiting_prompt_is_not_terminal_cleanup(tmp_path):
    store, task = _task(tmp_path)
    runtime, engine = _runtime(store, _finishing_router(ask_user=True))
    try:
        runtime.start_prompt(task, SESSION, "go", lambda _: True, threading.Event())
        assert TaskMachine(store).get_state(task) == "WAITING_USER"
        assert engine._owned
    finally:
        runtime.shutdown()


def _active(pid):
    try:
        rest = open(f"/proc/{pid}/stat").read().rsplit(")", 1)[1].split()
        return rest[0] != "Z"
    except FileNotFoundError:
        return False


@pytest.mark.parametrize("parent_exits_first", [False, True])
def test_release_escalates_owned_descendant_even_after_leader_exit(tmp_path, parent_exits_first):
    environ = isolated_environ(tmp_path)
    marker = tmp_path / "child.pid"
    # A child in the launched engine session ignores TERM; its leader either
    # exits naturally first or exits on TERM. No process-name targeting.
    script = tmp_path / "local_engine.py"
    script.write_text(
        "import os,signal,time\n"
        "pid=os.fork()\n"
        "if pid == 0:\n"
        " signal.signal(signal.SIGTERM,signal.SIG_IGN)\n"
        f" open({str(marker) + '.new'!r},'w').write(str(os.getpid()))\n"
        f" os.replace({str(marker) + '.new'!r},{str(marker)!r})\n"
        " while True: time.sleep(.01)\n"
        "else:\n"
        f" {'time.sleep(.15)' if parent_exits_first else 'time.sleep(60)'}\n"
    )
    record = make_connection_record(new_connection_id(), (sys.executable, "-B", str(script)), environ=environ)
    process = EngineProcess(record, str(tmp_path), environ=environ)
    child = None
    try:
        deadline = time.monotonic() + 5
        while not marker.exists() and time.monotonic() < deadline:
            time.sleep(.01)
        assert marker.exists()
        child = int(marker.read_text())
        if parent_exits_first:
            assert process._exited_event.wait(5)
            assert not process.wait_exited(0)  # leader exit is not group exit
        process.release(grace=.2)
        assert process.wait_exited(0)
        assert not _active(child)
        process.release(grace=.2)  # repeated cleanup
    finally:
        process.release(grace=.2)
        if child is not None and _active(child):
            os.kill(child, signal.SIGKILL)  # exact test-owned pid, containment


@pytest.mark.parametrize("outcome", ["COMPLETED", "FAILED", "disconnect", "exception"])
def test_real_acp_cleanup_releases_writer_for_another_task(tmp_path, monkeypatch, outcome):
    from cortex_runtime.acp.orchestrating_runtime import OrchestratingRuntime
    from cortex_runtime.bootstrap import bootstrap_task
    from cortex_runtime.delegation import capacity
    from cortex_runtime.delegation.workspace_isolation import DirectProjectWorkspaceProvisioner
    from cortex_runtime.portable.identity import SourceEvent
    from orchestration_fakes import FakeConfigSnapshotSource
    from test_delegation_live_adapters_open_cleanup import _fake_adapter, _task as _project_task

    store, task = _project_task(tmp_path)
    second = bootstrap_task(store, SourceEvent(origin="test", event_id="second"), "Second", "owner").task_id
    adapter, connection = _fake_adapter(store, tmp_path)
    workspace = DirectProjectWorkspaceProvisioner(store, str(store.project_root))
    adapter._workspace_lock_factory = workspace.writer_lock
    adapter._workspace_lock_timeout_seconds = .1
    # The test concerns real transport/process/lock lifetime, not engine policy.
    monkeypatch.setattr(adapter, "_apply_worker_mode_floor", lambda *a, **kw: None)
    runtime = OrchestratingRuntime(store, adapter, workspace, FakeConfigSnapshotSource(), allow_unqualified_delivery=True)
    adapter.open("shared_project_root", dict(task_id=second, attempt_id="co_unrelated"), "instructions")
    unrelated = adapter._managers["co_unrelated"]._process
    process = None
    def run(*a, **kw):
        nonlocal process
        TaskMachine(store).create(task, "go")
        adapter.open("shared_project_root", dict(task_id=task, attempt_id="xa_writer", assignment_id="fixture"), "instructions")
        process = adapter._managers["xa_writer"]._process
        assert process.alive()
        if outcome == "disconnect":
            runtime.disconnect_session(SESSION, trigger="stdin_eof")
        elif outcome == "exception":
            raise RuntimeError("injected controller error")
        else:
            # Simulate the already-committed runtime decision; completion gates
            # themselves are independently exercised by the existing suite.
            with store.connection() as db:
                db.execute("UPDATE runtime_tasks SET state=? WHERE task_id=?", (outcome, task))
        return StopReason.END_TURN
    monkeypatch.setattr(runtime, "_run", run)
    try:
        if outcome == "exception":
            with pytest.raises(RuntimeError, match="injected controller"):
                runtime.start_prompt(task, SESSION, "go", lambda _: True, threading.Event())
        else:
            runtime.start_prompt(task, SESSION, "go", lambda _: True, threading.Event())
        assert process is not None and process.wait_exited(0)
        assert "xa_writer" not in adapter._workspace_locks
        assert capacity.count_active(store, connection) == 1
        assert unrelated.alive()  # another task's owned coordinator is untouched
        assert TaskMachine(store).get_state(task) == (outcome if outcome in {"COMPLETED", "FAILED"} else "PLANNING")
        runtime._cleanup_task_resources(task)  # idempotent, no relabeling
        # A different task acquires the actual project advisory lock and opens
        # a real ACP engine, without /cortex-stop in the first chat.
        adapter.open("shared_project_root", dict(task_id=second, attempt_id="xa_second", assignment_id="second"), "instructions")
        assert adapter._managers["xa_second"]._process.alive()
    finally:
        runtime.shutdown()


def test_failed_open_does_not_unlock_before_shutdown_or_on_unknown_exit(tmp_path, monkeypatch):
    from cortex_runtime.contracts import StoreError
    from cortex_runtime.delegation.workspace_isolation import DirectProjectWorkspaceProvisioner
    from cortex_runtime.engine.client import EngineClient
    from cortex_runtime.engine.session_manager import EngineSessionManager
    from test_delegation_live_adapters_open_cleanup import _fake_adapter, _task as _project_task

    store, task = _project_task(tmp_path)
    adapter, _ = _fake_adapter(store, tmp_path)
    workspace = DirectProjectWorkspaceProvisioner(store, str(store.project_root))
    adapter._workspace_lock_factory = workspace.writer_lock
    adapter._workspace_lock_timeout_seconds = .1
    original_call = EngineClient.call
    original_shutdown = EngineSessionManager.shutdown
    manager = None
    def call(self, method, *args, **kwargs):
        if method == "initialize":
            raise RuntimeError("injected initialize")
        return original_call(self, method, *args, **kwargs)
    def shutdown(self, **kwargs):
        nonlocal manager
        manager = self
        # During teardown it is still genuinely alive and writer-isolated.
        assert self._process.alive()
        with pytest.raises(StoreError, match="workspace_writer_busy"):
            with workspace.writer_lock(timeout=.02):
                pass
        raise RuntimeError("shutdown unavailable")
    monkeypatch.setattr(EngineClient, "call", call)
    monkeypatch.setattr(EngineSessionManager, "shutdown", shutdown)
    try:
        with pytest.raises(RuntimeError, match="injected initialize"):
            adapter.open("shared_project_root", dict(task_id=task, attempt_id="xa_partial", assignment_id="fixture"), "instructions")
        assert adapter.lookup_owned("xa_partial") == "alive"
        from cortex_runtime.recovery import registry
        row = registry.lookup_process(store, "xa_partial")
        assert row is not None and registry.probe_liveness(row) == "alive"
        with pytest.raises(StoreError, match="workspace_writer_busy"):
            with workspace.writer_lock(timeout=.02):
                pass
        assert manager is not None
        monkeypatch.setattr(EngineSessionManager, "shutdown", original_shutdown)
        adapter.terminate_owned("xa_partial")
        assert manager._process.wait_exited(0)
        with workspace.writer_lock(timeout=.02):
            pass
    finally:
        monkeypatch.setattr(EngineSessionManager, "shutdown", original_shutdown)
        adapter.shutdown()


def test_unknown_group_identity_is_not_signalled(tmp_path, monkeypatch):
    from cortex_runtime.engine import process_identity
    from engine_fakes import default_scenario, fake_launch_command, write_scenario

    environ = isolated_environ(tmp_path)
    record = make_connection_record(new_connection_id(), fake_launch_command(
        write_scenario(tmp_path, default_scenario())), environ=environ)
    process = EngineProcess(record, str(tmp_path), environ=environ)
    try:
        with monkeypatch.context() as patch:
            patch.setattr(process_identity, "probe_group", lambda _: "unknown")
            patch.setattr(os, "killpg", lambda *a: pytest.fail("unknown identity signalled"))
            process.terminate(grace=.01)
            assert not process.wait_exited(0)
    finally:
        process.release(grace=.2)


def test_legacy_timeout_marker_is_not_absence_evidence(tmp_path):
    from cortex_runtime.engine import process_identity
    from cortex_runtime.recovery import registry
    from engine_fakes import default_scenario, fake_launch_command, write_scenario

    environ = isolated_environ(tmp_path)
    record = make_connection_record(new_connection_id(), fake_launch_command(
        write_scenario(tmp_path, default_scenario())), environ=environ)
    process = EngineProcess(record, str(tmp_path), environ=environ)
    try:
        identity = process._group_identity
        assert identity is not None
        row = dict(pid=identity.pid, pgid=identity.pgid, start_ticks=identity.start_ticks,
                   boot_id=identity.boot_id, role="worker", terminated_at="legacy",
                   termination_reason="timeout_alive")
        assert registry.probe_liveness(row) == "alive"
        process.release(grace=.2)
        assert registry.probe_liveness(row) == "absent"
        assert process_identity.probe_group(identity) == "absent"
    finally:
        process.release(grace=.2)


def test_cross_process_unknown_liveness_never_signals(monkeypatch):
    from types import SimpleNamespace
    from cortex_runtime.delegation.live_adapters import EngineSessionManagerAdapter
    from cortex_runtime.recovery import registry

    adapter = EngineSessionManagerAdapter.__new__(EngineSessionManagerAdapter)
    adapter._store = SimpleNamespace()
    monkeypatch.setattr(registry, "lookup_process", lambda *a: dict(pgid=1))
    monkeypatch.setattr(registry, "probe_liveness", lambda *a: "unknown")
    monkeypatch.setattr(os, "killpg", lambda *a: pytest.fail("unknown process signalled"))
    assert not adapter._terminate_cross_process("fixture")


def test_cross_process_identity_lost_before_escalation_is_not_signalled(monkeypatch):
    from types import SimpleNamespace
    from cortex_runtime.delegation.live_adapters import EngineSessionManagerAdapter
    from cortex_runtime.engine import process
    from cortex_runtime.recovery import registry

    adapter = EngineSessionManagerAdapter.__new__(EngineSessionManagerAdapter)
    adapter._store = SimpleNamespace()
    states = iter(["alive", "unknown"])
    signals = []
    monkeypatch.setattr(registry, "lookup_process", lambda *a: dict(pgid=1))
    monkeypatch.setattr(registry, "probe_liveness", lambda *a: next(states))
    monkeypatch.setattr(process, "TERMINATE_GRACE_SECONDS", 0)
    monkeypatch.setattr(os, "killpg", lambda pgid, sig: signals.append(sig))
    assert not adapter._terminate_cross_process("fixture")
    assert signals == [signal.SIGTERM]


def test_metadata_error_and_repeated_cleanup_do_not_strand_real_engine(tmp_path, monkeypatch):
    from cortex_runtime.delegation import session_skills
    from test_delegation_live_adapters_open_cleanup import _fake_adapter, _task as _project_task

    store, task = _project_task(tmp_path)
    adapter, _ = _fake_adapter(store, tmp_path)
    adapter.open("shared_project_root", dict(task_id=task, attempt_id="xa_metadata"), "instructions")
    process = adapter._managers["xa_metadata"]._process
    def fail(*a, **kw):
        raise RuntimeError("metadata unavailable")
    monkeypatch.setattr(session_skills, "revoke_session_skills", fail)
    try:
        threads = [threading.Thread(target=adapter.terminate_owned, args=("xa_metadata",)) for _ in range(2)]
        for thread in threads:
            thread.start()
        for thread in threads:
            thread.join(10)
            assert not thread.is_alive()
        assert process.wait_exited(0)
        assert adapter.lookup_owned("xa_metadata") == "absent"
    finally:
        adapter.shutdown()


@pytest.mark.parametrize("outcome", ["COMPLETED", "FAILED", "exception"])
def test_direct_question_answer_uses_the_resource_exit_boundary(tmp_path, monkeypatch, outcome):
    from cortex_runtime.acp import questions_store

    store, task = _task(tmp_path)
    TaskMachine(store).create(task, "go")
    runtime, engine = _runtime(store, _finishing_router())
    question = questions_store.create_if_absent(store, task, "direct-answer", "Continue?")
    def continue_turn(*a, **kw):
        if outcome == "exception":
            raise RuntimeError("injected answer failure")
        with store.connection() as db:
            db.execute("UPDATE runtime_tasks SET state=? WHERE task_id=?", (outcome, task))
        return StopReason.END_TURN
    monkeypatch.setattr(runtime, "_continuation_loop", continue_turn)
    try:
        if outcome == "exception":
            with pytest.raises(RuntimeError, match="injected answer"):
                runtime.answer_question(task, SESSION, question, "yes", lambda _: True, threading.Event())
        else:
            runtime.answer_question(task, SESSION, question, "yes", lambda _: True, threading.Event())
            assert TaskMachine(store).get_state(task) == outcome
        assert not engine._owned
        assert task not in runtime._coordinator_sessions
    finally:
        runtime.shutdown()


def test_old_connection_disconnect_does_not_stop_replacement_turn(tmp_path, monkeypatch):
    store, task = _task(tmp_path)
    runtime, engine = _runtime(store, _finishing_router(ask_user=True))
    try:
        runtime.start_prompt(task, SESSION, "go", lambda _: True, threading.Event())
        monkeypatch.setattr(runtime, "_run", lambda *a, **kw: StopReason.END_TURN)
        runtime.start_prompt(task, "replacement", "go", lambda _: True, threading.Event())
        assert engine._owned and not runtime.transport_lost(task)
        assert not runtime.disconnect_session(SESSION, trigger="stdin_eof")
        assert engine._owned and not runtime.transport_lost(task)
        assert runtime._task_requester_sessions[task] == "replacement"
    finally:
        runtime.shutdown()


def test_automatic_cleanup_does_not_claim_a_replacement_process_attempt(tmp_path):
    store, task = _task(tmp_path)
    runtime, engine = _runtime(store, _finishing_router(ask_user=True))
    try:
        runtime.start_prompt(task, SESSION, "go", lambda _: True, threading.Event())
        # The durable coordinator now names a different runtime's replacement.
        # It is not in this runtime's cached/adapter-owned attempt set.
        with store.connection() as db:
            db.execute("UPDATE task_orchestration SET coordinator_attempt_ref='co_foreign' WHERE task_id=?", (task,))
        engine._owned["co_foreign"] = "foreign-session"
        runtime._cleanup_task_resources(task)
        assert "co_foreign" not in engine.terminate_calls
        assert engine._owned["co_foreign"] == "foreign-session"
    finally:
        engine._owned.pop("co_foreign", None)  # remove the foreign fixture, not an OS process
        runtime.shutdown()


def test_explicit_answer_reconnect_owns_current_session(tmp_path, monkeypatch):
    from cortex_runtime.acp import questions_store

    store, task = _task(tmp_path)
    runtime, engine = _runtime(store, _finishing_router(ask_user=True))
    try:
        runtime.start_prompt(task, SESSION, "go", lambda _: True, threading.Event())
        question = questions_store.create_if_absent(store, task, "reconnect-answer", "Continue?")
        def continuation(*args, **kwargs):
            assert runtime._task_requester_sessions[task] == "replacement"
            assert not runtime.disconnect_session(SESSION, trigger="stdin_eof")
            assert engine._owned and not runtime.transport_lost(task)
            return StopReason.END_TURN
        monkeypatch.setattr(runtime, "_continuation_loop", continuation)
        runtime.answer_question(task, "replacement", question, "yes", lambda _: True, threading.Event())
    finally:
        runtime.shutdown()


def test_old_eof_and_replacement_admission_are_serialized(tmp_path, monkeypatch):
    store, task = _task(tmp_path)
    runtime, engine = _runtime(store, _finishing_router(ask_user=True))
    reached, proceed, replacement_started = (threading.Event() for _ in range(3))
    errors = []
    original_release = runtime._release_task_host_requesters
    requester = lambda *a: True
    def release(*args, **kwargs):
        if threading.current_thread().name == "old-eof" and not reached.is_set():
            reached.set()
            assert proceed.wait(5)
        return original_release(*args, **kwargs)
    def replace():
        try:
            replacement_started.set()
            runtime.start_prompt(task, "replacement", "go", lambda _: True, threading.Event(),
                                 host_permission_requester=requester)
        except BaseException as error:
            errors.append(error)
    try:
        runtime.start_prompt(task, SESSION, "go", lambda _: True, threading.Event())
        monkeypatch.setattr(runtime, "_release_task_host_requesters", release)
        # The real recovery path opens a new coordinator after old cleanup.
        monkeypatch.setattr(runtime, "_run", lambda *a, **kw: (
            runtime._ensure_coordinator(task), StopReason.END_TURN)[1])
        old = threading.Thread(target=runtime.disconnect_session, args=(SESSION,), name="old-eof")
        old.start()
        assert reached.wait(5)
        replacement = threading.Thread(target=replace)
        replacement.start()
        assert replacement_started.wait(5)
        # Give the competing admission a chance to expose an unfenced EOF.
        replacement.join(.1)
        proceed.set()
        old.join(5)
        replacement.join(5)
        assert not old.is_alive() and not replacement.is_alive()
        assert not errors
        assert engine._owned and not runtime.transport_lost(task)
        assert runtime._task_permission_requesters[task] is requester
        assert runtime._task_requester_sessions[task] == "replacement"
    finally:
        proceed.set()
        runtime.shutdown()


def test_unconfirmed_cleanup_revokes_owned_authority_before_shutdown(tmp_path, monkeypatch):
    from cortex_runtime.contracts import StoreError
    from cortex_runtime.orchestration.gateway import _authority, channel_authority
    from cortex_runtime.trusted_channel import issue_channel, resolve_channel

    store, task = _task(tmp_path)
    runtime, engine = _runtime(store, _finishing_router(ask_user=True))
    try:
        runtime.start_prompt(task, SESSION, "go", lambda _: True, threading.Event())
        coordinator = runtime._coordinator_channels[task]
        with store.connection() as db:
            db.execute("UPDATE runtime_tasks SET state='PLANNING' WHERE task_id=?", (task,))
        owned = [coordinator]
        watchdogs = []
        for attempt in ("xa_opening", "co_replacement"):
            channel_id, secret = issue_channel(
                store, task, "worker", coordinator.owner_epoch,
                parent_channel_id=coordinator.channel_id, attempt_id=attempt)
            context = resolve_channel(store, channel_id, secret)
            watchdog = real_jail_for(store, (channel_id, secret, None))["alive"]
            if attempt == "xa_opening":
                owned.append(context)
                watchdogs.append(watchdog)
            else:
                unrelated, unrelated_watchdog = context, watchdog
        # An adapter-owned in-flight open need not have a cached worker session.
        monkeypatch.setattr(engine, "owned_attempts", lambda _: ("xa_opening",), raising=False)
        def unconfirmed(attempt):
            with store.connection() as db:
                for channel in owned:
                    with pytest.raises(StoreError, match="gateway_channel_inactive"):
                        _authority(db, channel)
                channel_authority(db, unrelated)
            assert all(not alive() for alive in watchdogs)
            assert unrelated_watchdog()
            raise RuntimeError("shutdown unconfirmed")
        monkeypatch.setattr(engine, "terminate_owned", unconfirmed)
        runtime._cleanup_task_resources(task)
        # Assert again outside the cleanup's intentionally best-effort catch.
        with store.connection() as db:
            for channel in owned:
                with pytest.raises(StoreError, match="gateway_channel_inactive"):
                    _authority(db, channel)
            channel_authority(db, unrelated)
        assert all(not alive() for alive in watchdogs)
        assert engine._owned
        assert TaskMachine(store).get_state(task) == "PLANNING"
    finally:
        monkeypatch.undo()
        runtime.shutdown()


def test_stale_generation_cleanup_preserves_replacement_requester(tmp_path, monkeypatch):
    store, task = _task(tmp_path)
    runtime, engine = _runtime(store, _finishing_router(ask_user=True))
    try:
        runtime.start_prompt(task, SESSION, "go", lambda _: True, threading.Event())
        old_generation = runtime._task_resource_generations[task]
        requester = lambda *a: True
        monkeypatch.setattr(runtime, "_run", lambda *a, **kw: StopReason.END_TURN)
        runtime.start_prompt(task, "replacement", "go", lambda _: True, threading.Event(),
                             host_permission_requester=requester)
        owned = dict(engine._owned)
        runtime._cleanup_task_resources(task, generation=old_generation)
        assert engine._owned == owned and not runtime.transport_lost(task)
        assert runtime._task_permission_requesters[task] is requester
    finally:
        runtime.shutdown()


def test_active_worker_answer_does_not_adopt_or_cleanup_the_turn(tmp_path, monkeypatch):
    from cortex_runtime.acp import questions_store
    from dataclasses import replace

    store, task = _task(tmp_path)
    runtime, engine = _runtime(store, _finishing_router(ask_user=True))
    requester = lambda *a: True
    event = threading.Event()
    def run(*a, **kw):
        TaskMachine(store).create(task, "go")
        runtime._ensure_coordinator(task)
        question = questions_store.create_if_absent(store, task, "worker-answer", "Continue?")
        question = replace(question, assignment_ref="worker-fixture")
        runtime.answer_question(task, "other-connection", question, "yes", lambda _: True, threading.Event())
        assert runtime._task_requester_sessions[task] == SESSION
        assert runtime._task_permission_requesters[task] is requester
        assert runtime._active_cancel_events[task] is event
        assert engine._owned and not event.is_set()
        return StopReason.END_TURN
    monkeypatch.setattr(runtime, "_run", run)
    try:
        runtime.start_prompt(task, SESSION, "go", lambda _: True, event,
                             host_permission_requester=requester)
    finally:
        runtime.shutdown()


def test_failed_coordinator_open_without_cached_session_revokes_channel(tmp_path, monkeypatch):
    from cortex_runtime.contracts import StoreError
    from cortex_runtime.trusted_channel import resolve_channel

    store, task = _task(tmp_path)
    runtime, engine = _runtime(store, _finishing_router())
    observed = []
    def fail_open(workspace, launch, instructions):
        observed.append((launch["channel_id"], launch["channel_secret"]))
        raise RuntimeError("partial open")
    monkeypatch.setattr(engine, "open", fail_open)
    # One failed open, not the transient-failure retries.
    from cortex_runtime.engine.retry_policy import RetryWindow
    runtime._engine_retry_window = RetryWindow(window_seconds=0)
    try:
        with pytest.raises(RuntimeError, match="partial open"):
            runtime.start_prompt(task, SESSION, "go", lambda _: True, threading.Event())
        assert len(observed) == 1 and task not in runtime._coordinator_sessions
        with pytest.raises(StoreError, match="channel_revoked"):
            resolve_channel(store, *observed[0])
    finally:
        runtime.shutdown()


@pytest.mark.parametrize("stage", ["before_delegate", "before_channel"])
def test_cleanup_fences_worker_channel_issuance(tmp_path, monkeypatch, stage):
    from cortex_runtime.contracts import StoreError

    store, task = _task(tmp_path)
    runtime, engine = _runtime(store, _finishing_router(ask_user=True))
    try:
        runtime.start_prompt(task, SESSION, "go", lambda _: True, threading.Event())
        coordinator = runtime._coordinator_channels[task]
        if stage == "before_delegate":
            runtime._cleanup_task_resources(task)
        else:
            original = runtime._provisioning._step_channel
            def step(*args, **kwargs):
                runtime._cleanup_task_resources(task)
                return original(*args, **kwargs)
            monkeypatch.setattr(runtime._provisioning, "_step_channel", step)
        with pytest.raises(StoreError, match="gateway_channel_inactive"):
            runtime._provisioning.delegate(coordinator, "worker", "brief", "backend-dev", "check", "late")
        with store.connection() as db:
            assert not db.execute(
                "SELECT 1 FROM trusted_channels WHERE task_id=? AND role='worker' AND revoked_at IS NULL",
                (task,)).fetchall()
        assert not engine._owned
    finally:
        runtime.shutdown()


@pytest.mark.parametrize("in_flight", [False, True])
def test_real_partial_worker_cleanup_revokes_authority_but_retains_quarantine(tmp_path, monkeypatch, in_flight):
    from cortex_runtime.acp.orchestrating_runtime import OrchestratingRuntime
    from cortex_runtime.contracts import StoreError
    from cortex_runtime.delegation import capacity
    from cortex_runtime.delegation.workspace_isolation import DirectProjectWorkspaceProvisioner
    from cortex_runtime.engine.client import EngineClient
    from cortex_runtime.engine.session_manager import EngineSessionManager
    from cortex_runtime.orchestration.gateway import _authority
    from cortex_runtime.trusted_channel import resolve_channel
    from orchestration_fakes import FakeConfigSnapshotSource
    from test_command_run_arguments import _worker
    from test_delegation_live_adapters_open_cleanup import _fake_adapter

    _, store, channel_id, secret = _worker(tmp_path)
    channel = resolve_channel(store, channel_id, secret)
    adapter, connection = _fake_adapter(store, tmp_path)
    workspace = DirectProjectWorkspaceProvisioner(store, str(store.project_root))
    adapter._workspace_lock_factory = workspace.writer_lock
    adapter._workspace_lock_timeout_seconds = .1
    runtime = OrchestratingRuntime(store, adapter, workspace, FakeConfigSnapshotSource(),
                                   allow_unqualified_delivery=True)
    alive = real_jail_for(store, (channel_id, secret, None))["alive"]
    original_call, original_shutdown = EngineClient.call, EngineSessionManager.shutdown
    reached, proceed = threading.Event(), threading.Event()
    failures, processes = [], []
    def call(self, method, *args, **kwargs):
        if method == "initialize":
            processes.append(self._process)
            reached.set()
            if in_flight:
                assert proceed.wait(5)
            raise RuntimeError("injected partial open")
        return original_call(self, method, *args, **kwargs)
    def shutdown(self, **kwargs):
        raise RuntimeError("unconfirmed shutdown")
    def opening():
        try:
            adapter.open("shared_project_root", dict(task_id=channel.task_id,
                         attempt_id=channel.attempt_id, assignment_id=channel.assignment_id,
                         channel_id=channel_id, channel_secret=secret), "instructions")
        except BaseException as error:
            failures.append(error)
    monkeypatch.setattr(EngineClient, "call", call)
    monkeypatch.setattr(EngineSessionManager, "shutdown", shutdown)
    thread = threading.Thread(target=opening)
    try:
        with store.connection() as db:
            assert _authority(db, channel)[0] == ["."]
        assert alive()
        thread.start()
        assert reached.wait(5)
        if not in_flight:
            thread.join(5)
            assert not thread.is_alive()
        assert channel.attempt_id in adapter.owned_attempts(channel.task_id)
        runtime._cleanup_task_resources(channel.task_id)
        with store.connection() as db:
            with pytest.raises(StoreError, match="gateway_channel_inactive"):
                _authority(db, channel)
        assert not alive()
        proceed.set()
        thread.join(5)
        assert not thread.is_alive() and len(failures) == 1
        assert "injected partial open" in str(failures[0])
        assert processes[0].alive()
        assert capacity.count_active(store, connection) == 1
        with pytest.raises(StoreError, match="workspace_writer_busy"):
            with workspace.writer_lock(timeout=.02):
                pass
        monkeypatch.setattr(EngineSessionManager, "shutdown", original_shutdown)
        adapter.terminate_owned(channel.attempt_id)
        assert processes[0].wait_exited(0)
        assert capacity.count_active(store, connection) == 0
    finally:
        proceed.set()
        thread.join(5)
        monkeypatch.setattr(EngineSessionManager, "shutdown", original_shutdown)
        runtime.shutdown()


def test_late_coordinator_open_cannot_repopulate_cleanup_cache(tmp_path, monkeypatch):
    from cortex_runtime.contracts import StoreError

    store, task = _task(tmp_path)
    runtime, engine = _runtime(store, _finishing_router())
    original = engine.open
    def opening(*args, **kwargs):
        session = original(*args, **kwargs)
        runtime._cleanup_task_resources(task)
        return session
    monkeypatch.setattr(engine, "open", opening)
    try:
        with pytest.raises(StoreError, match="channel_revoked"):
            runtime.start_prompt(task, SESSION, "go", lambda _: True, threading.Event())
        assert not engine._owned and task not in runtime._coordinator_sessions
    finally:
        runtime.shutdown()


def test_foreign_worker_replacement_is_not_claimed_by_old_assignment_cache(tmp_path, monkeypatch):
    from cortex_runtime.contracts import StoreError
    from cortex_runtime.orchestration.gateway import channel_authority
    from cortex_runtime.trusted_channel import issue_channel, resolve_channel
    from test_command_run_arguments import _worker

    _, store, channel_id, secret = _worker(tmp_path)
    old = resolve_channel(store, channel_id, secret)
    runtime, engine = _runtime(store, _finishing_router())
    with store.connection() as db:
        ref = db.execute("SELECT idempotency_key FROM delegation_provisioning WHERE attempt_id=?",
                         (old.attempt_id,)).fetchone()[0]
    foreign_id, foreign_secret = issue_channel(
        store, old.task_id, "coordinator", old.owner_epoch, attempt_id="co_foreign")
    worker_id, worker_secret = issue_channel(
        store, old.task_id, "worker", old.owner_epoch, parent_channel_id=foreign_id,
        attempt_id="xa_foreign", assignment_id=old.assignment_id)
    foreign = resolve_channel(store, worker_id, worker_secret)
    runtime._worker_sessions[(old.task_id, ref)] = "old-session"
    runtime._open_assignments[old.task_id] = {ref}
    monkeypatch.setattr(engine, "owned_attempts", lambda _: (old.attempt_id,), raising=False)
    with store.connection() as db:
        db.execute("UPDATE delegation_provisioning SET attempt_id=?,channel_id=?,engine_session_ref=? "
                   "WHERE idempotency_key=?", ("xa_foreign", worker_id, "foreign-session", ref))
    try:
        runtime._cleanup_task_resources(old.task_id)
        with store.connection() as db:
            with pytest.raises(StoreError, match="gateway_channel_inactive"):
                channel_authority(db, old)
            channel_authority(db, foreign)
            channel_authority(db, resolve_channel(store, foreign_id, foreign_secret))
        assert "xa_foreign" not in engine.terminate_calls
    finally:
        runtime.shutdown()


def test_delayed_eof_is_fenced_by_turn_generation_even_on_same_session(tmp_path, monkeypatch):
    store, task = _task(tmp_path)
    runtime, engine = _runtime(store, _finishing_router(ask_user=True))
    captured = threading.Event()
    resource_lock = runtime._task_resource_lock
    requester = lambda *a: True
    def lock(task_id):
        if threading.current_thread().name == "delayed-eof":
            captured.set()
        return resource_lock(task_id)
    results = []
    try:
        runtime.start_prompt(task, SESSION, "go", lambda _: True, threading.Event())
        with resource_lock(task):
            monkeypatch.setattr(runtime, "_task_resource_lock", lock)
            eof = threading.Thread(target=lambda: results.append(runtime.disconnect_session(SESSION)),
                                   name="delayed-eof")
            eof.start()
            assert captured.wait(5)
            monkeypatch.setattr(runtime, "_run", lambda *a, **kw: StopReason.END_TURN)
            runtime.start_prompt(task, SESSION, "go", lambda _: True, threading.Event(),
                                 host_permission_requester=requester)
        eof.join(5)
        assert not eof.is_alive() and results == [False]
        assert engine._owned and not runtime.transport_lost(task)
        assert runtime._task_permission_requesters[task] is requester
    finally:
        runtime.shutdown()
