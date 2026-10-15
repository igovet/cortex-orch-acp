"""T029: observability.metrics -- diagnostics_summary/resource_caps, each
metric declaring its own unit and availability.

Every Store is rooted at tmp_path (never the real checkout); no engine or
network I/O.
"""
from __future__ import annotations

import json
import uuid

from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.configuration.policy import PolicyCeiling
from cortex_runtime.configuration.store import serialize_ceiling
from cortex_runtime.observability import events as observability_events
from cortex_runtime.observability import metrics as observability_metrics
from cortex_runtime.observability import usage as observability_usage
from cortex_runtime.portable.engine_contract import UsageBasis
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.store import Store


def _task(tmp_path):
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    result = bootstrap_task(store, SourceEvent(origin="cli", event_id=str(uuid.uuid4())), "Build a page.", "owner-1")
    return store, result.task_id


def _insert_plan_event(store, task_id, tasks, *, revision=1):
    payload = json.dumps({"revision": revision, "plan": {"tasks": tasks}})
    with store.connection() as db:
        db.execute(
            "INSERT OR IGNORE INTO runtime_tasks(task_id,state,goal,created_at,updated_at) VALUES(?,?,?,?,?)",
            (task_id, "PLANNING", "Build a page.", "2026-01-01T00:00:00+00:00", "2026-01-01T00:00:00+00:00"),
        )
        db.execute(
            "INSERT INTO runtime_events(task_id,type,actor,payload_json,created_at) VALUES(?,?,?,?,?)",
            (task_id, "PLAN_ACCEPTED", "coordinator", payload, "2026-01-01T00:00:00+00:00"),
        )


def _insert_route_assignment(
    store, task_id, *, node_id, assignment_ref, attempt_id, dispatched_at,
    accepted_at, engine="codex-acp", model="gpt-5.6", role="worker-backend-dev",
    revision=1, route_audit=True, event_node_id=None,
):
    with store.connection() as db:
        db.execute(
            "INSERT INTO runtime_nodes VALUES(?,?,?,?,?,?,?,?,?,?,?)",
            (task_id, revision, node_id, role, "Implement the assigned task.", "[]", "[]",
             "execution", "completed", assignment_ref, "{}"),
        )
        db.execute(
            "INSERT INTO delegation_provisioning "
            "(idempotency_key,task_id,attempt_id,role_id,step,status,owner_epoch,created_at,updated_at) "
            "VALUES(?,?,?,?,?,?,?,?,?)",
            (assignment_ref, task_id, attempt_id, role, "complete", "ready", 1, dispatched_at, dispatched_at),
        )
        db.execute(
            "INSERT INTO task_events "
            "(task_id,event_type,assignment_ref,dedup_key,payload_json,origin,created_at) "
            "VALUES(?,?,?,?,?,?,?)",
            (task_id, "delegation_requested", assignment_ref, f"dispatch:{assignment_ref}", "{}", "runtime", dispatched_at),
        )
        if route_audit:
            db.execute(
                "INSERT INTO attempt_route_audit "
                "(id,attempt_id,task_id,role,engine,connection_id,model,params_fingerprint,"
                "config_revision_id,decision,decision_detail_json,predecessor_attempt_id,recorded_at) "
                "VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?)",
                (f"audit:{attempt_id}", attempt_id, task_id, role, engine, "connection-1", model,
                 "fingerprint", None, "authorized", "{}", None, dispatched_at),
            )
        if accepted_at is not None:
            db.execute(
                "INSERT INTO runtime_events(task_id,type,actor,payload_json,created_at) VALUES(?,?,?,?,?)",
                (
                    task_id, "WORKER_COMPLETED", "runtime",
                    json.dumps({"node": event_node_id or node_id, "assignment_ref": assignment_ref, "result": {}}), accepted_at,
                ),
            )


def _insert_revision(store, task_id, ceiling, *, revision=1):
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        db.execute(
            "INSERT INTO configuration_revisions "
            "(id, task_id, revision, route_id, route_engine, route_connection_id, parameters, "
            " ceiling_snapshot, state, confirmation_evidence_ref, error_code, created_at, updated_at) "
            "VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?)",
            (
                f"cr_{revision}", task_id, revision, "route-1", "codex-acp", "conn-1", "{}",
                serialize_ceiling(ceiling), "selected", None, None, store._now(), store._now(),
            ),
        )
        db.commit()


def test_diagnostics_summary_returns_every_declared_metric(tmp_path):
    store, task_id = _task(tmp_path)
    summary = observability_metrics.diagnostics_summary(store, task_id)
    for key in (
        "resource_caps", "recovery_latency", "repeat_calls", "schema_validation_errors",
        "manual_interventions", "duplicate_publications", "tokens", "concurrency",
        "route_acceptance_latency", "route_fix_cycles", "base_prompt",
    ):
        assert key in summary
    # No configuration revision recorded yet -> honestly unavailable, never fabricated.
    assert summary["resource_caps"]["availability"] == "unavailable_reason"
    # No fallback attempts recorded yet -> honestly unavailable.
    assert summary["recovery_latency"]["availability"] == "unavailable_reason"
    assert summary["route_acceptance_latency"]["availability"] == "unavailable_reason"
    # The complete task event query observed no requested review fix cycles.
    assert summary["route_fix_cycles"]["total_review_fix_cycles"] == 0
    # No usage recorded at all yet -> unavailable, never treated as 0 tokens used.
    assert summary["tokens"]["availability"] == "unavailable_reason"


def test_base_prompt_summary_counts_secret_free_events(tmp_path):
    store, task_id = _task(tmp_path)
    observability_events.append_event(
        store, task_id, "base_prompt", "unified", "observed",
        attempt_id="attempt-1", route_engine="codex-acp",
    )
    observability_events.append_event(
        store, task_id, "base_prompt", "native", "observed",
        attempt_id="attempt-2", route_engine="claude-agent-acp",
    )

    events = observability_events.list_events(store, task_id, category="base_prompt")
    assert [event["detail"] for event in events] == [{}, {}]
    assert observability_metrics.diagnostics_summary(store, task_id)["base_prompt"] == {
        "unit": "sessions", "availability": "available", "basis": "observed",
        "counts": {"unified": 1, "native": 1},
        "counts_by_engine": {"codex-acp": {"unified": 1}, "claude-agent-acp": {"native": 1}},
        "editions": {},
        "sessions_with_unresolved_catalog_entries": 0,
        "unresolved_catalog_entry_ids": [],
    }


def test_base_prompt_summary_splits_counts_by_recorded_prompt_edition(tmp_path):
    store, task_id = _task(tmp_path)
    observability_events.append_event(
        store, task_id, "base_prompt", "unified", "observed",
        attempt_id="attempt-1", route_engine="codex-acp", detail={"prompt_edition": 1},
    )
    observability_events.append_event(
        store, task_id, "base_prompt", "unified", "observed",
        attempt_id="attempt-2", route_engine="claude-agent-acp", detail={"prompt_edition": 2},
    )
    # No edition recorded -> excluded from the per-edition breakdown, but
    # still counted in the existing per-mode ``counts``.
    observability_events.append_event(
        store, task_id, "base_prompt", "native", "observed",
        attempt_id="attempt-3", route_engine="opencode",
    )

    summary = observability_metrics.diagnostics_summary(store, task_id)["base_prompt"]
    assert summary["counts"] == {"unified": 2, "native": 1}
    assert summary["editions"] == {"1": 1, "2": 1}
    assert summary["counts_by_engine"] == {
        "codex-acp": {"unified": 1}, "claude-agent-acp": {"unified": 1}, "opencode": {"native": 1},
    }


def test_base_prompt_summary_counts_unresolved_catalog_entries_as_aggregate_ids_only(tmp_path):
    store, task_id = _task(tmp_path)
    observability_events.append_event(
        store, task_id, "base_prompt", "native", "observed",
        attempt_id="attempt-1", route_engine="codex-acp",
        detail={"unresolved_catalog_count": 2, "unresolved_catalog_ids": ["index", "skill-authoring"]},
    )
    observability_events.append_event(
        store, task_id, "base_prompt", "unified", "observed",
        attempt_id="attempt-2", route_engine="claude-agent-acp",
        detail={"unresolved_catalog_count": 1, "unresolved_catalog_ids": ["index"]},
    )
    # A clean session carries no unresolved detail at all.
    observability_events.append_event(
        store, task_id, "base_prompt", "unified", "observed",
        attempt_id="attempt-3", route_engine="claude-agent-acp",
    )

    summary = observability_metrics.diagnostics_summary(store, task_id)["base_prompt"]
    assert summary["sessions_with_unresolved_catalog_entries"] == 2
    assert summary["unresolved_catalog_entry_ids"] == ["index", "skill-authoring"]


def test_route_outcome_metrics_group_accepted_results_and_fix_cycles(tmp_path):
    store, task_id = _task(tmp_path)
    _insert_plan_event(store, task_id, [{"id": "alpha", "role": "worker-backend-dev", "complexity": "normal"}])
    _insert_route_assignment(
        store, task_id, node_id="alpha", assignment_ref="assignment-alpha", attempt_id="attempt-alpha",
        dispatched_at="2026-01-01T00:00:00+00:00", accepted_at="2026-01-01T00:00:12+00:00",
    )
    with store.connection() as db:
        db.execute(
            "INSERT INTO runtime_events(task_id,type,actor,payload_json,created_at) VALUES(?,?,?,?,?)",
            (
                task_id, "REVIEW_CHANGES_REQUESTED", "runtime",
                json.dumps({"review": "_review_1", "fixes": ["_fix_1_alpha"]}),
                "2026-01-01T00:00:13+00:00",
            ),
        )
    _insert_route_assignment(
        store, task_id, node_id="_fix_1_alpha", assignment_ref="assignment-alpha-fix-1",
        attempt_id="attempt-alpha-fix-1", dispatched_at="2026-01-01T00:00:15+00:00",
        accepted_at="2026-01-01T00:00:20+00:00", engine="claude-agent-acp", model="opus",
    )

    summary = observability_metrics.diagnostics_summary(store, task_id)
    latency = summary["route_acceptance_latency"]
    assert latency["availability"] == "available"
    assert latency["basis"] == "derived"
    assert latency["sample_count"] == 2
    by_model = {group["model"]: group for group in latency["groups"]}
    assert by_model["gpt-5.6"]["mean_seconds"] == 12
    assert by_model["opus"]["mean_seconds"] == 5
    assert by_model["opus"]["complexity"] == "normal"
    assert by_model["opus"]["role"] == "worker-backend-dev"

    cycles = summary["route_fix_cycles"]
    assert cycles["total_review_fix_cycles"] == 1
    assert cycles["attributed_review_fix_cycles"] == 1
    assert cycles["unattributed_review_fix_cycles"] == 0
    assert cycles["groups"] == [{
        "engine": "claude-agent-acp", "model": "opus", "role": "worker-backend-dev",
        "complexity": "normal", "fix_cycle_count": 1, "fix_assignment_count": 1,
    }]


def test_route_acceptance_latency_discloses_missing_route_evidence(tmp_path):
    store, task_id = _task(tmp_path)
    _insert_plan_event(store, task_id, [{"id": "alpha", "role": "worker-backend-dev"}])
    _insert_route_assignment(
        store, task_id, node_id="alpha", assignment_ref="assignment-alpha", attempt_id="attempt-alpha",
        dispatched_at="2026-01-01T00:00:00+00:00", accepted_at="2026-01-01T00:00:12+00:00",
        route_audit=False,
    )

    metric = observability_metrics.diagnostics_summary(store, task_id)["route_acceptance_latency"]
    assert metric["availability"] == "unavailable_reason"
    assert metric["sample_count"] == 0
    assert metric["excluded"]["missing_route_audit"] == 1
    assert "mean_seconds" not in metric


def test_route_acceptance_latency_requires_exact_node_binding(tmp_path):
    store, task_id = _task(tmp_path)
    _insert_plan_event(store, task_id, [{"id": "alpha", "role": "worker-backend-dev"}])
    _insert_route_assignment(
        store, task_id, node_id="alpha", assignment_ref="assignment-alpha", attempt_id="attempt-alpha",
        dispatched_at="2026-01-01T00:00:00+00:00", accepted_at="2026-01-01T00:00:12+00:00",
        event_node_id="other-node",
    )

    metric = observability_metrics.diagnostics_summary(store, task_id)["route_acceptance_latency"]
    assert metric["availability"] == "unavailable_reason"
    assert metric["sample_count"] == 0
    assert metric["excluded"]["missing_node_binding"] == 1


def test_resource_caps_never_reports_a_monetary_cap_without_enforceability(tmp_path):
    store, task_id = _task(tmp_path)
    ceiling = PolicyCeiling(limits=dict(attempts=5, calls=20, tokens=100000, concurrency=2))
    _insert_revision(store, task_id, ceiling)
    caps = observability_metrics.resource_caps(store, task_id)
    assert caps["availability"] == "available"
    assert caps["call_token_concurrency_limits"] == dict(attempts=5, calls=20, tokens=100000, concurrency=2)
    assert caps["monetary_cap"] is None
    assert "no_enforced_spend_limit" in caps["monetary_cap_basis"]


def test_resource_caps_reports_monetary_cap_only_when_enforceable(tmp_path):
    store, task_id = _task(tmp_path)
    ceiling = PolicyCeiling(spend_limit=500, spend_limit_enforceable=True)
    _insert_revision(store, task_id, ceiling)
    caps = observability_metrics.resource_caps(store, task_id)
    assert caps["monetary_cap"] == 500
    assert caps["monetary_cap_basis"] == "enforced"


def test_schema_validation_errors_and_duplicate_publications_are_counted(tmp_path):
    store, task_id = _task(tmp_path)
    observability_events.append_event(store, task_id, "validation", "invalid_field", "observed")
    observability_events.append_event(store, task_id, "validation", "invalid_field", "observed")
    observability_events.append_event(store, task_id, "publication", "duplicate_attempt", "observed")
    summary = observability_metrics.diagnostics_summary(store, task_id)
    assert summary["schema_validation_errors"] == dict(unit="count", availability="available", basis="observed", total=2)
    assert summary["duplicate_publications"]["total"] == 1


def test_schema_validation_errors_include_submission_rejections(tmp_path):
    store, task_id = _task(tmp_path)
    observability_events.append_event(store, task_id, "submission_rejection", "invalid_arguments", "observed")
    summary = observability_metrics.diagnostics_summary(store, task_id)
    assert summary["schema_validation_errors"] == dict(unit="count", availability="available", basis="observed", total=1)


def test_repeat_calls_and_tokens_reflect_usage_records(tmp_path):
    store, task_id = _task(tmp_path)
    observability_usage.record_call_usage(
        store, task_id=task_id, attempt_id="att-1", engine_session_ref="sess-1",
        usage_basis=UsageBasis.ENGINE_REPORTED, usage_values=dict(input_tokens=10), stop_reason="end_turn", failed=False,
    )
    observability_usage.record_call_usage(
        store, task_id=task_id, attempt_id="att-1", engine_session_ref="sess-1",
        usage_basis=UsageBasis.ENGINE_REPORTED, usage_values=dict(input_tokens=5), stop_reason="end_turn", failed=False,
    )
    summary = observability_metrics.diagnostics_summary(store, task_id)
    assert summary["repeat_calls"]["sessions_with_repeat_calls"] == 1
    assert summary["repeat_calls"]["total_repeat_calls"] == 1
    assert summary["tokens"]["availability"] == "available"
    assert summary["tokens"]["total_calls"] == 2
