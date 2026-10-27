"""Approved installed adapters follow owner connections without account changes."""
from contextlib import contextmanager
from dataclasses import replace
from pathlib import Path
import json

import pytest

from cortex_runtime.configuration import management
from cortex_runtime.connections import discovery, engine_update, registry, trust
from cortex_runtime.connections.model import AuthMode, ConnectionRecord, ConnectionState, SecretFile


def install(env, engine, version):
    package = discovery.owned_root(env) / engine / version / "node_modules" / discovery._PACKAGE_NAME[engine]
    (package / "dist").mkdir(parents=True)
    (package / "dist/index.js").write_text("// fixture")
    (package / "package.json").write_text(json.dumps({"version": version, "bin": "dist/index.js"}))
    return (env["CORTEX_NODE_BINARY"], str(package / "dist/index.js"))


def setup(tmp_path, engine, old):
    env = {"HOME": str(tmp_path), "XDG_CONFIG_HOME": str(tmp_path / "config"),
           "CORTEX_NODE_BINARY": "/usr/bin/node"}
    command = install(env, engine, old)
    record = ConnectionRecord(
        id="cn_0123456789ab", name="Keep", engine=engine, provider="custom",
        auth_mode=AuthMode.API_KEY, key_kind="openai_api_key" if engine == "codex-acp" else "anthropic_api_key",
        endpoint="https://example.invalid", launch_command=command,
        credential_ref=SecretFile(path=str(tmp_path / "private-key")),
        approved_fingerprint=trust.fingerprint(engine, "custom", "https://example.invalid", command),
        state=ConnectionState.CONFIGURED, qualified=True, created_at="before", updated_at="before",
        max_concurrent_sessions=2)
    with registry.transaction(env) as state:
        state.connections[record.id] = record
    return env, record


@pytest.mark.parametrize("engine,old,new", [
    ("codex-acp", "2.1.1", "2.3.0"), ("claude-agent-acp", "0.88.0", "0.97.0")])
def test_switch_preserves_fields_and_private_backup(tmp_path, capsys, engine, old, new):
    env, record = setup(tmp_path, engine, old)
    before = registry.registry_path(env).read_bytes()
    command = install(env, engine, new)
    assert management.follow_approved_adapters(environ=env) == [
        dict(engine=engine, old_version=old, new_version=new)]
    actual = registry.read_registry(env).connections[record.id]
    assert actual == replace(record, launch_command=command, approved_fingerprint=trust.fingerprint(
        engine, record.provider, record.endpoint, command))
    backup = registry.registry_path(env).with_name("connections.adapter-backup.json")
    assert backup.read_bytes() == before
    assert backup.stat().st_mode & 0o777 == 0o600
    assert capsys.readouterr().err == f"cortex: approved adapter {engine} {old} -> {new}\n"


@pytest.mark.parametrize("old,candidate", [("2.1.1", "99.0.0"), ("2.3.0", "2.3.0"),
                                          ("2.3.0", "2.1.1")])
def test_no_write_for_unqualified_same_or_older(tmp_path, monkeypatch, old, candidate):
    env, record = setup(tmp_path, "codex-acp", old)
    if candidate != old:
        install(env, "codex-acp", candidate)
    before = registry.registry_path(env).read_bytes()
    monkeypatch.setattr(registry, "atomic_write_private", lambda *a, **k: pytest.fail("unexpected write"))
    assert management.follow_approved_adapters(environ=env) == []
    assert registry.registry_path(env).read_bytes() == before


def test_revision_conflict_preserves_concurrent_edit(tmp_path, monkeypatch):
    env, record = setup(tmp_path, "codex-acp", "2.1.1")
    install(env, "codex-acp", "2.3.0")
    original = registry.transaction

    @contextmanager
    def concurrent(environ):
        with original(environ) as state:
            state.connections[record.id] = replace(record, name="Concurrent")
        with original(environ) as state:
            yield state

    monkeypatch.setattr(registry, "transaction", concurrent)
    with pytest.raises(management.RevisionConflict):
        management.follow_approved_adapters(environ=env)
    assert registry.read_registry(env).connections[record.id] == replace(record, name="Concurrent")
    assert not registry.registry_path(env).with_name("connections.adapter-backup.json").exists()


def test_missing_declared_bin_is_not_selected(tmp_path):
    env, record = setup(tmp_path, "codex-acp", "2.1.1")
    command = install(env, "codex-acp", "2.3.0")
    Path(command[1]).unlink()
    assert management.follow_approved_adapters(environ=env) == []
    assert registry.read_registry(env).connections[record.id] == record


def test_startup_follows_existing_connections(monkeypatch):
    import cortex_acp
    from cortex_runtime.connections import onboarding

    calls = []
    monkeypatch.setattr(discovery, "discover_all", lambda: {})
    monkeypatch.setattr(onboarding, "sync_discovered_connections", lambda found: calls.append("discover"))
    monkeypatch.setattr(management, "follow_approved_adapters", lambda: calls.append("follow"))
    monkeypatch.setattr(engine_update, "start_background_update", lambda: calls.append("update"))
    cortex_acp._ensure_first_run_configuration()
    assert calls == ["discover", "follow", "update"]


def test_opt_out(tmp_path):
    env, record = setup(tmp_path, "codex-acp", "2.1.1")
    install(env, "codex-acp", "2.3.0")
    env["CORTEX_ENGINE_FOLLOW"] = "off"
    assert management.follow_approved_adapters(environ=env) == []
    assert registry.read_registry(env).connections[record.id] == record


@pytest.mark.parametrize("follow_off", [False, True])
def test_existing_two_engine_registry_at_acp_startup(tmp_path, monkeypatch, follow_off):
    import cortex_acp

    env, codex = setup(tmp_path, "codex-acp", "2.1.1")
    old_claude = install(env, "claude-agent-acp", "0.88.0")
    claude = replace(codex, id="cn_abcdef123456", name="Claude account",
                     engine="claude-agent-acp", key_kind="anthropic_api_key",
                     launch_command=old_claude,
                     credential_ref=SecretFile(path=str(tmp_path / "claude-key")),
                     approved_fingerprint=trust.fingerprint("claude-agent-acp", codex.provider,
                                                            codex.endpoint, old_claude))
    with registry.transaction(env) as state:
        state.connections[claude.id] = claude
    new_codex = install(env, "codex-acp", "2.3.0")
    new_claude = install(env, "claude-agent-acp", "0.97.0")
    path = registry.registry_path(env)
    backup = path.with_name("connections.adapter-backup.json")
    before = path.read_bytes()
    # Neither version has an SDK override, so the installer uses plain version
    # directories rather than +sdk names for both Claude installations.
    assert "0.88.0" not in engine_update._CLAUDE_SDK_OVERRIDES
    assert "0.97.0" not in engine_update._CLAUDE_SDK_OVERRIDES
    for key in ("HOME", "XDG_CONFIG_HOME", "CORTEX_NODE_BINARY"):
        monkeypatch.setenv(key, env[key])
    monkeypatch.setenv("XDG_DATA_HOME", str(tmp_path / ".local" / "share"))
    monkeypatch.setenv("XDG_STATE_HOME", str(tmp_path / "state"))
    monkeypatch.setenv("CORTEX_ENGINE_UPDATE", "off")
    monkeypatch.setenv("CORTEX_ENGINE_FOLLOW", "off" if follow_off else "on")
    errors = []
    follow = management.follow_approved_adapters

    def capture_failure():
        try:
            return follow()
        except Exception as exc:
            errors.append(exc)
            raise

    monkeypatch.setattr(management, "follow_approved_adapters", capture_failure)
    cortex_acp._ensure_first_run_configuration()
    assert not errors, errors
    actual = registry.read_registry(env).connections
    if follow_off:
        assert actual == {codex.id: codex, claude.id: claude}
        assert path.read_bytes() == before
        assert not backup.exists()
    else:
        expected = {}
        for record, command in ((codex, new_codex), (claude, new_claude)):
            expected[record.id] = replace(record, launch_command=command,
                approved_fingerprint=trust.fingerprint(record.engine, record.provider, record.endpoint, command))
        assert actual == expected
        assert backup.read_bytes() == before
        assert backup.stat().st_mode & 0o777 == 0o600
        after = path.read_bytes()
        writes = []
        monkeypatch.setattr(registry, "atomic_write_private", lambda *a, **k: writes.append(a))
        cortex_acp._ensure_first_run_configuration()
        assert writes == []
        assert path.read_bytes() == after
        assert backup.read_bytes() == before


@pytest.mark.parametrize("latest,changed", [("2.3.0", True), ("99.0.0", False)])
def test_update_installs_then_selects_only_approved(tmp_path, monkeypatch, latest, changed):
    env, record = setup(tmp_path, "codex-acp", "2.1.1")
    monkeypatch.setattr(engine_update, "_npm", lambda env: "/usr/bin/npm")

    def npm(args, timeout, environ):
        if args[1] == "view":
            return json.dumps(latest)
        staging = Path(args[args.index("--prefix") + 1])
        package = staging / "node_modules" / discovery._PACKAGE_NAME[record.engine]
        (package / "dist").mkdir(parents=True)
        (package / "dist/index.js").write_text("// fixture")
        (package / "package.json").write_text(json.dumps({"version": latest, "bin": "dist/index.js"}))
        return ""

    monkeypatch.setattr(engine_update, "_run", npm)
    assert engine_update.update_engine(record.engine, environ=env) == latest
    actual = registry.read_registry(env).connections[record.id]
    assert (actual.launch_command != record.launch_command) is changed
