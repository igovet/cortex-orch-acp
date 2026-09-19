"""D-T035-03: per-role model/effort route policy.

Pure ``configuration.route_policy`` coverage (precedence, parsing/malformed
handling, effort resolution/refusal) plus the ceiling-still-wins guarantee
and the real ``EngineSessionManagerAdapter._authorize_dispatch`` wiring
(malformed-file recording, refusal reason, default-effort application).
"""
from __future__ import annotations

import json
import uuid

import pytest

from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.catalog.discovery import build_catalog, parse_observation
from cortex_runtime.configuration import route_policy as route_policy_module
from cortex_runtime.configuration.policy import PolicyCeiling, RouteProfile
from cortex_runtime.configuration.resolution import ConfigurationInputs
from cortex_runtime.configuration.service import ConfigurationService
from cortex_runtime.connections.model import AuthMode, ConnectionRecord, ConnectionState, EngineNativeDir
from cortex_runtime.delegation.live_adapters import EngineSessionManagerAdapter
from cortex_runtime.observability import events as observability_events
from cortex_runtime.ownership import current_owner_epoch
from cortex_runtime.portable.errors import PortableError
from cortex_runtime.portable.identity import OwnershipEpoch, SourceEvent
from cortex_runtime.routing import authorization as routing_authorization
from cortex_runtime.store import Store


# --- helpers -------------------------------------------------------------

def _entry(connection, effort="high", model="model-a"):
    options = [
        {"id": "model", "name": "Model", "category": "model", "type": "select", "currentValue": model,
         "options": [{"value": model, "name": model}]},
        {"id": "effort", "name": "Effort", "category": "thought_level", "type": "select", "currentValue": effort,
         "options": [{"value": effort, "name": effort}]},
    ]
    snapshot = build_catalog({connection.id: connection}, {connection.id: parse_observation(options)})
    return snapshot.entries[0]


def _connection(connection_id="cn_" + "a" * 12, engine="codex-acp"):
    now = "2026-01-01T00:00:00+00:00"
    return ConnectionRecord(
        id=connection_id, name="Work", engine=engine, provider="acme", auth_mode=AuthMode.NATIVE_ACCOUNT,
        key_kind=None, endpoint=None, launch_command=None,
        credential_ref=EngineNativeDir(path=f"/tmp/{connection_id}"), approved_fingerprint="fp",
        state=ConnectionState.CONFIGURED, qualified=True, created_at=now, updated_at=now,
    )


def _bootstrap(tmp_path, owner="owner-1"):
    project = tmp_path / "project"
    project.mkdir()
    store = Store(project / ".cortex", project_root=project)
    result = bootstrap_task(store, SourceEvent(origin="cli", event_id=str(uuid.uuid4())), "Do work.", owner)
    with store.connection() as db:
        epoch = current_owner_epoch(db, result.task_id)
    return store, result.task_id, OwnershipEpoch(epoch=epoch, owner=owner), project


def _propose(store, task_id, epoch, *, effort="high", model="model-a", engine="codex-acp"):
    service = ConfigurationService(store)
    connection = _connection(engine=engine)
    entry = _entry(connection, effort=effort, model=model)
    inputs = ConfigurationInputs(route=entry.route, parameters={"thought_level": effort})
    row = service.propose(task_id, entry, inputs, expected_revision=0, current_epoch=epoch)
    return row


def _adapter(store, home=None):
    # An isolated "HOME" (see route_policy._candidate_paths, which honors
    # an explicit environ's own "HOME" first) so these tests never read a
    # real ~/.cortex or ~/.config/cortex file on the machine running them.
    environ = {"HOME": str(home)} if home is not None else {"HOME": "/nonexistent-cortex-test-home"}
    return EngineSessionManagerAdapter(
        connection_record_factory=lambda task_id: None,
        workspace_root_factory=lambda task_id: "/tmp/ws",
        store=store,
        environ=environ,
    )


# --- precedence ------------------------------------------------------------

def _write(path, data):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(data), encoding="utf-8")


def test_absent_everywhere_is_no_constraint(tmp_path):
    policy, malformed = route_policy_module.load_route_policy(
        project_root=tmp_path / "proj", environ={"HOME": str(tmp_path / "home")},
    )
    assert policy is None and malformed is None


def test_precedence_project_beats_env_beats_home_cortex_beats_home_config(tmp_path):
    project = tmp_path / "proj"
    home = tmp_path / "home"
    env_file = tmp_path / "env-policy.json"
    project.mkdir()
    home.mkdir()
    environ = {"HOME": str(home), route_policy_module.ENV_VAR: str(env_file)}

    _write(home / ".config" / "cortex" / "route-policy.json", {"default": [{"engine": "e-config", "efforts": ["*"]}]})
    _write(home / ".cortex" / "route-policy.json", {"default": [{"engine": "e-home-cortex", "efforts": ["*"]}]})
    _write(env_file, {"default": [{"engine": "e-env", "efforts": ["*"]}]})
    _write(project / ".cortex" / "route-policy.json", {"default": [{"engine": "e-project", "efforts": ["*"]}]})

    def _engines(pr):
        policy, malformed = route_policy_module.load_route_policy(project_root=pr, environ=environ)
        assert malformed is None
        return {e.engine for e in policy.default}

    assert _engines(project) == {"e-project"}
    (project / ".cortex" / "route-policy.json").unlink()
    assert _engines(project) == {"e-env"}
    env_file.unlink()
    assert _engines(project) == {"e-home-cortex"}
    (home / ".cortex" / "route-policy.json").unlink()
    assert _engines(project) == {"e-config"}


# --- malformed --------------------------------------------------------------

def test_malformed_json_is_recorded_not_raised(tmp_path):
    (tmp_path / "bad.json").write_text("not json at all {{{", encoding="utf-8")
    policy, malformed = route_policy_module.load_route_policy(
        project_root=None, environ={route_policy_module.ENV_VAR: str(tmp_path / "bad.json")},
    )
    assert policy is None
    assert malformed is not None
    assert malformed["path"] == str(tmp_path / "bad.json")


def test_malformed_shape_is_recorded_not_raised(tmp_path):
    _write(tmp_path / "bad.json", {"roles": "not-a-mapping"})
    policy, malformed = route_policy_module.load_route_policy(
        project_root=None, environ={route_policy_module.ENV_VAR: str(tmp_path / "bad.json")},
    )
    assert policy is None
    assert malformed is not None and "roles" in malformed["reason"]


# --- resolve_effort ----------------------------------------------------------

def _policy(**kwargs):
    return route_policy_module.RoutePolicy(**kwargs)


def test_resolve_effort_allowed_passes():
    entries = route_policy_module._parse_entries("implementation", [
        {"engine": "codex-acp", "model": "luna", "efforts": ["high", "xhigh", "max"], "default_effort": "high"},
    ])
    policy = _policy(roles={"implementation": entries}, default=None)
    assert policy.resolve_effort("implementation", "codex-acp", "luna", "xhigh") == "xhigh"


def test_resolve_effort_disallowed_effort_refused_with_readable_reason():
    entries = route_policy_module._parse_entries("implementation", [
        {"engine": "codex-acp", "model": "luna", "efforts": ["high", "xhigh", "max"], "default_effort": "high"},
    ])
    policy = _policy(roles={"implementation": entries}, default=None)
    with pytest.raises(PortableError) as excinfo:
        policy.resolve_effort("implementation", "codex-acp", "luna", "low")
    message = excinfo.value.remediation
    assert "implementation" in message
    assert "'low'" in message
    assert "codex-acp" in message and "luna" in message


def test_resolve_effort_disallowed_route_refused():
    entries = route_policy_module._parse_entries("implementation", [
        {"engine": "codex-acp", "model": "luna", "efforts": ["high"], "default_effort": "high"},
    ])
    policy = _policy(roles={"implementation": entries}, default=None)
    with pytest.raises(PortableError) as excinfo:
        policy.resolve_effort("implementation", "claude-agent-acp", "sonnet-5", "high")
    assert excinfo.value.code == "route_policy_not_authorized"


def test_unlisted_role_falls_back_to_default_bucket():
    default_entries = route_policy_module._parse_entries("default", [
        {"engine": "codex-acp", "model": "*", "efforts": ["*"]},
    ])
    policy = _policy(roles={}, default=default_entries)
    assert policy.resolve_effort("some-other-role", "codex-acp", "any-model", "whatever") == "whatever"


def test_no_entries_for_role_or_default_means_no_restriction():
    policy = _policy(roles={}, default=None)
    assert policy.resolve_effort("anything", "codex-acp", "m", "high") == "high"
    assert policy.resolve_effort("anything", "codex-acp", "m", None) is None


def test_default_effort_applied_when_nothing_selected():
    entries = route_policy_module._parse_entries("implementation", [
        {"engine": "codex-acp", "model": "terra", "efforts": ["medium", "high"], "default_effort": "medium"},
    ])
    policy = _policy(roles={"implementation": entries}, default=None)
    assert policy.resolve_effort("implementation", "codex-acp", "terra", None) == "medium"


# --- D-T035-04: coordinator-facing allowed-routes table -----------------

def test_render_table_uses_the_short_role_alias_and_lists_every_entry():
    entries = route_policy_module._parse_entries("worker-backend-dev", [
        {"engine": "codex-acp", "model": "gpt-5.6-terra", "efforts": ["medium", "high"], "default_effort": "medium"},
        {"engine": "claude-agent-acp", "model": "sonnet", "efforts": ["medium"], "default_effort": "medium"},
    ])
    policy = _policy(roles={"worker-backend-dev": entries}, default=None)
    table = route_policy_module.render_table(policy)
    assert table == (
        "backend-dev: codex-acp/gpt-5.6-terra {high,medium} default=medium | "
        "claude-agent-acp/sonnet {medium} default=medium"
    )


def test_render_table_includes_the_default_bucket_and_skips_empty_roles():
    default_entries = route_policy_module._parse_entries("default", [
        {"engine": "codex-acp", "model": "*", "efforts": ["*"]},
    ])
    policy = _policy(roles={"worker-explorer": ()}, default=default_entries)
    table = route_policy_module.render_table(policy)
    assert "worker-explorer" not in table
    assert table == "Any role not listed above may use codex-acp/* {*}."


def test_render_table_of_no_policy_roles_and_no_default_is_empty():
    policy = _policy(roles={}, default=None)
    assert route_policy_module.render_table(policy) == ""


# --- ceiling still wins over a permissive policy ------------------------

def test_ceiling_still_wins_over_permissive_policy(tmp_path):
    """A route policy naming (and permitting) a route the permission
    ceiling forbids grants nothing -- the ceiling's own route-authorization
    check (``routing.authorization.authorize_dispatch``) still refuses it,
    regardless of what a fully permissive route policy would allow for
    that same engine/model."""
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    result = bootstrap_task(store, SourceEvent(origin="cli", event_id=str(uuid.uuid4())), "Do work.", "owner-1")
    task_id = result.task_id
    profile = RouteProfile(role_routes={"reviewer": {"rt_b"}})
    ceiling = PolicyCeiling(allowed_routes={"rt_a"})  # rt_b is outside the ceiling

    # A maximally permissive route policy for the same role/engine/model.
    entries = route_policy_module._parse_entries("reviewer", [{"engine": "codex-acp", "efforts": ["*"]}])
    policy = _policy(roles={"reviewer": entries}, default=None)
    assert policy.resolve_effort("reviewer", "codex-acp", "any-model", "anything") == "anything"

    with pytest.raises(PortableError) as excinfo:
        routing_authorization.authorize_dispatch(
            store, task_id=task_id, role="reviewer", route_profile=profile, coordinator_route_id="rt_a",
            candidate_route_id="rt_b", ceiling=ceiling,
        )
    assert excinfo.value.code == "role_route_not_authorized"


# --- EngineSessionManagerAdapter._authorize_dispatch wiring ---------------

def test_authorize_dispatch_disallowed_effort_refused(tmp_path):
    store, task_id, epoch, project = _bootstrap(tmp_path)
    row = _propose(store, task_id, epoch, effort="low", model="luna", engine="codex-acp")
    _write(project / ".cortex" / "route-policy.json", {
        "roles": {"coordinator": [{"engine": "codex-acp", "model": "luna", "efforts": ["high", "xhigh"], "default_effort": "high"}]},
    })
    adapter = _adapter(store)
    launch = dict(task_id=task_id, config_revision=row["id"])
    record = type("Rec", (), {"engine": "codex-acp", "id": "cn_x"})()
    with pytest.raises(PortableError) as excinfo:
        adapter._authorize_dispatch(record, task_id, launch, project_root=project)
    assert excinfo.value.code == "route_policy_not_authorized"
    assert "coordinator" in excinfo.value.remediation


def test_authorize_dispatch_allowed_effort_passes(tmp_path):
    store, task_id, epoch, project = _bootstrap(tmp_path)
    row = _propose(store, task_id, epoch, effort="high", model="luna", engine="codex-acp")
    _write(project / ".cortex" / "route-policy.json", {
        "roles": {"coordinator": [{"engine": "codex-acp", "model": "luna", "efforts": ["high", "xhigh"], "default_effort": "high"}]},
    })
    adapter = _adapter(store)
    launch = dict(task_id=task_id, config_revision=row["id"])
    record = type("Rec", (), {"engine": "codex-acp", "id": "cn_x"})()
    adapter._authorize_dispatch(record, task_id, launch, project_root=project)  # must not raise


def test_authorize_dispatch_applies_default_effort_when_none_selected(tmp_path):
    store, task_id, epoch, project = _bootstrap(tmp_path)
    # Propose with no effort parameter at all.
    service = ConfigurationService(store)
    connection = _connection(engine="codex-acp")
    options = [{"id": "model", "name": "Model", "category": "model", "type": "select", "currentValue": "terra",
                "options": [{"value": "terra", "name": "terra"}]}]
    snapshot = build_catalog({connection.id: connection}, {connection.id: parse_observation(options)})
    entry = snapshot.entries[0]
    inputs = ConfigurationInputs(route=entry.route, parameters={})
    row = service.propose(task_id, entry, inputs, expected_revision=0, current_epoch=epoch)
    _write(project / ".cortex" / "route-policy.json", {
        "roles": {"coordinator": [{"engine": "codex-acp", "model": "terra", "efforts": ["medium", "high"], "default_effort": "medium"}]},
    })
    adapter = _adapter(store)
    launch = dict(task_id=task_id, config_revision=row["id"])
    record = type("Rec", (), {"engine": "codex-acp", "id": "cn_x"})()
    adapter._authorize_dispatch(record, task_id, launch, project_root=project)
    assert launch["parameters"]["effort"] == "medium"


def test_authorize_dispatch_absent_policy_changes_nothing(tmp_path):
    store, task_id, epoch, project = _bootstrap(tmp_path)
    row = _propose(store, task_id, epoch, effort="high", model="luna", engine="codex-acp")
    adapter = _adapter(store)
    launch = dict(task_id=task_id, config_revision=row["id"])
    record = type("Rec", (), {"engine": "codex-acp", "id": "cn_x"})()
    adapter._authorize_dispatch(record, task_id, launch, project_root=project)  # must not raise
    assert "parameters" not in launch


def test_authorize_dispatch_malformed_policy_recorded_and_does_not_crash(tmp_path):
    store, task_id, epoch, project = _bootstrap(tmp_path)
    row = _propose(store, task_id, epoch, effort="high", model="luna", engine="codex-acp")
    (project / ".cortex").mkdir(exist_ok=True)
    (project / ".cortex" / "route-policy.json").write_text("{not valid json", encoding="utf-8")
    adapter = _adapter(store)
    launch = dict(task_id=task_id, config_revision=row["id"])
    record = type("Rec", (), {"engine": "codex-acp", "id": "cn_x"})()
    adapter._authorize_dispatch(record, task_id, launch, project_root=project)  # must not raise
    events = observability_events.list_events(store, task_id, category="route_policy")
    assert len(events) == 1
    assert events[0]["outcome"] == "malformed"


def test_the_default_policy_is_written_where_the_rest_of_the_configuration_lives(tmp_path, monkeypatch):
    """Every other Cortex config honours XDG_CONFIG_HOME (see
    connections.registry). A user who moved their configuration would
    otherwise get this one file written somewhere their own setup never
    looks -- and `load_route_policy` reads the XDG location, so the file
    would be written dead."""
    import sys
    from pathlib import Path

    sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "scripts"))
    import cortex_acp_setup

    home = tmp_path / "home"
    xdg = tmp_path / "elsewhere"
    (home).mkdir()
    xdg.mkdir()
    environ = {"HOME": str(home), "XDG_CONFIG_HOME": str(xdg)}

    result = cortex_acp_setup.init_route_policy(environ=environ)
    assert result["created"] is True
    written = xdg / "cortex" / "route-policy.json"
    assert written.is_file(), result
    assert not (home / ".config" / "cortex" / "route-policy.json").exists()

    # Idempotent: a second call writes nothing.
    again = cortex_acp_setup.init_route_policy(environ=environ)
    assert again["created"] is False


def test_a_first_run_creates_the_policy_without_a_setup_step(tmp_path):
    """A person installing this agent configures it in their editor and
    expects it to work. They should have to run a setup step only for the
    one thing that needs them -- connecting an engine to their account.
    A policy file in their own configuration directory is not that."""
    from cortex_runtime.configuration.route_policy import ensure_default_policy, load_route_policy

    home = tmp_path / "home"
    home.mkdir()
    environ = {"HOME": str(home)}

    result = ensure_default_policy(environ=environ)
    assert result["created"] is True, result
    written = home / ".config" / "cortex" / "route-policy.json"
    assert written.is_file()

    policy, malformed = load_route_policy(project_root=None, environ=environ)
    assert malformed is None
    assert policy is not None, "the file it just wrote must be the file it then reads"

    # Idempotent: a second start changes nothing.
    again = ensure_default_policy(environ=environ)
    assert again["created"] is False


def test_a_first_run_never_overwrites_a_policy_the_user_already_has(tmp_path):
    from cortex_runtime.configuration.route_policy import ensure_default_policy

    home = tmp_path / "home"
    (home / ".config" / "cortex").mkdir(parents=True)
    mine = home / ".config" / "cortex" / "route-policy.json"
    mine.write_text(json.dumps({"roles": {}, "default": []}), encoding="utf-8")

    result = ensure_default_policy(environ={"HOME": str(home)})
    assert result["created"] is False
    assert json.loads(mine.read_text()) == {"roles": {}, "default": []}


def test_an_unwritable_configuration_directory_never_stops_the_agent(tmp_path):
    """Startup must survive a home it cannot write."""
    from cortex_runtime.configuration.route_policy import ensure_default_policy

    result = ensure_default_policy(environ={"HOME": "/proc/nonexistent-and-unwritable"})
    assert result["created"] is False
    assert "reason" in result


def test_the_fallback_bucket_is_never_offered_as_a_role_name():
    """Live, twice. First the table's own `default:` line was read as a role
    id and delegated with role="default". Relabelling it "any other role"
    kept the "<role>: <routes>" shape of every other row, so the next
    coordinator delegated with role="any other role" -- two assignments
    refused before it noticed (2026-09-19). The fallback must not take the
    shape of a row at all."""
    from cortex_runtime.configuration.route_policy import RoutePolicy, RoutePolicyEntry, render_table

    policy = RoutePolicy(
        roles={"worker-backend-dev": (RoutePolicyEntry(engine="codex-acp", model="luna"),)},
        default=(RoutePolicyEntry(engine="claude-agent-acp", model="sonnet"),),
    )
    table = render_table(policy)

    assert "backend-dev: codex-acp/luna {*}" in table
    # No line but a real role's may carry the "<name>: <routes>" shape.
    rows = [line for line in table.splitlines() if ": " in line.split(" {")[0]]
    assert rows == ["backend-dev: codex-acp/luna {*}"], table
    assert "Any role not listed above may use claude-agent-acp/sonnet {*}." in table
