"""No-paid regression for same-turn scalar answers and recovery planning."""
import json
import threading

import pytest

from cortex_runtime.acp import questions_store
from cortex_runtime.acp.orchestrating_runtime import PromptOutcome
from cortex_runtime.delegation import mcp_tools
from cortex_runtime.orchestration.questions import resume_answered
from cortex_runtime.orchestration.state import TaskMachine
from cortex_runtime.portable.engine_contract import StopReason

from test_acp_steering_and_questions import _task, _make_runtime
from runtime_scenarios import propose_inspection, publish_result
from orchestration_fakes import coordinator_prompt_count


class ScriptedElicitor:
    def __init__(self, answers):
        self.answers = iter(answers)
        self.calls = []

    def elicit(self, session_id, question, options):
        self.calls.append((session_id, question, options))
        return next(self.answers, None)


@pytest.mark.parametrize("recovery_answer", ["Continue", "Submit the inspection plan now"])
def test_scalar_answer_then_recovery_gets_a_real_planning_turn(tmp_path, recovery_answer):
    store, task_id = _task(tmp_path)
    planning_turns = []

    def router(store, channel, *_):
        def behavior(store, channel, ref, message, index):
            if channel.role != "coordinator":
                return publish_result(store, channel)
            context = TaskMachine(store).context(task_id)
            if context["state"] == "PLANNING":
                planning_turns.append((ref, context["user_answers"]))
                if index == 0:
                    mcp_tools.dispatch(store, channel, "user_question_request", dict(
                        question="Which summary format?", options=["Bulleted list", "Paragraph"],
                        request_key="summary-format"))
                elif index < 4:
                    # The live engine ended three turns without a proposal.
                    return PromptOutcome(StopReason.END_TURN, "Acknowledged")
                else:
                    assert recovery_answer in message
                    propose_inspection(store, channel)
            else:
                mcp_tools.dispatch(store, channel, "synthesis_submit", dict(text="Reviewed and verified"))
            return PromptOutcome(StopReason.END_TURN, "")

        return behavior

    runtime, engine = _make_runtime(store, router)
    elicitor = ScriptedElicitor(["Bulleted list", recovery_answer])
    try:
        outcome = runtime.start_prompt(task_id, "s1", "Inspect", lambda _: True,
                                       threading.Event(), host_elicitor=elicitor)
        assert outcome == StopReason.END_TURN
        context = TaskMachine(store).context(task_id)
        assert context["state"] == "COMPLETED", (context["blocker"], len(planning_turns))
        assert len(planning_turns) == 5
        assert len({ref for ref, _ in planning_turns}) == 1
        assert len(elicitor.calls) == 2
        assert planning_turns[1][1][0]["answer_text"] == "Bulleted list"
        assert planning_turns[4][1][-1]["answer_text"] == recovery_answer
        assert coordinator_prompt_count(engine, planning_turns[0][0]) == 6
        assert resume_answered(store, task_id) is False
        with store.connection() as db:
            rows = db.execute("SELECT payload_json FROM runtime_events WHERE task_id=? "
                              "AND type='USER_ANSWER_RECEIVED' ORDER BY id", (task_id,)).fetchall()
            answers = [json.loads(row[0]) for row in rows]
            questions = db.execute("SELECT id,status FROM pending_questions WHERE task_id=?", (task_id,)).fetchall()
        assert len(answers) == len({answer["question_id"] for answer in answers}) == 2
        assert len(questions) == 2
        assert all(row["status"] == "answered" for row in questions)
        assert questions_store.answer(store, answers[0]["question_id"], "Paragraph", "elicitation") is False
    finally:
        runtime.shutdown()


@pytest.mark.parametrize("answer", ["Continue", None])
def test_recovery_keeps_three_prompt_bound_and_decline_does_not_reset_it(tmp_path, answer):
    store, task_id = _task(tmp_path)

    def router(store, channel, *_):
        def behavior(*_):
            return PromptOutcome(StopReason.END_TURN, "No proposal")
        return behavior

    runtime, engine = _make_runtime(store, router)
    elicitor = ScriptedElicitor([answer, None])
    try:
        runtime.start_prompt(task_id, "s1", "Inspect", lambda _: True,
                             threading.Event(), host_elicitor=elicitor)
        context = TaskMachine(store).context(task_id)
        assert context["state"] == "WAITING_USER"
        assert context["blocker"] == "structured_proposal_missing"
        assert len(engine.prompt_calls) == (6 if answer else 3)
        assert len(elicitor.calls) == (2 if answer else 1)
        with store.connection() as db:
            count = db.execute("SELECT COUNT(*) FROM runtime_events WHERE task_id=? "
                               "AND type='USER_ANSWER_RECEIVED'", (task_id,)).fetchone()[0]
        assert count == (1 if answer else 0)
        assert len(questions_store.list_open(store, task_id)) == 1
    finally:
        runtime.shutdown()
