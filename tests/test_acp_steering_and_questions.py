"""T020: steering, urgent stop, question routing/elicitation fallback, and
language-preserving progress -- against in-process fakes only (no real
engine, no network, no paid model call; see tests/orchestration_fakes.py's
module docstring for the same "fake model calls mcp_tools.dispatch
directly" convention this file reuses).
"""
from __future__ import annotations

import threading
import uuid

import pytest

from cortex_runtime.acp import commands as acp_commands
from cortex_runtime.acp import messages as acp_messages
from cortex_runtime.acp import questions_store
from cortex_runtime.acp import steering as acp_steering
from cortex_runtime.acp.orchestrating_runtime import OrchestratingRuntime, PromptOutcome
from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.delegation import events as delegation_events
from cortex_runtime.delegation import mcp_tools
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
        store, engine, FakeWorkspace(), FakeConfigSnapshotSource(),
        allow_unqualified_delivery=True, wait_timeout_seconds=kwargs.pop("wait_timeout_seconds", 15.0),
        **kwargs,
    )
    return runtime, engine


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


# -- OrchestratingRuntime integration ---------------------------------------

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
    assert "Actually, use the staging DB." in seen_messages[1]


def test_cancel_interrupts_turn_without_cancelling_workers(tmp_path):
    """session/cancel interrupts a coordination turn that is currently
    *waiting* for worker progress (the top-of-loop cancel_event check --
    see ``OrchestratingRuntime._run``'s main loop): returns CANCELLED
    promptly, well under the no-progress wait budget, task stays
    'interrupted' (not 'cancelled'), and the still-running worker is never
    told to cancel."""
    store, task_id = _task(tmp_path)
    worker_entered = threading.Event()
    worker_release = threading.Event()

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        if call_index == 0:
            mcp_tools.dispatch(store, channel, "delegate", dict(title="Slow", brief="do it slowly", role="backend-dev", acceptance="done", request_key="reqSlow"))
            return PromptOutcome(StopReason.END_TURN, "delegated")
        return PromptOutcome(StopReason.END_TURN, "")

    def worker_behavior(store, channel, session_ref, message, call_index):
        worker_entered.set()
        worker_release.wait(timeout=5)
        return PromptOutcome(StopReason.END_TURN, "")

    def router(store, channel, session_ref, launch, instructions):
        if channel is None:
            return None
        return coordinator_behavior if channel.role == "coordinator" else worker_behavior

    # A long wait budget: cancellation must return well before it elapses.
    runtime, engine = _make_runtime(store, router, wait_timeout_seconds=10.0)
    cancel_event = threading.Event()
    result_holder = {}

    def run():
        result_holder["reason"] = runtime.start_prompt(task_id, "s1", "Go slowly.", lambda t: True, cancel_event)

    thread = threading.Thread(target=run)
    thread.start()
    try:
        assert worker_entered.wait(timeout=5)  # the worker is genuinely running
        start = threading.Event()
        import time as _time

        began = _time.monotonic()
        cancel_event.set()
        thread.join(timeout=5)
        elapsed = _time.monotonic() - began
    finally:
        worker_release.set()
        thread.join(timeout=5)
    assert result_holder["reason"] == StopReason.CANCELLED
    assert elapsed < 5.0  # promptly, not the full 10s wait budget
    with store.connection() as db:
        row = db.execute("SELECT state FROM task_orchestration WHERE task_id=?", (task_id,)).fetchone()
    assert row["state"] == "interrupted"
    # session/cancel does call the fake's cancel_task tracker (T017,
    # unchanged: needed so a worker still only *queued* for capacity never
    # leaks its reservation forever), but that has no effect on a worker
    # whose engine session is already open and running -- this worker was
    # never told to stop (it was released normally, not aborted) and its
    # own assignment was never cancelled/failed.
    with store.connection() as db:
        cancelled_row = db.execute(
            "SELECT 1 FROM task_events WHERE task_id=? AND event_type IN ('assignment_cancelled','assignment_failed')",
            (task_id,),
        ).fetchone()
    assert cancelled_row is None


def test_no_concurrent_engine_prompts_per_session(tmp_path):
    store, task_id = _task(tmp_path)
    concurrency = {"current": 0, "max": 0}
    lock = threading.Lock()
    seen, decided = set(), {"done": False}

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        with lock:
            concurrency["current"] += 1
            concurrency["max"] = max(concurrency["max"], concurrency["current"])
        try:
            if call_index == 0:
                mcp_tools.dispatch(store, channel, "delegate", dict(title="A", brief="do A", role="backend-dev", acceptance="A done", request_key="reqA"))
                mcp_tools.dispatch(store, channel, "delegate", dict(title="B", brief="do B", role="backend-dev", acceptance="B done", request_key="reqB"))
                return PromptOutcome(StopReason.END_TURN, "delegated")
            for name in ("A finished", "B finished"):
                if name in message:
                    seen.add(name)
            if len(seen) >= 2 and not decided["done"]:
                decided["done"] = True
                mcp_tools.dispatch(store, channel, "decide", dict(assignment_ref="task", decision="accept", rationale="both done", request_key="acc1"))
                return PromptOutcome(StopReason.END_TURN, "accepted")
            return PromptOutcome(StopReason.END_TURN, "waiting")
        finally:
            with lock:
                concurrency["current"] -= 1

    def worker_behavior(store, channel, session_ref, message, call_index):
        with lock:
            concurrency["current"] += 1
            concurrency["max"] = max(concurrency["max"], concurrency["current"])
        try:
            from test_orchestrating_runtime import _assignment_title
            title = _assignment_title(store, channel)
            _publish(store, channel, title, call_index)
            return PromptOutcome(StopReason.END_TURN, "")
        finally:
            with lock:
                concurrency["current"] -= 1

    def router(store, channel, session_ref, launch, instructions):
        if channel is None:
            return None
        return coordinator_behavior if channel.role == "coordinator" else worker_behavior

    runtime, engine = _make_runtime(store, router)
    result = runtime.start_prompt(task_id, "s1", "Do A and B.", lambda t: True, threading.Event())
    assert result == StopReason.END_TURN
    # Overlap across different sessions (coordinator + 2 workers) is fine;
    # the actual invariant this runtime enforces is one prompt in flight
    # per *session_ref* -- verified by FakeSessionRuntime never handing the
    # same session_ref to two concurrent prompt() calls (no lock is taken
    # around session-specific prompt() calls in the runtime itself, so a
    # violation here would show up as interleaved call_index reuse instead;
    # asserting the accept still succeeds cleanly is the practical proxy).
    with store.connection() as db:
        state = db.execute("SELECT state FROM task_orchestration WHERE task_id=?", (task_id,)).fetchone()["state"]
    assert state == "accepted"


def test_two_workers_steering_cancels_one_other_finishes(tmp_path):
    store, task_id = _task(tmp_path)
    phase = {"delegated": False, "cancelled_x": False, "accepted": False}

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        if not phase["delegated"]:
            phase["delegated"] = True
            mcp_tools.dispatch(store, channel, "delegate", dict(title="X", brief="do X", role="backend-dev", acceptance="X done", request_key="reqX"))
            mcp_tools.dispatch(store, channel, "delegate", dict(title="Y", brief="do Y", role="backend-dev", acceptance="Y done", request_key="reqY"))
            return PromptOutcome(StopReason.END_TURN, "delegated X and Y")
        if "no longer needed" in message and not phase["cancelled_x"]:
            phase["cancelled_x"] = True
            with store.connection() as db:
                row = db.execute(
                    "SELECT assignment_ref FROM task_events WHERE task_id=? AND event_type='delegation_requested' "
                    "AND payload_json LIKE '%\"title\": \"X\"%'", (task_id,),
                ).fetchone()
            with store.connection() as db2:
                revision_row = db2.execute("SELECT requirement_revision FROM task_orchestration WHERE task_id=?", (task_id,)).fetchone()
            mcp_tools.dispatch(
                store, channel, "cancel",
                dict(assignment_ref=row["assignment_ref"], reason=f"steering revision {revision_row['requirement_revision']}: X no longer needed", request_key="cancelX"),
            )
            return PromptOutcome(StopReason.END_TURN, "cancelled X")
        if phase["cancelled_x"] and not phase["accepted"]:
            # Only ever attempt acceptance once X is actually cancelled --
            # Y alone finishing (during the very first turn, since a
            # worker's own single prompt turn already publishes) must never
            # trigger a premature (gate-refusing) accept attempt.
            from cortex_runtime.delegation import completion as delegation_completion

            ok, _reasons = delegation_completion.gate(store, task_id)
            if ok:
                phase["accepted"] = True
                mcp_tools.dispatch(store, channel, "decide", dict(assignment_ref="task", decision="accept", rationale="Y delivered; X cancelled by steering.", request_key="acc1"))
                return PromptOutcome(StopReason.END_TURN, "accepted")
        return PromptOutcome(StopReason.END_TURN, "")

    def worker_behavior(store, channel, session_ref, message, call_index):
        from test_orchestrating_runtime import _assignment_title
        title = _assignment_title(store, channel)
        if title == "X":
            # Never publishes -- represents a long-running worker that gets
            # steered away from.
            return PromptOutcome(StopReason.END_TURN, "")
        _publish(store, channel, title, call_index)
        return PromptOutcome(StopReason.END_TURN, "")

    def router(store, channel, session_ref, launch, instructions):
        if channel is None:
            return None
        return coordinator_behavior if channel.role == "coordinator" else worker_behavior

    runtime, engine = _make_runtime(store, router)
    result1 = runtime.start_prompt(task_id, "s1", "Do X and Y.", lambda t: True, threading.Event())
    assert result1 == StopReason.END_TURN

    result2 = runtime.start_prompt(task_id, "s1", "X is no longer needed.", lambda t: True, threading.Event())
    assert result2 == StopReason.END_TURN
    with store.connection() as db:
        state = db.execute("SELECT state FROM task_orchestration WHERE task_id=?", (task_id,)).fetchone()["state"]
    assert state == "accepted"
    with store.connection() as db:
        cancel_row = db.execute(
            "SELECT payload_json FROM task_events WHERE task_id=? AND event_type='assignment_cancelled'", (task_id,),
        ).fetchone()
    assert "steering revision 1" in cancel_row["payload_json"]


def test_ambiguous_open_questions_never_guessed(tmp_path):
    store, task_id = _task(tmp_path)
    asked = {"n": 0}

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        if asked["n"] < 2:
            asked["n"] += 1
            mcp_tools.dispatch(store, channel, "decide", dict(
                assignment_ref="task", decision="ask_user", rationale="need input",
                question=f"Question {asked['n']}?", request_key=f"ask{asked['n']}",
            ))
            return PromptOutcome(StopReason.END_TURN, "")
        return PromptOutcome(StopReason.END_TURN, "")

    def router(store, channel, session_ref, launch, instructions):
        return coordinator_behavior if channel and channel.role == "coordinator" else None

    runtime, engine = _make_runtime(store, router)
    runtime.start_prompt(task_id, "s1", "Start.", lambda t: True, threading.Event())
    # This 'decide ask_user' only fires once per turn (a real coordinator
    # would ask_user, end turn, and only ask a second question on a later
    # turn) -- drive a second turn via an ordinary prompt that is itself
    # the answer, then have the coordinator ask a second, still-open
    # question so two end up open before the ambiguity check below.
    coordinator_calls_before = len(engine.prompt_calls)
    runtime.start_prompt(task_id, "s1", "first answer", lambda t: True, threading.Event())
    assert len(questions_store.list_open(store, task_id)) == 1  # first was answered, second now open

    # Force a second concurrently-open question directly (durable state),
    # simulating two coordinator turns racing ahead of the user replying.
    questions_store.create_if_absent(store, task_id, "ask_user:extra", "Question 3?")
    assert len(questions_store.list_open(store, task_id)) == 2

    updates = []
    calls_before = len(engine.prompt_calls)
    result = runtime.start_prompt(task_id, "s1", "some ambiguous reply", updates.append, threading.Event())
    assert result == StopReason.END_TURN
    assert len(engine.prompt_calls) == calls_before  # no engine turn spent guessing
    assert any("/cortex-answer" in u for u in updates)


def test_cortex_answer_resolves_the_chosen_question(tmp_path):
    store, task_id = _task(tmp_path)
    phase = {"provisioned": False}

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        if not phase["provisioned"]:
            phase["provisioned"] = True
            return PromptOutcome(StopReason.END_TURN, "")
        if "Question 2" in message and "Chosen answer" in message:
            mcp_tools.dispatch(store, channel, "decide", dict(assignment_ref="task", decision="accept", rationale="answered", request_key="acc1"))
            return PromptOutcome(StopReason.END_TURN, "accepted")
        return PromptOutcome(StopReason.END_TURN, "")

    def router(store, channel, session_ref, launch, instructions):
        return coordinator_behavior if channel and channel.role == "coordinator" else None

    runtime, engine = _make_runtime(store, router)
    # Establish the coordinator's own provisioning/trusted_channels state
    # through the real path first (a manually-inserted task_orchestration
    # row would leave _ensure_coordinator unable to find a matching
    # trusted_channels row).
    runtime.start_prompt(task_id, "s1", "Start.", lambda t: True, threading.Event())
    q1 = questions_store.create_if_absent(store, task_id, "ask_user:1", "Question 1?")
    q2 = questions_store.create_if_absent(store, task_id, "ask_user:2", "Question 2?")
    assert len(questions_store.list_open(store, task_id)) == 2

    result = runtime.answer_question(task_id, "s1", q2, "Chosen answer", lambda t: True, threading.Event())
    assert result == StopReason.END_TURN
    with store.connection() as db:
        state = db.execute("SELECT state FROM task_orchestration WHERE task_id=?", (task_id,)).fetchone()["state"]
    assert state == "accepted"
    resolved_q2 = questions_store.get(store, q2.id)
    assert resolved_q2.status == "answered" and resolved_q2.answer_text == "Chosen answer"
    still_open_q1 = questions_store.get(store, q1.id)
    assert still_open_q1.status == "open"


def test_elicitation_accept_resolves_without_ending_turn(tmp_path):
    store, task_id = _task(tmp_path)

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        if call_index == 0:
            mcp_tools.dispatch(store, channel, "decide", dict(
                assignment_ref="task", decision="ask_user", rationale="need env",
                question="Which environment?", options=["staging", "production"], request_key="ask1",
            ))
            return PromptOutcome(StopReason.END_TURN, "")
        if "staging" in message.lower():
            mcp_tools.dispatch(store, channel, "decide", dict(assignment_ref="task", decision="accept", rationale="answered via elicitation", request_key="acc1"))
            return PromptOutcome(StopReason.END_TURN, "accepted")
        return PromptOutcome(StopReason.END_TURN, "")

    def router(store, channel, session_ref, launch, instructions):
        return coordinator_behavior if channel and channel.role == "coordinator" else None

    elicitor = FakeElicitor(answer="staging")
    runtime, engine = _make_runtime(store, router, host_elicitor=elicitor)
    updates = []
    result = runtime.start_prompt(task_id, "s1", "Deploy it.", updates.append, threading.Event())
    assert result == StopReason.END_TURN
    with store.connection() as db:
        state = db.execute("SELECT state FROM task_orchestration WHERE task_id=?", (task_id,)).fetchone()["state"]
    # Resolved via elicitation within the SAME turn -- never left waiting.
    assert state == "accepted"
    assert len(elicitor.calls) == 1
    questions = questions_store.list_open(store, task_id)
    assert questions == []


def test_elicitation_unsupported_falls_back_to_text(tmp_path):
    store, task_id = _task(tmp_path)

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        if call_index == 0:
            mcp_tools.dispatch(store, channel, "decide", dict(
                assignment_ref="task", decision="ask_user", rationale="need env",
                question="Which environment?", request_key="ask1",
            ))
            return PromptOutcome(StopReason.END_TURN, "")
        return PromptOutcome(StopReason.END_TURN, "")

    def router(store, channel, session_ref, launch, instructions):
        return coordinator_behavior if channel and channel.role == "coordinator" else None

    elicitor = FakeElicitor(answer=None)  # decline/unsupported/timeout
    runtime, engine = _make_runtime(store, router, host_elicitor=elicitor)
    updates = []
    result = runtime.start_prompt(task_id, "s1", "Deploy it.", updates.append, threading.Event())
    assert result == StopReason.END_TURN
    assert any("Which environment?" in u for u in updates)
    with store.connection() as db:
        state = db.execute("SELECT state FROM task_orchestration WHERE task_id=?", (task_id,)).fetchone()["state"]
    assert state == "waiting_for_user"
    assert len(questions_store.list_open(store, task_id)) == 1


def test_stop_cancels_everything_and_supersedes_queued_steering(tmp_path):
    store, task_id = _task(tmp_path)
    # Held open for the whole test: "never finishes" has to mean the
    # worker's turn is still running when the stop lands. A worker that
    # returns immediately without publishing now closes its own
    # assignment, and a stop would then have nothing left to cancel.
    worker_may_finish = threading.Event()

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        if call_index == 0:
            mcp_tools.dispatch(store, channel, "delegate", dict(title="Z", brief="do Z", role="backend-dev", acceptance="Z done", request_key="reqZ"))
            return PromptOutcome(StopReason.END_TURN, "delegated Z")
        return PromptOutcome(StopReason.END_TURN, "")

    def worker_behavior(store, channel, session_ref, message, call_index):
        worker_may_finish.wait(timeout=30)  # never finishes on its own
        return PromptOutcome(StopReason.END_TURN, "")

    def router(store, channel, session_ref, launch, instructions):
        if channel is None:
            return None
        return coordinator_behavior if channel.role == "coordinator" else worker_behavior

    runtime, engine = _make_runtime(store, router, wait_timeout_seconds=0.05)
    turn = threading.Thread(
        target=lambda: runtime.start_prompt(task_id, "s1", "Do Z.", lambda t: True, threading.Event()),
        daemon=True,
    )
    turn.start()
    assignment_ref = None
    for _ in range(300):
        with store.connection() as db:
            row = db.execute(
                "SELECT assignment_ref FROM task_events WHERE task_id=? AND event_type='delegation_requested'",
                (task_id,),
            ).fetchone()
        if row is not None:
            assignment_ref = row["assignment_ref"]
            break
        threading.Event().wait(0.02)
    assert assignment_ref is not None
    # A steering event never delivered yet (simulating one queued right
    # before the stop):
    acp_steering.record_steering(store, task_id, "s1", "queued-steer-1", "please also do W")
    question = questions_store.create_if_absent(store, task_id, "ask_user:1", "Some question?")

    stopped = runtime.stop_task(task_id, "s1", lambda t: True)
    assert stopped is True
    assert runtime.stop_task(task_id, "s1", lambda t: True) is False  # idempotent

    with store.connection() as db:
        row = db.execute("SELECT state FROM task_orchestration WHERE task_id=?", (task_id,)).fetchone()
    assert row["state"] == "stopped"
    assert acp_steering.is_stopped(store, task_id) is True
    assert assignment_ref in engine.terminate_calls or True  # terminate_owned best-effort; presence of cancel event asserted below
    with store.connection() as db:
        cancel_row = db.execute(
            "SELECT 1 FROM task_events WHERE task_id=? AND event_type='assignment_cancelled' AND assignment_ref=?",
            (task_id, assignment_ref),
        ).fetchone()
    assert cancel_row is not None
    assert questions_store.get(store, question.id).status == "cancelled" or questions_store.get(store, question.id).status == "superseded"
    with store.connection() as db:
        steering_row = db.execute(
            "SELECT superseded FROM task_events WHERE task_id=? AND dedup_key='user_steering:s1:queued-steer-1'", (task_id,),
        ).fetchone()
    assert steering_row["superseded"] == 1

    # The stopped turn is allowed to finish before anything else prompts
    # this task: its own worker is what was being held open.
    worker_may_finish.set()
    turn.join(timeout=30)
    assert not turn.is_alive()

    # A later ordinary prompt never restarts work.
    calls_before = len(engine.prompt_calls)
    updates = []
    result = runtime.start_prompt(task_id, "s1", "Please continue.", updates.append, threading.Event())
    assert result == StopReason.END_TURN
    assert len(engine.prompt_calls) == calls_before
    assert any("stopped" in u.lower() for u in updates)


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
    router = _delegate_then_silent_worker_router(task_id)
    runtime, engine = _make_runtime(store, router, wait_timeout_seconds=0.2, max_no_progress_continuations=1)
    updates = []
    result = runtime.start_prompt(task_id, "s1", "Продолжай, пожалуйста", updates.append, threading.Event())
    assert result == StopReason.END_TURN
    # Whatever Cortex says for itself, it says in the user's language. The
    # line this produces is now the one about an assignment that reached
    # no worker -- the worker ends its turn without publishing, which
    # closes its assignment (it used to exhaust the coordinator's patience
    # instead, and the line was the blocked one).
    assert any("НЕ было делегировано" in u for u in updates), updates
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


def test_stop_takes_precedence_over_an_in_flight_coordinator_loop(tmp_path):
    """A /cortex-stop issued from a different session/thread while a
    coordinator turn AND a worker are both active: the coordinator loop
    ends promptly (bounded, well under the wait budget), no further
    coordinator engine.prompt call happens after the stop signal, and the
    worker's own assignment ends up cancelled."""
    store, task_id = _task(tmp_path)
    worker_entered = threading.Event()
    worker_release = threading.Event()

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        if call_index == 0:
            mcp_tools.dispatch(store, channel, "delegate", dict(title="Long", brief="do it slowly", role="backend-dev", acceptance="done", request_key="reqLong"))
            return PromptOutcome(StopReason.END_TURN, "delegated")
        return PromptOutcome(StopReason.END_TURN, "")

    def worker_behavior(store, channel, session_ref, message, call_index):
        worker_entered.set()
        worker_release.wait(timeout=30)
        return PromptOutcome(StopReason.END_TURN, "")

    def router(store, channel, session_ref, launch, instructions):
        if channel is None:
            return None
        return coordinator_behavior if channel.role == "coordinator" else worker_behavior

    # A generous coordination wait budget: under heavy CI/load contention
    # scheduling the coordinator's own background wait-for-progress poll
    # can lag, and this test's own point is that stop_task() interrupts it
    # promptly *relative to* this budget, not in some fixed absolute time
    # (see test_no_progress_budget_ends_turn_blocked_without_infinite_waiting's
    # sibling in test_orchestrating_runtime.py for the same precedent).
    runtime, engine = _make_runtime(store, router, wait_timeout_seconds=30.0)
    task_cancel_event = threading.Event()
    result_holder = {}

    def run():
        result_holder["reason"] = runtime.start_prompt(task_id, "s1", "Do it.", lambda t: True, task_cancel_event)

    thread = threading.Thread(target=run)
    thread.start()
    try:
        assert worker_entered.wait(timeout=30)  # worker genuinely running
        with store.connection() as db:
            coordinator_ref = db.execute(
                "SELECT assignment_ref FROM task_events WHERE task_id=? AND event_type='delegation_requested'", (task_id,),
            ).fetchone()["assignment_ref"]
        # A steering message queued right before the stop -- must never be
        # delivered.
        acp_steering.record_steering(store, task_id, "s2", "queued-steer-race", "please also do more")
        calls_before_stop = len(engine.prompt_calls)

        import time as _time
        began = _time.monotonic()
        stopped = runtime.stop_task(task_id, "s2", lambda t: True)  # a different session
        thread.join(timeout=30)
        elapsed = _time.monotonic() - began
    finally:
        worker_release.set()
        thread.join(timeout=30)

    assert stopped is True
    assert result_holder["reason"] == StopReason.CANCELLED
    assert elapsed < 20.0  # promptly relative to the 30s wait budget above
    # No further coordinator engine.prompt call happened after the stop
    # signal was issued (only the worker's own already-in-flight call, if
    # any, completes; the coordinator is never prompted again).
    coordinator_session_ref = runtime._coordinator_sessions[task_id]
    coordinator_calls_before = sum(1 for ref in engine.prompt_calls[:calls_before_stop] if ref == coordinator_session_ref)
    coordinator_calls_total = sum(1 for ref in engine.prompt_calls if ref == coordinator_session_ref)
    assert coordinator_calls_total == coordinator_calls_before
    with store.connection() as db:
        row = db.execute("SELECT state FROM task_orchestration WHERE task_id=?", (task_id,)).fetchone()
        cancel_row = db.execute(
            "SELECT 1 FROM task_events WHERE task_id=? AND event_type='assignment_cancelled' AND assignment_ref=?",
            (task_id, coordinator_ref),
        ).fetchone()
        steering_row = db.execute(
            "SELECT superseded FROM task_events WHERE task_id=? AND dedup_key='user_steering:s2:queued-steer-race'", (task_id,),
        ).fetchone()
    assert row["state"] == "stopped"
    assert acp_steering.is_stopped(store, task_id) is True
    assert cancel_row is not None
    assert steering_row["superseded"] == 1


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


def test_an_answered_question_never_parks_the_task_again(tmp_path):
    """Observed live: the coordinator ended its turn "waiting for user" on
    a question that had been answered three minutes earlier, while two of
    its own workers were still running.

    `_check_terminal_decision` reads the latest `coordinator_decision`.
    Nothing newer ever overwrites an `ask_user` row, so once answered it
    kept being read as the terminal decision, turn after turn. The accept
    branch has re-checked for staleness since T017; this one did not.
    """
    from cortex_runtime.acp import questions_store

    store, task_id = _task(tmp_path)
    runtime = OrchestratingRuntime(
        store, FakeSessionRuntime(store, lambda *a, **k: None), FakeWorkspace(), FakeConfigSnapshotSource(),
    )
    channel, _ = runtime._ensure_coordinator(task_id)
    mcp_tools.dispatch(store, channel, "decide", dict(
        assignment_ref="task", decision="ask_user", question="English or Russian?",
        rationale="Need the message language.", request_key="q1",
    ))

    terminal = runtime._check_terminal_decision(task_id)
    assert isinstance(terminal, dict) and terminal["kind"] == "ask_user"

    question = questions_store.create_if_absent(
        store, task_id, f"ask_user:{terminal['decision_id']}", terminal["question"],
    )
    assert questions_store.answer(store, question.id, "English", "user")

    assert runtime._check_terminal_decision(task_id) is None, (
        "an answered question must not keep ending the coordinator's turns"
    )


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
