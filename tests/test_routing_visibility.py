import uuid

import pytest

from cortex_runtime.acp.orchestrating_runtime import OrchestratingRuntime
from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.delegation import provisioning
from cortex_runtime.orchestration import pipeline
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.routing import store as routing_store
from cortex_runtime.store import Store


OLD_ROW = {
    # A rank version 3 row: a cross-tier stand-in and count-based balance.
    "effort": "low",
    "model_name": "Sonnet 5",
    "route_decision": {
        "rank_version": 3,
        "role_tier": "light",
        "tier": "light",
        "chosen_cross_tier": True,
        "cross_tier": {"from_tier": "standard", "routes": ["claude-agent-acp/claude-sonnet-5"]},
        "model_hint_status": "honoured",
        "decided_by": "hint",
        "balance": {"window": 20, "tolerance": 2, "counts": {"codex-acp": 3, "claude-agent-acp": 0}},
        "chosen": "claude-agent-acp/claude-sonnet-5",
        "chosen_connection_id": "connection",
        "candidates": [{"route": "claude-agent-acp/claude-sonnet-5", "connection_id": "connection",
                        "used_percent": None, "balance_excess": 0, "cross_tier": True}],
    },
}

NEW_ROW = {
    "effort": "high",
    "model_name": "Opus 5.5 (1M)",
    "route_decision": {
        "rank_version": 4,
        "role_tier": "deep",
        "tier": "deep",
        "model_hint_status": "ignored:near_quota_limit",
        "decided_by": "usage",
        "chosen": "claude-agent-acp/opus[1m]",
        "chosen_connection_id": "connection",
        "candidates": [
            {"route": "claude-agent-acp/opus[1m]", "connection_id": "connection", "used_percent": 42.4},
            {"route": "codex-acp/gpt-6-sol", "connection_id": "codex", "used_percent": 88.0},
        ],
    },
}


def _audited_worker(tmp_path, detail):
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    task_id = bootstrap_task(
        store, SourceEvent(origin="cli", event_id=str(uuid.uuid4())), "Route visibility.", "owner",
    ).task_id
    now = "2026-09-24T00:00:00+00:00"
    with store.connection() as db:
        db.execute(
            "INSERT INTO configuration_revisions "
            "(id, task_id, revision, route_id, route_engine, route_connection_id, parameters, "
            "ceiling_snapshot, state, created_at, updated_at) VALUES (?,?,?,?,?,?,?,?,?,?,?)",
            ("cfg", task_id, 1, "route", "claude-agent-acp", "connection", "{}", "{}", "confirmed", now, now),
        )
        db.execute(
            "INSERT INTO delegation_provisioning "
            "(idempotency_key, task_id, attempt_id, role_id, step, status, config_revision, "
            "owner_epoch, created_at, updated_at) VALUES (?,?,?,?,?,?,?,?,?,?)",
            ("assignment", task_id, "attempt", "worker-debugger", "ready", "done", "cfg", 1, now, now),
        )
        db.commit()
    routing_store.record_attempt_audit(
        store, attempt_id="attempt", task_id=task_id, role="worker-debugger",
        engine="claude-agent-acp", connection_id="connection",
        model=detail["route_decision"]["chosen"].split("/", 1)[1],
        params_fingerprint="fingerprint", config_revision_id="cfg", decision="authorized", detail=detail,
    )
    runtime = OrchestratingRuntime.__new__(OrchestratingRuntime)
    runtime._store = store
    return store, task_id, runtime


def test_worker_card_and_pipeline_render_the_recorded_route_decision(tmp_path):
    store, task_id, runtime = _audited_worker(tmp_path, NEW_ROW)
    # The card shortens an ignored hint's reason; pipeline.md keeps it.
    assert runtime._delegation_route_label(task_id, "assignment") == (
        "claude · Opus 5.5 (1M) · high · deep · hint ignored · by usage · usage 42%")
    assert pipeline._recorded_route_summary(store, task_id, "assignment") == (
        "claude · Opus 5.5 (1M) · high · deep · hint ignored:near_quota_limit · by usage · usage 42%")


def test_an_old_row_with_balance_and_cross_tier_still_renders_without_counts(tmp_path):
    store, task_id, runtime = _audited_worker(tmp_path, OLD_ROW)
    full = "claude · Sonnet 5 · low · light→standard · hint honoured · by hint · usage unknown"
    assert pipeline._recorded_route_summary(store, task_id, "assignment") == full
    # 81 characters: the card drops the hint (what decided it still says so).
    card = runtime._delegation_route_label(task_id, "assignment")
    assert card == "claude · Sonnet 5 · low · light→standard · by hint · usage unknown"
    for line in (full, card):
        assert "balance" not in line and "3:0" not in line and "codex-acp=" not in line


def test_a_fallback_tier_shows_the_task_tier_and_the_routed_tier():
    from cortex_runtime.routing.decision_display import route_decision_summary

    detail = {"effort": "medium", "model_name": "Sonnet 5", "route_decision": {
        "role_tier": "light", "tier": "standard", "decided_by": "only_candidate",
        "chosen": "claude-agent-acp/sonnet", "chosen_connection_id": "c",
        "candidates": [{"route": "claude-agent-acp/sonnet", "connection_id": "c", "used_percent": 7}]}}
    assert route_decision_summary("claude-agent-acp", "sonnet", detail) == (
        "claude · Sonnet 5 · medium · light→standard · by only_candidate · usage 7%")


@pytest.mark.parametrize("decided_by", ["pin", "preference", "hint", "quota", "diversify", "usage", "affinity",
                                        "used_percent", "engine_order", "only_candidate"])
def test_every_decided_by_value_renders(decided_by):
    from cortex_runtime.routing.decision_display import route_decision_summary

    detail = {"effort": "low", "route_decision": {"tier": "light", "decided_by": decided_by}}
    assert route_decision_summary("codex-acp", "gpt-6-luna", detail) == (
        f"codex · gpt-6-luna (alias) · low · light · by {decided_by} · usage unknown")


def test_a_card_line_never_exceeds_the_card_limit():
    from cortex_runtime.routing.decision_display import CARD_SUMMARY_MAX, route_decision_summary

    assert CARD_SUMMARY_MAX == 80
    detail = {"effort": "medium", "route_decision": {
        "role_tier": "standard", "tier": "standard", "model_hint_status": "ignored:provider_not_allowed",
        "decided_by": "only_candidate"}}
    model = "claude-some-very-long-future-model-identifier-5-1[1m]"
    full = route_decision_summary("claude-agent-acp", model, detail)
    assert len(full) > CARD_SUMMARY_MAX and "hint ignored:provider_not_allowed" in full
    card = route_decision_summary("claude-agent-acp", model, detail, compact=True)
    assert len(card) <= CARD_SUMMARY_MAX
    # The alias marker, then the hint go first; what decided it and the usage stay.
    assert "(alias)" not in card and "hint" not in card
    assert card.endswith("by only_candidate · usage unknown")
    assert "claude-some-very-long-future-model-identifier-5-1[1m]" not in card and "…" in card
    # An effort too long to fit is clipped to the limit rather than overflowing.
    clipped = route_decision_summary("codex-acp", "gpt-6-luna", dict(detail, effort="e" * 200), compact=True)
    assert len(clipped) == CARD_SUMMARY_MAX and clipped.endswith("…")


def test_route_summary_omits_unrecorded_decision_fields():
    from cortex_runtime.routing.decision_display import route_decision_summary

    assert route_decision_summary("codex-acp", "gpt-5.6-luna", {}) == (
        "codex · gpt-5.6-luna (alias)"
    )
    assert route_decision_summary(None, "gpt-5.6-luna", {}) is None


@pytest.mark.parametrize("audit_key,effort_basis", [("luna", "luna:light"), ("stretch", "stretch:light")])
def test_old_and_neutral_policy_audits_render_without_rewriting(tmp_path, audit_key, effort_basis):
    from copy import deepcopy
    from cortex_runtime.routing.decision_display import route_decision_summary
    detail = deepcopy(NEW_ROW)
    detail["route_decision"][audit_key] = dict(status="light_tier", chosen=True)
    detail["route_decision"]["effort_basis"] = effort_basis
    original = deepcopy(detail)
    store, task_id, runtime = _audited_worker(tmp_path, detail)
    assert pipeline._recorded_route_summary(store, task_id, "assignment") == (
        "claude · Opus 5.5 (1M) · high · deep · hint ignored:near_quota_limit · by usage · usage 42%")
    assert route_decision_summary("claude-agent-acp", "opus[1m]", detail)
    assert detail == original
    audit = routing_store.list_audit_for_attempt(store, "attempt")[-1]
    assert audit["decision_detail"] == original
