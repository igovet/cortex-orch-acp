"""Owner profiles (format 2): capture, explicit save, journaled activation,
format-1 migration and the secrets rule."""
import json

import pytest

from cortex_runtime.configuration import management, routing_profiles
from cortex_runtime.configuration import store as configuration_store
from cortex_runtime.configuration.policy import PolicyCeiling
from cortex_runtime.connections import onboarding, registry
from cortex_runtime.connections.model import AuthMode
from cortex_runtime.routing import auto_route
from model_policy_fixtures import routing_fixture



def update_routing(target, revision, rules, **kwargs):
    # Tests explicitly supply a full v2 policy; legacy snapshots below stay legacy.
    return management.update_routing(target, revision, routing_fixture(rules), **kwargs)


SECRET = "SENTINEL-SECRET-VALUE-7f3a"
ZERO = "sha256:" + "0" * 64
FILES = ("routing.json", "defaults.json", "mcp-servers.json", "connections.json")


@pytest.fixture
def env(tmp_path):
    home = tmp_path / "home"
    home.mkdir()
    return {"HOME": str(home), "XDG_CONFIG_HOME": str(tmp_path / "config"),
            "XDG_STATE_HOME": str(tmp_path / "state")}


def _root(env):
    return registry.config_root(env)


def _write(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value) if not isinstance(value, str) else value)
    path.chmod(0o600)


def _files(env, names=FILES):
    root = _root(env)
    return {name: (root / name).read_bytes() if (root / name).exists() else None for name in names}


def _read(env):
    response = management.execute({"surface": "routing_profiles", "action": "read"}, environ=env)
    assert response["ok"], response
    return response["result"]


def _run(env, action, **kwargs):
    view = _read(env)
    request = dict(surface="routing_profiles", action=action, expected_revision=view["revision"], **kwargs)
    if action in ("create", "save", "activate"):
        request.setdefault("expected_revisions", view["current_revisions"])
    return management.execute(request, environ=env)


def _ok(response):
    assert response["ok"], response
    return response["result"]


@pytest.fixture
def owner(env):
    """Every owner section populated; returns the connection id."""
    record = onboarding.create_connection(
        name="Codex key", engine="codex-acp", provider="openai", auth_mode=AuthMode.API_KEY,
        key_kind="openai_api_key", launch_command=("/usr/bin/node", "/opt/codex-acp/dist/index.js"), environ=env)
    _write(_root(env) / "mcp-servers.json", {"version": 1, "servers": {"github": {
        "command": "/usr/bin/gh-mcp", "args": ["--token", SECRET], "env": {"GITHUB_TOKEN": SECRET},
        "enabled": True, "allowed_tools": ["search"], "network": True}}})
    management.update_user_defaults("absent", defaults={"language": "ru"}, environ=env)
    update_routing("user", "absent", {"providers": ["claude-agent-acp"]}, environ=env)
    return record.id


def _edit_profiles(env, change):
    path = routing_profiles._paths(env)[0]
    document = json.loads(path.read_text())
    change(document)
    _write(path, document)


def _v1(env, active="mix", extra=None):
    profiles = {"mix": {"description": "Mixed", "rules": {"deny": ["opencode/*"]},
                        "created_at": "2026-09-01T00:00:00+00:00", "updated_at": "2026-09-02T00:00:00+00:00"}}
    profiles.update(extra or {})
    document = {"format": 1, "active": active, "profiles": profiles,
                "applied": {"profile": "mix", "routing_revision": "sha256:" + "a" * 64,
                            "profile_revision": "sha256:" + "b" * 64, "at": "2026-09-02T00:00:00+00:00"}}
    _write(_root(env) / "routing-profiles.json", document)
    _write(_root(env) / "routing.json", {"deny": ["opencode/*"]})
    return (_root(env) / "routing-profiles.json").read_bytes()


# --- format 1 migration ------------------------------------------------------------

def test_format1_read_is_migrated_in_memory_without_writing(env):
    original = _v1(env)
    management.update_user_defaults("absent", defaults={"language": "ru"}, environ=env)
    view = _read(env)
    assert view["format"] == 2 and view["migrated_from"] == 1
    assert view["profiles"][0]["sections"] == ["routing"]
    assert view["profiles"][0]["settings"] == {"routing": {"deny": ["opencode/*"]}}
    assert view["state"] == "in_sync" and view["active"] == "mix"
    assert view["applied"]["revisions"] == {"routing": "sha256:" + "a" * 64}
    assert (_root(env) / "routing-profiles.json").read_bytes() == original
    routing = management.read_routing(environ=env)["sources"][-1]["revision"]
    update_routing("user", routing, {"deny": ["codex-acp/*"]}, environ=env)
    changed = _read(env)
    assert changed["state"] == "modified"
    assert changed["differences"] == [{"section": "routing", "key": k} for k in ("deny", "enabled", "model_policy_version", "models")]
    assert (_root(env) / "routing-profiles.json").read_bytes() == original


def test_first_mutation_migrates_with_private_backup_and_verification(env):
    original = _v1(env, extra={"old": {"description": "", "rules": {}}})
    result = _ok(_run(env, "delete", name="old"))
    assert result["notes"][0] == "migrated_from_format_1"
    assert result["migrated_from"] is None and result["active"] == "mix"
    stored = json.loads((_root(env) / "routing-profiles.json").read_text())
    assert stored["format"] == 2 and stored["active"] == "mix"
    assert stored["profiles"]["mix"]["settings"]["routing"] == {"deny": ["opencode/*"]}
    assert stored["profiles"]["mix"]["created_at"] == "2026-09-01T00:00:00+00:00"
    assert stored["applied"]["revisions"]["routing"] == "sha256:" + "a" * 64
    backups = list((_root(env) / ".migration-backups").iterdir())
    assert len(backups) == 1 and backups[0].name.startswith("routing-profiles.format-1.")
    assert backups[0].read_bytes() == original and backups[0].stat().st_mode & 0o777 == 0o600
    assert "migrated_from_format_1" not in _ok(_run(env, "update", name="mix", description="x"))["notes"]


def test_migration_verification_failure_restores_original_bytes(env, monkeypatch):
    original = _v1(env)
    real = management._replace_file

    def corrupt(path, body):
        if path.name == "routing-profiles.json" and b'"format": 2' in body:
            body = body.replace(b'"opencode/*"', b'"codex-acp/*"')
        real(path, body)

    monkeypatch.setattr(management, "_replace_file", corrupt)
    response = _run(env, "update", name="mix", description="changed")
    assert response["error"]["code"] == "profile_migration_failed"
    assert (_root(env) / "routing-profiles.json").read_bytes() == original


def test_unknown_or_string_format_fails_closed(env):
    path = routing_profiles._paths(env)[0]
    for document in ({"format": 3, "profiles": {}}, {"format": "2", "profiles": {}}):
        _write(path, document)
        response = management.execute({"surface": "routing_profiles", "action": "read"}, environ=env)
        assert response["error"]["code"] == "unsupported_profiles_format"
    saved = update_routing("user", "absent", {"deny": ["opencode/*"]}, environ=env)
    assert saved["profile"] == {"state": "unavailable", "code": "unsupported_profiles_format"}
    assert auto_route.load_overrides(environ=env) == routing_fixture({"deny": ["opencode/*"]})
    path.unlink()
    target = path.parent / "target"
    target.write_text("{}")
    path.symlink_to(target)
    response = management.execute({"surface": "routing_profiles", "action": "read"}, environ=env)
    assert response["error"]["code"] == "unsafe_storage"


# --- capture and state ---------------------------------------------------------------

def test_create_captures_all_sections_and_activates_without_touching_files(env, owner):
    before = _files(env)
    result = _ok(_run(env, "create", name="work", description="Work setup"))
    assert result["active"] == "work" and result["state"] == "in_sync" and result["matches_profile"] == ["work"]
    profile = result["profiles"][0]
    assert profile["sections"] == ["routing", "user_defaults", "mcp_servers", "connections"]
    assert profile["settings"]["routing"] == routing_fixture({"providers": ["claude-agent-acp"]})
    assert profile["settings"]["user_defaults"]["defaults"] == {"language": "ru"}
    assert profile["settings"]["mcp_servers"] == {"github": {"enabled": True, "allowed_tools": ["search"],
                                                             "network": True}}
    assert profile["settings"]["connections"] == {owner: {"max_concurrent_sessions": 4}}
    assert profile["summary"]["language"] == "ru" and profile["summary"]["enabled_mcp_servers"] == 1
    assert profile["summary"]["ceiling_set"] is False
    assert _files(env) == before
    assert result["applied"]["revisions"] == result["current_revisions"]
    stored = (_root(env) / "routing-profiles.json")
    assert stored.stat().st_mode & 0o777 == 0o600
    assert SECRET not in stored.read_text() and "gh-mcp" not in stored.read_text()
    assert SECRET not in json.dumps(result)


def test_hand_added_mcp_definition_in_profile_is_refused(env, owner):
    _ok(_run(env, "create", name="work"))
    _edit_profiles(env, lambda doc: doc["profiles"]["work"]["settings"]["mcp_servers"]["github"].update(
        env={"GITHUB_TOKEN": "x"}))
    error = management.execute({"surface": "routing_profiles", "action": "read"}, environ=env)["error"]
    assert error["code"] == "invalid_profiles"
    assert error["field"] == "profiles.work.settings.mcp_servers.github.env"
    assert "x" not in error["message"].split()


def _edit_language(env, _):
    management.update_user_defaults(management.read_user_defaults(env)["revision"], defaults={"language": "en"},
                                    environ=env)


def _edit_ceiling(env, _):
    management.update_user_defaults(management.read_user_defaults(env)["revision"],
                                    ceiling={"permission_level": "STANDARD"}, environ=env)


def _edit_mcp(env, _):
    management.update_mcp_servers(management.read_mcp_servers(env)["revision"],
                                  [{"op": "set_enabled", "name": "github", "enabled": False}], environ=env)


def _edit_connection(env, connection_id):
    management.update_connection(management.read_connections(env)["revision"], connection_id,
                                 "set_max_concurrent_sessions", max_concurrent_sessions=2, environ=env)


def _edit_routing(env, _):
    revision = management.read_routing(environ=env)["sources"][-1]["revision"]
    update_routing("user", revision, {"providers": ["codex-acp"]}, environ=env)


def _edit_picker(env, _):
    configuration_store.write_user_defaults({"language": "en", "parameters": {}}, PolicyCeiling(), environ=env)


@pytest.mark.parametrize("edit, section, key", [
    (_edit_language, "user_defaults", "language"),
    (_edit_ceiling, "user_defaults", "ceiling"),
    (_edit_mcp, "mcp_servers", "github"),
    (_edit_connection, "connections", None),
    (_edit_routing, "routing", "providers"),
    (_edit_picker, "user_defaults", "language"),
])
def test_each_owner_section_edit_marks_modified(env, owner, edit, section, key):
    _ok(_run(env, "create", name="work"))
    edit(env, owner)
    view = _read(env)
    assert view["state"] == "modified" and view["modified_sections"] == [section]
    assert view["differences"] == [{"section": section, "key": key or owner}]
    assert view["matches_profile"] == []


def test_project_edits_do_not_mark_modified(env, owner, tmp_path):
    from cortex_runtime.project_storage import canonical_project, project_store_directory
    from cortex_runtime.store import Store

    project = tmp_path / "project"
    project.mkdir()
    root = canonical_project(str(project))
    store = Store(project_store_directory(root), initialize=True, project_root=root)
    _ok(_run(env, "create", name="work"))
    before = (_root(env) / "routing-profiles.json").read_bytes()
    update_routing("project", "absent", {"deny": ["codex-acp/*"]}, project_root=str(root), environ=env)
    management.update_project_ceiling(store, str(root), "absent", {"limits": {"attempts": 3}}, environ=env)
    view = routing_profiles.read_profiles(str(root), env)
    assert view["state"] == "in_sync" and view["shadowed_by"] == "project"
    assert (_root(env) / "routing-profiles.json").read_bytes() == before
    assert auto_route._effective_profile_label(str(root), env) is None


# --- save and create --------------------------------------------------------------------

def test_save_to_current_overwrites_active_and_upgrades_partial_profile(env):
    _v1(env)
    management.update_user_defaults("absent", defaults={"language": "ru"}, environ=env)
    assert _read(env)["state"] == "in_sync"
    saved = _ok(_run(env, "save", name="mix"))
    assert "migrated_from_format_1" in saved["notes"]
    profile = saved["profiles"][0]
    assert profile["sections"] == list(routing_profiles.PROFILE_SECTIONS)
    assert profile["settings"]["user_defaults"]["defaults"] == {"language": "ru"}
    assert profile["created_at"] == "2026-09-01T00:00:00+00:00" and saved["state"] == "in_sync"
    _edit_language(env, None)
    assert _read(env)["modified_sections"] == ["user_defaults"]
    assert _ok(_run(env, "save", name="mix"))["profiles"][0]["settings"]["user_defaults"]["defaults"] == \
        {"language": "en"}


def test_save_refusals(env, owner):
    _ok(_run(env, "create", name="a"))
    _ok(_run(env, "create", name="b"))
    _edit_language(env, None)
    before = _files(env, FILES + ("routing-profiles.json",))
    assert _run(env, "save", name="a")["error"]["code"] == "profile_not_active"
    view = _read(env)
    stale = dict(view["current_revisions"], user_defaults=ZERO)
    conflict = management.execute(dict(surface="routing_profiles", action="save", expected_revision=view["revision"],
                                       expected_revisions=stale, name="b"), environ=env)
    assert conflict["error"]["code"] == "revision_conflict"
    assert conflict["error"]["current_revision"] == view["current_revisions"]["user_defaults"]
    partial = management.execute(dict(surface="routing_profiles", action="save", expected_revision=view["revision"],
                                      expected_revisions={"routing": ZERO}, name="b"), environ=env)
    assert partial["error"]["code"] == "invalid_expected_revisions"
    assert _files(env, FILES + ("routing-profiles.json",)) == before
    _ok(_run(env, "deactivate"))
    assert _run(env, "save", name="b")["error"]["code"] == "no_active_profile"


def test_create_as_new_from_modified_keeps_previous_profile_content(env, owner):
    _ok(_run(env, "create", name="a"))
    _edit_language(env, None)
    created = _ok(_run(env, "create", name="b"))
    assert created["active"] == "b" and created["state"] == "in_sync" and created["matches_profile"] == ["b"]
    settings = {item["name"]: item["settings"] for item in created["profiles"]}
    assert settings["a"]["user_defaults"]["defaults"] == {"language": "ru"}
    assert settings["b"]["user_defaults"]["defaults"] == {"language": "en"}
    assert _run(env, "create", name="b")["error"]["code"] == "profile_exists"
    for name in ("Bad", "-bad", "x" * 41):
        assert _run(env, "create", name=name)["error"]["code"] == "invalid_profile_name"


# --- activation -------------------------------------------------------------------------------

def test_activate_materializes_every_section_and_preserves_unmanaged_parts(env, owner):
    root = _root(env)
    _write(root / "routing.json", routing_fixture({"providers": ["claude-agent-acp"], "_comment": "kept"}))
    defaults = json.loads((root / "defaults.json").read_text())
    defaults["defaults"]["future_key"] = 1
    defaults["other"] = True
    _write(root / "defaults.json", defaults)
    connection_before = management.read_connections(env)["connections"][0]
    _ok(_run(env, "create", name="a"))
    _edit_routing(env, None)
    _edit_language(env, None)
    _edit_mcp(env, None)
    _edit_connection(env, owner)
    management.update_mcp_servers(management.read_mcp_servers(env)["revision"], [{"op": "upsert", "name": "extra", "server": {
        "command": "/usr/bin/extra-mcp", "allowed_tools": ["x"]}}], environ=env)
    assert _run(env, "activate", name="a")["error"]["code"] == "profile_modified"
    result = _ok(_run(env, "activate", name="a", discard_modifications=True))
    assert result["state"] == "in_sync" and result["active"] == "a" and result["pending"] is None
    assert result["applied"]["revisions"] == result["current_revisions"]
    assert json.loads((root / "routing.json").read_text()) == routing_fixture({"providers": ["claude-agent-acp"], "_comment": "kept"})
    written = json.loads((root / "defaults.json").read_text())
    assert written["defaults"] == {"future_key": 1, "language": "ru"} and written["other"] is True
    servers = json.loads((root / "mcp-servers.json").read_text())["servers"]
    assert servers["github"]["enabled"] is True and servers["github"]["env"] == {"GITHUB_TOKEN": SECRET}
    assert servers["github"]["args"] == ["--token", SECRET] and servers["github"]["command"] == "/usr/bin/gh-mcp"
    assert servers["extra"]["enabled"] is False and servers["extra"]["command"] == "/usr/bin/extra-mcp"
    connection = management.read_connections(env)["connections"][0]
    assert connection["max_concurrent_sessions"] == 4
    for key in ("name", "launch_command", "credential", "state", "engine"):
        assert connection[key] == connection_before[key]
    for name in FILES:
        assert (root / name).stat().st_mode & 0o777 == 0o600
    assert auto_route._effective_profile_label(None, env) == {"name": "a", "state": "in_sync"}


def test_revert_to_saved_with_discard(env, owner):
    _ok(_run(env, "create", name="a"))
    _edit_language(env, None)
    _edit_routing(env, None)
    reverted = _ok(_run(env, "activate", name="a", discard_modifications=True))
    assert reverted["state"] == "in_sync"
    assert management.read_user_defaults(env)["defaults"]["language"] == "ru"
    assert _ok(_run(env, "activate", name="a"))["state"] == "in_sync"


def _hand_edit_activation_refused(env, change, code, field):
    _ok(_run(env, "create", name="a"))
    _ok(_run(env, "create", name="b"))
    _edit_profiles(env, change)
    before = _files(env, FILES + ("routing-profiles.json",))
    error = _run(env, "activate", name="a", discard_modifications=True)["error"]
    assert error["code"] == code and error["field"] == field
    assert _files(env, FILES + ("routing-profiles.json",)) == before
    assert _read(env)["pending"] is None and _read(env)["active"] == "b"


def test_activate_validates_everything_before_first_write(env, owner):
    def reference(doc):
        doc["profiles"]["a"]["settings"]["routing"] = routing_fixture({"providers": ["codex-acp"]})
        doc["profiles"]["a"]["settings"]["user_defaults"]["defaults"].update(
            route_id="xr_" + "1" * 12, route_connection_id="cn_ffffffffffff", route_model="gpt-5")
    _hand_edit_activation_refused(env, reference, "profile_reference_missing",
                                  "settings.user_defaults.defaults.route_connection_id")


def test_activate_refuses_policy_the_mcp_loader_rejects(env, owner):
    def policy(doc):
        doc["profiles"]["a"]["settings"]["routing"] = routing_fixture({"providers": ["codex-acp"]})
        doc["profiles"]["a"]["settings"]["mcp_servers"]["github"] = {"enabled": True}
    _hand_edit_activation_refused(env, policy, "invalid_configuration", "settings.mcp_servers")


def test_activate_refuses_a_missing_mcp_server(env, owner):
    _ok(_run(env, "create", name="a"))
    _ok(_run(env, "create", name="b"))
    management.update_mcp_servers(management.read_mcp_servers(env)["revision"],
                                  [{"op": "remove", "name": "github"}], environ=env)
    _edit_routing(env, None)
    before = _files(env, FILES + ("routing-profiles.json",))
    error = _run(env, "activate", name="a", discard_modifications=True)["error"]
    assert error["code"] == "profile_mcp_server_missing" and error["field"] == "settings.mcp_servers.github"
    assert _files(env, FILES + ("routing-profiles.json",)) == before


@pytest.mark.parametrize("section", routing_profiles.PROFILE_SECTIONS)
def test_activate_stale_revision_per_section_conflicts(env, owner, section):
    _ok(_run(env, "create", name="a"))
    before = _files(env, FILES + ("routing-profiles.json",))
    view = _read(env)
    response = management.execute(dict(surface="routing_profiles", action="activate", name="a",
                                       expected_revision=view["revision"],
                                       expected_revisions=dict(view["current_revisions"], **{section: ZERO})),
                                  environ=env)
    assert response["error"]["code"] == "revision_conflict"
    assert _files(env, FILES + ("routing-profiles.json",)) == before


def _diverge_everything(env, owner):
    _ok(_run(env, "create", name="a"))
    _ok(_run(env, "create", name="b"))
    _edit_routing(env, None)
    _edit_language(env, None)
    _edit_mcp(env, None)
    _edit_connection(env, owner)
    _ok(_run(env, "save", name="b"))


def test_journal_is_written_before_backing_files(env, owner, monkeypatch):
    _diverge_everything(env, owner)
    order = []
    replace_file, atomic, write_registry = (management._replace_file, registry.atomic_write_private,
                                            registry.write_registry)

    def record(name):
        if not order or order[-1] != name:
            order.append(name)

    def journaled_replace(path, body):
        if path.name == "routing-profiles.json":
            record("journal" if b'"operation": "activate"' in body else "profiles")
        else:
            record(path.name)
        replace_file(path, body)

    def recorded_atomic(path, data, *, directory):
        record(path.name)
        atomic(path, data, directory=directory)

    def recorded_registry(state, environ=None):
        record("connections.json")
        write_registry(state, environ)

    monkeypatch.setattr(management, "_replace_file", journaled_replace)
    monkeypatch.setattr(registry, "atomic_write_private", recorded_atomic)
    monkeypatch.setattr(registry, "write_registry", recorded_registry)
    _ok(_run(env, "activate", name="a"))
    assert order == ["journal", "routing.json", "defaults.json", "mcp-servers.json", "connections.json", "profiles"]


def test_failed_backing_write_rolls_back_in_process(env, owner, monkeypatch):
    _diverge_everything(env, owner)
    before = _files(env)
    view = _read(env)
    atomic = registry.atomic_write_private

    def failing(path, data, *, directory):
        if path.name == "mcp-servers.json":
            raise RuntimeError("simulated failure")
        atomic(path, data, directory=directory)

    monkeypatch.setattr(registry, "atomic_write_private", failing)
    with pytest.raises(RuntimeError, match="simulated failure"):
        routing_profiles.mutate("activate", view["revision"], name="a", expected_revisions=view["current_revisions"],
                                environ=env)
    monkeypatch.undo()
    assert _files(env) == before
    after = _read(env)
    assert after["pending"] is None and after["active"] == "b" and after["state"] == "in_sync"


def _interrupt(env, monkeypatch):
    view = _read(env)
    atomic, replace_file = registry.atomic_write_private, management._replace_file
    calls = []

    def failing_atomic(path, data, *, directory):
        if path.name == "mcp-servers.json":
            raise RuntimeError("simulated failure")
        atomic(path, data, directory=directory)

    def failing_restore(path, body):
        if path.name == "routing.json":
            calls.append(path)
            if len(calls) > 1:
                raise OSError("restore failed")
        replace_file(path, body)

    monkeypatch.setattr(registry, "atomic_write_private", failing_atomic)
    monkeypatch.setattr(management, "_replace_file", failing_restore)
    with pytest.raises(RuntimeError):
        routing_profiles.mutate("activate", view["revision"], name="a", expected_revisions=view["current_revisions"],
                                environ=env)
    monkeypatch.undo()


def test_unrecoverable_failure_is_interrupted_and_recoverable(env, owner, monkeypatch):
    _diverge_everything(env, owner)
    _interrupt(env, monkeypatch)
    view = _read(env)
    assert view["state"] == "interrupted" and view["pending"]["profile"] == "a" and view["active"] == "b"
    assert _run(env, "delete", name="a")["error"]["code"] == "profile_interrupted"
    recovered = _ok(_run(env, "activate", name="a"))
    assert recovered["state"] == "in_sync" and recovered["pending"] is None and recovered["active"] == "a"
    _ok(_run(env, "save", name="a"))
    _ok(_run(env, "activate", name="b", discard_modifications=True))
    _interrupt(env, monkeypatch)
    assert _read(env)["state"] == "interrupted"
    detached = _ok(_run(env, "deactivate"))
    assert detached["state"] == "none" and detached["pending"] is None


# --- metadata, removal, other surfaces ----------------------------------------------------------

def test_update_is_metadata_only_and_legacy_fields_are_refused(env, owner):
    _ok(_run(env, "create", name="a"))
    view = _read(env)
    for action, extra in (("update", {"rules": {}}), ("create", {"from_current": True}),
                          ("activate", {"expected_routing_revision": "absent"})):
        request = dict(surface="routing_profiles", action=action, expected_revision=view["revision"], name="a",
                       **extra)
        assert management.execute(request, environ=env)["error"]["code"] == "unsupported_request_field"
    renamed = _ok(_run(env, "update", name="a", rename_to="work", description="Renamed"))
    assert renamed["active"] == "work" and renamed["applied"]["profile"] == "work"
    assert renamed["profiles"][0]["description"] == "Renamed" and renamed["state"] == "in_sync"
    before = _files(env)
    deleted = _ok(_run(env, "delete", name="work"))
    assert deleted["profiles"] == [] and deleted["active"] is None
    assert _files(env) == before


def test_user_routing_save_no_longer_writes_through(env, owner, tmp_path):
    _ok(_run(env, "create", name="a"))
    revision = management.read_routing(environ=env)["sources"][-1]["revision"]
    saved = update_routing("user", revision, {"deny": ["opencode/*"]}, environ=env)
    assert saved["profile"] == {"name": "a", "state": "modified"}
    assert _read(env)["profiles"][0]["settings"]["routing"] == routing_fixture({"providers": ["claude-agent-acp"]})
    project = tmp_path / "project"
    (project / ".cortex").mkdir(parents=True)
    written = update_routing("project", "absent", {"deny": ["codex-acp/*"]},
                                        project_root=str(project), environ=env)
    assert written["profile"] is None


def test_effective_label_tracks_routing_match_only(env, owner):
    _ok(_run(env, "create", name="a"))
    assert auto_route._effective_profile_label(None, env) == {"name": "a", "state": "in_sync"}
    _edit_language(env, None)
    assert auto_route._effective_profile_label(None, env) == {"name": "a", "state": "in_sync"}
    _edit_routing(env, None)
    assert auto_route._effective_profile_label(None, env) is None


def test_removed_connection_limit_is_skipped_with_note(env, owner):
    _ok(_run(env, "create", name="a"))
    _ok(_run(env, "create", name="b"))
    _edit_profiles(env, lambda doc: doc["profiles"]["a"]["settings"]["connections"].update(
        cn_ffffffffffff={"max_concurrent_sessions": 2}))
    assert _read(env)["notes"] == []
    result = _ok(_run(env, "activate", name="a"))
    assert result["state"] == "in_sync" and "connection_missing:cn_ffffffffffff" in result["notes"]
    assert _read(env)["notes"] == ["connection_missing:cn_ffffffffffff"]
