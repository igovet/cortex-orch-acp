"""Identified Workspace answers, using disposable stores and fake handoffs only."""
import copy
from concurrent.futures import ThreadPoolExecutor

import pytest

from cortex_runtime.acp import questions_store, sessions
from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.orchestration import question_batches
from cortex_runtime.orchestration.questions import request, resume_answered
from cortex_runtime.orchestration.state import TaskMachine
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.trusted_channel import resolve_channel
from cortex_runtime.workspace_api import WorkspaceAPI, WorkspaceError
from cortex_runtime.workspace_api.storage import bind_execution_tx
from cortex_runtime.workspace_contract import ContractError, validate
from test_workspace_api import api_for, create_command, query
from test_worker_wait_continuation import bound


def asking(tmp_path, *, batch=False, dispatch=None):
    base = api_for(tmp_path)
    api = WorkspaceAPI(base.store, project_id=base.project_id, actor_id=base.actor_id,
                       dispatch_answer=dispatch if dispatch is not None else lambda answer: None)
    task = api.handle(create_command())["target"]
    boot = bootstrap_task(api.store, SourceEvent(origin="workspace-test", event_id="one"), "Build", "owner")
    machine = TaskMachine(api.store)
    machine.create(boot.task_id, "Build")
    with api.store.connection() as db:
        db.execute("INSERT INTO task_orchestration(task_id,state,updated_at) VALUES (?,'planning','now')", (boot.task_id,))
        db.commit()
    sessions.create_session(api.store, "incoming", str(tmp_path), {}, {}, [])
    sessions.bind_task(api.store, "incoming", boot.task_id)
    execution = api.store.immediate_write(lambda db: bind_execution_tx(db, task, boot.task_id, "incoming"))
    channel = resolve_channel(api.store, boot.channel_id, boot.secret)
    forms = [dict(question="Choose size?", options=["small", "large"]),
             dict(question="Choose format?", options=["JSON", "CSV"])]
    args = dict(questions=forms, request_key="ask") if batch else dict(**forms[0], request_key="ask")
    saved = request(api.store, channel, args)
    return api, execution, saved


def decisions(api, execution):
    return api.handle(query("decisions.list", execution))["items"]


def command(items, key="answer", texts=None):
    root = items[0]
    answers = [dict(decisionId=d["decisionId"], questionId=d["questionId"],
                    expectedVersion=d["version"], optionId=None,
                    text=(texts or ["custom answer"] * len(items))[i])
               for i, d in enumerate(items)]
    return dict(create_command(key), operation="decisions.answer", target=root["batchId"] or root["decisionId"],
                expectedVersion=root["version"], ownerEpoch=root["ownerEpoch"],
                payload=dict(executionId=root["executionId"], acpSessionId=root["acpSessionId"],
                             batchId=root["batchId"], answers=answers))


@pytest.mark.parametrize("batch", [False, True])
def test_exact_answer_and_existing_resume_once(tmp_path, batch):
    calls = []
    def handoff(value):
        # The answer and public receipt must already be committed.
        assert api.handle(query("requests.get", "answer"))["outcome"] == "recorded"
        assert all(questions_store.get(api.store, q).status == "answered" for q in value["question_ids"])
        calls.append(value)
        assert resume_answered(api.store, value["task_id"])
    api, execution, saved = asking(tmp_path, batch=batch, dispatch=handoff)
    items = decisions(api, execution)
    assert len(items) == (2 if batch else 1)
    for d in items:
        validate("Decision", d)
        assert d["allowedOperations"] == ["decisions.answer"]
        assert api.handle(query("decisions.get", d["decisionId"])) == d
    text = "  first line\n\nsecond line\r\nlast  "
    cmd = command(items, texts=[text] * len(items))
    if not batch:
        cmd["payload"]["answers"][0].update(optionId=items[0]["options"][1]["optionId"], text="")
        text = "large"
    receipt = api.handle(cmd)
    assert receipt["outcome"] == "recorded"
    assert api.handle(cmd) == receipt
    assert api.handle(query("requests.get", "answer")) == receipt
    assert len(calls) == 1
    for d in items:
        assert questions_store.get(api.store, d["questionId"]).answer_text == text
    assert api.handle(dict(cmd, requestId="loser"))["outcome"] == "already_resolved"
    changed = copy.deepcopy(cmd)
    changed["payload"]["answers"][0].update(optionId=None, text="different")
    assert api.handle(changed)["outcome"] == "payload_conflict"


def test_concurrent_distinct_and_identical_requests_one_handoff(tmp_path):
    calls = []
    api, execution, _ = asking(tmp_path, dispatch=calls.append)
    cmd = command(decisions(api, execution))
    with ThreadPoolExecutor(max_workers=6) as pool:
        receipts = list(pool.map(lambda n: api.handle(dict(cmd, requestId="actor-" + str(n))), range(6)))
    assert sum(r["outcome"] == "recorded" for r in receipts) == len(calls) == 1
    winner = next(r for r in receipts if r["outcome"] == "recorded")
    with ThreadPoolExecutor(max_workers=4) as pool:
        replayed = list(pool.map(lambda _: api.handle(dict(cmd, requestId=winner["requestId"])), range(4)))
    assert replayed == [winner] * 4
    assert len(calls) == 1


@pytest.mark.parametrize("kind", ["version", "member_version", "epoch", "session", "requirement", "superseded",
                                   "partial", "duplicate", "option", "question", "blank", "binding"])
def test_invalid_answers_are_atomic(tmp_path, kind):
    calls = []
    api, execution, _ = asking(tmp_path, batch=True, dispatch=calls.append)
    items = decisions(api, execution)
    cmd = command(items)
    if kind == "version": cmd["expectedVersion"] = "999"
    if kind == "member_version": cmd["payload"]["answers"][1]["expectedVersion"] = "999"
    if kind == "epoch": cmd["ownerEpoch"] = "999"
    if kind == "session": cmd["payload"]["acpSessionId"] = "foreign"
    if kind == "partial": cmd["payload"]["answers"].pop()
    if kind == "duplicate": cmd["payload"]["answers"][1] = cmd["payload"]["answers"][0]
    if kind == "option": cmd["payload"]["answers"][0]["optionId"] = "foreign-option"
    if kind == "question": cmd["payload"]["answers"][0]["questionId"] = "foreign-question"
    if kind == "blank": cmd["payload"]["answers"][0]["text"] = " \n "
    if kind in {"requirement", "superseded", "binding"}:
        with api.store.connection() as db:
            if kind == "requirement":
                db.execute("UPDATE task_orchestration SET requirement_revision=requirement_revision+1")
            elif kind == "superseded":
                db.execute("UPDATE pending_questions SET status='superseded'")
            else:
                db.execute("UPDATE acp_host_sessions SET closed_at='now'")
            db.commit()
    receipt = api.handle(cmd)
    assert receipt["outcome"] != "recorded"
    assert not calls
    assert all(questions_store.get(api.store, d["questionId"]).answer_text is None for d in items)


def test_attachment_refusal_is_visible_and_never_drops_input(tmp_path):
    api, execution, _ = asking(tmp_path)
    cmd = command(decisions(api, execution))
    cmd["payload"]["attachments"] = [{"name": "image.png"}]
    with pytest.raises(ContractError):
        api.handle(cmd)
    assert all(d["status"] == "pending" for d in decisions(api, execution))
    assert api.handle(query("capabilities.get"))["attachments"] is True


def test_unconnected_adapter_does_not_advertise_answer(tmp_path):
    api = api_for(tmp_path)
    assert "decisions.answer" not in api.handle(query("capabilities.get"))["operations"]


def test_answer_and_receipt_rollback_together(tmp_path, monkeypatch):
    import cortex_runtime.workspace_api as module
    calls = []
    api, execution, _ = asking(tmp_path, batch=True, dispatch=calls.append)
    items = decisions(api, execution)
    def fail(*args, **kwargs):
        raise RuntimeError("injected receipt failure")
    monkeypatch.setattr(module, "record", fail)
    with pytest.raises(RuntimeError, match="injected"):
        api.handle(command(items))
    assert not calls
    assert all(questions_store.get(api.store, d["questionId"]).status == "open" for d in items)


@pytest.mark.parametrize("batch", [False, True])
def test_multiline_numeric_text_is_never_parsed_as_option(tmp_path, batch):
    api, execution, _ = asking(tmp_path, batch=batch)
    items = decisions(api, execution)
    text = " 2\n\nfirst\r\nsecond  "
    assert api.handle(command(items, texts=[text] * len(items)))["outcome"] == "recorded"
    assert all(questions_store.get(api.store, d["questionId"]).answer_text == text for d in items)


@pytest.mark.parametrize("batch", [False, True])
def test_worker_answers_share_wait_and_exact_continuation_claim(bound, batch):
    from cortex_runtime.delegation import mcp_tools
    from cortex_runtime.orchestration import worker_waits
    store, _, coordinator, workers = bound
    calls = []
    api = WorkspaceAPI(store, project_id="project-1", actor_id="local-owner", dispatch_answer=calls.append)
    api.activate(request_id="activate")
    task = api.handle(create_command())["target"]
    sessions.create_session(store, "incoming", str(store.project_root), {}, {}, [])
    sessions.bind_task(store, "incoming", coordinator.task_id)
    execution = store.immediate_write(lambda db: bind_execution_tx(db, task, coordinator.task_id, "incoming"))
    forms = [dict(question="Choose size?", options=["small", "large"]),
             dict(question="Choose format?", options=["JSON", "CSV"])]
    args = dict(questions=forms, request_key="ask") if batch else dict(**forms[0], request_key="ask")
    mcp_tools.dispatch(store, workers[0], "ask_question", args)
    items = decisions(api, execution)
    assert len(items) == (2 if batch else 1)
    receipt = api.handle(command(items))
    assert receipt["outcome"] == "recorded" and len(calls) == 1
    wait = worker_waits.list_waits(store, coordinator.task_id)[0]
    assert wait["status"] == "ready"
    claim = worker_waits.claim(store, coordinator.task_id, wait["assignment_ref"], wait["engine_session_ref"])
    assert claim and claim[0]["status"] == "resuming"
    assert worker_waits.claim(store, coordinator.task_id, wait["assignment_ref"], wait["engine_session_ref"]) is None


@pytest.mark.parametrize("kind", ["access", "capacity", "policy"])
def test_non_question_waits_never_offered_or_answered(bound, kind):
    from cortex_runtime.delegation import mcp_tools
    from cortex_runtime.orchestration import worker_waits
    from cortex_runtime.orchestration.state import event
    store, _, coordinator, workers = bound
    calls = []
    api = WorkspaceAPI(store, project_id="project-1", actor_id="local-owner", dispatch_answer=calls.append)
    api.activate(request_id="activate")
    task = api.handle(create_command())["target"]
    sessions.create_session(store, "incoming", str(store.project_root), {}, {}, [])
    sessions.bind_task(store, "incoming", coordinator.task_id)
    execution = store.immediate_write(lambda db: bind_execution_tx(db, task, coordinator.task_id, "incoming"))
    mcp_tools.dispatch(store, workers[0], "ask_question",
                       dict(question="Choose?", options=["one", "two"], request_key="ask"))
    items = decisions(api, execution)
    wait = worker_waits.list_waits(store, coordinator.task_id)[0]
    store.immediate_write(lambda db: event(db, coordinator.task_id, "WORKER_WAIT_CHANGED", dict(wait, kind=kind)))
    assert decisions(api, execution) == []
    with pytest.raises(WorkspaceError, match="unsupported"):
        api.handle(query("decisions.get", items[0]["decisionId"]))
    assert api.handle(command(items))["outcome"] == "unsupported"
    assert questions_store.get(store, items[0]["questionId"]).status == "open"
    assert not calls


def test_paging_versions_and_bound_scope(tmp_path):
    api, execution, _ = asking(tmp_path, batch=True)
    first = api.handle(query("decisions.list", execution, limit=1))
    validate("DecisionPage", first)
    second = api.handle(query("decisions.list", execution, limit=1, cursor=first["nextCursor"]))
    assert len(second["items"]) == 1 and second["nextCursor"] is None
    assert first["items"][0]["questionId"] != second["items"][0]["questionId"]
    with pytest.raises(WorkspaceError, match="invalid_cursor"):
        api.handle(query("decisions.list", execution, limit=2, cursor=first["nextCursor"]))
    with pytest.raises(WorkspaceError, match="invalid_cursor"):
        api.handle(query("decisions.list", execution, limit=1,
                         cursor=first["nextCursor"].rsplit(".", 1)[0] + ".9999999999999999999"))
    api.store.immediate_write(lambda db: db.execute("UPDATE task_orchestration SET requirement_revision=1"))
    with pytest.raises(WorkspaceError, match="invalid_cursor"):
        api.handle(query("decisions.list", execution, limit=1, cursor=first["nextCursor"]))
    with pytest.raises(WorkspaceError, match="not_found"):
        api.handle(query("decisions.list", "foreign-execution"))


def test_failed_handoff_is_explicit_and_never_replayed(tmp_path):
    calls = []
    def fail(value):
        calls.append(value)
        raise RuntimeError("handoff unavailable")
    api, execution, _ = asking(tmp_path, dispatch=fail)
    cmd = command(decisions(api, execution))
    with pytest.raises(WorkspaceError, match="answer_continuation_unconfirmed"):
        api.handle(cmd)
    receipt = api.handle(query("requests.get", "answer"))
    assert receipt["outcome"] == "recorded"
    assert api.handle(cmd) == receipt
    assert len(calls) == 1


@pytest.mark.parametrize("batch", [False, True])
def test_agent_answer_resumes_real_controller_with_fake_engine(tmp_path, monkeypatch, batch):
    import threading
    from cortex_runtime.acp.orchestrating_runtime import OrchestratingRuntime, PromptOutcome
    from cortex_runtime.delegation import mcp_tools
    from cortex_runtime.portable.engine_contract import StopReason
    from cortex_runtime.store import Store
    from orchestration_fakes import FakeSessionRuntime, FakeConfigSnapshotSource, FakeWorkspace
    from test_workspace_start import agent_for, start_command
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    turns = []
    text = "  exact\n\nmultiline\r\nanswer  "
    def router(store, channel, ref, launch, instructions):
        def behavior(store, channel, ref, message, index):
            turns.append(message)
            if len(turns) == 1:
                forms = [dict(question="Choose size?", options=["small", "large"]),
                         dict(question="Choose format?", options=["JSON", "CSV"])]
                args = dict(questions=forms, request_key="ask") if batch else dict(**forms[0], request_key="ask")
                mcp_tools.dispatch(store, channel, "user_question_request", args)
            else:
                with store.connection() as db:
                    rows = db.execute("SELECT answer_text FROM pending_questions WHERE task_id=?",
                                      (channel.task_id,)).fetchall()
                assert [r[0] for r in rows] == [text] * (2 if batch else 1)
                mcp_tools.dispatch(store, channel, "answer_submit", dict(text="Answered after question.", request_key="done"))
            return PromptOutcome(StopReason.END_TURN, "Not state evidence.")
        return behavior
    runtime = OrchestratingRuntime(store, FakeSessionRuntime(store, router), FakeWorkspace(),
                                   FakeConfigSnapshotSource(), allow_unqualified_delivery=True)
    try:
        agent, api = agent_for(tmp_path, monkeypatch, runtime)
        task = api.handle(create_command())["target"]
        started = agent._workspace_request(start_command(task))
        execution = started["executionId"]
        assert api.handle(query("executions.get", execution))["state"] == "waiting_user"
        done = threading.Event()
        original = agent._run_late_answer
        def run(*args, **kwargs):
            try:
                return original(*args, **kwargs)
            finally:
                done.set()
        monkeypatch.setattr(agent, "_run_late_answer", run)
        items = decisions(api, execution)
        cmd = command(items, texts=[text] * len(items))
        from cortex_runtime.acp.protocol import RpcError
        attached = copy.deepcopy(cmd)
        attached["payload"]["answers"][0]["attachments"] = [{"name": "image.png"}]
        with pytest.raises(RpcError, match="invalid_workspace_request"):
            agent._workspace_request(attached)
        first = agent._workspace_request(cmd)
        assert first["outcome"] == "recorded" and done.wait(10)
        assert len(turns) == 2
        assert api.handle(query("executions.get", execution))["state"] == "result"
        assert agent._workspace_request(cmd) == first
        assert len(turns) == 2
        with store.connection() as db:
            rows = db.execute("SELECT text FROM host_transcript WHERE message_id LIKE 'question-answer:%'").fetchall()
        assert [r[0] for r in rows] == [text] * len(items)
    finally:
        runtime.shutdown()


def test_recorded_answer_crossing_runner_exit_is_handed_off(tmp_path, monkeypatch):
    import threading
    from cortex_runtime.acp.agent import Agent
    from cortex_runtime.acp.runtime import UnavailableRuntime
    api, execution, saved = asking(tmp_path)
    agent = Agent(UnavailableRuntime(), environ={"HOME": str(tmp_path / "home")})
    agent.stores[str(tmp_path)] = api.store
    active = threading.Event()
    runtime_id = api.handle(query("executions.get", execution))["runtimeTaskId"]
    session = dict(project=str(tmp_path), task_id=runtime_id, lock=threading.Lock(), active=active)
    agent.sessions["incoming"] = session
    assert api.handle(command(decisions(api, execution)))["outcome"] == "recorded"
    question = questions_store.get(api.store, saved["question_id"])
    # The old runner has read its final waiting snapshot but has not released
    # session["active"]. It can no longer observe this newly recorded answer.
    agent._run_late_answer("incoming", session, question, question.answer_text, True)
    handed_off = threading.Event()
    monkeypatch.setattr(agent, "_run_late_answer", lambda *a: handed_off.set())
    agent._finish_prompt("incoming", session, active)
    assert handed_off.wait(3)


def test_active_agent_runner_observes_recorded_answer_without_second_runtime_turn(tmp_path, monkeypatch):
    import threading
    from cortex_runtime.acp.agent import Agent
    from cortex_runtime.acp.runtime import UnavailableRuntime
    api, execution, _ = asking(tmp_path)
    agent = Agent(UnavailableRuntime(), environ={"HOME": str(tmp_path / "home")})
    agent.stores[str(tmp_path)] = api.store
    active = threading.Event()
    session = dict(project=str(tmp_path), task_id=api.handle(query("executions.get", execution))["runtimeTaskId"],
                   lock=threading.Lock(), active=active)
    agent.sessions["incoming"] = session
    done = threading.Event()
    original = agent._run_late_answer
    def run(*args, **kwargs):
        try:
            return original(*args, **kwargs)
        finally:
            done.set()
    monkeypatch.setattr(agent, "_run_late_answer", run)
    api.dispatch_answer = agent._dispatch_workspace_answer
    cmd = command(decisions(api, execution))
    assert api.handle(cmd)["outcome"] == "recorded" and done.wait(5)
    assert session["active"] is active
    assert questions_store.get(api.store, cmd["payload"]["answers"][0]["questionId"]).status == "answered"
    assert resume_answered(api.store, session["task_id"])
    assert not resume_answered(api.store, session["task_id"])


def test_mid_batch_answer_failure_rolls_back_shared_runtime_and_public_facts(tmp_path, monkeypatch):
    from cortex_runtime.orchestration import worker_waits
    api, execution, _ = asking(tmp_path, batch=True)
    items = decisions(api, execution)
    original = worker_waits.answer_tx
    calls = []
    def fail_second(*args, **kwargs):
        calls.append(args[1])
        return original(*args, **kwargs) if len(calls) == 1 else False
    monkeypatch.setattr(worker_waits, "answer_tx", fail_second)
    assert api.handle(command(items))["outcome"] == "superseded"
    assert len(calls) == 2
    assert all(questions_store.get(api.store, d["questionId"]).status == "open" for d in items)
    with api.store.connection() as db:
        assert not db.execute("SELECT 1 FROM runtime_requests WHERE operation='question_batch_answer'").fetchone()
        assert not db.execute("SELECT 1 FROM task_events WHERE event_type='progress'").fetchone()


@pytest.mark.parametrize("count,size", [(101, 20), (5, 16200)])
def test_decision_pages_are_bounded_and_always_advance(tmp_path, count, size):
    api, execution, _ = asking(tmp_path)
    task_id = api.handle(query("executions.get", execution))["runtimeTaskId"]
    def write(db):
        db.execute("UPDATE pending_questions SET text=?", ("😀" * size,))
        for index in range(count - 1):
            db.execute("INSERT INTO pending_questions(id,task_id,dedup_key,text,status,created_at) "
                       "VALUES (?,?,?,?,'open','now')",
                       ("extra-" + str(index), task_id, "extra-" + str(index), "😀" * size))
    api.store.immediate_write(write)
    cursor, ids, seen = None, [], set()
    while True:
        page = api.handle(query("decisions.list", execution, cursor=cursor))
        validate("DecisionPage", page)
        assert page["nextCursor"] not in seen
        ids.extend(d["decisionId"] for d in page["items"])
        cursor = page["nextCursor"]
        if cursor is None:
            break
        seen.add(cursor)
    assert len(ids) == len(set(ids)) == count
