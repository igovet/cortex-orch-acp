"""T029: observability.metrics -- diagnostics_summary/resource_caps, each
metric declaring its own unit and availability.

Every Store is rooted at tmp_path (never the real checkout); no engine or
network I/O.
"""
from __future__ import annotations

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
    ):
        assert key in summary
    # No configuration revision recorded yet -> honestly unavailable, never fabricated.
    assert summary["resource_caps"]["availability"] == "unavailable_reason"
    # No fallback attempts recorded yet -> honestly unavailable.
    assert summary["recovery_latency"]["availability"] == "unavailable_reason"
    # No usage recorded at all yet -> unavailable, never treated as 0 tokens used.
    assert summary["tokens"]["availability"] == "unavailable_reason"


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
