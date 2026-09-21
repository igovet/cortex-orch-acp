"""Dependent verification receives the exact predecessor report handoff rule."""

from cortex_runtime.engine.instruction_delivery_registry import delivery_for
from cortex_runtime.portable.roles import load_role, read_reference


def test_coordinator_routing_names_exact_predecessor_report():
    routing = read_reference("worker-routing", role_kind="coordinator")
    assert "exact\npublished predecessor `report_ref`" in routing
    assert "`worker_result` event" in routing
    assert "`read_report` before checking" in routing


def test_coordinator_delivery_reads_report_before_qa_or_self_check():
    role = load_role("orchestrator")
    body = role.body_portable
    assert "On `worker_result`, read its exact published `report_ref`" in body
    assert "`read_report` before source review or another assignment" in body
    assert "Pass that ref to\n   independent QA" in body
    assert "rerun project checks" in body
    assert "native/private skill paths" in body

    claude = delivery_for("claude-agent-acp", "0.78.0").plan(role, [])
    assert body == claude.session_new_meta["systemPrompt"]["append"]


def test_qa_delivery_requires_predecessor_report_read():
    role = load_role("worker-qa-engineer")
    body = role.body_portable
    assert "read its exact assigned `report_ref` with\nbounded `read_report`" in body
    assert "if the brief omits that ref, ask\nthe coordinator" in body

    codex = delivery_for("codex-acp", "1.12.0").plan(role, [])
    claude = delivery_for("claude-agent-acp", "0.78.0").plan(role, [])
    assert body == codex.session_new_meta["systemPrompt"]
    assert body == claude.session_new_meta["systemPrompt"]["append"]
