"""PolicyCeiling dimensions, intersection, and narrowing enforcement (T008)."""
from __future__ import annotations

import pytest

from cortex_runtime.configuration.policy import (
    CANCELLATION,
    FallbackPolicy,
    PermissionLevel,
    PolicyCeiling,
    PROVIDER_RESTRICTION,
    REFUSAL,
    AUTHORIZATION_LOSS,
    require_narrowing,
)
from cortex_runtime.portable.errors import PortableError


def test_fallback_policy_rejects_hard_denylisted_failure_class_at_construction():
    for failure_class in (CANCELLATION, REFUSAL, AUTHORIZATION_LOSS, PROVIDER_RESTRICTION):
        with pytest.raises(PortableError) as excinfo:
            FallbackPolicy(allowed_failure_classes={failure_class}, data_recipient_consent=True)
        assert excinfo.value.code == "fallback_denylisted_failure_class"


def test_fallback_permits_only_consented_non_denylisted_class():
    policy = FallbackPolicy(allowed_failure_classes={"rate_limited"}, data_recipient_consent=True)
    assert policy.permits("rate_limited") is True
    assert policy.permits(CANCELLATION) is False
    unconsented = FallbackPolicy(allowed_failure_classes={"rate_limited"}, data_recipient_consent=False)
    assert unconsented.permits("rate_limited") is False


def test_spend_limit_requires_enforceable_telemetry():
    with pytest.raises(PortableError) as excinfo:
        PolicyCeiling(spend_limit=100, spend_limit_enforceable=False)
    assert excinfo.value.code == "unenforceable_spend_limit"
    PolicyCeiling(spend_limit=100, spend_limit_enforceable=True)  # accepted


@pytest.mark.parametrize(
    "narrower,wider,dimension",
    [
        (dict(allowed_routes={"xr_a"}), dict(allowed_routes={"xr_a", "xr_b"}), "allowed_routes"),
        (dict(allowed_connections={"cn_" + "a" * 12}), dict(allowed_connections={"cn_" + "a" * 12, "cn_" + "b" * 12}), "allowed_connections"),
        (dict(allowed_data_recipients={"acme"}), dict(allowed_data_recipients={"acme", "other"}), "allowed_data_recipients"),
        (dict(permission_level=PermissionLevel.READ_ONLY), dict(permission_level=PermissionLevel.ELEVATED), "permission_level"),
        (dict(limits={"attempts": 1}), dict(limits={"attempts": 5}), "limits"),
    ],
)
def test_narrowing_accepted_and_expansion_rejected_per_dimension(narrower, wider, dimension):
    ceiling = PolicyCeiling(**narrower)
    require_narrowing(ceiling, ceiling)  # narrowing to itself is always fine
    expanded = PolicyCeiling(**wider)
    with pytest.raises(PortableError) as excinfo:
        require_narrowing(expanded, ceiling)
    assert excinfo.value.code == "ceiling_violation"
    assert dimension in excinfo.value.field


def test_narrowing_rejects_spend_limit_expansion_and_accepts_narrowing():
    ceiling = PolicyCeiling(spend_limit=100, spend_limit_enforceable=True)
    require_narrowing(PolicyCeiling(spend_limit=50, spend_limit_enforceable=True), ceiling)
    with pytest.raises(PortableError) as excinfo:
        require_narrowing(PolicyCeiling(spend_limit=200, spend_limit_enforceable=True), ceiling)
    assert excinfo.value.code == "ceiling_violation"


def test_narrowing_rejects_fallback_expansion_on_every_fallback_dimension():
    ceiling = PolicyCeiling(
        fallback_policy=FallbackPolicy(allowed_failure_classes={"rate_limited"}, budget=10, data_recipient_consent=True)
    )
    require_narrowing(
        PolicyCeiling(fallback_policy=FallbackPolicy(allowed_failure_classes=set(), budget=5, data_recipient_consent=False)),
        ceiling,
    )
    with pytest.raises(PortableError):
        require_narrowing(
            PolicyCeiling(fallback_policy=FallbackPolicy(allowed_failure_classes={"rate_limited", "timeout"}, data_recipient_consent=True)),
            ceiling,
        )
    with pytest.raises(PortableError):
        require_narrowing(
            PolicyCeiling(fallback_policy=FallbackPolicy(allowed_failure_classes={"rate_limited"}, budget=99, data_recipient_consent=True)),
            ceiling,
        )
    with pytest.raises(PortableError):
        require_narrowing(
            PolicyCeiling(
                fallback_policy=FallbackPolicy(allowed_failure_classes=set(), data_recipient_consent=True)
            ),
            PolicyCeiling(fallback_policy=FallbackPolicy(allowed_failure_classes=set(), data_recipient_consent=False)),
        )


def test_intersect_takes_the_more_restrictive_value_per_dimension():
    user = PolicyCeiling(allowed_routes={"xr_a", "xr_b"}, permission_level=PermissionLevel.ELEVATED, limits={"attempts": 10})
    project = PolicyCeiling(allowed_routes={"xr_a"}, permission_level=PermissionLevel.STANDARD, limits={"attempts": 3})
    effective = user.intersect(project)
    assert effective.allowed_routes == {"xr_a"}
    assert effective.permission_level == PermissionLevel.STANDARD
    assert effective.limits == {"attempts": 3}


def test_intersect_never_lets_an_unenforceable_layer_grant_a_spend_limit():
    enforceable = PolicyCeiling(spend_limit=100, spend_limit_enforceable=True)
    unenforceable = PolicyCeiling()  # no spend_limit at all: unknown cost, never zero
    effective = enforceable.intersect(unenforceable)
    assert effective.spend_limit == 100 and effective.spend_limit_enforceable is True
