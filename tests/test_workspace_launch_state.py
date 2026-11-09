"""A started task whose engine has not opened yet: shown, explained and stoppable.

No native engine or paid call. Most fixtures write launch facts directly;
the retry regression runs the real runtime with an in-process failing engine.
"""
import threading

import pytest

from cortex_runtime.workspace_api import WorkspaceError, control, storage
from test_workspace_api import api_for, create_command, query, runtime_binding
from test_workspace_start import start_command


def launching(tmp_path, *, start_state="dispatched"):
    """A task the owner started: bound to its session and owner, runtime row not created yet."""
    api = api_for(tmp_path)
    task = api.handle(create_command())["target"]
    def write(db):
        db.execute("INSERT INTO tasks VALUES ('runtime-1',?,'now')", (api.store.project_root,))
        db.execute("INSERT INTO task_owners VALUES ('runtime-1','owner',1,'now')")
        db.execute("INSERT INTO acp_host_sessions VALUES ('session-1',?,'runtime-1','{}','{}','[]','now',NULL)",
                   (api.store.project_root,))
        db.execute("INSERT INTO source_events VALUES ('workspace:project-1','source-1','runtime-1','now',NULL,NULL)")
        db.execute("INSERT INTO workspace_starts VALUES ('we_1',?,'project-1','local-owner','start-1',"
                   "'workspace:project-1','source-1','dispatched',1,'now')", (task,))
        db.execute("UPDATE workspace_tasks SET execution_id='we_1' WHERE task_id=?", (task,))
        execution = storage.bind_execution_tx(db, task, "runtime-1", "session-1", start=dict(
            execution_id="we_1", source_origin="workspace:project-1", source_event_id="source-1"))
        db.execute("UPDATE workspace_starts SET state=? WHERE execution_id='we_1'", (start_state,))
        return execution
    return api, task, api.store.immediate_write(write)


def notice(api, key, n, **params):
    from cortex_runtime.acp import messages
    from cortex_runtime.delegation import events as delegation_events
    note = messages.render(key, "en", **params)
    delegation_events.append_event(api.store, "runtime-1", "progress", f"progress:{n}",
                                   dict(note=note, message_key=key, message_params=params), origin="runtime")
    return note


def seen(api, task, execution):
    return api.handle(query("tasks.get", task)), api.handle(query("executions.get", execution))


def test_a_launch_before_the_runtime_records_the_task_is_starting_not_unknown(tmp_path):
    api, task, execution = launching(tmp_path)
    shown, run = seen(api, task, execution)
    assert (shown["phase"], shown["executionState"], run["state"]) == ("planning", "starting", "starting")
    assert shown["runtimeState"] is None and not shown["warnings"] and not run["warnings"]


def test_each_launch_retry_is_the_task_reason_and_a_give_up_needs_the_owner(tmp_path):
    api, task, execution = launching(tmp_path)
    error = "EngineTransportClosed: No permissions to create a new namespace"
    notice(api, "engine_retry.launch", 1, error=error, retry=1, delay=5)
    second = notice(api, "engine_retry.launch", 2, error=error, retry=2, delay=10)
    shown, run = seen(api, task, execution)
    assert (shown["phase"], shown["executionState"]) == ("planning", "starting")
    assert shown["waitReason"] == second and shown["warnings"] == run["warnings"] == ["engine_launch_retrying"]
    gave_up = notice(api, "engine_retry.gave_up", 3, error=error, retries=11, minutes=8)
    shown, run = seen(api, task, execution)
    assert (shown["phase"], shown["executionState"], run["state"]) == ("blocked", "blocked", "blocked")
    assert shown["waitReason"] == gave_up and shown["warnings"] == ["engine_launch_failed"]
    notice(api, "engine_retry.recovered", 4, retries=11)
    shown, _ = seen(api, task, execution)
    assert (shown["executionState"], shown["waitReason"], shown["warnings"]) == ("starting", None, [])


def test_a_launch_stopped_before_the_runtime_task_exists_is_stopped(tmp_path):
    api, task, execution = launching(tmp_path)
    api.store.immediate_write(lambda db: db.execute("INSERT INTO task_stopped VALUES ('runtime-1','now')"))
    shown, run = seen(api, task, execution)
    assert (shown["phase"], shown["executionState"], run["state"]) == ("stopped", "stopped", "stopped")


@pytest.mark.parametrize("mutation", [
    "UPDATE task_owners SET epoch=0",
    "UPDATE acp_host_sessions SET project_root='elsewhere'",
    "UPDATE workspace_starts SET state='unknown'",
])
def test_a_missing_runtime_row_with_any_contradiction_stays_unknown(tmp_path, mutation):
    api, task, execution = launching(tmp_path)
    api.store.immediate_write(lambda db: db.execute(mutation))
    shown, run = seen(api, task, execution)
    assert shown["executionState"] == run["state"] == "unknown"


@pytest.mark.parametrize("stopped", [False, True])
def test_a_new_controller_doubts_an_open_launch_but_keeps_a_stopped_one(tmp_path, stopped):
    api, task, execution = launching(tmp_path)
    if stopped:
        api.store.immediate_write(lambda db: db.execute("INSERT INTO task_stopped VALUES ('runtime-1','now')"))
    restarted = api_for(tmp_path)
    assert restarted.handle(query("executions.get", execution))["state"] == ("stopped" if stopped else "unknown")


def test_a_runtime_bound_execution_that_loses_its_runtime_row_stays_unknown(tmp_path):
    api = api_for(tmp_path)
    task = api.handle(create_command())["target"]
    execution = runtime_binding(api, task)
    api.store.immediate_write(lambda db: db.execute("DELETE FROM runtime_tasks"))
    assert api.handle(query("executions.get", execution))["state"] == "unknown"


def test_real_launch_retry_before_runtime_row_remains_stoppable(tmp_path):
    from cortex_runtime.acp.orchestrating_runtime import OrchestratingRuntime
    from cortex_runtime.engine.client import EngineProtocolError
    from cortex_runtime.engine.retry_policy import RetryWindow
    from orchestration_fakes import FakeConfigSnapshotSource, FakeWorkspace
    from test_engine_retry_window import FlakyCoordinatorOpenEngine, _router

    api, task, execution = launching(tmp_path)
    agent = Agent(api)  # The host binding still has the original epoch.
    engine = FlakyCoordinatorOpenEngine(api.store, _router(),
        lambda: EngineProtocolError(-32603, "Internal error", None), failures=10)
    runtime = OrchestratingRuntime(api.store, engine, FakeWorkspace(), FakeConfigSnapshotSource(),
                                  allow_unqualified_delivery=True)
    runtime._engine_retry_window = RetryWindow(first_delay_seconds=.001, max_delay_seconds=.001)
    cancel = threading.Event()
    stops = []
    class StopRuntime:
        def cancel_task(self, task_id, session_id):
            stops.append((task_id, session_id))
            runtime.cancel_task(task_id, session_id)
    agent.runtime = StopRuntime()

    def emit(text):
        # Recovery really reacquires ownership on the second open, not just a notice fixture.
        if engine.coordinator_opens >= 2:
            with api.store.connection() as db:
                assert db.execute("SELECT epoch FROM task_owners").fetchone()[0] > 1
                assert not db.execute("SELECT 1 FROM runtime_tasks").fetchone()
            shown, run = seen(api, task, execution)
            assert shown["executionState"] == run["state"] == "starting"
            reply = control.handle(agent, dict(projectId="project-1", executionId=execution,
                action="pause", requestId="code-stop", expectedVersion=shown["version"]))
            assert reply["outcome"] == "recorded" and callable(reply["_settle"])
        return True

    with pytest.raises(EngineProtocolError):
        runtime.start_prompt("runtime-1", "session-1", "Build a thing.", emit, cancel)
    assert cancel.is_set()  # The real task stop interrupted the real retry wait.
    assert engine.coordinator_opens == 2 and stops == [("runtime-1", "session-1")]
    assert seen(api, task, execution)[0]["executionState"] == "stopped"


class Agent:
    """Only what control.handle reads: the API, the bound session and the runtime's stop."""
    def __init__(self, api):
        from cortex_runtime.bootstrap import _current_owner_epoch_obj
        with api.store.connection() as db:
            owner = _current_owner_epoch_obj(db, "runtime-1")
        self._workspace_apis = {"project-1": api}
        self.sessions = {"session-1": dict(session_id="session-1", project=api.store.project_root, task_id="runtime-1",
                                           owner_epoch=owner, lock=threading.Lock(), active=threading.Event())}
        self.stops = []
        agent = self
        class Runtime:
            def cancel_task(self, task_id, session_id):
                agent.stops.append((task_id, session_id))
                api.store.immediate_write(lambda db: db.execute("INSERT INTO task_stopped VALUES (?, 'now')", (task_id,)))
                return True
        self.runtime = Runtime()

    def _session_cancel(self, params):
        raise AssertionError("a launch has no turn to pause")


@pytest.mark.parametrize("retrying", [False, True])
def test_stop_on_a_launch_cancels_it_through_the_runtime(tmp_path, retrying):
    api, task, execution = launching(tmp_path)
    if retrying:
        notice(api, "engine_retry.gave_up", 1, error="EngineTransportClosed", retries=11, minutes=8)
    agent = Agent(api)
    version = api.handle(query("tasks.get", task))["version"]
    reply = control.handle(agent, dict(projectId="project-1", executionId=execution, action="pause",
                                       requestId="stop-1", expectedVersion=version))
    # Like any Stop, it answers once the cancelled launch turn has released the session.
    assert callable(reply.pop("_settle")) and reply == dict(outcome="recorded")
    assert agent.stops == [("runtime-1", "session-1")]
    assert api.handle(query("executions.get", execution))["state"] == "stopped"


@pytest.mark.parametrize("action", ["prompt", "resume", "worker_stop"])
def test_retry_epoch_exception_does_not_authorize_non_stop_controls(tmp_path, action):
    api, task, execution = launching(tmp_path)
    agent = Agent(api)
    api.store.immediate_write(lambda db: db.execute("UPDATE task_owners SET epoch=2"))
    with pytest.raises(WorkspaceError, match="execution_session_unavailable"):
        control.handle(agent, dict(projectId="project-1", executionId=execution, action=action,
                                   requestId="not-stop", expectedVersion="1"))
    assert not agent.stops


def test_stop_on_an_unbound_session_is_still_refused(tmp_path):
    api, task, execution = launching(tmp_path)
    agent = Agent(api)
    agent.sessions.clear()
    with pytest.raises(WorkspaceError, match="execution_session_unavailable"):
        control.handle(agent, dict(projectId="project-1", executionId=execution, action="pause",
                                   requestId="stop-1", expectedVersion="1"))
    assert agent.stops == []


def test_a_stopped_launch_no_longer_keeps_the_checkout_busy(tmp_path):
    api, task, execution = launching(tmp_path)
    api.dispatch_start = lambda start: None  # the same controller: a new one would treat the launch as unconfirmed
    other = api.handle(create_command("create-2"))["target"]
    assert api.handle(start_command(other, "start-2"))["outcome"] == "busy"
    api.store.immediate_write(lambda db: db.execute("INSERT INTO task_stopped VALUES ('runtime-1','now')"))
    assert api.handle(start_command(other, "start-3"))["outcome"] == "recorded"
