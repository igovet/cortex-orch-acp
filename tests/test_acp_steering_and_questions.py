"""T020: steering, urgent stop, question routing/elicitation fallback, and
language-preserving progress -- against in-process fakes only (no real
engine, no network, no paid model call; see tests/orchestration_fakes.py's
module docstring for the same "fake model calls mcp_tools.dispatch
directly" convention this file reuses).
"""
from __future__ import annotations

import threading
import uuid
import json
from pathlib import Path

import pytest

from cortex_runtime.acp import commands as acp_commands
from cortex_runtime.acp import messages as acp_messages
from cortex_runtime.acp import questions_store
from cortex_runtime.acp import replay_store as acp_replay_store
from cortex_runtime.acp import sessions as acp_sessions
from cortex_runtime.acp import steering as acp_steering
from cortex_runtime.acp.orchestrating_runtime import OrchestratingRuntime, PromptOutcome, _question_answer_value
from cortex_runtime.acp import pipeline_bootstrap
from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.delegation import events as delegation_events
from cortex_runtime.delegation import mcp_tools
from cortex_runtime.delegation.workspace_isolation import IsolatedSnapshotWorkspaceProvisioner
from cortex_runtime.portable.engine_contract import StopReason
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.store import Store

from orchestration_fakes import FakeConfigSnapshotSource, FakeSessionRuntime, FakeWorkspace


def _task(tmp_path, text="Build a page."):
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    result = bootstrap_task(store, SourceEvent(origin="cli", event_id=str(uuid.uuid4())), text, "owner-1")
    return store, result.task_id


def _make_runtime(store, router, **kwargs):
    engine = FakeSessionRuntime(store, router)
    runtime = OrchestratingRuntime(
        store, engine, IsolatedSnapshotWorkspaceProvisioner(store, str(store.project_root)), FakeConfigSnapshotSource(),
        allow_unqualified_delivery=True, wait_timeout_seconds=kwargs.pop("wait_timeout_seconds", 15.0),
        **kwargs,
    )
    # Acceptance tests need the real snapshot evidence available until the
    # gate runs; dedicated cleanup tests cover deletion separately.
    runtime._cleanup_isolated_workspace_if_integrated = lambda task_id, attempt_id: None
    return runtime, engine


def _host_session_for_card(store, task_id, session_id):
    acp_sessions.create_session(store, session_id, str(store.project_root), {}, {}, [])
    acp_sessions.bind_task(store, session_id, task_id)
    acp_replay_store.append_host_update(
        store, session_id,
        dict(sessionUpdate="tool_call", toolCallId="worker:card-a", status="in_progress"),
    )


def _publish(store, channel, label, call_index):
    report_id = "r_" + uuid.uuid4().hex[:12]
    filename = report_id + ".md"
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        task_dir = store._task_directory(db, channel.task_id)
        path = task_dir / filename
        path.write_text(f"# {label}\n", encoding="utf-8")
        info = path.stat()
        db.execute("INSERT INTO reports VALUES (?,?,?,?,?)", (report_id, channel.task_id, "report", filename, store._now()))
        db.execute(
            "INSERT INTO drafts(id,task_id,owner_thread_id,kind,template,path,created_at,device,inode,published_report_id,published_digest) "
            "VALUES (?,?,?,?,?,?,?,?,?,?,?)",
            ("d_" + uuid.uuid4().hex[:12], channel.task_id, channel.principal_thread_id, "report", "general",
             str(path), store._now(), info.st_dev, info.st_ino, report_id, "test-digest"),
        )
        db.commit()
    mcp_tools.dispatch(store, channel, "publish_result", dict(report_ref=report_id, summary=f"{label} finished", request_key=f"pub-{label}-{call_index}"))


def _publish_current_pipeline(store, channel):
    """Publish the coordinator's current plan before a task-level accept."""
    draft = store.call(
        "create_draft", dict(template="pipeline"),
        channel.principal_thread_id, channel.parent_principal_thread_id, trusted=True,
    )
    body = pipeline_bootstrap._fill_template(
        draft["markdown"], channel.task_id, "Test task", None,
    )
    Path(draft["draft_path"]).write_text(body, encoding="utf-8")
    return store.call(
        "write_report",
        dict(
            draft_id=draft["draft_id"], title="Current pipeline",
            summary="Coordinator-authored current pipeline edition",
            author="coordinator", request_key="test-current-pipeline-" + uuid.uuid4().hex,
        ),
        channel.principal_thread_id, channel.parent_principal_thread_id, trusted=True,
    )


class FakeElicitor:
    """Test-only HostElicitor: returns a scripted answer (or None for
    decline/unsupported/timeout)."""

    def __init__(self, answer=None):
        self.answer = answer
        self.calls = []

    def elicit(self, session_id, question, options):
        self.calls.append((session_id, question, options))
        return self.answer


# -- commands.py -------------------------------------------------------

def test_recognize_new_commands():
    assert acp_commands.recognize([dict(type="text", text="/cortex-stop")]) == (acp_commands.CORTEX_STOP, "")
    assert acp_commands.recognize([dict(type="text", text="/cortex-new-task Do the thing")]) == (acp_commands.CORTEX_NEW_TASK, "Do the thing")
    assert acp_commands.recognize([dict(type="text", text="/cortex-answer 2 Use staging")]) == (acp_commands.CORTEX_ANSWER, "2 Use staging")


def test_parse_answer_argument():
    assert acp_commands.parse_answer_argument("2 Use staging") == (2, "Use staging")
    assert acp_commands.parse_answer_argument("0 x") is None
    assert acp_commands.parse_answer_argument("not-a-number x") is None
    assert acp_commands.parse_answer_argument("2") is None


# -- messages.py ---------------------------------------------------------

def test_language_auto_detects_russian():
    assert acp_messages.select_language(None, ["Привет, помоги мне с задачей"]) == "ru"


def test_language_auto_defaults_english_for_latin_text():
    assert acp_messages.select_language(None, ["Please help with the task"]) == "en"


def test_language_explicit_override_wins_over_heuristic():
    assert acp_messages.select_language("en", ["Привет, помоги мне с задачей"]) == "en"


def test_question_options_label_is_localized():
    assert acp_messages.render("question_options_label", "en") == "Options:"
    assert acp_messages.render("question_options_label", "ru") == "Варианты ответа:"


def test_numeric_question_answer_resolves_only_valid_choice_index():
    question = questions_store.PendingQuestion(
        id="pq_test", task_id="task", assignment_ref=None, text="Pick?",
        options=["staging", "production"], status="open", answer_text=None,
        answer_source=None, created_at="now", answered_at=None,
    )
    assert _question_answer_value(question, "1") == "staging"
    assert _question_answer_value(question, "2") == "production"
    assert _question_answer_value(question, "0") == "0"
    assert _question_answer_value(question, "other") == "other"


def test_message_dedup_suppresses_identical_consecutive_signature():
    dedup = acp_messages.Dedup()
    assert dedup.should_emit("t1", "blocked_no_progress", {}) is True
    assert dedup.should_emit("t1", "blocked_no_progress", {}) is False
    assert dedup.should_emit("t1", "blocked_limit", dict(kind="max_tokens")) is True


# -- questions_store.py ---------------------------------------------------

def test_pending_question_create_is_idempotent_on_dedup_key(tmp_path):
    store, task_id = _task(tmp_path)
    a = questions_store.create_if_absent(store, task_id, "ask_user:1", "Which env?", options=["staging", "prod"])
    b = questions_store.create_if_absent(store, task_id, "ask_user:1", "Which env?", options=["staging", "prod"])
    assert a.id == b.id
    assert len(questions_store.list_open(store, task_id)) == 1


def test_pending_question_answer_then_late_answer_is_inert(tmp_path):
    store, task_id = _task(tmp_path)
    question = questions_store.create_if_absent(store, task_id, "ask_user:1", "Which env?")
    assert questions_store.answer(store, question.id, "staging", "text") is True
    # A second, late "answer" call for the same already-resolved question:
    assert questions_store.answer(store, question.id, "prod", "text") is False
    resolved = questions_store.get(store, question.id)
    assert resolved.answer_text == "staging"  # unchanged by the late call
    questions_store.record_late_answer(store, question.id, task_id, "prod", "text")


def test_pending_question_supersede_open(tmp_path):
    store, task_id = _task(tmp_path)
    questions_store.create_if_absent(store, task_id, "ask_user:1", "Q1")
    questions_store.create_if_absent(store, task_id, "ask_user:2", "Q2")
    assert questions_store.supersede_open(store, task_id, "task stopped") == 2
    assert questions_store.list_open(store, task_id) == []


# -- steering.py -----------------------------------------------------------

def test_requirement_revision_is_monotonic(tmp_path):
    store, task_id = _task(tmp_path)
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        db.execute("INSERT INTO task_orchestration (task_id, state, updated_at) VALUES (?,?,?)", (task_id, "running", "now"))
        db.commit()
    assert acp_steering.bump_requirement_revision(store, task_id) == 1
    assert acp_steering.bump_requirement_revision(store, task_id) == 2


def test_mark_stopped_is_idempotent(tmp_path):
    store, task_id = _task(tmp_path)
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        db.execute("INSERT INTO task_orchestration (task_id, state, updated_at) VALUES (?,?,?)", (task_id, "running", "now"))
        db.commit()
    assert acp_steering.mark_stopped(store, task_id) is True
    assert acp_steering.mark_stopped(store, task_id) is False
    assert acp_steering.is_stopped(store, task_id) is True


@pytest.mark.parametrize("client_cancel", [False, True])
def test_stop_retains_first_observed_transport_source(tmp_path, client_cancel):
    store, task_id = _task(tmp_path)
    with store.connection() as db:
        db.execute("INSERT INTO task_orchestration (task_id,state,updated_at) VALUES (?,?,?)",
                   (task_id, "running", "now"))
        db.commit()
    runtime, _engine = _make_runtime(store, lambda *_args: None)
    if client_cancel:
        assert runtime.cancel_task(task_id, "s1") is True
    else:
        assert runtime.stop_task(task_id, "s1", lambda _text: True) is True
    # A later stop through another path cannot rewrite the first observation.
    assert runtime.stop_task(task_id, "s2", lambda _text: True) is False
    with store.connection() as db:
        rows = db.execute(
            "SELECT payload_json FROM task_events WHERE task_id=? "
            "AND dedup_key='task_stop_observed'", (task_id,),
        ).fetchall()
        state = db.execute(
            "SELECT wait_detail_json FROM task_orchestration WHERE task_id=?", (task_id,),
        ).fetchone()
    source = "paseo_session_cancel" if client_cancel else "cortex_stop_command"
    assert len(rows) == 1
    assert json.loads(rows[0]["payload_json"]) == dict(
        kind="task_stop_observed", source=source, session_id="s1",
    )
    detail = json.loads(state["wait_detail_json"])
    assert detail["source"] == source
    assert ("/cortex-stop" in detail["explanation"]) is (not client_cancel)


def test_stop_marker_and_source_commit_atomically(tmp_path, monkeypatch):
    store, task_id = _task(tmp_path)
    def fail(*_args, **_kwargs):
        raise RuntimeError("provenance insert failed")
    monkeypatch.setattr(delegation_events, "append_event_tx", fail)
    with pytest.raises(RuntimeError, match="provenance insert failed"):
        acp_steering.mark_stopped(store, task_id, session_id="s1")
    assert not acp_steering.is_stopped(store, task_id)


def test_stop_closes_all_task_bound_acp_host_sessions(tmp_path):
    """A terminal task stop closes current and reconnect-bound host rows."""
    store, task_id = _task(tmp_path)
    for session_id in ("s1", "s2"):
        acp_sessions.create_session(store, session_id, str(store.project_root), {}, {}, [])
        acp_sessions.bind_task(store, session_id, task_id)

    runtime, _engine = _make_runtime(store, lambda *_args: None)
    assert runtime.stop_task(task_id, "s2", lambda _text: True) is True

    with store.connection() as db:
        rows = db.execute(
            "SELECT session_id, closed_at FROM acp_host_sessions "
            "WHERE task_id=? ORDER BY session_id",
            (task_id,),
        ).fetchall()
    assert [(row["session_id"], row["closed_at"] is not None) for row in rows] == [
        ("s1", True), ("s2", True),
    ]


# -- OrchestratingRuntime integration ---------------------------------------

def test_stop_terminates_terminal_worker_still_owned_by_task(tmp_path):
    """A terminal event can be recorded before its engine exits."""
    store, task_id = _task(tmp_path)
    other = bootstrap_task(
        store, SourceEvent(origin="cli", event_id=str(uuid.uuid4())),
        "Other task.", "owner-1",
    ).task_id
    runtime, engine = _make_runtime(store, lambda *_args: None)
    attempts = ((task_id, "assignment-1", "xa_still_running"),
                (other, "assignment-2", "xa_other_task"))
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        for owner, assignment_ref, attempt_id in attempts:
            db.execute(
                "INSERT INTO delegation_provisioning "
                "(idempotency_key,task_id,attempt_id,role_id,step,status,owner_epoch,created_at,updated_at) "
                "VALUES (?,?,?,?,?,?,?,?,?)",
                (assignment_ref, owner, attempt_id, "backend-dev", "complete", "complete", 1, "now", "now"),
            )
            engine._owned[attempt_id] = object()
        db.commit()
    delegation_events.append_event(
        store, task_id, "delegation_requested", "request-1", {},
        origin="coordinator", assignment_ref="assignment-1",
    )
    delegation_events.append_event(
        store, task_id, "assignment_cancelled", "cancel-1", {},
        origin="runtime", assignment_ref="assignment-1",
    )

    assert runtime.stop_task(task_id, "s1", lambda _text: True) is True
    assert engine.lookup_owned("xa_still_running") == "absent"
    assert "xa_still_running" in engine.terminate_calls
    assert engine.lookup_owned("xa_other_task") == "alive"
    assert "xa_other_task" not in engine.terminate_calls


def test_steering_persists_before_delivery_and_targets_same_task(tmp_path):
    store, task_id = _task(tmp_path)
    seen_messages = []

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        seen_messages.append(message)
        return PromptOutcome(StopReason.END_TURN, f"turn {call_index}")

    def router(store, channel, session_ref, launch, instructions):
        return coordinator_behavior if channel and channel.role == "coordinator" else None

    runtime, engine = _make_runtime(store, router)
    result1 = runtime.start_prompt(task_id, "s1", "Build it.", lambda t: True, threading.Event())
    assert result1 == StopReason.END_TURN

    result2 = runtime.start_prompt(task_id, "s1", "Actually, use the staging DB.", lambda t: True, threading.Event())
    assert result2 == StopReason.END_TURN

    # Persisted as a user_message on this exact task (never a new task).
    with store.connection() as db:
        rows = db.execute(
            "SELECT event_type, payload_json FROM task_events WHERE task_id=? ORDER BY id", (task_id,),
        ).fetchall()
    steering_rows = [r for r in rows if r["event_type"] == "progress" and '"kind": "user_steering"' in r["payload_json"]]
    assert len(steering_rows) == 1
    import json
    payload = json.loads(steering_rows[0]["payload_json"])
    assert payload["requirement_revision"] == 1
    assert "staging DB" in payload["text"]
    assert "Actually, use the staging DB." in seen_messages[3]
















def test_answered_question_race_continues_but_cancelled_question_does_not(tmp_path):
    store, task_id = _task(tmp_path)
    runtime, _engine = _make_runtime(store, lambda *_args: None)

    answered = questions_store.create_if_absent(store, task_id, "ask_user:answered", "Answered?")
    assert questions_store.answer(store, answered.id, "yes", "text") is True
    decision = {"question_id": answered.id, "question": "Answered?"}
    assert runtime._handle_ask_user(task_id, "s1", lambda _text: True, "en", decision, None) is True

    cancelled = questions_store.create_if_absent(store, task_id, "ask_user:cancelled", "Cancelled?")
    questions_store.supersede_open(store, task_id, "test")
    decision = {"question_id": cancelled.id, "question": "Cancelled?"}
    assert runtime._handle_ask_user(task_id, "s1", lambda _text: True, "en", decision, None) is False

    callback_question = questions_store.create_if_absent(store, task_id, "ask_user:callback", "Callback?")
    callback_decision = {"question_id": callback_question.id, "question": "Callback?"}

    def answer_during_emit(_text):
        assert questions_store.answer(store, callback_question.id, "during emit", "text") is True
        return True

    assert runtime._handle_ask_user(task_id, "s1", answer_during_emit, "en", callback_decision, None) is True


def test_losing_single_question_answer_is_recorded_late_and_not_delivered(tmp_path):
    store, task_id = _task(tmp_path)
    question = questions_store.create_if_absent(store, task_id, "ask_user:race", "Which environment?", options=["staging", "production"])
    from cortex_runtime.acp import replay_store
    first, won = replay_store.record_user_message_answer_event(
        store, task_id, "user:first", "s1", {"content": "1"},
        question_id=question.id, answer_text="staging",
    )
    assert won is True
    retry, retry_won = replay_store.record_user_message_answer_event(
        store, task_id, "user:first", "s1", {"content": "1"},
        question_id=question.id, answer_text="staging",
    )
    assert retry.id == first.id
    assert retry_won is True
    second, won = replay_store.record_user_message_answer_event(
        store, task_id, "user:second", "s2", {"content": "1"},
        question_id=question.id, answer_text="staging",
    )
    assert won is False
    losing_retry, losing_retry_won = replay_store.record_user_message_answer_event(
        store, task_id, "user:second", "s2", {"content": "1"},
        question_id=question.id, answer_text="staging",
    )
    assert losing_retry.id == second.id
    assert losing_retry_won is False
    with store.connection() as db:
        row = db.execute(
            "SELECT consumed_by, superseded FROM task_events "
            "WHERE task_id=? AND event_type='user_message' ORDER BY id DESC LIMIT 1",
            (task_id,),
        ).fetchone()
    assert row["consumed_by"] is None
    assert row["superseded"] == 1




def test_stale_continuation_state_write_cannot_resurrect_stopped_task(tmp_path):
    """A continuation that passed its cancel check must not overwrite stop.

    This is the deterministic form of the race where the loop has already
    decided to publish a transient ``blocked`` state while ``/cortex-stop``
    commits.  The state guard belongs to the same SQLite transaction as the
    write, so the late continuation write is inert.
    """
    store, task_id = _task(tmp_path)

    def router(store, channel, session_ref, launch, instructions):
        return lambda *args: PromptOutcome(StopReason.END_TURN, "")

    runtime, _engine = _make_runtime(store, router)
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        db.execute(
            "INSERT INTO task_orchestration (task_id, state, updated_at) VALUES (?,?,?)",
            (task_id, "running", "now"),
        )
        db.commit()
    assert runtime.stop_task(task_id, "s1", lambda _text: True) is True

    # Simulate the stale continuation after the stop transaction has won.
    runtime._set_state(task_id, "blocked", wait_reason="workers_running")

    with store.connection() as db:
        row = db.execute(
            "SELECT state FROM task_orchestration WHERE task_id=?", (task_id,),
        ).fetchone()
    assert row["state"] == "stopped"


def test_late_answer_after_stop_is_recorded_and_inert(tmp_path):
    store, task_id = _task(tmp_path)
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        db.execute("INSERT INTO task_orchestration (task_id, state, updated_at) VALUES (?,?,?)", (task_id, "waiting_for_user", "now"))
        db.commit()

    def router(store, channel, session_ref, launch, instructions):
        return None

    runtime, engine = _make_runtime(store, router)
    question = questions_store.create_if_absent(store, task_id, "ask_user:1", "Which env?")
    acp_steering.mark_stopped(store, task_id)

    calls_before = len(engine.prompt_calls)
    updates = []
    result = runtime.answer_question(task_id, "s1", question, "staging", updates.append, threading.Event())
    assert result == StopReason.END_TURN
    assert len(engine.prompt_calls) == calls_before  # no coordinator turn spent
    assert questions_store.get(store, question.id).status == "open"  # never resolved by the late answer
    assert any("stopped" in u.lower() for u in updates)


def _delegate_then_silent_worker_router(task_id):
    phase = {"delegated": False}

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        if not phase["delegated"]:
            phase["delegated"] = True
            mcp_tools.dispatch(store, channel, "delegate", dict(title="W", brief="do W (English worker report)", role="backend-dev", acceptance="W done", request_key="reqW"))
            return PromptOutcome(StopReason.END_TURN, "delegated W")
        return PromptOutcome(StopReason.END_TURN, "still waiting")

    def silent_worker(store, channel, session_ref, message, call_index):
        return PromptOutcome(StopReason.END_TURN, "This is an English worker report chunk, never shown to the user.")

    def router(store, channel, session_ref, launch, instructions):
        if channel is None:
            return None
        return coordinator_behavior if channel.role == "coordinator" else silent_worker

    return router


def test_russian_conversation_gets_russian_system_lines_and_no_worker_leak(tmp_path):
    store, task_id = _task(tmp_path, text="Сделай мне страницу")
    router = lambda *_: lambda *_: PromptOutcome(StopReason.END_TURN, "")
    runtime, engine = _make_runtime(store, router, wait_timeout_seconds=0.2, max_no_progress_continuations=1)
    updates = []
    result = runtime.start_prompt(task_id, "s1", "Продолжай, пожалуйста", updates.append, threading.Event())
    assert result == StopReason.END_TURN
    # Whatever Cortex says for itself, it says in the user's language. The
    # line this produces is now the one about an assignment that reached
    # no worker -- the worker ends its turn without publishing, which
    # closes its assignment (it used to exhaust the coordinator's patience
    # instead, and the line was the blocked one).
    assert any("Выполнение заблокировано" in u for u in updates), updates
    assert not any("was NOT delegated" in u for u in updates), updates
    assert not any(u.startswith("Blocked") for u in updates)
    assert not any("English worker report" in u for u in updates)


def test_explicit_language_override_wins_for_system_lines(tmp_path):
    """A white-box check of the exact selection mechanism ``_run`` uses for
    every Cortex system line (see ``_resolve_language``): without an
    override, Russian source text auto-selects "ru" (exercised end-to-end
    by ``test_russian_conversation_gets_russian_system_lines_and_no_worker_leak``
    above); an explicit per-task override always wins over the heuristic,
    regardless of source text language."""
    store, task_id = _task(tmp_path, text="Сделай мне страницу")
    runtime, engine = _make_runtime(store, lambda *a, **k: None)
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        db.execute(
            "INSERT INTO task_orchestration (task_id, state, updated_at, requirement_revision) VALUES (?,?,?,0)",
            (task_id, "running", "now"),
        )
        db.commit()
    delegation_events.append_event(
        store, task_id, "user_message", "user_message:s1:0",
        dict(content="Сделай мне страницу", session_id="s1"), origin="s1",
    )
    assert runtime._resolve_language(task_id) == "ru"
    acp_steering.set_language_override(store, task_id, "en")
    assert runtime._resolve_language(task_id) == "en"




def test_reconnected_same_session_gets_immediate_terminal_worker_card(tmp_path):
    store, task_id = _task(tmp_path)
    runtime, _engine = _make_runtime(store, lambda *args: None)
    _host_session_for_card(store, task_id, "same")
    updates = []

    def emit(update):
        updates.append(update)
        acp_replay_store.append_host_update(
            store, "same",
            dict(sessionUpdate="tool_call_update", toolCallId=update["tool_call_id"], status=update["status"]),
        )
        return True

    runtime._cancel_assignment(
        task_id, "card-a", "reconnected stop", session_id="same", emit_tool_call=emit,
    )

    assert updates and updates[-1]["status"] == "failed"
    with store.connection() as db:
        rows = db.execute(
            "SELECT update_json FROM host_updates WHERE session_id=? ORDER BY seq", ("same",),
        ).fetchall()
    assert '"status":"failed"' in rows[-1]["update_json"]


def test_reconnected_different_session_does_not_receive_worker_card_update(tmp_path):
    store, task_id = _task(tmp_path)
    runtime, _engine = _make_runtime(store, lambda *args: None)
    _host_session_for_card(store, task_id, "original")
    acp_sessions.create_session(store, "current", str(store.project_root), {}, {}, [])
    acp_sessions.bind_task(store, "current", task_id)
    leaked = []

    runtime._cancel_assignment(
        task_id, "card-a", "reconnected stop", session_id="current", emit_tool_call=leaked.append,
    )

    assert leaked == []
    with store.connection() as db:
        original = db.execute(
            "SELECT update_json FROM host_updates WHERE session_id=? ORDER BY seq DESC LIMIT 1", ("original",),
        ).fetchone()
        current = db.execute(
            "SELECT COUNT(*) FROM host_updates WHERE session_id=?", ("current",),
        ).fetchone()[0]
    assert '"status":"failed"' in original["update_json"]
    assert current == 0


# -- language config option / /cortex-language command (agent-level) ------

def _agent_for_language_tests(tmp_path, monkeypatch):
    import threading as _threading

    from cortex_runtime.acp.agent import Agent
    from test_acp_config_options import _isolate, _NoopRuntime, _RecordingTransport

    environ = _isolate(tmp_path, monkeypatch)
    agent = Agent(_NoopRuntime(), environ=environ)
    transport = _RecordingTransport()
    agent.bind_transport(transport)
    agent.initialized = True
    agent.client_capabilities = {}
    return agent, transport, environ


def test_language_option_present_pre_task_and_settable_via_set_config_option(tmp_path, monkeypatch):
    agent, transport, _environ = _agent_for_language_tests(tmp_path, monkeypatch)
    project = tmp_path / "project"
    project.mkdir()
    result = agent._session_new(dict(cwd=str(project), mcpServers=[]))
    session_id = result["sessionId"]
    ids = {opt["id"] for opt in result["configOptions"]}
    assert ids == {"language"}
    assert result["configOptions"][0]["currentValue"] == "auto"

    agent._session_set_config_option(1, dict(sessionId=session_id, configId="language", value="ru"))
    response = next(message for message in transport.responses if message.get("id") == 1)
    assert "result" in response, response
    language_option = next(o for o in response["result"]["configOptions"] if o["id"] == "language")
    assert language_option["currentValue"] == "ru"
    # Persisted as this session's own pending pre-task choice, and as the
    # T008-style user default (survives a fresh session/new on this HOME).
    result2 = agent._session_new(dict(cwd=str(project), mcpServers=[]))
    language_option2 = next(o for o in result2["configOptions"] if o["id"] == "language")
    assert language_option2["currentValue"] == "ru"


def test_language_option_mid_task_persists_on_task_orchestration(tmp_path, monkeypatch):
    from test_acp_config_options import _bootstrap_task_for_session

    agent, transport, _environ = _agent_for_language_tests(tmp_path, monkeypatch)
    project = tmp_path / "project"
    project.mkdir()
    result = agent._session_new(dict(cwd=str(project), mcpServers=[]))
    session_id = result["sessionId"]
    store, task_id = _bootstrap_task_for_session(agent, session_id, tmp_path)

    agent._session_set_config_option(7, dict(sessionId=session_id, configId="language", value="en"))
    response = next(message for message in transport.responses if message.get("id") == 7)
    assert "result" in response, response
    assert acp_steering.get_language_override(store, task_id) == "en"

    # Invalid value is rejected (never a silent guess).
    agent._session_set_config_option(8, dict(sessionId=session_id, configId="language", value="fr"))
    error_response = next(message for message in transport.responses if message.get("id") == 8)
    assert "error" in error_response
    assert acp_steering.get_language_override(store, task_id) == "en"  # unchanged


def test_cortex_language_command_recognized_and_applied(tmp_path, monkeypatch):
    agent, transport, _environ = _agent_for_language_tests(tmp_path, monkeypatch)
    project = tmp_path / "project"
    project.mkdir()
    result = agent._session_new(dict(cwd=str(project), mcpServers=[]))
    session_id = result["sessionId"]
    session = agent.sessions[session_id]
    cancel_event = threading.Event()
    recognized = acp_commands.recognize([dict(type="text", text="/cortex-language ru")])
    assert recognized == (acp_commands.CORTEX_LANGUAGE, "ru")
    agent._run_command_body(2, session_id, session, recognized, cancel_event)
    response = next(message for message in transport.responses if message.get("id") == 2)
    assert "result" in response, response
    assert session.get("pending_language") == "ru"


# -- deterministic regression for the claim/stop supersede race (lead fix) --

def test_stop_supersedes_steering_even_when_already_claimed_by_an_in_flight_loop(tmp_path):
    """Deterministic reproduction of the reported race: force the exact
    interleaving (claim the steering row's continuation *before* calling
    stop_task -- simulating an in-flight loop that claimed it a moment
    before the stop signal landed) rather than relying on real thread
    timing. mark_stopped's own atomic step (delivered_at IS NULL,
    regardless of consumed_by) must still supersede it; a subsequent
    release_claim (the loop's own post-stop cleanup) must never revive it,
    and a later claim_unconsumed must never reclaim it."""
    store, task_id = _task(tmp_path)
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        db.execute(
            "INSERT INTO task_orchestration (task_id, state, updated_at, requirement_revision) VALUES (?,?,?,0)",
            (task_id, "running", "now"),
        )
        db.commit()
    acp_steering.record_steering(store, task_id, "s2", "race-hook", "please also do more")
    with store.connection() as db:
        event_id = db.execute(
            "SELECT id FROM task_events WHERE task_id=? AND dedup_key='user_steering:s2:race-hook'", (task_id,),
        ).fetchone()["id"]

    # The hook: an in-flight loop already claimed this exact row (consumed_by
    # set) an instant before the stop signal is observed.
    claimed = delegation_events.claim_unconsumed(store, task_id, "cont_inflight")
    assert any(event.id == event_id for event in claimed)
    with store.connection() as db:
        pre = db.execute("SELECT consumed_by, superseded, delivered_at FROM task_events WHERE id=?", (event_id,)).fetchone()
    assert pre["consumed_by"] == "cont_inflight" and pre["superseded"] == 0 and pre["delivered_at"] is None

    def router(store, channel, session_ref, launch, instructions):
        return None

    runtime, engine = _make_runtime(store, router)
    assert acp_steering.mark_stopped(store, task_id) is True

    with store.connection() as db:
        after_stop = db.execute("SELECT consumed_by, superseded FROM task_events WHERE id=?", (event_id,)).fetchone()
    assert after_stop["superseded"] == 1  # superseded despite still being claimed

    # The loop's own post-stop cleanup releasing its now-stale claim must
    # never un-supersede it.
    delegation_events.release_claim(store, task_id, "cont_inflight")
    with store.connection() as db:
        after_release = db.execute("SELECT consumed_by, superseded FROM task_events WHERE id=?", (event_id,)).fetchone()
    assert after_release["superseded"] == 1

    # A later start_prompt/answer on this stopped task must never reclaim it.
    later_claim = delegation_events.claim_unconsumed(store, task_id, "cont_later")
    assert not any(event.id == event_id for event in later_claim)


# -- fresh-review HIGH fix: stop-before/racing-first-provisioning (task_stopped) --

def test_stop_before_any_coordinator_provisioning_is_never_lost(tmp_path):
    """bootstrap_task (a real task exists) -> stop_task *before* any
    start_prompt call ever ran -> is_stopped() True -> a subsequent
    start_prompt on that task never opens a coordinator/engine session at
    all and replies with the stopped explanation (see the previous bug:
    stopped_at lived on the lazily-created task_orchestration row, so a
    stop this early updated zero rows, returned True unconditionally, and
    was silently lost the moment the coordinator was later provisioned)."""
    store, task_id = _task(tmp_path)

    def router(store, channel, session_ref, launch, instructions):
        raise AssertionError("no engine session should ever be opened for a stopped task")

    runtime, engine = _make_runtime(store, router)

    assert acp_steering.is_stopped(store, task_id) is False
    stopped = runtime.stop_task(task_id, "s1", lambda t: True)
    assert stopped is True
    assert acp_steering.is_stopped(store, task_id) is True

    updates = []
    result = runtime.start_prompt(task_id, "s1", "Go.", updates.append, threading.Event())
    assert result == StopReason.END_TURN
    assert any("stopped" in u.lower() for u in updates)
    assert engine.open_calls == []  # never opened a coordinator session
    assert task_id not in runtime._coordinator_sessions
    with store.connection() as db:
        row = db.execute("SELECT 1 FROM task_orchestration WHERE task_id=?", (task_id,)).fetchone()
    assert row is None  # never provisioned at all


def test_stop_unknown_task_id_is_an_error_never_a_false_confirmation(tmp_path):
    store, task_id = _task(tmp_path)

    def router(store, channel, session_ref, launch, instructions):
        return None

    runtime, engine = _make_runtime(store, router)
    from cortex_runtime.portable.errors import PortableError

    with pytest.raises(PortableError) as excinfo:
        runtime.stop_task("t_does_not_exist", "s1", lambda t: True)
    assert excinfo.value.code == "unknown_task"
    assert acp_steering.is_stopped(store, "t_does_not_exist") is False


def test_terminal_client_cancellation_releases_coordinator_engine(tmp_path):
    """A client stop that returns a terminal CANCELLED outcome must not
    leave the coordinator process/reservation alive after the task state is
    persisted as cancelled."""
    store, task_id = _task(tmp_path)

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        return PromptOutcome(StopReason.CANCELLED, "cancelled")

    def router(store, channel, session_ref, launch, instructions):
        return coordinator_behavior if channel and channel.role == "coordinator" else None

    runtime, engine = _make_runtime(store, router)
    result = runtime.start_prompt(task_id, "s1", "Stop this.", lambda _text: True, threading.Event())
    assert result == StopReason.CANCELLED

    with store.connection() as db:
        row = db.execute(
            "SELECT state, coordinator_attempt_ref FROM task_orchestration WHERE task_id=?",
            (task_id,),
        ).fetchone()
    assert row["state"] == "recovering"
    assert row["coordinator_attempt_ref"] in engine.terminate_calls
    assert row["coordinator_attempt_ref"] not in engine._owned


def test_stop_races_the_first_start_prompt_between_bootstrap_and_provisioning(tmp_path, monkeypatch):
    """Deterministic hook forcing the exact interleaving: a concurrent
    /cortex-stop lands *inside* this exact start_prompt call, between the
    initial is_stopped() gate and the coordinator actually being
    provisioned/prompted -- simulating another thread's stop_task() call
    landing in that narrow window. No delegation/delivery must occur."""
    store, task_id = _task(tmp_path)
    prompted = {"coordinator": False}

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        prompted["coordinator"] = True
        return PromptOutcome(StopReason.END_TURN, "should never run")

    def router(store, channel, session_ref, launch, instructions):
        return coordinator_behavior if channel and channel.role == "coordinator" else None

    runtime, engine = _make_runtime(store, router)

    real_ensure_coordinator = runtime._ensure_coordinator

    def hooked_ensure_coordinator(task_id_arg):
        # The hook: a concurrent stop_task() call lands here, after this
        # start_prompt call's own initial is_stopped() check already
        # passed, but before the coordinator is actually provisioned.
        runtime.stop_task(task_id_arg, "s2", lambda t: True)
        return real_ensure_coordinator(task_id_arg)

    monkeypatch.setattr(runtime, "_ensure_coordinator", hooked_ensure_coordinator)

    updates = []
    result = runtime.start_prompt(task_id, "s1", "Go.", updates.append, threading.Event())
    assert result == StopReason.CANCELLED
    assert prompted["coordinator"] is False  # never actually delivered/prompted
    with store.connection() as db:
        delegated = db.execute(
            "SELECT 1 FROM task_events WHERE task_id=? AND event_type='delegation_requested'", (task_id,),
        ).fetchone()
    assert delegated is None
    assert acp_steering.is_stopped(store, task_id) is True




def test_the_language_chosen_before_a_task_reaches_the_task(tmp_path, monkeypatch):
    """The coordinator is told to answer in a language only through the
    *task's* own override. A choice made before any task exists lives on
    the session and in the user's defaults, and nothing carried it across:
    a brand-new thread had no override at all.

    The first fix read it one line too late -- with the new task_id already
    on the session, the same lookup answers with that task's own empty
    override instead of what the session is carrying. Live, the composer
    read "Russian" and every reply still came back in English.
    """
    from cortex_runtime.acp import steering as steering_module
    from cortex_runtime.acp.agent import Agent

    agent = Agent.__new__(Agent)
    agent.environ = {"HOME": str(tmp_path)}
    store, task_id = _task(tmp_path)

    # What the session carries before any task: the user's own default.
    session = {"task_id": None, "pending_language": "ru"}
    assert agent._current_language_for_session(store, session) == "ru"

    # Read once the task id is on the session, and the answer is the new
    # task's own override -- which is exactly nothing.
    session_with_task = {"task_id": task_id, "pending_language": "ru"}
    assert agent._current_language_for_session(store, session_with_task) is None

    # So the value has to be taken before, and then recorded on the task.
    steering_module.set_language_override(store, task_id, "ru")
    assert steering_module.get_language_override(store, task_id) == "ru"


@pytest.mark.parametrize('action',['cancel','stop','steer'])
def test_runtime_worker_in_flight_is_fenced_before_new_work(tmp_path,action):
    from cortex_runtime.orchestration.state import TaskMachine
    from cortex_runtime.contracts import StoreError
    from runtime_scenarios import propose_inspection,publish_result
    store,task_id=_task(tmp_path)
    entered=threading.Event();release=threading.Event();worker_channels=[]
    def router(store,channel,*_):
        def behavior(store,channel,ref,message,index):
            if channel.role=='coordinator':
                context=mcp_tools.dispatch(store,channel,'task_get_context',{})
                if context['state']=='PLANNING': propose_inspection(store,channel)
                else: mcp_tools.dispatch(store,channel,'synthesis_submit',dict(text='Reviewed and verified'))
                return PromptOutcome(StopReason.END_TURN,'Coordinator text')
            if not worker_channels:
                worker_channels.append(channel);entered.set();release.wait(5)
                # Revoked channel must never publish after cancel/steering.
                try: return publish_result(store,channel)
                except StoreError: return PromptOutcome(StopReason.CANCELLED,'Interrupted')
            return publish_result(store,channel)
        return behavior
    runtime,engine=_make_runtime(store,router)
    cancelled=threading.Event();outcomes=[]
    thread=threading.Thread(target=lambda:outcomes.append(runtime.start_prompt(task_id,'s1','Inspect',lambda _:True,cancelled)))
    thread.start()
    try:
        assert entered.wait(5)
        with pytest.raises(Exception,match='second_coordinator'):
            runtime.start_prompt(task_id,'s2','Concurrent',lambda _:True,threading.Event())
        if action=='stop': runtime.stop_task(task_id,'s1',lambda _:True)
        elif action=='cancel': runtime.cancel('s1')
        else: acp_steering.record_steering(store,task_id,'s2','steer','Не меняй database')
        if action=='steer':
            context=TaskMachine(store).context(task_id)
            assert context['state']=='RECOVERING'
            assert any('database' in p for p in context['policy']['deny_write'])
            with pytest.raises(StoreError): mcp_tools.dispatch(store,worker_channels[0],'project_read',dict(path='.'))
        release.set();thread.join(10)
        assert not thread.is_alive()
        state=TaskMachine(store).context(task_id)['state']
        assert state=={'cancel':'RECOVERING','stop':'CANCELLED','steer':'COMPLETED'}[action]
        if action!='steer': assert outcomes==[StopReason.CANCELLED]
    finally:
        release.set();cancelled.set();thread.join(10);runtime.shutdown()
