"""First-run management: isolated owner storage, no network or paid engine calls."""
import io
import json
import stat
import subprocess
from pathlib import Path

import pytest

from cortex_runtime.configuration import management as m
from cortex_runtime.connections import discovery, engine_update, onboarding, registry
from cortex_runtime.packaging.pinned import load_manifest
from cortex_runtime.portable.errors import PortableError
from cortex_runtime.routing import defaults


@pytest.fixture
def env(tmp_path, monkeypatch):
    home = tmp_path / "home"
    home.mkdir()
    node = home / "node"
    node.write_text("#!/bin/sh\nexit 99\n")
    node.chmod(0o755)
    values = dict(CORTEX_NODE_BINARY=str(node), HOME=str(home), XDG_CONFIG_HOME=str(tmp_path / "config"),
                  XDG_STATE_HOME=str(tmp_path / "state"), XDG_DATA_HOME=str(tmp_path / "data"),
                  CORTEX_ENGINE_UPDATE="off", CORTEX_ENGINE_FOLLOW="off", PATH="/usr/bin:/bin")
    for key, value in values.items():
        monkeypatch.setenv(key, value)
    monkeypatch.delenv("CORTEX_ROUTING", raising=False)
    monkeypatch.setattr(discovery, "discover_opencode", lambda **kw: None)
    return values


def request(env, surface="connections", action="discover_connections", **fields):
    return m.execute(dict(api_version=1, surface=surface, action=action, **fields), environ=env)


def installed(env, engine, version=None):
    version = version or load_manifest()["engines"][engine]["pinned_package_version"]
    pkg = discovery.owned_root(env) / engine / version / "node_modules" / discovery._PACKAGE_NAME[engine]
    pkg.mkdir(parents=True)
    (pkg / "index.js").write_text("// fake, never executed")
    (pkg / "package.json").write_text(json.dumps(dict(version=version, bin={"engine": "index.js"})))
    return pkg


def login(env, engine):
    directory = registry.client_native_dir(engine, env)
    directory.mkdir(parents=True, exist_ok=True)
    directory.chmod(0o700)
    marker = directory / onboarding._NATIVE_LOGIN_MARKER[engine]
    marker.write_text("not a real credential; contents must never be read")
    marker.chmod(0o600)
    return marker


def tree(tmp_path):
    return {str(p.relative_to(tmp_path)): (p.read_bytes() if p.is_file() else None)
            for p in tmp_path.rglob("*")}


def test_startup_seeds_once_and_never_changes_existing_bytes(env, monkeypatch):
    import cortex_acp

    monkeypatch.setattr(discovery, "discover_all", lambda: {})
    monkeypatch.setattr(engine_update, "start_background_update", lambda: None)
    cortex_acp._ensure_first_run_configuration()
    from cortex_runtime.configuration import routing_profiles
    profile = routing_profiles.read_profiles(environ=env)
    assert profile["active"] == "default" and profile["state"] == "in_sync"
    root = registry.config_root(env)
    before = {name: (root / name).read_bytes() for name in ("routing.json", "defaults.json")}
    assert json.loads(before["routing.json"]) == defaults.default_routing()
    assert json.loads(before["defaults.json"])["defaults"]["coordinator"] == defaults.default_coordinator()
    # Existing invalid owner files are not silently repaired either.
    (root / "routing.json").write_text("{owner-invalid")
    before["routing.json"] = b"{owner-invalid"
    cortex_acp._ensure_first_run_configuration()
    assert {name: (root / name).read_bytes() for name in before} == before


def test_fresh_seed_installs_one_applied_profile_and_disabled_toggleable_mcp(env, tmp_path):
    from cortex_runtime.configuration import mcp_servers, routing_profiles

    result = defaults.apply(env)
    assert all(result[key]["changed"] for key in ("routing", "defaults", "mcp_servers", "routing_profiles"))
    view = routing_profiles.read_profiles(environ=env)
    assert view["active"] == view["applied"]["profile"] == "default"
    assert view["state"] == "in_sync" and view["matches_profile"] == ["default"]
    assert [p["name"] for p in view["profiles"]] == ["default"]
    assert view["profiles"][0]["sections"] == list(routing_profiles.PROFILE_SECTIONS)
    assert not registry.registry_path(env).exists()  # no copied account identifiers
    root = registry.config_root(env)
    for name in ("routing.json", "defaults.json", "mcp-servers.json", "routing-profiles.json"):
        assert stat.S_IMODE((root / name).stat().st_mode) == 0o600
    assert mcp_servers.load_mcp_servers(str(tmp_path), environ=env) == []
    servers = m.read_mcp_servers(environ=env)
    assert {s["name"] for s in servers["servers"]} == {"codebase-memory-mcp", "playwright", "vivaldi"}
    assert all(s["enabled"] is False and s["valid"] for s in servers["servers"])
    before = {p.name: p.read_bytes() for p in root.glob("*.json")}
    assert not any(item["changed"] for item in defaults.apply(env).values())
    assert before == {p.name: p.read_bytes() for p in root.glob("*.json")}
    toggled = m.update_mcp_servers(servers["revision"], [
        {"op": "set_enabled", "name": "vivaldi", "enabled": True}], environ=env)
    assert next(s for s in toggled["servers"] if s["name"] == "vivaldi")["enabled"] is True
    assert routing_profiles.read_profiles(environ=env)["state"] == "modified"


@pytest.mark.parametrize("filename", ["routing.json", "defaults.json", "mcp-servers.json", "routing-profiles.json"])
def test_seeding_preserves_existing_owner_files_even_invalid(env, filename):
    root = registry.config_root(env)
    registry.ensure_private_dir(root, root=root, environ=env)
    path = root / filename
    path.write_bytes(b"{owner-invalid")
    path.chmod(0o600)
    defaults.apply(env)
    assert path.read_bytes() == b"{owner-invalid"
    if filename != "routing-profiles.json":
        assert not (root / "routing-profiles.json").exists()


def test_absent_profile_is_not_created_over_custom_working_settings(env):
    from cortex_runtime.configuration import routing_profiles

    defaults.apply(env)
    root = registry.config_root(env)
    # Simulate an older installation without profiles and with owner changes.
    (root / "routing-profiles.json").unlink()
    (root / "defaults.json").write_text('{"defaults":{"language":"en"}}')
    before = (root / "defaults.json").read_bytes()
    assert defaults.apply(env)["routing_profiles"]["status"] == "custom_settings"
    assert (root / "defaults.json").read_bytes() == before
    assert routing_profiles.read_profiles(environ=env)["active"] is None


def test_partial_first_run_can_finish_without_replacing_backing_files(env):
    defaults.apply(env)
    root = registry.config_root(env)
    before = {p.name: p.read_bytes() for p in root.glob("*.json") if p.name != "routing-profiles.json"}
    (root / "routing-profiles.json").unlink()
    assert defaults.apply(env)["routing_profiles"]["changed"]
    assert before == {p.name: p.read_bytes() for p in root.glob("*.json") if p.name != "routing-profiles.json"}


def test_profile_publication_failure_preserves_per_file_outcomes_and_retries(env, monkeypatch):
    create = defaults._create_private
    def fail_profile(path, body):
        if path.name == "routing-profiles.json":
            raise OSError("simulated full disk")
        return create(path, body)
    monkeypatch.setattr(defaults, "_create_private", fail_profile)
    result = defaults.apply(env)
    assert all(result[key]["changed"] for key in ("routing", "defaults", "mcp_servers"))
    assert result["routing_profiles"]["status"] == "invalid"
    assert not result["routing_profiles"]["changed"]
    monkeypatch.setattr(defaults, "_create_private", create)
    retried = defaults.apply(env)
    assert retried["routing_profiles"]["changed"]
    assert all(not retried[key]["changed"] for key in ("routing", "defaults", "mcp_servers"))


def test_seeding_failure_cannot_block_startup_or_other_helpers(env, monkeypatch):
    import cortex_acp

    calls = []
    def fail():
        raise RuntimeError("fake asset failure")
    monkeypatch.setattr(defaults, "apply", fail)
    monkeypatch.setattr(discovery, "discover_all", lambda: {})
    monkeypatch.setattr(engine_update, "start_background_update", lambda: calls.append("update"))
    cortex_acp._ensure_first_run_configuration()
    assert calls == ["update"]


def test_discovery_never_mutates_empty_storage_and_reports_all_engines(env, tmp_path):
    before = tree(tmp_path)
    response = request(env)
    assert response["ok"], response
    assert tree(tmp_path) == before
    assert response["result"]["revision"] == "absent"
    rows = response["result"]["engines"]
    assert [r["engine"] for r in rows] == ["codex-acp", "claude-agent-acp", "opencode"]
    assert all(r["status"] == "not_downloaded" and r["already_connected"] is None for r in rows)


@pytest.mark.parametrize("engine", ["codex-acp", "claude-agent-acp"])
def test_discovery_states_and_native_only_idempotent_onboarding(env, engine, tmp_path, monkeypatch):
    installed(env, engine)
    row = next(r for r in request(env)["result"]["engines"] if r["engine"] == engine)
    assert row["status"] == "downloaded_no_login"
    missing = request(env, action="onboard_connection", engine=engine,
                      expected_revision="absent", explicit_opt_in=True)
    assert missing["error"]["code"] == "native_login_required"
    marker = login(env, engine)
    # Guard both common reads: marker bytes must not enter the process.
    read_bytes, read_text = Path.read_bytes, Path.read_text
    def guarded_bytes(path, *args, **kw):
        assert path != marker
        return read_bytes(path, *args, **kw)
    def guarded_text(path, *args, **kw):
        assert path != marker
        return read_text(path, *args, **kw)
    monkeypatch.setattr(Path, "read_bytes", guarded_bytes)
    monkeypatch.setattr(Path, "read_text", guarded_text)
    row = next(r for r in request(env)["result"]["engines"] if r["engine"] == engine)
    assert row["status"] == "ready_to_connect" and row["native_login_detected"]
    response = request(env, action="onboard_connection", engine=engine,
                       expected_revision="absent", explicit_opt_in=True)
    assert response["ok"], response
    result = response["result"]
    assert result["connection"]["auth_mode"] == "native_account"
    assert result["connection"]["state"] == "configured"
    record = registry.read_registry(env).connections[result["connection"]["id"]]
    assert not record.credential_ref.owned and record.credential_ref.path == str(marker.parent)
    data = registry.registry_path(env).read_bytes()
    again = request(env, action="onboard_connection", engine=engine,
                    expected_revision=result["revision"], explicit_opt_in=True)
    assert again["ok"] and not again["result"]["changed"]
    assert registry.registry_path(env).read_bytes() == data
    row = next(r for r in request(env)["result"]["engines"] if r["engine"] == engine)
    assert row["status"] == "connected" and row["already_connected"] == record.id
    assert str(marker) not in json.dumps(response)


@pytest.mark.parametrize("opt_in", [None, False, 1, "true"])
def test_onboarding_requires_actual_opt_in_before_mutating(env, opt_in):
    response = request(env, action="onboard_connection", engine="codex-acp",
                       expected_revision="absent", explicit_opt_in=opt_in)
    assert response["error"]["code"] == "explicit_opt_in_required"
    assert not registry.config_root(env).exists()


@pytest.mark.parametrize("extra", [
    {"api_key": "sk-not-a-real-key"}, {"auth_mode": "api_key"}, {"key_kind": "openai_api_key"},
    {"binary": "/arbitrary"}, {"launch_command": ["arbitrary"]}, {"credential_ref": "/arbitrary"},
])
def test_onboarding_rejects_keys_paths_and_commands(env, extra):
    response = request(env, action="onboard_connection", engine="codex-acp",
                       expected_revision="absent", explicit_opt_in=True, **extra)
    assert response["error"]["code"] == "unsupported_request_field"
    assert not registry.config_root(env).exists()


def test_onboarding_checks_revision_again_at_creation_boundary(env, monkeypatch):
    installed(env, "codex-acp")
    login(env, "codex-acp")
    real_create = onboarding.create_referenced_connection
    def racing_create(**kw):
        real_create(name="concurrent", engine="claude-agent-acp", provider="anthropic",
                    launch_command=("/usr/bin/node", "/fake/index.js"), environ=env)
        return real_create(**kw)
    monkeypatch.setattr(onboarding, "create_referenced_connection", racing_create)
    response = request(env, action="onboard_connection", engine="codex-acp",
                       expected_revision="absent", explicit_opt_in=True)
    assert response["error"]["code"] == "revision_conflict"
    assert {r.engine for r in registry.read_registry(env).connections.values()} == {"claude-agent-acp"}


def test_onboarding_rejects_stale_revision_and_unqualified_adapter(env):
    installed(env, "codex-acp", "999.0.0")
    login(env, "codex-acp")
    stale = request(env, action="onboard_connection", engine="codex-acp",
                    expected_revision="sha256:" + "0" * 64, explicit_opt_in=True)
    assert stale["error"]["code"] == "revision_conflict"
    response = request(env, action="onboard_connection", engine="codex-acp",
                       expected_revision="absent", explicit_opt_in=True)
    assert response["error"]["code"] == "engine_version_unqualified"
    assert not registry.registry_path(env).exists()


def test_preset_apply_is_absent_only_and_reset_backs_up_both_files(env):
    initial = request(env, surface="default_preset", action="read")
    assert initial["ok"] and not registry.config_root(env).exists()
    result = request(env, surface="default_preset", action="apply_default_preset")
    assert result["ok"] and result["result"]["changed"]
    root = registry.config_root(env)
    original = {n: (root / (n + ".json")).read_bytes() for n in ("routing", "defaults")}
    assert not request(env, surface="default_preset", action="apply_default_preset")["result"]["changed"]
    for n in original:
        (root / (n + ".json")).write_bytes(b"{owner-" + n.encode())
    old = {n: (root / (n + ".json")).read_bytes() for n in original}
    applied = request(env, surface="default_preset", action="apply_default_preset")["result"]
    assert all(not v["changed"] and v["status"] not in ("current", "created")
               for n, v in applied["settings"].items() if n in ("routing", "defaults"))
    assert {n: (root / (n + ".json")).read_bytes() for n in original} == old
    reset = request(env, surface="default_preset", action="reset_default_preset",
                    expected_revision=applied["revision"], confirm=True)
    assert reset["ok"], reset
    for n, backup in reset["result"]["backups"].items():
        assert Path(backup).read_bytes() == old[n]
        assert stat.S_IMODE(Path(backup).stat().st_mode) == 0o600
        assert (root / (n + ".json")).read_bytes() == original[n]
        assert stat.S_IMODE((root / (n + ".json")).stat().st_mode) == 0o600


def test_reset_requires_confirmation_and_combined_revision(env):
    view = request(env, surface="default_preset", action="apply_default_preset")["result"]
    root = registry.config_root(env)
    missing = request(env, surface="default_preset", action="reset_default_preset",
                      expected_revision=view["revision"])
    assert missing["error"]["code"] == "confirmation_required"
    # A change to defaults alone invalidates the combined revision.
    (root / "defaults.json").write_text("{}")
    stale = request(env, surface="default_preset", action="reset_default_preset",
                    expected_revision=view["revision"], confirm=True)
    assert stale["error"]["code"] == "revision_conflict"
    assert not list(root.glob("*.cortex-backup-*"))


@pytest.mark.parametrize("existing", [False, True])
def test_reset_rolls_back_if_second_write_fails(env, monkeypatch, existing):
    if existing:
        m.apply_default_preset(environ=env)
    revision = m.read_default_preset(environ=env)["revision"]
    root = registry.config_root(env)
    before = {n: (root / (n + ".json")).read_bytes() if existing else None for n in ("routing", "defaults")}
    real_replace = m._replace_file
    def failing(path, body):
        if path.name == "defaults.json":
            raise OSError("fake disk failure")
        return real_replace(path, body)
    monkeypatch.setattr(m, "_replace_file", failing)
    response = request(env, surface="default_preset", action="reset_default_preset",
                       expected_revision=revision, confirm=True)
    assert response["error"]["code"] == "storage_error"
    assert {n: (root / (n + ".json")).read_bytes() if (root / (n + ".json")).exists() else None
            for n in before} == before


def test_reset_refuses_invalid_asset(env, tmp_path, monkeypatch):
    m.apply_default_preset(environ=env)
    revision = m.read_default_preset(environ=env)["revision"]
    asset = tmp_path / "bad-template.json"
    asset.write_text("{}")
    monkeypatch.setattr(defaults, "TEMPLATE_PATH", asset)
    response = request(env, surface="default_preset", action="reset_default_preset",
                       expected_revision=revision, confirm=True)
    assert response["error"]["code"] == "invalid_default_preset"


@pytest.mark.parametrize("engine", ["codex-acp", "claude-agent-acp"])
def test_install_downloads_exact_pin_to_owned_root_without_connecting(env, monkeypatch, engine):
    from test_engine_update import FAKE_NPM
    npm = Path(env["HOME"]) / "npm"
    npm.write_text(FAKE_NPM)
    npm.chmod(0o755)
    monkeypatch.setattr(engine_update, "_npm", lambda environ: str(npm))
    calls = []
    real_run = engine_update._run
    def run(args, timeout, environ):
        calls.append((args, timeout))
        return real_run(args, timeout, environ)
    monkeypatch.setattr(engine_update, "_run", run)
    response = request(env, action="install_engine", engine=engine)
    assert response["ok"], response
    pin = load_manifest()["engines"][engine]["pinned_package_version"]
    assert response["result"]["version"] == pin
    assert len(calls) == 1 and calls[0][0][1] == "install" and calls[0][1] == 60
    assert calls[0][0][-1] == discovery._PACKAGE_NAME[engine] + "@" + pin
    found = discovery.discover_engine(engine, environ=env)
    assert Path(found.launch_command[1]).is_relative_to(discovery.owned_root(env))
    assert not registry.registry_path(env).exists()
    assert request(env, action="install_engine", engine=engine)["ok"]
    assert len(calls) == 1  # idempotent owned install, no npm/latest lookup


def test_install_timeout_and_unsupported_inputs_are_safe(env, monkeypatch):
    def timeout(*args, **kw):
        raise subprocess.TimeoutExpired("fake npm", 60, output="private-output")
    monkeypatch.setattr(engine_update, "install_pinned_engine", timeout)
    failed = request(env, action="install_engine", engine="codex-acp")
    assert failed["error"]["code"] == "engine_install_failed"
    assert "private-output" not in json.dumps(failed)
    assert request(env, action="install_engine", engine="opencode")["error"]["code"] == "engine_install_unsupported"
    assert request(env, action="install_engine", engine="unknown")["error"]["code"] == "unsupported_engine"
    assert request(env, action="install_engine", engine="codex-acp",
                   version="latest")["error"]["code"] == "unsupported_request_field"


def test_discovery_is_non_mutating_with_installed_adapters(env, tmp_path):
    installed(env, "codex-acp")
    installed(env, "claude-agent-acp")
    login(env, "codex-acp")
    before = tree(tmp_path)
    assert request(env)["ok"]
    assert tree(tmp_path) == before


def test_reset_refuses_symlink_without_changing_target(env, tmp_path):
    m.apply_default_preset(environ=env)
    revision = m.read_default_preset(environ=env)["revision"]
    path = registry.config_root(env) / "routing.json"
    target = tmp_path / "unrelated.json"
    target.write_text("owner-private")
    path.unlink()
    path.symlink_to(target)
    response = request(env, surface="default_preset", action="reset_default_preset",
                       expected_revision=revision, confirm=True)
    assert response["error"]["code"] == "unsafe_storage"
    assert target.read_text() == "owner-private"
    assert not list(path.parent.glob("*.cortex-backup-*"))


def test_explicit_opencode_onboarding_remains_native_and_does_not_enable_routing(env, monkeypatch):
    from cortex_runtime.engine import opencode_install
    login(env, "opencode")
    found = discovery.EngineDiscovery("opencode", "2.1.0", ("/fake/opencode", "acp"),
                                      str(registry.client_native_dir("opencode", env)))
    monkeypatch.setattr(discovery, "discover_opencode", lambda **kw: found)
    verified = []
    monkeypatch.setattr(opencode_install, "verify_install", lambda install, environ: verified.append(install))
    response = request(env, action="onboard_connection", engine="opencode",
                       expected_revision="absent", explicit_opt_in=True)
    assert response["ok"], response
    assert verified and response["result"]["connection"]["auth_mode"] == "native_account"
    assert not (registry.config_root(env) / "routing.json").exists()


def test_installer_does_not_use_client_adapter_or_publish_wrong_version(env, monkeypatch):
    client = Path(env["HOME"]) / ".local/share/zed/adapter"
    client.mkdir(parents=True)
    (client / "package.json").write_text('{"version":"2.0.1"}')
    monkeypatch.setattr(engine_update, "_npm", lambda environ: "/fake/npm")
    def wrong_package(args, timeout, environ):
        staging = Path(args[args.index("--prefix") + 1])
        package = staging / "node_modules" / discovery._PACKAGE_NAME["codex-acp"]
        package.mkdir(parents=True)
        (package / "package.json").write_text('{"version":"999.0.0","bin":"index.js"}')
        (package / "index.js").write_text("// fake")
        return ""
    monkeypatch.setattr(engine_update, "_run", wrong_package)
    response = request(env, action="install_engine", engine="codex-acp")
    assert response["error"]["code"] == "engine_install_failed"
    root = discovery.owned_root(env) / "codex-acp"
    assert not discovery._subdirs(root) and not list(root.glob(".tmp-*"))
    assert (client / "package.json").read_text() == '{"version":"2.0.1"}'


def test_installer_never_confuses_a_broken_pin_with_another_qualified_install(env, monkeypatch):
    pinned = installed(env, "codex-acp")
    (pinned / "index.js").unlink()
    installed(env, "codex-acp", "2.1.1")
    monkeypatch.setattr(engine_update, "_npm", lambda environ: "/fake/npm")
    def unexpected(*args, **kwargs):
        pytest.fail("A broken existing owned pin must fail closed, not run npm or substitute.")
    monkeypatch.setattr(engine_update, "_run", unexpected)
    response = request(env, action="install_engine", engine="codex-acp")
    assert response["error"]["code"] == "engine_install_failed"


def test_main_dispatches_new_management_actions(env):
    output = io.StringIO()
    assert m.main(io.StringIO('{"surface":"connections","action":"discover_connections"}'), output) == 0
    assert len(json.loads(output.getvalue())["result"]["engines"]) == 3


@pytest.mark.parametrize("engine", discovery.SUPPORTED_ENGINES)
def test_automatic_install_connect_refresh_in_one_pass(env, monkeypatch, engine):
    from cortex_runtime.connections import automatic
    from cortex_runtime.catalog import refresh
    from types import SimpleNamespace

    env["CORTEX_ENGINE_UPDATE"] = "on"
    marker = login(env, engine)
    calls = []
    def install(which, **kw):
        installed(env, which)
        calls.append(("install", which))
    def observe(record, workspace, **kw):
        assert record.id in registry.read_registry(env).connections
        assert record.auth_mode.value == "native_account"
        assert kw["timeout"] == 20 and Path(workspace).is_dir()
        calls.append(("models", record.engine))
        return SimpleNamespace(success=True)
    real_read = Path.read_bytes
    def read(path):
        assert path != marker, "automatic setup must never read a credential"
        return real_read(path)
    monkeypatch.setattr(Path, "read_bytes", read)
    monkeypatch.setattr(engine_update, "update_engine", install)
    monkeypatch.setattr(refresh, "perform_refresh", observe)
    automatic.run(environ=env)
    records = registry.read_registry(env).connections
    assert [r.engine for r in records.values()] == [engine]
    assert calls.index(("install", engine)) < calls.index(("models", engine))
    assert automatic.read_status(engine, env) == "ready"
    other = next(e for e in discovery.SUPPORTED_ENGINES if e != engine)
    assert automatic.read_status(other, env) == "login_required"


def test_automatic_respects_removal_and_explicit_restore(env, monkeypatch):
    from cortex_runtime.connections import automatic
    env["CORTEX_ENGINE_UPDATE"] = "on"
    for engine in discovery.SUPPORTED_ENGINES:
        installed(env, engine)
        login(env, engine)
    monkeypatch.setattr(engine_update, "update_engine", lambda *a, **kw: None)
    monkeypatch.setattr(automatic, "refresh_connection", lambda *a, **kw: "ready")
    automatic.run(environ=env)
    before = registry.read_registry(env)
    assert len(before.connections) == 2
    record = next(r for r in before.connections.values() if r.engine == "codex-acp")
    onboarding.remove(record.id, environ=env)
    automatic.run(environ=env)
    state = registry.read_registry(env)
    assert len(state.connections) == 1
    assert state.auto_connect_disabled == {"codex-acp"}
    assert automatic.read_status("codex-acp", env) == "opted_out"
    row = request(env)["result"]["engines"][0]
    assert row["auto_connect_disabled"] and row["status"] == "ready_to_connect"
    result = request(env, action="onboard_connection", engine="codex-acp",
                     expected_revision=request(env)["result"]["revision"], explicit_opt_in=True)
    assert result["ok"] and result["result"]["catalog_status"] == "ready"
    assert len(registry.read_registry(env).connections) == 2
    # The owner-removal opt-out survives manual restoration and later startup.
    assert "codex-acp" in registry.read_registry(env).auto_connect_disabled


def test_automatic_serializes_concurrent_startups_and_never_rewrites_existing(env, monkeypatch):
    import threading
    from cortex_runtime.connections import automatic
    env["CORTEX_ENGINE_UPDATE"] = "on"
    installed(env, "codex-acp")
    login(env, "codex-acp")
    entered, release = threading.Event(), threading.Event()
    calls = []
    def update(engine, **kw):
        calls.append(engine)
        if engine == "codex-acp":
            entered.set()
            assert release.wait(5)
    monkeypatch.setattr(engine_update, "update_engine", update)
    monkeypatch.setattr(automatic, "refresh_connection", lambda *a, **kw: "ready")
    worker = threading.Thread(target=automatic.run, kwargs={"environ": env})
    worker.start()
    try:
        assert entered.wait(5)
        assert automatic.read_status("codex-acp", env) == "downloading"
        automatic.run(environ=env)
        assert calls == ["codex-acp"]
    finally:
        release.set()
        worker.join(5)
    assert not worker.is_alive()
    before = registry.registry_path(env).read_bytes()
    automatic.run(environ=env)
    assert registry.registry_path(env).read_bytes() == before


def test_automatic_existing_copy_works_offline_and_catalog_failure_is_visible(env, monkeypatch):
    from cortex_runtime.connections import automatic
    env["CORTEX_ENGINE_UPDATE"] = "on"
    installed(env, "codex-acp")
    login(env, "codex-acp")
    def offline(*a, **kw):
        raise OSError("private registry error must not be exposed")
    monkeypatch.setattr(engine_update, "update_engine", offline)
    monkeypatch.setattr(automatic, "refresh_connection", lambda *a, **kw: "catalog_failed")
    automatic.run(environ=env)
    assert len(registry.read_registry(env).connections) == 1
    rows = request(env)["result"]["engines"]
    assert rows[0]["automatic_setup"] == "catalog_failed"
    assert rows[1]["automatic_setup"] == "download_failed"
    assert "private registry" not in json.dumps(rows)


def test_automatic_off_is_non_mutating_and_stale_progress_is_interrupted(env, tmp_path):
    from cortex_runtime.connections import automatic
    before = tree(tmp_path)
    automatic.run(environ=env)
    assert tree(tmp_path) == before
    automatic._status("codex-acp", "downloading", env)
    assert automatic.read_status("codex-acp", env) == "interrupted"


def test_removal_racing_automatic_creation_stays_removed(env):
    found = discovery.EngineDiscovery(engine="codex-acp", version="2.3.0",
                                     launch_command=("/usr/bin/node", "/fake/index.js"),
                                     native_credential_dir=str(registry.client_native_dir("codex-acp", env)))
    login(env, "codex-acp")
    record = onboarding.create_referenced_connection(name="owner", engine="codex-acp", provider="openai",
                                                    launch_command=found.launch_command, environ=env)
    onboarding.remove(record.id, environ=env)
    assert onboarding.sync_discovered_connections({"codex-acp": found}, environ=env) == []
    assert not registry.read_registry(env).connections


@pytest.mark.parametrize("disabled", ["codex-acp", ["unknown"], ["codex-acp", "codex-acp"], [1]])
def test_registry_rejects_malformed_automatic_opt_out(disabled):
    with pytest.raises(PortableError):
        registry._parse_registry_bytes(json.dumps({"schema_version": 1, "auto_connect_disabled": disabled}).encode())


def test_manual_registration_refreshes_without_rolling_back_on_failure(env, monkeypatch):
    from cortex_runtime.connections import automatic
    from cortex_runtime.catalog import refresh
    installed(env, "codex-acp")
    login(env, "codex-acp")
    calls = []
    def broken(record, workspace, **kw):
        calls.append(record.id)
        raise RuntimeError("not returned to Settings")
    monkeypatch.setattr(refresh, "perform_refresh", broken)
    response = request(env, action="onboard_connection", engine="codex-acp",
                       expected_revision="absent", explicit_opt_in=True)
    assert response["ok"]
    assert response["result"]["catalog_status"] == "catalog_failed"
    assert list(registry.read_registry(env).connections) == calls


def test_automatic_models_are_selectable_without_manual_catalog_refresh(env, monkeypatch):
    from cortex_runtime.connections import automatic
    from cortex_runtime.catalog import refresh, store as catalog_store
    from cortex_runtime.acp import config_options

    env["CORTEX_ENGINE_UPDATE"] = "on"
    installed(env, "codex-acp")
    login(env, "codex-acp")
    monkeypatch.setattr(engine_update, "update_engine", lambda *a, **kw: None)
    options = json.loads((Path(__file__).parent / "fixtures/catalog/codex_acp_config_options.json").read_text())["configOptions"]
    # Replace only engine transport; keep the real catalog builder, persistence,
    # discovery provenance and ACP selector boundary.
    monkeypatch.setattr(refresh, "_read_catalog_from_engine", lambda *a, **kw: options)
    monkeypatch.setattr(refresh, "_codex_effort_ceiling", lambda *a, **kw: None)
    automatic.run(environ=env)
    record = next(iter(registry.read_registry(env).connections.values()))
    snapshot = catalog_store.latest_connection_snapshot(record.id, env)
    assert snapshot is not None
    assert {e.route.model for e in snapshot.entries} == {"model-a", "model-b"}
    assert automatic.read_status("codex-acp", env) == "ready"
    selectable = config_options.selectable_entries_from_snapshots({record.id: record}, environ=env)
    assert {e.route.model for e in selectable} == {"model-a", "model-b"}
