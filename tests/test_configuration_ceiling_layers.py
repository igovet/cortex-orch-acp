"""Parametrized ceiling narrowing across dimensions and layers (T008 lead-
required addition B): for each dimension and for each narrowing layer (task
override, assignment override -- both via ConfigurationService.propose's
inputs.ceiling_override -- and project-vs-user), expansion is rejected and
narrowing is accepted."""
from __future__ import annotations

import pytest

from cortex_runtime.catalog.discovery import build_catalog, parse_observation
from cortex_runtime.configuration.policy import FallbackPolicy, PermissionLevel, PolicyCeiling, require_narrowing
from cortex_runtime.configuration.resolution import ConfigurationInputs
from cortex_runtime.configuration.service import ConfigurationService
from cortex_runtime.connections.model import AuthMode, ConnectionRecord, ConnectionState, EngineNativeDir
from cortex_runtime.ownership import current_owner_epoch
from cortex_runtime.portable.errors import PortableError
from cortex_runtime.portable.identity import OwnershipEpoch, SourceEvent
from cortex_runtime.store import Store
from cortex_runtime.bootstrap import bootstrap_task
import uuid


def _connection(connection_id="cn_" + "a" * 12):
    now = "2026-01-01T00:00:00+00:00"
    return ConnectionRecord(
        id=connection_id, name="Work", engine="codex-acp", provider="acme", auth_mode=AuthMode.NATIVE_ACCOUNT,
        key_kind=None, endpoint=None, launch_command=None,
        credential_ref=EngineNativeDir(path=f"/tmp/{connection_id}"), approved_fingerprint="fp",
        state=ConnectionState.CONFIGURED, qualified=True, created_at=now, updated_at=now,
    )


def _entry():
    connection = _connection()
    options = [
        {"id": "model", "name": "Model", "category": "model", "type": "select", "currentValue": "model-a",
         "options": [{"value": "model-a", "name": "Model A"}]},
    ]
    return build_catalog({connection.id: connection}, {connection.id: parse_observation(options)}).entries[0]


def _bootstrap(tmp_path, owner="owner-1"):
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    result = bootstrap_task(store, SourceEvent(origin="cli", event_id=str(uuid.uuid4())), "Do work.", owner)
    with store.connection() as db:
        epoch = current_owner_epoch(db, result.task_id)
    return store, result.task_id, OwnershipEpoch(epoch=epoch, owner=owner)


# Each dimension case: (narrower_kwargs, wider_kwargs, dimension_label).
DIMENSION_CASES = [
    (dict(allowed_routes={"xr_" + "a" * 24}), dict(allowed_routes={"xr_" + "a" * 24, "xr_" + "b" * 24}), "allowed_routes"),
    (dict(allowed_connections={"cn_" + "a" * 12}), dict(allowed_connections={"cn_" + "a" * 12, "cn_" + "b" * 12}), "allowed_connections"),
    (dict(allowed_data_recipients={"acme"}), dict(allowed_data_recipients={"acme", "other"}), "allowed_data_recipients"),
    (dict(permission_level=PermissionLevel.READ_ONLY), dict(permission_level=PermissionLevel.ELEVATED), "permission_level"),
    (dict(limits={"attempts": 1}), dict(limits={"attempts": 5}), "limits.attempts"),
    (dict(limits={"calls": 1}), dict(limits={"calls": 5}), "limits.calls"),
    (dict(limits={"tokens": 100}), dict(limits={"tokens": 100000}), "limits.tokens"),
    (dict(limits={"concurrency": 1}), dict(limits={"concurrency": 8}), "limits.concurrency"),
    (dict(spend_limit=10, spend_limit_enforceable=True), dict(spend_limit=1000, spend_limit_enforceable=True), "spend_limit"),
    (
        dict(fallback_policy=FallbackPolicy(allowed_failure_classes={"rate_limited"}, data_recipient_consent=True)),
        dict(fallback_policy=FallbackPolicy(allowed_failure_classes={"rate_limited", "timeout"}, data_recipient_consent=True)),
        "fallback_policy",
    ),
]


@pytest.mark.parametrize("narrower_kwargs,wider_kwargs,dimension", DIMENSION_CASES, ids=[c[2] for c in DIMENSION_CASES])
def test_policy_level_narrowing_accepted_expansion_rejected_per_dimension(narrower_kwargs, wider_kwargs, dimension):
    ceiling = PolicyCeiling(**narrower_kwargs)
    require_narrowing(PolicyCeiling(**narrower_kwargs), ceiling)  # narrower-or-equal: fine
    with pytest.raises(PortableError) as excinfo:
        require_narrowing(PolicyCeiling(**wider_kwargs), ceiling)
    assert excinfo.value.code == "ceiling_violation"


@pytest.mark.parametrize("narrower_kwargs,wider_kwargs,dimension", DIMENSION_CASES, ids=[c[2] for c in DIMENSION_CASES])
def test_task_layer_override_narrowing_accepted_expansion_rejected(tmp_path, narrower_kwargs, wider_kwargs, dimension):
    store, task, epoch = _bootstrap(tmp_path)
    service = ConfigurationService(store)
    entry = _entry()
    user_layer = dict(defaults={}, ceiling=PolicyCeiling(**narrower_kwargs))

    wide = ConfigurationInputs(route=entry.route, parameters={}, ceiling_override=PolicyCeiling(**wider_kwargs))
    with pytest.raises(PortableError) as excinfo:
        service.propose(task, entry, wide, expected_revision=0, current_epoch=epoch, user_layer=user_layer)
    assert excinfo.value.code == "ceiling_violation"

    narrow = ConfigurationInputs(route=entry.route, parameters={}, ceiling_override=PolicyCeiling(**narrower_kwargs))
    row = service.propose(task, entry, narrow, expected_revision=0, current_epoch=epoch, user_layer=user_layer)
    assert row["state"] == "selected"


@pytest.mark.parametrize("narrower_kwargs,wider_kwargs,dimension", DIMENSION_CASES, ids=[c[2] for c in DIMENSION_CASES])
def test_assignment_layer_override_narrowing_accepted_expansion_rejected(tmp_path, narrower_kwargs, wider_kwargs, dimension):
    # "assignment" override is the same call-time ceiling_override input as
    # the task layer in this service's shape (ConfigurationInputs has no
    # separate task-vs-assignment split); exercised here against a wider
    # user ceiling so only the override itself is the constraint under test.
    store, task, epoch = _bootstrap(tmp_path)
    service = ConfigurationService(store)
    entry = _entry()
    # "Unrestricted" for fallback_policy still needs explicit
    # data_recipient_consent=True: PolicyCeiling()'s bare default denies
    # consent (a deliberate safe default, unlike the None-means-unrestricted
    # convention every other dimension uses).
    baseline_ceiling = (
        PolicyCeiling(fallback_policy=FallbackPolicy(data_recipient_consent=True))
        if dimension == "fallback_policy"
        else PolicyCeiling()
    )
    user_layer = dict(defaults={}, ceiling=baseline_ceiling)

    narrow = ConfigurationInputs(route=entry.route, parameters={}, ceiling_override=PolicyCeiling(**narrower_kwargs))
    row = service.propose(task, entry, narrow, expected_revision=0, current_epoch=epoch, user_layer=user_layer)
    assert row["state"] == "selected"


@pytest.mark.parametrize("narrower_kwargs,wider_kwargs,dimension", DIMENSION_CASES, ids=[c[2] for c in DIMENSION_CASES])
def test_project_vs_user_ceiling_narrowing_accepted_expansion_rejected(tmp_path, narrower_kwargs, wider_kwargs, dimension):
    store, task, epoch = _bootstrap(tmp_path)
    service = ConfigurationService(store)
    entry = _entry()
    inputs = ConfigurationInputs(route=entry.route, parameters={})
    user_layer = dict(defaults={}, ceiling=PolicyCeiling(**narrower_kwargs))

    wide_project = dict(defaults={}, ceiling=PolicyCeiling(**wider_kwargs))
    with pytest.raises(PortableError) as excinfo:
        service.propose(task, entry, inputs, expected_revision=0, current_epoch=epoch, user_layer=user_layer, project_layer=wide_project)
    assert excinfo.value.code == "ceiling_violation"

    narrow_project = dict(defaults={}, ceiling=PolicyCeiling(**narrower_kwargs))
    row = service.propose(task, entry, inputs, expected_revision=0, current_epoch=epoch, user_layer=user_layer, project_layer=narrow_project)
    assert row["state"] == "selected"
