"""T026: a non-Codex (claude-agent-acp) coordinator delegates one managed
assignment, processes the worker's published result, and participates in
explicit ``decide`` acceptance -- through the exact same production
``OrchestratingRuntime``/``ProvisioningService``/``delegation.mcp_tools``
code path T016's own codex-acp-flavored test exercises (see
``tests/test_orchestrating_runtime.py``'s
``test_one_prompt_delegates_two_assignments_and_accepts_both_results``,
which this mirrors with a single assignment and
``FakeConfigSnapshotSource(engine="claude-agent-acp", version="0.78.0")``
instead of the default codex-acp metadata).

This now succeeds end to end (T026 fresh dist re-verification): the real
coordinator/worker roles carry non-negotiable ``critical_rules``, but
claude-agent-acp 0.78.0's own ``session/new`` ``_meta.systemPrompt``
channel is QUALIFIED (see
``engine.instruction_delivery_registry.ClaudeAgentAcp0_78Delivery``'s own
docstring for the full evidence chain), so no
``allow_unqualified_delivery`` escape hatch is needed at all -- unlike a
hypothetical UNQUALIFIED/UNSUPPORTED engine, which ``portable.
instruction_delivery.qualify_delivery`` would still refuse (see
``tests/test_instruction_delivery_qualification.py``'s own coverage of
that refusal boundary, still enforced for any *other*/unrecognized
claude-agent-acp version).

Uses the in-process ``FakeSessionRuntime`` (never a real/fake ACP
subprocess) so the full delegate -> provision -> worker-report ->
coordinator-decide cycle, including the real background executor and
completion gate, is exercised deterministically; see
``tests/test_engine_claude_coordinator_live_session.py`` for the
complementary real-subprocess coverage of the coordinator's own
negotiate/create_session wire path (including the actual
``_meta.systemPrompt`` payload observed on the wire).
"""
from __future__ import annotations

import json
import threading
import uuid
from pathlib import Path

from cortex_runtime.acp.orchestrating_runtime import OrchestratingRuntime, PromptOutcome
from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.delegation import mcp_tools
from cortex_runtime.delegation.workspace_isolation import IsolatedSnapshotWorkspaceProvisioner
from cortex_runtime.portable.engine_contract import StopReason
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.store import Store

from orchestration_fakes import FakeConfigSnapshotSource, FakeSessionRuntime


def _task(tmp_path):
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    result = bootstrap_task(store, SourceEvent(origin="cli", event_id=str(uuid.uuid4())), "Ship the report.", "owner-1")
    return store, result.task_id


def _assignment_title(store, channel):
    with store.connection() as db:
        row = db.execute(
            "SELECT idempotency_key FROM delegation_provisioning WHERE attempt_id=?", (channel.attempt_id,),
        ).fetchone()
        if row is None:
            return None
        event = db.execute(
            "SELECT payload_json FROM task_events WHERE task_id=? AND event_type='delegation_requested' AND assignment_ref=?",
            (channel.task_id, row["idempotency_key"]),
        ).fetchone()
    return json.loads(event["payload_json"])["title"] if event is not None else None


def _publish(store, channel, label):
    report_id = "r_" + uuid.uuid4().hex[:12]
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        task_dir = store._task_directory(db, channel.task_id)
        path = task_dir / f"{report_id}.md"
        path.write_text(f"# {label}\n", encoding="utf-8")
        info = path.stat()
        db.execute("INSERT INTO reports VALUES (?,?,?,?,?)", (report_id, channel.task_id, "report", path.name, store._now()))
        db.execute(
            "INSERT INTO drafts(id,task_id,owner_thread_id,kind,template,path,created_at,device,inode,published_report_id,published_digest) "
            "VALUES (?,?,?,?,?,?,?,?,?,?,?)",
            ("d_" + uuid.uuid4().hex[:12], channel.task_id, channel.principal_thread_id, "report", "general",
             str(path), store._now(), info.st_dev, info.st_ino, report_id, "test-digest"),
        )
        db.commit()
    mcp_tools.dispatch(store, channel, "publish_result", dict(report_ref=report_id, summary=f"{label} finished", request_key=f"pub-{label}"))


def _publish_current_pipeline(store, channel):
    """Give task-level acceptance a real coordinator-authored edition."""
    draft = store.call(
        "create_draft", {"template": "pipeline", "request_key": "pipeline-current"},
        channel.principal_thread_id, trusted=True,
    )
    path = Path(draft["draft_path"])
    body = path.read_text(encoding="utf-8")
    values = {
        "{{CURRENT_OBJECTIVE_AND_DECISION_BASIS}}": "Objective: ship the report. Decision basis: delegated work is reviewable.",
        "{{CURRENT_REQUIREMENTS_AND_CONSTRAINTS}}": "Requirements: assignment result is complete and reviewable.",
        "{{CURRENT_WORK_GRAPH}}": "The delegated assignment has completed.",
        "{{CURRENT_ASSIGNMENTS_AND_ROUTING}}": "Coordinator reviewed the backend-dev assignment.",
        "{{CURRENT_EVIDENCE_AND_VERIFICATION}}": "The worker report is published and available for review.",
        "{{CURRENT_DECISIONS_QUESTIONS_AND_REMAINING_WORK}}": "Decision: accept the completed work.",
    }
    for marker, value in values.items():
        body = body.replace(marker, value)
    for marker in draft["replaceable_markers"]:
        if marker.startswith("<!--"):
            body = body.replace(marker, "Evidence links: the worker report is the selected result.")
    path.write_text(body, encoding="utf-8")
    return store.call(
        "write_report", {
            "title": "Current pipeline", "summary": "Coordinator current plan.",
            "author": "coordinator", "draft_id": draft["draft_id"], "request_key": "pipeline-current-publish",
        }, channel.principal_thread_id, trusted=True,
    )


def test_claude_agent_acp_coordinator_delegates_and_accepts_one_assignment(tmp_path):
    store, task_id = _task(tmp_path)
    decided = {"done": False}
    opened_roles = []

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        if call_index == 0:
            mcp_tools.dispatch(store, channel, "delegate", dict(
                title="Assignment A", brief="do A", role="backend-dev", acceptance="A done", request_key="reqA",
            ))
            return PromptOutcome(StopReason.END_TURN, "Delegated A.")
        if "Assignment A finished" in message and not decided["done"]:
            decided["done"] = True
            _publish_current_pipeline(store, channel)
            mcp_tools.dispatch(store, channel, "decide", dict(
                assignment_ref="task", decision="accept",
                rationale="Assignment A delivered an acceptable result.", request_key="acceptall",
            ))
            return PromptOutcome(StopReason.END_TURN, "Accepted.")
        return PromptOutcome(StopReason.END_TURN, "Waiting for the remaining result.")

    def worker_behavior(store, channel, session_ref, message, call_index):
        title = _assignment_title(store, channel)
        _publish(store, channel, title)
        return PromptOutcome(StopReason.END_TURN, "")

    def router(store, channel, session_ref, launch, instructions):
        if channel is None:
            return None
        # F: no allow_unqualified_delivery escape hatch was used to reach
        # here at all -- QUALIFIED admits both roles unconditionally.
        opened_roles.append(channel.role)
        return coordinator_behavior if channel.role == "coordinator" else worker_behavior

    engine = FakeSessionRuntime(store, router)
    runtime = OrchestratingRuntime(
        store, engine, IsolatedSnapshotWorkspaceProvisioner(store, str(tmp_path)),
        FakeConfigSnapshotSource(engine="claude-agent-acp", version="0.78.0"),
        wait_timeout_seconds=30.0,
    )
    cancel_event = threading.Event()
    updates = []
    result = runtime.start_prompt(task_id, "s1", "Ship the report.", updates.append, cancel_event)

    assert result == StopReason.END_TURN
    with store.connection() as db:
        state = db.execute("SELECT state FROM task_orchestration WHERE task_id=?", (task_id,)).fetchone()["state"]
    assert state == "accepted"
    assert any("Accepted" in u for u in updates)
    # No raw worker chunk (never "do A" verbatim as a coordinator update).
    assert not any("do A" in u for u in updates)
    # F: both a coordinator and a worker session actually opened under
    # engine="claude-agent-acp" -- the whole cycle ran, not merely a
    # precheck.
    assert set(opened_roles) == {"coordinator", "worker"}


def test_claude_agent_acp_pinned_delivery_mechanism_is_the_documented_qualified_class():
    """Ties the successful cycle above back to the exact, named,
    evidence-reviewed class (never a bare/anonymous stand-in) -- see that
    class's own docstring for the full T026 dist-evidence review."""
    from cortex_runtime.engine.instruction_delivery_registry import ClaudeAgentAcp0_78Delivery, delivery_for
    from cortex_runtime.portable.instruction_delivery import DeliveryQualification

    mechanism = delivery_for("claude-agent-acp", "0.78.0")
    assert isinstance(mechanism, ClaudeAgentAcp0_78Delivery)

    class _Role:
        body_portable = "You are a backend developer."

    plan = mechanism.plan(_Role(), [])
    assert plan.qualification == DeliveryQualification.QUALIFIED
    assert plan.session_new_meta == {
        "systemPrompt": {"append": "You are a backend developer."},
        "claudeCode": {"options": {"disallowedTools": ["Agent", "Task"], "strictMcpConfig": True}},
    }


def test_the_result_reaches_the_coordinator_under_the_ref_delegate_gave_it(tmp_path):
    """Live defect (2026-09-19): the continuation named the worker's attempt
    id, so the coordinator saw an assignment it had never heard of, refused
    a real result and a real question, and the task deadlocked with two
    correct workers. The ref the coordinator is shown must be the one
    ``delegate`` handed back -- the only one ``clarify``/``decide`` accept."""
    store, task_id = _task(tmp_path)
    delegated = {}
    seen_messages = []

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        if call_index == 0:
            delegated["ref"] = mcp_tools.dispatch(store, channel, "delegate", dict(
                title="Assignment A", brief="do A", role="backend-dev", acceptance="A done",
                request_key="reqA",
            ))["assignment_ref"]
            return PromptOutcome(StopReason.END_TURN, "Delegated A.")
        seen_messages.append(message)
        if "Assignment A finished" in message:
            _publish_current_pipeline(store, channel)
            mcp_tools.dispatch(store, channel, "decide", dict(
                assignment_ref="task", decision="accept", rationale="Delivered.", request_key="acceptall",
            ))
            return PromptOutcome(StopReason.END_TURN, "Accepted.")
        return PromptOutcome(StopReason.END_TURN, "Waiting.")

    def worker_behavior(store, channel, session_ref, message, call_index):
        _publish(store, channel, _assignment_title(store, channel))
        return PromptOutcome(StopReason.END_TURN, "")

    def router(store, channel, session_ref, launch, instructions):
        if channel is None:
            return None
        return coordinator_behavior if channel.role == "coordinator" else worker_behavior

    engine = FakeSessionRuntime(store, router)
    runtime = OrchestratingRuntime(
        store, engine, IsolatedSnapshotWorkspaceProvisioner(store, str(tmp_path)),
        FakeConfigSnapshotSource(engine="claude-agent-acp", version="0.78.0"),
        wait_timeout_seconds=30.0,
    )
    runtime.start_prompt(task_id, "s1", "Ship the report.", [].append, threading.Event())

    result_lines = [m for m in seen_messages if "result" in m]
    assert result_lines, seen_messages
    assert any(f"Assignment {delegated['ref']} result" in m for m in result_lines), result_lines
