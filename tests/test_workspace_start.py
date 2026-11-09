"""Explicit starts use fake dispatch only; all storage is disposable."""
from concurrent.futures import ThreadPoolExecutor

import pytest

from cortex_runtime.workspace_api import WorkspaceAPI, WorkspaceError
from cortex_runtime.workspace_api import storage
from cortex_runtime.workspace_contract import validate
from test_workspace_api import api_for, create_command, query


def start_command(task, key="start-1", version="1"):
    return dict(create_command(key), operation="tasks.start", target=task,
                expectedVersion=version, payload={})


def connected(tmp_path, dispatch):
    base = api_for(tmp_path)
    return WorkspaceAPI(base.store, project_id=base.project_id, actor_id=base.actor_id,
                        dispatch_start=dispatch)


def test_explicit_start_claim_precedes_dispatch_and_replay_is_read_only(tmp_path):
    calls = []
    def dispatch(start):
        with api.store.connection() as db:
            assert db.execute("SELECT state FROM workspace_starts WHERE execution_id=?",
                              (start["execution_id"],)).fetchone()[0] == "claimed"
        calls.append(start)
    api = connected(tmp_path, dispatch)
    task = api.handle(create_command())["target"]
    first = api.handle(start_command(task))
    assert first["outcome"] == "recorded"
    assert len(calls) == 1
    with api.store.connection() as db:
        row = db.execute("SELECT * FROM workspace_starts").fetchone()
        assert row["state"] == "dispatched"
        assert row["execution_id"] == first["executionId"]
    assert api.handle(start_command(task)) == first
    assert api.handle(query("requests.get", "start-1")) == first
    assert len(calls) == 1
    assert api.handle(dict(start_command(task), expectedVersion="2"))["outcome"] == "payload_conflict"
    execution = api.handle(query("executions.get", first["executionId"]))
    validate("Execution", execution)
    assert execution["state"] == "unknown"  # a claim is not engine progress


def test_a_start_with_no_engine_is_refused_with_its_reason_and_leaves_the_task_startable(tmp_path):
    calls = []
    api = connected(tmp_path, calls.append)
    task = api.handle(create_command())["target"]
    api.start_blocker = lambda: "No engine is ready."
    refused = api.handle(start_command(task))
    assert refused["outcome"] == "unsupported" and refused["reason"] == "No engine is ready."
    assert calls == []
    with api.store.connection() as db:
        assert db.execute("SELECT COUNT(*) FROM workspace_starts").fetchone()[0] == 0
        assert db.execute("SELECT execution_id FROM workspace_tasks WHERE task_id=?", (task,)).fetchone()[0] is None
    api.start_blocker = lambda: None
    assert api.handle(start_command(task, "start-2"))["outcome"] == "recorded"
    assert len(calls) == 1


def test_concurrent_keys_one_checkout_one_start(tmp_path):
    calls = []
    api = connected(tmp_path, calls.append)
    tasks = [api.handle(create_command(str(n)))["target"] for n in range(8)]
    with ThreadPoolExecutor(max_workers=8) as pool:
        receipts = list(pool.map(lambda pair: api.handle(start_command(pair[1], "s"+str(pair[0]))),
                                 enumerate(tasks)))
    assert sum(r["outcome"] == "recorded" for r in receipts) == len(calls) == 1
    assert sum(r["outcome"] == "busy" for r in receipts) == 7


class Crash(BaseException):
    pass


@pytest.mark.parametrize("where", ["before_claim", "after_claim", "after_dispatch"])
def test_crash_never_replays_and_keeps_checkout_busy(tmp_path, monkeypatch, where):
    calls = []
    api = connected(tmp_path, calls.append)
    task = api.handle(create_command())["target"]
    original = storage.claim_start
    def crash(*args, **kwargs):
        if where == "before_claim":
            raise Crash()
        value = original(*args, **kwargs)
        if where == "after_claim":
            raise Crash()
        return value
    monkeypatch.setattr(storage, "claim_start", crash)
    if where == "after_dispatch":
        def dispatch(start):
            calls.append(start)
            raise Crash()
        api.dispatch_start = dispatch
    with pytest.raises(Crash):
        api.handle(start_command(task))
    restarted = WorkspaceAPI(api.store, project_id=api.project_id, actor_id=api.actor_id,
                             dispatch_start=calls.append)
    restarted.activate(request_id="reconnect")
    old_count = len(calls)
    receipt = restarted.handle(query("requests.get", "start-1"))
    assert restarted.handle(start_command(task)) == receipt
    assert len(calls) == old_count
    execution = restarted.handle(query("executions.get", receipt["executionId"]))
    assert execution["state"] == "unknown"
    other = restarted.handle(create_command("other"))["target"]
    assert restarted.handle(start_command(other, "other-start"))["outcome"] == "busy"


@pytest.mark.parametrize("patch,outcome", [
    ({"expectedVersion": "9"}, "stale_version"),
    ({"ownerEpoch": "1"}, "stale_epoch"),
    ({"target": "missing-task"}, "invalid_request"),
])
def test_start_preconditions(tmp_path, patch, outcome):
    calls = []
    api = connected(tmp_path, calls.append)
    task = api.handle(create_command())["target"]
    assert api.handle(dict(start_command(task), **patch))["outcome"] == outcome
    assert not calls


def test_same_key_concurrent_dispatch_once(tmp_path):
    calls = []
    api = connected(tmp_path, calls.append)
    task = api.handle(create_command())["target"]
    with ThreadPoolExecutor(max_workers=4) as pool:
        receipts = list(pool.map(lambda _: api.handle(start_command(task)), range(8)))
    assert all(r == receipts[0] for r in receipts)
    assert len(calls) == 1



def test_prepare_append_failure_rolls_back_intent_and_receipt(tmp_path, monkeypatch):
    api = connected(tmp_path, lambda start: pytest.fail("dispatch after rolled back prepare"))
    task = api.handle(create_command())["target"]
    before = api.handle(query("project.snapshot"))
    def fail(*args, **kwargs):
        raise RuntimeError("injected outbox failure")
    monkeypatch.setattr(storage, "append_public_tx", fail)
    with pytest.raises(RuntimeError):
        api.handle(start_command(task))
    with api.store.connection() as db:
        assert db.execute("SELECT count(*) FROM workspace_starts").fetchone()[0] == 0
        assert db.execute("SELECT count(*) FROM workspace_requests WHERE operation='tasks.start'").fetchone()[0] == 0
    assert api.handle(query("project.snapshot")) == before


def test_pending_start_blocks_generic_bootstrap(tmp_path):
    from cortex_runtime.bootstrap import bootstrap_task
    from cortex_runtime.portable.identity import SourceEvent
    from cortex_runtime.contracts import StoreError
    api = connected(tmp_path, lambda start: None)
    task = api.handle(create_command())["target"]
    api.handle(start_command(task))
    with pytest.raises(StoreError, match="workspace_checkout_busy"):
        bootstrap_task(api.store, SourceEvent(origin="generic", event_id="one"), "Other task", "owner")
    with api.store.connection() as db:
        assert db.execute("SELECT count(*) FROM tasks").fetchone()[0] == 0


def test_start_table_is_additive_and_never_installed_by_inspection(tmp_path):
    from cortex_runtime.store import Store
    from cortex_runtime.split import _validate_format
    api = api_for(tmp_path)
    with api.store.connection() as db:
        db.execute("DROP TABLE workspace_starts")
        _validate_format(db)  # pre-start format-12 extension remains splittable
        db.execute("PRAGMA wal_checkpoint(TRUNCATE)")
    before = api.store.path.read_bytes()
    Store(api.store.directory, initialize=False, project_root=tmp_path)
    assert api.store.path.read_bytes() == before
    with pytest.raises(WorkspaceError, match="workspace_storage_unavailable"):
        api.handle(query("project.snapshot"))
    Store(api.store.directory, project_root=tmp_path)
    assert api.handle(query("project.snapshot"))["tasks"]["items"] == []


class Transport:
    def __init__(self):
        self.responses = []
    def send_response(self, value):
        self.responses.append(value)
        return True
    def send_notification(self, value):
        return True


def agent_for(tmp_path, monkeypatch, runtime):
    from cortex_runtime.acp.agent import Agent
    agent = Agent(runtime, environ={"HOME": str(tmp_path.parent / (tmp_path.name + "-home")),
                                   "XDG_CONFIG_HOME": str(tmp_path.parent / (tmp_path.name + "-config")),
                                   "XDG_STATE_HOME": str(tmp_path.parent / (tmp_path.name + "-state"))})
    agent.transport = Transport()
    monkeypatch.setattr(agent, "_default_selection_from_user_defaults", lambda: None)
    monkeypatch.setattr(agent, "_config_options_for_session", lambda *args: [])
    session = agent._session_new(dict(cwd=str(tmp_path), mcpServers=[]))["sessionId"]
    agent._apply_access_mode_selection(agent._store_for(str(tmp_path)), agent.sessions[session], session,
                                       "guarded", origin="acp_set_mode")
    agent.activate_workspace(session, project_id="project-1", actor_id="local-owner", request_id="activate-1")
    monkeypatch.setattr(agent, "_launch_prompt", lambda args: agent._run_prompt(*args))
    return agent, agent._workspace_apis["project-1"]


@pytest.mark.parametrize("kind", ["answer", "work"])
def test_real_prompt_orchestration_result_and_drift(tmp_path, monkeypatch, kind):
    from cortex_runtime.acp.orchestrating_runtime import OrchestratingRuntime, PromptOutcome
    from cortex_runtime.delegation import mcp_tools
    from cortex_runtime.portable.engine_contract import StopReason
    from cortex_runtime.store import Store
    from orchestration_fakes import FakeSessionRuntime, FakeConfigSnapshotSource, FakeWorkspace
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    calls = []
    def router(store, channel, ref, launch, instructions):
        def behavior(store, channel, ref, message, index):
            calls.append(channel.role)
            if channel.role == "coordinator":
                context = mcp_tools.dispatch(store, channel, "task_get_context", {})
                if kind == "answer":
                    mcp_tools.dispatch(store, channel, "answer_submit",
                                       dict(text="An informational answer.", request_key="answer"))
                elif context["state"] == "PLANNING":
                    mcp_tools.dispatch(store, channel, "plan_submit", dict(plan=dict(
                        tasks=[dict(id="N1", role="backend-dev", objective="Write a file",
                                    dependencies=[], scope=["output.txt"], model="auto")],
                        verification=[["python3", "-B", "-c", "pass"]]), request_key="plan"))
                else:
                    mcp_tools.dispatch(store, channel, "synthesis_submit", dict(text="Checked work."))
            else:
                with store.connection() as db:
                    role = db.execute("SELECT role_id FROM delegation_provisioning WHERE attempt_id=?",
                                      (channel.attempt_id,)).fetchone()[0]
                if role != "worker-code-reviewer":
                    (tmp_path / "output.txt").write_text("implemented")
                draft = store.call("create_draft", dict(template="general", request_key="draft-" + channel.attempt_id),
                                   channel.principal_thread_id, channel.parent_principal_thread_id, trusted=True)
                mcp_tools.dispatch(store, channel, "draft_write", dict(draft_id=draft["draft_id"],
                    content=draft["required_first_line"] + "\n\n# Report\n\nObserved.\n", request_key="body"))
                published = store.call("write_report", dict(
                    draft_id=draft["draft_id"], title="Observed", summary="Observed evidence",
                    author="worker", request_key="publish-" + channel.attempt_id),
                    channel.principal_thread_id, channel.parent_principal_thread_id, trusted=True)
                args = dict(report=published["report_id"], summary="Observed", checks=[], request_key="result")
                if role == "worker-code-reviewer":
                    args["verdict"] = "APPROVE"
                mcp_tools.dispatch(store, channel, "result_submit", args)
            return PromptOutcome(StopReason.END_TURN, "Not authoritative chat text.")
        return behavior
    runtime = OrchestratingRuntime(store, FakeSessionRuntime(store, router), FakeWorkspace(),
                                   FakeConfigSnapshotSource(), allow_unqualified_delivery=True)
    try:
        agent, api = agent_for(tmp_path, monkeypatch, runtime)
        task = api.handle(create_command(description="Explain the project" if kind == "answer" else "Write a file"))["target"]
        started = agent._workspace_request(start_command(task))
        execution = api.handle(query("executions.get", started["executionId"]))
        assert execution["state"] == "result"
        assert not agent.transport.responses
        from cortex_runtime.workspace_api.projections import ReconnectState
        reconnect, cursor = ReconnectState(api.project_id, "0"), None
        while True:
            page = api.handle(query("events.page", cursor=cursor))
            for event in page["items"]:
                assert reconnect.accept(event)
            cursor = page["nextCursor"]
            if cursor is None:
                break
        result = api.handle(query("results.get", execution["resultId"]))
        validate("Result", result)
        assert result["kind"] == kind
        assert result["text"] != "Not authoritative chat text."
        assert result["label"] == "Runtime result; not accepted or merged"
        # Both a completed answer and reviewed, verified work may be accepted.
        # Neither is merge authority: owner acceptance/integration is still separate.
        assert result["acceptanceAvailable"] is True
        assert result["integrationAvailable"] is False
        with store.connection() as db:
            start = db.execute("SELECT * FROM workspace_starts").fetchone()
            source = db.execute("SELECT * FROM source_events").fetchone()
            assert (source["origin"], source["event_id"]) == (start["source_origin"], start["source_event_id"])
            assert source["task_id"] == execution["runtimeTaskId"]
            events = [r[0] for r in db.execute("SELECT type FROM runtime_events")]
            for evidence in result["evidence"]:
                assert db.execute("SELECT 1 FROM reports WHERE id=? AND task_id=?",
                                  (evidence["reference"], execution["runtimeTaskId"])).fetchone()
                assert len(evidence["digest"]) == 64
        if kind == "work":
            assert result["evidence"]
            assert "REVIEW_STARTED" in events and "VERIFICATION_PASSED" in events
            assert result["reviewStatus"] == "APPROVE" and result["verificationStatus"] == "passed"
            assert result["checks"] and all(c["outcome"] == "passed" for c in result["checks"])
            assert result["observedWorkspaceDigest"] and result["workspaceBindingValid"]
            (tmp_path / "output.txt").write_text("external change")
            drift = api.handle(query("results.get", execution["resultId"]))
            assert drift["observedWorkspaceDigest"] is None
            assert not drift["workspaceBindingValid"] and "workspace_drift" in drift["warnings"]
        else:
            assert calls == ["coordinator"]
            assert result["reviewStatus"] == result["verificationStatus"] == "not_applicable"
        assert api.handle(start_command(task)) == started
        another = api.handle(create_command("another", "Explain the project"))["target"]
        assert api.handle(start_command(another, "another-start"))["outcome"] == "recorded"
    finally:
        runtime.shutdown()


def test_end_turn_is_not_a_result(tmp_path, monkeypatch):
    from cortex_runtime.orchestration.state import TaskMachine
    from cortex_runtime.portable.engine_contract import StopReason
    class Runtime:
        def start_prompt(self, task_id, session_id, text, emit, cancel_event,
                         source_origin=None, source_event_id=None):
            TaskMachine(agent._store_for(str(tmp_path))).create(task_id, text)
            return StopReason.END_TURN
    agent, api = agent_for(tmp_path, monkeypatch, Runtime())
    task = api.handle(create_command())["target"]
    started = api.handle(start_command(task))
    execution = api.handle(query("executions.get", started["executionId"]))
    assert execution["state"] != "result"
    with pytest.raises(WorkspaceError, match="result_unavailable"):
        api.handle(query("results.get", "wr_" + started["executionId"]))

def test_capture_and_binding_rollback_together(tmp_path, monkeypatch):
    from cortex_runtime.contracts import StoreError
    from cortex_runtime.acp.runtime import UnavailableRuntime
    agent, api = agent_for(tmp_path, monkeypatch, UnavailableRuntime())
    task = api.handle(create_command())["target"]
    original = storage.bind_execution_tx
    def fail(*args, **kwargs):
        original(*args, **kwargs)
        raise StoreError("injected_capture_failure")
    monkeypatch.setattr(storage, "bind_execution_tx", fail)
    started = api.handle(start_command(task))
    with api.store.connection() as db:
        for table in ("tasks", "source_events", "workspace_executions"):
            assert db.execute("SELECT count(*) FROM " + table).fetchone()[0] == 0
        assert not db.execute("SELECT 1 FROM acp_host_sessions WHERE task_id IS NOT NULL").fetchone()
    assert not list(api.store.directory.glob("t_*/*.md"))
    assert api.handle(query("executions.get", started["executionId"]))["state"] == "unknown"
    assert api.handle(start_command(task)) == started
    assert not agent.transport.responses


def test_generic_meta_does_not_supply_capture_identity_or_lose_null_id_reply(tmp_path, monkeypatch):
    from cortex_runtime.portable.engine_contract import StopReason
    seen = []
    class Runtime:
        def start_prompt(self, task_id, session_id, text, emit, cancel_event,
                         source_origin=None, source_event_id=None):
            seen.append((source_origin, source_event_id))
            return StopReason.END_TURN
    agent, api = agent_for(tmp_path, monkeypatch, Runtime())
    session_id = next(iter(agent.sessions))
    agent._session_prompt(None, dict(sessionId=session_id, prompt=[dict(type="text", text="Ordinary prompt")],
        _meta=dict(source_origin="forged", source_event_id="forged", executionId="forged")))
    assert len(seen) == 1
    assert seen[0][0] == "acp:" + session_id and seen[0][1] != "forged"
    assert agent.transport.responses[-1]["id"] is None
    with api.store.connection() as db:
        assert db.execute("SELECT count(*) FROM workspace_executions").fetchone()[0] == 0


def test_dispatch_failure_is_unknown_not_retryable(tmp_path):
    def fail(start):
        raise RuntimeError("not public")
    api = connected(tmp_path, fail)
    task = api.handle(create_command())["target"]
    receipt = api.handle(start_command(task))
    assert receipt["outcome"] == "recorded"
    assert api.handle(query("executions.get", receipt["executionId"]))["state"] == "unknown"
    assert api.handle(start_command(task)) == receipt
