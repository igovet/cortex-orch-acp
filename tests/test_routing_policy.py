"""T027: RouteProfile, attempt-route-audit, fallback-consent store, and
pre-dispatch/fallback authorization -- unit-level (no engine subprocess).
See ``tests/test_delegation_routing_dispatch.py`` for the real
``EngineSessionManagerAdapter``/fake-engine-subprocess integration.
"""
from __future__ import annotations

import uuid

import pytest

from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.configuration.policy import (
    CAPACITY_UNAVAILABLE,
    ENGINE_FAILURE,
    FallbackPolicy,
    PolicyCeiling,
    RouteProfile,
)
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.portable.errors import PortableError
from cortex_runtime.routing import authorization as routing_authorization
from cortex_runtime.routing import store as routing_store
from cortex_runtime.orchestration.state import review_provider_preference
from cortex_runtime.store import Store


def _task(tmp_path):
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    result = bootstrap_task(store, SourceEvent(origin="cli", event_id=str(uuid.uuid4())), "Do work.", "owner-1")
    return store, result.task_id


class _Conn:
    def __init__(self, id, provider):
        self.id = id
        self.provider = provider


# --- RouteProfile ------------------------------------------------------

def test_route_profile_default_is_single_route_for_every_role():
    profile = RouteProfile()
    assert profile.is_default_single_route()
    assert profile.allowed_routes_for("worker", "rt_a") == frozenset({"rt_a"})
    assert profile.allowed_routes_for("reviewer", "rt_a") == frozenset({"rt_a"})


def test_route_profile_mixed_team_role_not_listed_falls_back_to_coordinator_route():
    profile = RouteProfile(role_routes={"reviewer": {"rt_b"}})
    assert profile.allowed_routes_for("reviewer", "rt_a") == frozenset({"rt_b"})
    # Generic buckets (no resolved role identity) still default to the
    # coordinator route when absent from an explicit profile.
    assert profile.allowed_routes_for("worker", "rt_a") == frozenset({"rt_a"})
    assert profile.allowed_routes_for("coordinator", "rt_a") == frozenset({"rt_a"})
    # A NAMED role absent from an explicit profile is denied by default --
    # a mixed-team profile is a closed allowlist, not "unlisted roles
    # inherit the coordinator route" (review fix, T027).
    assert profile.allowed_routes_for("implementer", "rt_a") == frozenset()


def test_route_profile_empty_role_entry_means_never_dispatched():
    profile = RouteProfile(role_routes={"reviewer": frozenset()})
    assert profile.allowed_routes_for("reviewer", "rt_a") == frozenset()


# --- allowed_routes_for_role narrows by ceiling -------------------------

def test_allowed_routes_for_role_narrowed_by_ceiling_allowed_routes():
    profile = RouteProfile(role_routes={"reviewer": {"rt_a", "rt_b"}})
    ceiling = PolicyCeiling(allowed_routes={"rt_a"})
    allowed = routing_authorization.allowed_routes_for_role(profile, ceiling, "reviewer", "rt_a")
    assert allowed == frozenset({"rt_a"})


# --- authorize_dispatch --------------------------------------------------

def test_authorize_dispatch_rejects_role_not_in_profile(tmp_path):
    store, task_id = _task(tmp_path)
    profile = RouteProfile(role_routes={"reviewer": {"rt_b"}})
    with pytest.raises(PortableError) as excinfo:
        routing_authorization.authorize_dispatch(
            store, task_id=task_id, role="reviewer", route_profile=profile, coordinator_route_id="rt_a",
            candidate_route_id="rt_a", ceiling=PolicyCeiling(),
        )
    assert excinfo.value.code == "role_route_not_authorized"


def test_authorize_dispatch_permits_default_single_route(tmp_path):
    store, task_id = _task(tmp_path)
    routing_authorization.authorize_dispatch(
        store, task_id=task_id, role="worker", route_profile=RouteProfile(), coordinator_route_id="rt_a",
        candidate_route_id="rt_a", ceiling=PolicyCeiling(),
    )  # no raise


def test_authorize_dispatch_enforces_attempt_budget(tmp_path):
    store, task_id = _task(tmp_path)
    ceiling = PolicyCeiling(limits={"attempts": 2})
    profile = RouteProfile()
    for i in range(2):
        routing_authorization.authorize_dispatch(
            store, task_id=task_id, role="coordinator", route_profile=profile, coordinator_route_id="rt_a",
            candidate_route_id="rt_a", ceiling=ceiling,
        )
        routing_store.record_attempt_audit(
            store, attempt_id=f"a{i}", task_id=task_id, role="coordinator", engine="codex-acp",
            connection_id="cn_x", model="m", params_fingerprint="fp", config_revision_id=None,
            decision="authorized", detail={},
        )
    with pytest.raises(PortableError) as excinfo:
        routing_authorization.authorize_dispatch(
            store, task_id=task_id, role="coordinator", route_profile=profile, coordinator_route_id="rt_a",
            candidate_route_id="rt_a", ceiling=ceiling,
        )
    assert excinfo.value.code == "attempt_budget_exhausted"


# --- authorize_fallback: anti-bypass (hard denylist + unknown classes) --

@pytest.mark.parametrize("failure_class", ["cancellation", "refusal", "authorization_loss", "provider_restriction"])
def test_authorize_fallback_never_permits_hard_denylist(tmp_path, failure_class):
    store, task_id = _task(tmp_path)
    ceiling = PolicyCeiling(
        fallback_policy=FallbackPolicy(allowed_failure_classes=None, data_recipient_consent=True, budget=10)
    )
    current = _Conn("cn_a", "acme")
    target = _Conn("cn_b", "acme")
    with pytest.raises(PortableError) as excinfo:
        routing_authorization.authorize_fallback(
            store, task_id=task_id, ceiling=ceiling, failure_class=failure_class, role="worker",
            route_profile=RouteProfile(), coordinator_route_id="rt_a",
            current_connection=current, target_connection=target, target_route_id="rt_b",
        )
    assert excinfo.value.code == "fallback_not_authorized"


def test_authorize_fallback_rejects_class_outside_known_eligible_set(tmp_path):
    store, task_id = _task(tmp_path)
    ceiling = PolicyCeiling(fallback_policy=FallbackPolicy(allowed_failure_classes=None, data_recipient_consent=True))
    current = _Conn("cn_a", "acme")
    target = _Conn("cn_b", "acme")
    with pytest.raises(PortableError) as excinfo:
        routing_authorization.authorize_fallback(
            store, task_id=task_id, ceiling=ceiling, failure_class="some_other_class", role="worker",
            route_profile=RouteProfile(), coordinator_route_id="rt_a",
            current_connection=current, target_connection=target, target_route_id="rt_b",
        )
    assert excinfo.value.code == "fallback_not_authorized"


def test_authorize_fallback_without_policy_stops_and_reports(tmp_path):
    store, task_id = _task(tmp_path)
    ceiling = PolicyCeiling()  # default FallbackPolicy(): no classes, no consent
    current = _Conn("cn_a", "acme")
    target = _Conn("cn_b", "acme")
    with pytest.raises(PortableError) as excinfo:
        routing_authorization.authorize_fallback(
            store, task_id=task_id, ceiling=ceiling, failure_class=ENGINE_FAILURE, role="worker",
            route_profile=RouteProfile(), coordinator_route_id="rt_a",
            current_connection=current, target_connection=target, target_route_id="rt_b",
        )
    assert excinfo.value.code == "fallback_not_authorized"


def test_authorize_fallback_same_provider_never_requires_consent_record(tmp_path):
    store, task_id = _task(tmp_path)
    ceiling = PolicyCeiling(
        allowed_routes={"rt_a", "rt_b"},
        fallback_policy=FallbackPolicy(
            allowed_target_routes={"rt_b"}, allowed_failure_classes={ENGINE_FAILURE}, data_recipient_consent=True,
        ),
    )
    current = _Conn("cn_a", "acme")
    target = _Conn("cn_b", "acme")  # same provider
    routing_authorization.authorize_fallback(
        store, task_id=task_id, ceiling=ceiling, failure_class=ENGINE_FAILURE, role="worker",
        route_profile=RouteProfile(role_routes={"worker": {"rt_a", "rt_b"}}), coordinator_route_id="rt_a",
        current_connection=current, target_connection=target, target_route_id="rt_b",
    )  # no raise: no durable consent record needed for a same-provider fallback


def test_authorize_fallback_cross_provider_requires_durable_consent_record(tmp_path):
    store, task_id = _task(tmp_path)
    ceiling = PolicyCeiling(
        allowed_routes={"rt_a", "rt_b"},
        fallback_policy=FallbackPolicy(
            allowed_target_routes={"rt_b"}, allowed_failure_classes={ENGINE_FAILURE}, data_recipient_consent=True,
        ),
    )
    current = _Conn("cn_a", "acme")
    target = _Conn("cn_b", "other-vendor")
    kwargs = dict(
        store=store, task_id=task_id, ceiling=ceiling, failure_class=ENGINE_FAILURE, role="worker",
        route_profile=RouteProfile(role_routes={"worker": {"rt_a", "rt_b"}}), coordinator_route_id="rt_a",
        current_connection=current, target_connection=target, target_route_id="rt_b",
    )
    # No durable consent record yet -> stop and report, no attempt.
    with pytest.raises(PortableError) as excinfo:
        routing_authorization.authorize_fallback(**kwargs)
    assert excinfo.value.code == "fallback_consent_required"

    routing_store.record_consent(
        store, task_id=task_id, connection_id="cn_b", provider="other-vendor",
        data_scope="task workspace and prompt history", granted_by="owner-1",
    )
    routing_authorization.authorize_fallback(**kwargs)  # now authorized

    # data_recipient_consent=False on the policy still refuses even with a
    # durable record present -- both are required, never either alone.
    ceiling_no_flag = PolicyCeiling(
        allowed_routes={"rt_a", "rt_b"},
        fallback_policy=FallbackPolicy(allowed_target_routes={"rt_b"}, allowed_failure_classes={ENGINE_FAILURE}, data_recipient_consent=False),
    )
    with pytest.raises(PortableError) as excinfo:
        routing_authorization.authorize_fallback(**{**kwargs, "ceiling": ceiling_no_flag})
    # FallbackPolicy.permits() itself already requires data_recipient_consent;
    # with the flag False the class is simply not permitted at all -- the
    # narrower fallback_consent_required code is reserved for "the policy
    # allows this in principle but the durable per-recipient record is
    # missing", a distinct, stricter failure than a blanket policy refusal.
    assert excinfo.value.code == "fallback_not_authorized"


def test_authorize_fallback_route_must_be_within_fallback_policy_targets(tmp_path):
    store, task_id = _task(tmp_path)
    ceiling = PolicyCeiling(
        allowed_routes={"rt_a", "rt_b", "rt_c"},
        fallback_policy=FallbackPolicy(allowed_target_routes={"rt_b"}, allowed_failure_classes={ENGINE_FAILURE}, data_recipient_consent=True),
    )
    current = _Conn("cn_a", "acme")
    target = _Conn("cn_c", "acme")
    with pytest.raises(PortableError) as excinfo:
        routing_authorization.authorize_fallback(
            store, task_id=task_id, ceiling=ceiling, failure_class=ENGINE_FAILURE, role="worker",
            route_profile=RouteProfile(role_routes={"worker": {"rt_a", "rt_b", "rt_c"}}), coordinator_route_id="rt_a",
            current_connection=current, target_connection=target, target_route_id="rt_c",
        )
    assert excinfo.value.code == "fallback_route_not_allowed"


def test_authorize_fallback_route_must_be_within_role_route_profile_too(tmp_path):
    """Fallback never bypasses the role's own route-profile authorization,
    even when the fallback_policy itself allows the target route."""
    store, task_id = _task(tmp_path)
    ceiling = PolicyCeiling(
        allowed_routes={"rt_a", "rt_b"},
        fallback_policy=FallbackPolicy(allowed_target_routes={"rt_b"}, allowed_failure_classes={ENGINE_FAILURE}, data_recipient_consent=True),
    )
    current = _Conn("cn_a", "acme")
    target = _Conn("cn_b", "acme")
    with pytest.raises(PortableError) as excinfo:
        routing_authorization.authorize_fallback(
            store, task_id=task_id, ceiling=ceiling, failure_class=ENGINE_FAILURE, role="worker",
            route_profile=RouteProfile(role_routes={"worker": {"rt_a"}}),  # rt_b NOT authorized for worker
            coordinator_route_id="rt_a",
            current_connection=current, target_connection=target, target_route_id="rt_b",
        )
    assert excinfo.value.code == "fallback_role_route_not_authorized"


def test_authorize_fallback_budget_exhausted(tmp_path):
    store, task_id = _task(tmp_path)
    ceiling = PolicyCeiling(
        allowed_routes={"rt_a", "rt_b"},
        fallback_policy=FallbackPolicy(
            allowed_target_routes={"rt_b"}, allowed_failure_classes={CAPACITY_UNAVAILABLE},
            data_recipient_consent=True, budget=1,
        ),
    )
    current = _Conn("cn_a", "acme")
    target = _Conn("cn_b", "acme")
    kwargs = dict(
        store=store, task_id=task_id, ceiling=ceiling, failure_class=CAPACITY_UNAVAILABLE, role="worker",
        route_profile=RouteProfile(role_routes={"worker": {"rt_a", "rt_b"}}), coordinator_route_id="rt_a",
        current_connection=current, target_connection=target, target_route_id="rt_b",
    )
    routing_authorization.authorize_fallback(**kwargs)  # 1st: authorized
    routing_store.record_attempt_audit(
        store, attempt_id="fb1", task_id=task_id, role="worker", engine="codex-acp", connection_id="cn_b",
        model="m", params_fingerprint="fp", config_revision_id=None, decision="fallback", detail={},
        predecessor_attempt_id="a0",
    )
    with pytest.raises(PortableError) as excinfo:
        routing_authorization.authorize_fallback(**kwargs)  # budget of 1 already used
    assert excinfo.value.code == "fallback_budget_exhausted"


# --- consent store -------------------------------------------------------

def test_consent_record_lifecycle(tmp_path):
    store, task_id = _task(tmp_path)
    assert not routing_store.has_active_consent(store, task_id, "cn_b")
    consent_id = routing_store.record_consent(
        store, task_id=task_id, connection_id="cn_b", provider="other-vendor",
        data_scope="prompt history", granted_by="owner-1",
    )
    assert routing_store.has_active_consent(store, task_id, "cn_b")
    routing_store.revoke_consent(store, consent_id)
    assert not routing_store.has_active_consent(store, task_id, "cn_b")


# --- audit store -----------------------------------------------------------

def test_attempt_audit_records_fallback_predecessor_link(tmp_path):
    store, task_id = _task(tmp_path)
    routing_store.record_attempt_audit(
        store, attempt_id="a0", task_id=task_id, role="worker", engine="codex-acp", connection_id="cn_a",
        model="model-a", params_fingerprint="fp0", config_revision_id="cr_1", decision="authorized", detail={"reason": "ok"},
    )
    routing_store.record_attempt_audit(
        store, attempt_id="a1", task_id=task_id, role="worker", engine="claude-agent-acp", connection_id="cn_b",
        model="model-a", params_fingerprint="fp1", config_revision_id="cr_1", decision="fallback",
        detail={"failure_class": ENGINE_FAILURE}, predecessor_attempt_id="a0",
    )
    audit = routing_store.list_audit_for_task(store, task_id)
    assert [row["attempt_id"] for row in audit] == ["a0", "a1"]
    assert audit[1]["decision"] == "fallback"
    assert audit[1]["predecessor_attempt_id"] == "a0"
    assert audit[1]["engine"] == "claude-agent-acp"
    assert audit[1]["connection_id"] == "cn_b"


def test_attempt_audit_is_immutable(tmp_path):
    store, task_id = _task(tmp_path)
    routing_store.record_attempt_audit(
        store, attempt_id="a0", task_id=task_id, role="worker", engine="codex-acp", connection_id="cn_a",
        model="model-a", params_fingerprint="fp0", config_revision_id=None, decision="authorized", detail={},
    )
    import sqlite3
    with store.connection() as db:
        with pytest.raises(sqlite3.IntegrityError):
            db.execute("UPDATE attempt_route_audit SET decision='fallback' WHERE attempt_id='a0'")


def test_fingerprint_params_stable_and_order_independent():
    a = routing_authorization.fingerprint_params({"effort": "high", "mode": "default"})
    b = routing_authorization.fingerprint_params({"mode": "default", "effort": "high"})
    c = routing_authorization.fingerprint_params({"effort": "low", "mode": "default"})
    assert a == b
    assert a != c


def test_review_provider_preference_uses_completed_execution_audit(tmp_path):
    store, task_id = _task(tmp_path)
    with store.connection() as db:
        revision = 1
        db.execute("INSERT INTO runtime_tasks VALUES(?,?,?,? ,NULL,NULL,0,NULL,?,?)",
                   (task_id, "REVIEWING", revision, "objective", "now", "now"))
        for node_id, assignment, attempt, role, stage, status in (
            ("implementation", "as_execution", "xa_execution", "worker-backend-dev", "execution", "completed"),
            ("review", "as_review", "xa_review", "worker-code-reviewer", "review", "pending"),
        ):
            db.execute(
                "INSERT INTO runtime_nodes VALUES(?,?,?,?,?,?,?,?,?,?,?)",
                (task_id, revision, node_id, role, "objective", "[]", "[\".\"]", stage, status, assignment, None),
            )
            db.execute(
                "INSERT INTO delegation_provisioning "
                "(idempotency_key,task_id,assignment_id,attempt_id,role_id,step,status,owner_epoch,created_at,updated_at) "
                "VALUES(?,?,?,?,?,'ready','ready',1,'now','now')",
                (assignment, task_id, assignment, attempt, role),
            )
        db.commit()
    routing_store.record_attempt_audit(
        store, attempt_id="xa_execution", task_id=task_id, role="worker-backend-dev",
        engine="claude-agent-acp", connection_id="cn_claude", model="sonnet",
        params_fingerprint="fp", config_revision_id=None, decision="authorized", detail={},
    )
    assert review_provider_preference(store, task_id, "xa_review") == (
        "codex-acp", "prefer_codex_after_claude_execution",
    )
    routing_store.record_attempt_audit(
        store, attempt_id="xa_other", task_id=task_id, role="worker-qa-engineer",
        engine="codex-acp", connection_id="cn_codex", model="gpt-5.6-luna",
        params_fingerprint="fp", config_revision_id=None, decision="authorized", detail={},
    )
    assert review_provider_preference(store, task_id, "xa_review") == (
        "codex-acp", "prefer_codex_after_claude_execution",
    )
