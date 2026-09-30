"""Owner configuration management API (``configuration.management``).

Every test isolates HOME/XDG_CONFIG_HOME/XDG_STATE_HOME under tmp_path, so
no real user configuration is read or written, and no engine binary runs
(OpenCode discovery and binary trust are replaced by fakes).
"""
from __future__ import annotations

import io
import json
from pathlib import Path
import stat
from types import SimpleNamespace

import pytest

from cortex_runtime.configuration import management
from cortex_runtime.configuration import store as configuration_store
from cortex_runtime.configuration.mcp_servers import load_mcp_servers
from cortex_runtime.configuration.policy import PermissionLevel, PolicyCeiling
from cortex_runtime.connections import onboarding, registry
from cortex_runtime.connections.model import AuthMode
from cortex_runtime.portable.errors import PortableError
from cortex_runtime.routing import auto_route
from cortex_runtime.store import Store

OPENCODE_ROUTE = "opencode/anthropic/claude-sonnet-4-5"


@pytest.fixture
def home(tmp_path, monkeypatch):
    config, state, user_home = tmp_path / "config", tmp_path / "state", tmp_path / "home"
    for path in (config, state, user_home):
        path.mkdir()
    monkeypatch.setenv("XDG_CONFIG_HOME", str(config))
    monkeypatch.setenv("XDG_STATE_HOME", str(state))
    monkeypatch.setenv("HOME", str(user_home))
    monkeypatch.delenv("CORTEX_ROUTING", raising=False)
    monkeypatch.delenv("CORTEX_OPENCODE_BINARY", raising=False)
    return SimpleNamespace(config=config / "cortex", home=user_home, tmp=tmp_path)


def _codes(error):
    return {(item["field"], item["code"]) for item in error.errors}


def _write_json(path: Path, value) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value))
    path.chmod(0o600)


def _api_key_connection(secret=b"sk-TEST-SECRET-VALUE"):
    record = onboarding.create_connection(
        name="Codex key", engine="codex-acp", provider="openai", auth_mode=AuthMode.API_KEY,
        key_kind="openai_api_key", launch_command=("/usr/bin/node", "/opt/codex-acp/dist/index.js"),
    )
    return onboarding.set_api_key(record.id, lambda: secret)


# --- user defaults -------------------------------------------------------------

def test_user_defaults_round_trip_uses_revisions_and_the_runtime_loader(home):
    first = management.read_user_defaults()
    assert first["revision"] == "absent" and first["defaults"] == {} and first["exists"] is False
    written = management.update_user_defaults(
        "absent", defaults={"language": "ru", "parameters": {"thought_level": "medium"}},
        ceiling={"permission_level": "STANDARD", "limits": {"attempts": 8}},
    )
    assert written["changed"] and written["revision"].startswith("sha256:")
    assert written["ceiling"]["permission_level"] == "STANDARD" and written["ceiling_set"] is True
    loaded = configuration_store.read_user_defaults()
    assert loaded["defaults"] == {"language": "ru", "parameters": {"thought_level": "medium"}}
    assert loaded["ceiling"] == PolicyCeiling(permission_level=PermissionLevel.STANDARD, limits={"attempts": 8})
    path = home.config / "defaults.json"
    assert stat.S_IMODE(path.stat().st_mode) == 0o600
    assert management.read_user_defaults()["revision"] == written["revision"]
    again = management.update_user_defaults(written["revision"], defaults={"language": None})
    assert "language" not in again["defaults"] and again["ceiling"]["permission_level"] == "STANDARD"
    cleared = management.update_user_defaults(again["revision"], ceiling=None)
    assert configuration_store.read_user_defaults()["ceiling"] == PolicyCeiling()
    assert cleared["defaults"] == {"parameters": {"thought_level": "medium"}}


def test_user_defaults_concurrent_external_edit_is_reported_not_overwritten(home):
    seen = management.read_user_defaults()
    configuration_store.write_user_defaults({"route_model": "external"}, PolicyCeiling())
    with pytest.raises(management.RevisionConflict) as caught:
        management.update_user_defaults(seen["revision"], defaults={"language": "en"})
    assert caught.value.code == "revision_conflict"
    assert caught.value.current_revision == management.read_user_defaults()["revision"]
    assert configuration_store.read_user_defaults()["defaults"] == {"route_model": "external"}
    with pytest.raises(PortableError) as caught:
        management.update_user_defaults("latest", defaults={"language": "en"})
    assert caught.value.code == "invalid_expected_revision"


def test_user_defaults_reports_every_invalid_setting_and_writes_nothing(home):
    with pytest.raises(management.ConfigValidationError) as caught:
        management.update_user_defaults(
            "absent",
            defaults={"colour": "red", "language": "de", "route_id": "bogus", "parameters": {"Bad Name": "x"}},
            ceiling={"permission_level": "ROOT", "limits": {"minutes": 1}, "spend_limit": 5,
                     "fallback_policy": {"allowed_failure_classes": ["refusal"]}, "extra": 1},
        )
    codes = _codes(caught.value)
    assert {
        ("defaults.colour", "unsupported_setting"),
        ("defaults.language", "invalid_value"),
        ("defaults.route_id", "invalid_value"),
        ("defaults.parameters.Bad Name", "invalid_value"),
        ("ceiling.permission_level", "invalid_value"),
        ("ceiling.limits.minutes", "unsupported_setting"),
        ("ceiling.spend_limit", "unenforceable_spend_limit"),
        ("ceiling.fallback_policy.allowed_failure_classes", "fallback_denylisted_failure_class"),
        ("ceiling.extra", "unsupported_setting"),
    } <= codes
    assert management.error_payload(caught.value)["errors"]
    assert not (home.config / "defaults.json").exists()


def test_user_default_route_is_a_complete_group_naming_a_registered_connection(home):
    with pytest.raises(management.ConfigValidationError) as caught:
        management.update_user_defaults("absent", defaults={"route_id": "xr_" + "a" * 24})
    assert ("defaults", "route_incomplete") in _codes(caught.value)
    route = {"route_id": "xr_" + "a" * 24, "route_connection_id": "cn_" + "b" * 12, "route_model": "gpt-6-luna"}
    with pytest.raises(management.ConfigValidationError) as caught:
        management.update_user_defaults("absent", defaults=route)
    assert ("defaults.route_connection_id", "unknown_connection") in _codes(caught.value)
    record = _api_key_connection()
    written = management.update_user_defaults("absent", defaults=dict(route, route_connection_id=record.id))
    assert written["defaults"]["route_connection_id"] == record.id
    removed = management.update_user_defaults(written["revision"], defaults=dict.fromkeys(management.ROUTE_DEFAULT_KEYS))
    assert removed["defaults"] == {}


def test_user_defaults_keep_other_keys_and_fail_closed_on_a_malformed_file(home):
    configuration_store.write_user_defaults({"effort": "high"}, PolicyCeiling())
    seen = management.read_user_defaults()
    assert seen["preserved_keys"] == ["effort"]
    written = management.update_user_defaults(seen["revision"], defaults={"language": "en"})
    assert written["defaults"] == {"effort": "high", "language": "en"}
    path = home.config / "defaults.json"
    path.write_text("{not json")
    with pytest.raises(PortableError) as caught:
        management.read_user_defaults()
    assert caught.value.code == "invalid_user_defaults"
    with pytest.raises(PortableError) as caught:
        management.update_user_defaults(management._revision(path.read_bytes()), defaults={"language": "ru"})
    assert caught.value.code == "invalid_user_defaults" and path.read_text() == "{not json"


# --- project ceiling -------------------------------------------------------------

def test_project_ceiling_narrows_the_user_ceiling_and_keeps_project_defaults(home, tmp_path):
    store = Store(tmp_path / "private")
    project = str(tmp_path)
    allowed = ["cn_" + "a" * 12, "cn_" + "b" * 12]
    management.update_user_defaults("absent", ceiling={"permission_level": "STANDARD", "allowed_connections": allowed})
    seen = management.read_project_ceiling(store, project)
    assert seen["revision"] == "absent" and seen["ceiling"] is None
    assert seen["effective_ceiling"]["permission_level"] == "STANDARD"
    with pytest.raises(management.ConfigValidationError) as caught:
        management.update_project_ceiling(store, project, "absent",
                                          {"permission_level": "ELEVATED", "allowed_connections": allowed[:1]})
    assert caught.value.errors[0]["code"] == "ceiling_violation"
    assert configuration_store.read_project_defaults(store, project) is None
    narrower = {"permission_level": "READ_ONLY", "allowed_connections": allowed[:1]}
    written = management.update_project_ceiling(store, project, "absent", narrower)
    assert written["narrows_user_ceiling"] and written["effective_ceiling"]["permission_level"] == "READ_ONLY"
    assert written["effective_ceiling"]["allowed_connections"] == allowed[:1]
    assert configuration_store.read_project_defaults(store, project)["ceiling"].permission_level == PermissionLevel.READ_ONLY
    with pytest.raises(management.RevisionConflict):
        management.update_project_ceiling(store, project, "absent", narrower)
    configuration_store.write_project_defaults(
        store, project, {"effort": "low"}, configuration_store.read_project_defaults(store, project)["ceiling"])
    current = management.read_project_ceiling(store, project)
    assert current["project_defaults"] == {"effort": "low"} and current["revision"] != written["revision"]
    with pytest.raises(PortableError) as caught:
        management.update_project_ceiling(store, project, current["revision"], None)
    assert caught.value.code == "project_defaults_present"
    replaced = management.update_project_ceiling(store, project, current["revision"], dict(narrower, allowed_connections=[]))
    assert replaced["project_defaults"] == {"effort": "low"} and replaced["ceiling"]["allowed_connections"] == []
    other = management.update_project_ceiling(store, "/other-project", "absent", narrower)
    removed = management.update_project_ceiling(store, "/other-project", other["revision"], None)
    assert removed["exists"] is False and removed["revision"] == "absent"


def test_project_ceiling_reports_a_user_ceiling_that_tightened_later(home, tmp_path):
    store = Store(tmp_path / "private")
    management.update_project_ceiling(store, str(tmp_path), "absent", {"permission_level": "STANDARD"})
    management.update_user_defaults("absent", ceiling={"permission_level": "READ_ONLY"})
    view = management.read_project_ceiling(store, str(tmp_path))
    assert view["narrows_user_ceiling"] is False and view["violation"]["code"] == "ceiling_violation"
    assert view["effective_ceiling"]["permission_level"] == "READ_ONLY"


# --- routing ---------------------------------------------------------------------

def test_routing_read_reports_precedence_and_the_effective_source(home, tmp_path, monkeypatch):
    project = tmp_path / "project"
    (project / ".cortex").mkdir(parents=True)
    _write_json(home.config / "routing.json", {"deny": ["haiku"], "future_key": 1})
    view = management.read_routing(str(project))
    assert view["precedence"] == ["project", "user"]
    assert view["effective_source"] == "user" and view["effective_rules"] == {"deny": ["haiku"]}
    assert view["sources"][1]["ignored_keys"] == ["future_key"] and view["sources"][0]["exists"] is False
    assert view["builtin"]["default_auto_deny_engines"] == ["opencode"]
    assert view["builtin"]["optional_engines"] == ["opencode"]
    env_file = tmp_path / "env-routing.json"
    _write_json(env_file, {"providers": ["codex-acp"]})
    monkeypatch.setenv("CORTEX_ROUTING", str(env_file))
    view = management.read_routing(str(project))
    assert view["precedence"] == ["project", "env", "user"] and view["effective_source"] == "env"
    assert view["sources"][1]["writable"] is False
    _write_json(project / ".cortex" / "routing.json", {"pin": {"*": "codex-acp/gpt-6-luna"}})
    view = management.read_routing(str(project))
    assert view["effective_source"] == "project"
    assert auto_route.load_overrides(str(project)) == view["effective_rules"]


def test_routing_update_validates_like_the_loader_and_keeps_unknown_keys(home):
    path = home.config / "routing.json"
    _write_json(path, {"deny": ["gpt-5.5", "haiku"], "future_key": {"x": 1}})
    user = management.read_routing()["sources"][-1]
    with pytest.raises(management.ConfigValidationError) as caught:
        management.update_routing("user", user["revision"], {
            "deny": ["haiku"], "tiers": {"m": "huge"}, "providers": ["gemini"],
            "engine_affinity": {"cooking": "codex-acp"}, "pins": {},
        })
    assert {
        ("rules.tiers.m", "invalid_value"), ("rules.providers", "invalid_value"),
        ("rules.engine_affinity.cooking", "invalid_value"), ("rules.pins", "unsupported_setting"),
    } <= _codes(caught.value)
    rules = {"deny": ["gpt-5.5", "haiku"], "pin": {"worker-security-auditor": "claude-agent-acp/opus:high"},
             "engine_affinity": {"testing": None}}
    written = management.update_routing("user", user["revision"], rules)
    assert written["effective_source"] == "user" and written["shadowed_by"] is None and written["changed"]
    on_disk = json.loads(path.read_text())
    assert on_disk == dict(rules, future_key={"x": 1})
    assert auto_route.load_overrides() == on_disk
    assert stat.S_IMODE(path.stat().st_mode) == 0o600
    with pytest.raises(management.RevisionConflict):
        management.update_routing("user", user["revision"], rules)


def test_routing_user_write_reports_a_shadowing_project_file_and_env_is_read_only(home, tmp_path):
    project = tmp_path / "project"
    (project / ".cortex").mkdir(parents=True)
    _write_json(project / ".cortex" / "routing.json", {"deny": ["haiku"]})
    written = management.update_routing("user", "absent", {"deny": ["gpt-5.5"]}, project_root=str(project))
    assert written["shadowed_by"] == "project" and written["effective_rules"] == {"deny": ["haiku"]}
    updated = management.update_routing("project", written["sources"][0]["revision"],
                                         {"deny": ["haiku"], "providers": ["claude-agent-acp"]}, project_root=str(project))
    assert updated["effective_source"] == "project" and updated["effective_rules"]["providers"] == ["claude-agent-acp"]
    with pytest.raises(PortableError) as caught:
        management.update_routing("env", "absent", {})
    assert caught.value.code == "routing_source_read_only"
    empty = tmp_path / "empty"
    empty.mkdir()
    with pytest.raises(PortableError) as caught:
        management.update_routing("project", "absent", {}, project_root=str(empty))
    assert caught.value.code == "project_store_missing"


def test_routing_malformed_or_symlinked_file_is_reported_and_never_overwritten(home):
    path = home.config / "routing.json"
    path.parent.mkdir(parents=True)
    path.write_text("[1, 2")
    view = management.read_routing()
    assert view["effective_source"] == "user" and view["effective_valid"] is False and view["effective_rules"] is None
    with pytest.raises(PortableError) as caught:
        management.update_routing("user", view["sources"][-1]["revision"], {"deny": ["haiku"]})
    assert caught.value.code == "invalid_routing_overrides" and path.read_text() == "[1, 2"
    path.unlink()
    target = home.tmp / "elsewhere.json"
    target.write_text("{}")
    path.symlink_to(target)
    assert management.read_routing()["sources"][-1]["error"]["code"] == "unsafe_storage"
    with pytest.raises(PortableError) as caught:
        management.update_routing("user", "absent", {"deny": ["haiku"]})
    assert caught.value.code == "unsafe_storage" and target.read_text() == "{}"


def test_routing_opencode_routes_need_exact_provider_model_ids(home):
    with pytest.raises(management.ConfigValidationError) as caught:
        management.update_routing("user", "absent", {
            "auto_allow": ["opencode/sonnet", "anthropic/claude-sonnet-4-5"],
            "pin": {"*": "opencode/claude-sonnet", "worker-qa-engineer": OPENCODE_ROUTE + ":turbo"},
        })
    assert {
        ("rules.auto_allow", "opencode_model_not_exact"),
        ("rules.auto_allow", "ambiguous_engine_prefix"),
        ("rules.pin.*", "opencode_model_not_exact"),
        ("rules.pin.worker-qa-engineer", "invalid_effort"),
    } <= _codes(caught.value)
    assert not (home.config / "routing.json").exists()
    management.update_routing("user", "absent", {
        "auto_allow": [OPENCODE_ROUTE], "pin": {"worker-qa-engineer": OPENCODE_ROUTE + ":high"},
        "providers": ["codex-acp", "opencode"],
    })
    overrides = auto_route.load_overrides()
    assert overrides["auto_allow"] == [OPENCODE_ROUTE]
    assert auto_route._pin_for(overrides, "worker-qa-engineer") == ("opencode", "anthropic/claude-sonnet-4-5", "high")

    def entry(model):
        return SimpleNamespace(route=SimpleNamespace(engine="opencode", model=model))

    assert auto_route._default_auto_denied(overrides, entry("anthropic/claude-sonnet-4-5")) is False
    assert auto_route._default_auto_denied(overrides, entry("openai/gpt-5.1")) is True


# --- MCP servers -------------------------------------------------------------------

def _server(**extra):
    return dict({"command": "/opt/index/bin/server", "args": ["--stdio"],
                 "allowed_tools": ["search", "read"], "read_only_tools": ["search"]}, **extra)


def test_mcp_env_values_are_never_returned_and_can_be_kept(home):
    secret = "tok-SUPER-SECRET-123"
    written = management.update_mcp_servers("absent", [
        {"op": "upsert", "name": "index", "server": _server(env={"INDEX_TOKEN": secret})}])
    assert secret not in json.dumps(written)
    assert written["servers"][0]["config"]["env"] == [{"name": "INDEX_TOKEN", "value_set": True}]
    path = home.config / "mcp-servers.json"
    assert stat.S_IMODE(path.stat().st_mode) == 0o600
    kept = management.update_mcp_servers(written["revision"], [
        {"op": "upsert", "name": "index",
         "server": _server(args=["--stdio", "--fast"], env={"INDEX_TOKEN": {"keep": True}})}])
    assert secret not in json.dumps(kept)
    (loaded,) = load_mcp_servers(str(home.tmp))
    assert loaded["env"] == [{"name": "INDEX_TOKEN", "value": secret}] and loaded["args"] == ["--stdio", "--fast"]
    assert secret not in json.dumps(management.read_mcp_servers())
    assert secret not in json.dumps(management.execute({"action": "read", "surface": "all"}))
    with pytest.raises(management.ConfigValidationError) as caught:
        management.update_mcp_servers(kept["revision"], [
            {"op": "upsert", "name": "index", "server": _server(env={"OTHER": {"keep": True}})}])
    assert ("operations[0].server.env.OTHER", "unknown_env_value") in _codes(caught.value)
    assert secret not in json.dumps(management.error_payload(caught.value))


def test_mcp_update_is_validated_by_the_loader_all_or_nothing(home):
    with pytest.raises(management.ConfigValidationError) as caught:
        management.update_mcp_servers("absent", [
            {"op": "upsert", "name": "cortex", "server": _server()},
            {"op": "upsert", "name": "bad", "server": _server(read_only_tools=["delete"])},
            {"op": "upsert", "name": "odd", "server": _server(shell=True)},
            {"op": "remove", "name": "missing"},
            {"op": "rename", "name": "x"},
        ])
    assert {
        ("operations[0].name", "invalid_server_name"),
        ("operations[1].server", "invalid_mcp_registry"),
        ("operations[2].server.shell", "unsupported_setting"),
        ("operations[3].name", "unknown_server"),
        ("operations[4].op", "unsupported_operation"),
    } <= _codes(caught.value)
    assert not (home.config / "mcp-servers.json").exists()


def test_mcp_disable_remove_conflict_and_malformed_registry(home):
    first = management.update_mcp_servers("absent", [
        {"op": "upsert", "name": "index", "server": _server(engines=["opencode"], roles=["worker"])}])
    assert load_mcp_servers(str(home.tmp))[0]["engines"] == ["opencode"]
    disabled = management.update_mcp_servers(first["revision"], [{"op": "set_enabled", "name": "index", "enabled": False}])
    assert disabled["servers"][0]["enabled"] is False and load_mcp_servers(str(home.tmp)) == []
    with pytest.raises(management.RevisionConflict):
        management.update_mcp_servers(first["revision"], [{"op": "remove", "name": "index"}])
    removed = management.update_mcp_servers(disabled["revision"], [{"op": "remove", "name": "index"}])
    assert removed["servers"] == []
    path = home.config / "mcp-servers.json"
    path.write_text('{"servers": [')
    with pytest.raises(PortableError) as caught:
        management.read_mcp_servers()
    assert caught.value.code == "invalid_mcp_registry"
    with pytest.raises(PortableError):
        management.update_mcp_servers(management._revision(path.read_bytes()), [{"op": "remove", "name": "index"}])
    assert path.read_text() == '{"servers": ['


# --- connections ----------------------------------------------------------------------

def test_connections_view_never_exposes_credential_values_or_paths(home):
    record = _api_key_connection()
    view = management.read_connections()
    text = json.dumps(view)
    assert "sk-TEST-SECRET-VALUE" not in text and str(registry.secret_path(record.id)) not in text
    (item,) = view["connections"]
    assert item["credential"] == {"kind": "secret_file"} and item["state"] == "configured"
    engines = {entry["engine"]: entry for entry in view["engines"]}
    assert engines["opencode"]["optional"] is True and engines["opencode"]["onboarding"] == "explicit_opt_in"
    assert engines["opencode"]["qualified_versions"] == ["2.x.x"] and engines["opencode"]["connection_ids"] == []
    assert engines["codex-acp"]["connection_ids"] == [record.id]


def test_connection_rename_limit_validate_and_remove_use_revisions(home):
    record = _api_key_connection()
    seen = management.read_connections()
    renamed = management.update_connection(seen["revision"], record.id, "rename", name="Work Codex")
    assert renamed["connections"][0]["name"] == "Work Codex"
    with pytest.raises(management.RevisionConflict):
        management.update_connection(seen["revision"], record.id, "rename", name="Stale")
    with pytest.raises(management.ConfigValidationError):
        management.update_connection(renamed["revision"], record.id, "set_max_concurrent_sessions",
                                     max_concurrent_sessions=65)
    limited = management.update_connection(renamed["revision"], record.id, "set_max_concurrent_sessions",
                                           max_concurrent_sessions=2)
    stored = registry.read_registry().connections[record.id]
    assert stored.max_concurrent_sessions == 2 and stored.approved_fingerprint == record.approved_fingerprint
    registry.secret_path(record.id).unlink()
    validated = management.update_connection(limited["revision"], record.id, "validate")
    assert validated["connections"][0]["state"] == "authorization_required"
    management.update_user_defaults("absent", defaults={
        "route_id": "xr_" + "a" * 24, "route_connection_id": record.id, "route_model": "gpt-6-luna"})
    removed = management.update_connection(validated["revision"], record.id, "remove")
    assert removed["connections"] == [] and removed["notes"] == ["user_default_route_uses_removed_connection"]
    with pytest.raises(PortableError) as caught:
        management.update_connection(removed["revision"], record.id, "rename", name="x")
    assert caught.value.code == "unknown_connection"


def _fake_opencode(monkeypatch, home, version="2.0.19"):
    from cortex_runtime.connections import discovery
    from cortex_runtime.engine import opencode_install

    binary = home.tmp / "bin" / "opencode"
    binary.parent.mkdir()
    binary.write_text("#!/bin/sh\n")
    binary.chmod(0o755)
    found = discovery.EngineDiscovery(engine="opencode", version=version, launch_command=(str(binary), "acp"),
                                      native_credential_dir=str(home.home / ".local" / "share" / "opencode"))
    calls = []

    def discover(binary=None, environ=None):
        calls.append(binary)
        return found

    monkeypatch.setattr(discovery, "discover_opencode", discover)
    monkeypatch.setattr(opencode_install, "trusted_binary", lambda path, environ=None: Path(path))
    return binary, calls


@pytest.mark.parametrize("version", ["2.0.19", "2.0.20", "2.7.3"])
def test_opencode_is_onboarded_only_on_explicit_opt_in_and_never_activated(home, monkeypatch, version):
    binary, calls = _fake_opencode(monkeypatch, home, version=version)
    seen = management.read_connections()
    with pytest.raises(PortableError) as caught:
        management.onboard_opencode(seen["revision"], explicit_opt_in=False)
    assert caught.value.code == "explicit_opt_in_required" and calls == []
    response = management.execute({"action": "onboard_opencode", "surface": "connections",
                                   "expected_revision": seen["revision"], "explicit_opt_in": True,
                                   "binary": str(binary)})
    assert response["ok"], response
    result = response["result"]
    assert result["connection"]["engine"] == "opencode"
    assert result["connection"]["launch_command"] == [str(binary), "acp"]
    assert result["connection"]["credential"] == {"kind": "engine_native_dir", "owned": False}
    assert result["opencode"] == {"version": version, "next_action": "opencode_auth_login",
                                  "automatic_routing": "excluded_until_routing_opts_in"}
    assert calls == [str(binary)]
    assert not (home.config / "routing.json").exists()
    assert management.read_routing()["effective_source"] == "builtin"
    again = management.onboard_opencode(result["revision"], explicit_opt_in=True, binary=str(binary))
    assert [item["id"] for item in again["connections"]] == [result["connection"]["id"]]
    found = management.execute({"action": "discover_opencode", "surface": "connections"})["result"]
    assert found["found"] and found["qualified"] and found["onboarded_connection_ids"] == [result["connection"]["id"]]


@pytest.mark.parametrize("version", ["1.99.99", "3.0.0"])
def test_opencode_unqualified_version_is_refused_without_registering(home, monkeypatch, version):
    binary, _ = _fake_opencode(monkeypatch, home, version=version)
    seen = management.read_connections()
    response = management.execute({"action": "onboard_opencode", "surface": "connections",
                                   "expected_revision": seen["revision"], "explicit_opt_in": True})
    assert response["ok"] is False and response["error"]["code"] == "opencode_version_unqualified"
    assert management.read_connections()["connections"] == []


# --- route catalog: enabling and disabling engines and models ------------------------

def _observe(record, models):
    """Cache a genuine, engine-confirmed catalog snapshot for ``record``."""
    from cortex_runtime.catalog import discovery as catalog_discovery
    from cortex_runtime.catalog import store as catalog_store

    raw = [dict(id="model", name="Model", type="select", category="model", currentValue=models[0][0],
                options=[dict(value=value, name=name) for value, name in models])]
    catalog_store.save_snapshot(catalog_discovery.build_catalog(
        {record.id: record}, {record.id: catalog_discovery.parse_observation(raw)}))


def _claude_connection():
    record = onboarding.create_connection(
        name="Claude key", engine="claude-agent-acp", provider="anthropic", auth_mode=AuthMode.API_KEY,
        key_kind="anthropic_api_key", launch_command=("/usr/bin/node", "/opt/claude-agent-acp/dist/index.js"),
    )
    return onboarding.set_api_key(record.id, lambda: b"sk-ant-TEST-SECRET")


def _catalog_fixture(home, monkeypatch):
    """Codex and Claude API-key connections and a logged-in OpenCode v2
    connection, each with an observed catalog."""
    from dataclasses import replace as replace_record
    from cortex_runtime.connections.model import ConnectionState

    codex, claude = _api_key_connection(), _claude_connection()
    _observe(codex, [("gpt-6-luna", "GPT-6 Luna"), ("gpt-5.6-luna", "GPT-5.6 Luna"), ("gpt-6-sol", "GPT-6 Sol")])
    _observe(claude, [("claude-sonnet-5", "Sonnet 5"), ("claude-haiku-4-5-20251001", "Haiku 4.5")])
    binary, _ = _fake_opencode(monkeypatch, home)
    onboarded = management.onboard_opencode(management.read_connections()["revision"], explicit_opt_in=True,
                                            binary=str(binary))["connection"]
    with registry.transaction(None) as state:
        state.connections[onboarded["id"]] = replace_record(state.connections[onboarded["id"]],
                                                            state=ConnectionState.CONFIGURED)
    opencode = registry.read_registry(None).connections[onboarded["id"]]
    _observe(opencode, [("anthropic/claude-sonnet-4-5", "Claude Sonnet 4.5"), ("openai/gpt-5.1", "GPT-5.1")])
    return codex, claude, opencode


def _routes(view):
    return {f"{item['engine']}/{item['model']}": item for item in view["routes"]}


def _engines(view):
    return {item["engine"]: item for item in view["engines"]}


def test_route_catalog_reports_every_route_with_its_effective_state_and_writes_nothing(home, monkeypatch):
    codex, _claude, opencode = _catalog_fixture(home, monkeypatch)
    registry_before = registry.registry_path(None).read_bytes()
    catalog_dir = registry.state_root(None) / "catalog"
    catalog_before = sorted(str(path) for path in catalog_dir.rglob("*"))
    view = management.read_route_catalog()
    assert view["effective_source"] == "builtin" and view["effective_valid"] is True and view["catalog_problems"] == []
    routes = _routes(view)
    luna = routes["codex-acp/gpt-6-luna"]
    assert (luna["in_selector"], luna["disabled"], luna["automatic"], luna["observed"]) == (True, False, "eligible", True)
    assert luna["route_id"].startswith("xr_") and luna["connection_id"] == codex.id and luna["model_name"] == "GPT-6 Luna"
    older = routes["codex-acp/gpt-5.6-luna"]
    assert (older["automatic"], older["superseded_by"], older["in_selector"]) == ("superseded", "codex-acp/gpt-6-luna", True)
    haiku = routes["claude-agent-acp/claude-haiku-4-5-20251001"]
    assert (haiku["in_selector"], haiku["default_auto_excluded"], haiku["automatic"]) == (True, "family", "excluded_by_default")
    optional = routes[OPENCODE_ROUTE]
    assert (optional["in_selector"], optional["default_auto_excluded"], optional["automatic"]) \
        == (True, "optional_engine", "excluded_by_default")
    engines = _engines(view)
    assert set(engines) == {"codex-acp", "claude-agent-acp", "opencode"}
    assert engines["opencode"]["optional"] is True and [item["id"] for item in engines["opencode"]["connections"]] == [opencode.id]
    assert all(item["disabled"] is False and item["automatic"] is True for item in engines.values())
    assert registry.registry_path(None).read_bytes() == registry_before
    assert sorted(str(path) for path in catalog_dir.rglob("*")) == catalog_before
    assert "sk-" not in json.dumps(view)


def test_engine_and_model_toggles_are_routing_rules_the_runtime_honours(home, monkeypatch):
    codex, _claude, _opencode = _catalog_fixture(home, monkeypatch)
    with pytest.raises(management.ConfigValidationError) as caught:
        management.update_routing("user", "absent", {"deny": ["*", "gpt-*", "gemini/*"],
                                                   "auto_allow": ["opencode/*", "claude-agent-acp/*"]})
    assert sum(1 for item in caught.value.errors if item["code"] == "unsupported_wildcard") == 5
    assert not (home.config / "routing.json").exists()
    rules = {"deny": ["codex-acp/*", "claude-agent-acp/claude-sonnet-5"], "auto_allow": [OPENCODE_ROUTE],
             "providers": ["claude-agent-acp", "opencode"]}
    written = management.update_routing("user", "absent", rules)
    assert auto_route.load_overrides() == rules
    # The runtime honours what the settings wrote: selector, dispatch, automatic routing.
    assert auto_route.is_denied("codex-acp", "gpt-6-luna") and auto_route.is_denied("codex-acp", "a-future-model")
    assert not auto_route.is_denied("claude-agent-acp", "claude-haiku-4-5-20251001")
    with pytest.raises(PortableError) as refused:
        auto_route.refuse_if_denied("claude-agent-acp", "claude-sonnet-5")
    assert refused.value.code == "route_denied"
    view = management.read_route_catalog()
    routes, engines = _routes(view), _engines(view)
    luna = routes["codex-acp/gpt-6-luna"]
    assert (luna["disabled"], luna["engine_disabled"], luna["in_selector"], luna["automatic"]) == (True, True, False, "disabled")
    assert luna["deny_tokens"] == ["codex-acp/*"] and luna["matches"]["user"]["deny"] == ["codex-acp/*"]
    assert (engines["codex-acp"]["disabled"], engines["codex-acp"]["automatic"]) == (True, False)
    sonnet = routes["claude-agent-acp/claude-sonnet-5"]
    assert (sonnet["disabled"], sonnet["engine_disabled"], sonnet["deny_tokens"]) \
        == (True, False, ["claude-agent-acp/claude-sonnet-5"])
    assert routes["claude-agent-acp/claude-haiku-4-5-20251001"]["automatic"] == "excluded_by_default"
    allowed = routes[OPENCODE_ROUTE]
    assert (allowed["automatic"], allowed["auto_allow_tokens"], allowed["in_selector"]) == ("eligible", [OPENCODE_ROUTE], True)
    assert routes["opencode/openai/gpt-5.1"]["automatic"] == "excluded_by_default"

    # Automatic routing is not disabling: an engine outside providers stays selectable.
    management.update_routing("user", written["sources"][-1]["revision"],
                              {"providers": ["opencode"], "auto_allow": [OPENCODE_ROUTE]})
    view = management.read_route_catalog()
    luna = _routes(view)["codex-acp/gpt-6-luna"]
    assert (luna["disabled"], luna["in_selector"], luna["automatic"]) == (False, True, "engine_excluded_from_automatic_routing")
    assert not auto_route.is_denied("codex-acp", "gpt-6-luna")
    assert (_engines(view)["codex-acp"]["disabled"], _engines(view)["codex-acp"]["automatic"]) == (False, False)
    assert luna["connection_id"] == codex.id


def test_route_catalog_follows_routing_precedence_and_fails_closed(home, monkeypatch, tmp_path):
    _catalog_fixture(home, monkeypatch)
    project = tmp_path / "project"
    (project / ".cortex").mkdir(parents=True)
    management.update_routing("user", "absent", {"deny": ["claude-agent-acp/*"]})
    management.update_routing("project", "absent", {"providers": ["claude-agent-acp"]}, project_root=str(project))
    view = management.read_route_catalog(str(project))
    assert view["effective_source"] == "project" and view["precedence"] == ["project", "user"]
    routes = _routes(view)
    sonnet = routes["claude-agent-acp/claude-sonnet-5"]
    assert (sonnet["disabled"], sonnet["automatic"]) == (False, "eligible")
    assert sonnet["matches"] == {"project": {"deny": [], "auto_allow": []},
                                 "user": {"deny": ["claude-agent-acp/*"], "auto_allow": []}}
    assert routes["codex-acp/gpt-6-luna"]["automatic"] == "engine_excluded_from_automatic_routing"
    assert _engines(view)["claude-agent-acp"]["matches"] == {"project": {"disabled": False, "automatic": True},
                                                               "user": {"disabled": True, "automatic": True}}
    owner_only = management.read_route_catalog()
    assert owner_only["effective_source"] == "user"
    assert _routes(owner_only)["claude-agent-acp/claude-sonnet-5"]["automatic"] == "disabled"
    (project / ".cortex" / "routing.json").write_text("{broken")
    broken = management.read_route_catalog(str(project))
    assert broken["effective_valid"] is False
    assert {item["automatic"] for item in broken["routes"]} == {"routing_invalid"}
    response = management.execute({"action": "read", "surface": "route_catalog", "project": str(project)})
    assert response["ok"] and response["result"]["surface"] == "route_catalog"
    assert management.execute({"action": "update", "surface": "route_catalog"})["error"]["code"] == "unsupported_action"
    assert management.execute({"action": "read", "surface": "route_catalog", "path": "/etc"})["error"]["code"] \
        == "unsupported_request_field"


# --- JSON request/response -------------------------------------------------------------

def test_execute_returns_structured_errors_and_main_answers_json(home):
    assert management.execute({"api_version": 2, "action": "read", "surface": "routing"})["error"]["code"] \
        == "unsupported_api_version"
    assert management.execute({"action": "read", "surface": "secrets"})["error"]["code"] == "unknown_surface"
    assert management.execute({"action": "read", "surface": "routing", "path": "/etc"})["error"]["code"] \
        == "unsupported_request_field"
    assert management.execute({"action": "delete", "surface": "routing"})["error"]["code"] == "unsupported_action"
    response = management.execute({"action": "update", "surface": "user_defaults", "expected_revision": "absent",
                                   "defaults": {"language": "xx"}})
    assert response["ok"] is False and response["error"]["code"] == "invalid_configuration"
    assert response["error"]["errors"][0]["field"] == "defaults.language"
    out = io.StringIO()
    assert management.main(io.StringIO(json.dumps({"action": "read", "surface": "all"})), out) == 0
    answer = json.loads(out.getvalue())
    assert answer["ok"] and set(answer["result"]) == {"user_defaults", "routing", "routing_profiles",
                                                   "mcp_servers", "connections"}
    assert all(item["ok"] for item in answer["result"].values())
    out = io.StringIO()
    assert management.main(io.StringIO("not json"), out) == 1
    assert json.loads(out.getvalue())["error"]["code"] == "invalid_request"


def test_execute_project_ceiling_uses_the_project_store(home, tmp_path):
    project = tmp_path / "project"
    project.mkdir()
    Store(project / ".cortex", project_root=str(project))
    read = management.execute({"action": "read", "surface": "project_ceiling", "project": str(project)})
    assert read["ok"], read
    updated = management.execute({"action": "update", "surface": "project_ceiling", "project": str(project),
                                  "expected_revision": read["result"]["revision"],
                                  "ceiling": {"permission_level": "READ_ONLY"}})
    assert updated["ok"], updated
    assert updated["result"]["ceiling"]["permission_level"] == "READ_ONLY"
    everything = management.execute({"action": "read", "surface": "all", "project": str(project)})
    assert everything["result"]["project_ceiling"]["result"]["revision"] == updated["result"]["revision"]
    empty = tmp_path / "empty"
    empty.mkdir()
    missing = management.execute({"action": "read", "surface": "project_ceiling", "project": str(empty)})
    assert missing["error"]["code"] == "project_store_missing"


# --- routing.json models: automatic-routing selection, tiers and policy ---------

OPENCODE_MODEL = "anthropic/claude-sonnet-4-5"


def test_routing_models_are_validated_with_field_paths_and_round_trip(home):
    with pytest.raises(management.ConfigValidationError) as caught:
        management.update_routing("user", "absent", {"models": {
            "codex-acp": {"auto": ["gpt-6-luna", "gpt-6-luna"], "tiers": {"gpt-6-luna": "huge"},
                          "policy": {"gpt-6-luna": {"effort_ladder": ["max", "high"], "request_names": ["use"]}}},
            "opencode": {"auto": ["claude-sonnet-4-5"]},
            "gemini-acp": {}}})
    assert {
        ("rules.models.codex-acp.auto[1]", "duplicate_model"),
        ("rules.models.codex-acp.tiers.gpt-6-luna", "invalid_value"),
        ("rules.models.codex-acp.policy.gpt-6-luna.effort_ladder", "invalid_value"),
        ("rules.models.codex-acp.policy.gpt-6-luna.request_names[0]", "reserved_word"),
        ("rules.models.opencode.auto[0]", "model_not_exact"),
        ("rules.models.gemini-acp", "unknown_engine"),
    } <= _codes(caught.value)
    assert not (home.config / "routing.json").exists()
    rules = {"deny": ["haiku"], "models": {
        "opencode": {"auto": [OPENCODE_MODEL], "tiers": {OPENCODE_MODEL: "standard"}},
        "codex-acp": {"policy": {"gpt-6-sol": {"preferred_for": ["review"], "max_effort": None}}}}}
    written = management.update_routing("user", "absent", rules)
    assert written["effective_rules"] == rules and auto_route.load_overrides() == rules
    assert "models" in management.ROUTING_KEYS and len(management.ROUTING_KEYS) == 7
    builtin = written["builtin"]
    assert builtin["policy_attributes"] == ["effort_floor", "max_effort", "effort_ladder", "also_standard_for",
                                            "preferred_for", "request_names"]
    assert builtin["selection_modes"] == ["builtin", "listed"] and "ultra" not in builtin["policy_efforts"]
    assert builtin["tier_sources"] == ["configured", "legacy_tiers", "builtin", "position"]
    assert builtin["work_kind_labels"]["design"] == "Design and architecture"
    assert "worker-architect" in builtin["work_kind_roles"]["design"]
    assert builtin["builtin_affinity"]["design"] == "claude-agent-acp"
    # The whole managed rule set is replaced: omitting models removes it.
    management.update_routing("user", written["written_revision"], {"deny": ["haiku"]})
    assert json.loads((home.config / "routing.json").read_text()) == {"deny": ["haiku"]}


def test_route_catalog_reports_selection_tier_source_and_policy(home, monkeypatch):
    _catalog_fixture(home, monkeypatch)
    management.update_routing("user", "absent", {"models": {
        "codex-acp": {"auto": ["gpt-5.6-luna", "gpt-9-missing"], "tiers": {"gpt-6-sol": "flagship"}},
        "opencode": {"auto": ["anthropic/claude-sonnet-4-5"], "tiers": {"anthropic/claude-sonnet-4-5": "standard"},
                     "policy": {"anthropic/claude-sonnet-4-5": {"preferred_for": ["writing"]}}}}})
    view = management.read_route_catalog()
    routes, engines = _routes(view), _engines(view)
    older = routes["codex-acp/gpt-5.6-luna"]
    assert (older["automatic"], older["auto_selected"], older["selection"], older["superseded_by"]) == (
        "eligible", True, "listed", "codex-acp/gpt-6-luna")
    assert routes["codex-acp/gpt-6-luna"]["automatic"] == "not_selected"
    luna_policy = routes["codex-acp/gpt-6-luna"]["policy"]
    assert luna_policy["effort_floor"] == "high" and luna_policy["attribute_sources"]["effort_floor"] == "builtin"
    sol = routes["codex-acp/gpt-6-sol"]
    assert (sol["tier"], sol["tier_source"], sol["configured_tier"], sol["automatic"]) == (
        "flagship", "configured", "flagship", "not_selected")
    optional = routes[OPENCODE_ROUTE]
    assert (optional["automatic"], optional["tier"], optional["tier_source"]) == ("eligible", "standard", "configured")
    assert optional["policy"]["preferred_for"] == ["writing"]
    assert optional["preferred_for"] == ["writing"] and optional["preferred_inactive"] is False
    assert optional["policy"]["attribute_sources"] == {"preferred_for": "configured"}
    assert routes["opencode/openai/gpt-5.1"]["automatic"] == "not_selected"
    haiku = routes["claude-agent-acp/claude-haiku-4-5-20251001"]
    assert (haiku["selection"], haiku["auto_selected"], haiku["automatic"]) == ("builtin", None, "excluded_by_default")
    assert (engines["codex-acp"]["selection"], engines["codex-acp"]["selected_models"],
            engines["codex-acp"]["unmatched_selected"]) == ("listed", ["gpt-5.6-luna", "gpt-9-missing"], ["gpt-9-missing"])
    assert (engines["claude-agent-acp"]["selection"], engines["claude-agent-acp"]["selected_models"],
            engines["claude-agent-acp"]["unmatched_selected"]) == ("builtin", None, [])


def test_opencode_acp_observation_exposes_byteplus_only_when_offered(home, monkeypatch):
    _codex, _claude, opencode = _catalog_fixture(home, monkeypatch)
    target = "byteplus-plan/glm-5.3-flash"
    management.update_routing("user", "absent", {"models": {"opencode": {"auto": [target]}}})
    before = management.read_route_catalog()
    assert "opencode/" + target not in _routes(before)
    assert _engines(before)["opencode"]["unmatched_selected"] == [target]
    _observe(opencode, [(target, "GLM 5.3 Flash")])
    after = management.read_route_catalog()
    offered = _routes(after)["opencode/" + target]
    assert offered["observed"] and offered["in_selector"] and offered["automatic"] == "eligible"
    assert offered["auto_selected"] is True
    assert _engines(after)["opencode"]["unmatched_selected"] == []
