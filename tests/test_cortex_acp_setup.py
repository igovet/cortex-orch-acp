"""Tests for scripts/cortex_acp_setup.py (T031 first-run setup)."""
from __future__ import annotations

import json
import sys
from types import SimpleNamespace
from pathlib import Path

import pytest

SCRIPTS = Path(__file__).resolve().parents[1] / "scripts"
sys.path.insert(0, str(SCRIPTS))
sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "src"))

import cortex_acp_setup as setup  # noqa: E402
from cortex_runtime.connections.model import AuthMode  # noqa: E402


def _isolate(tmp_path, monkeypatch):
    config_home = tmp_path / "config"
    state_home = tmp_path / "state"
    home = tmp_path / "home"
    for path in (config_home, state_home, home):
        path.mkdir()
    monkeypatch.setenv("XDG_CONFIG_HOME", str(config_home))
    monkeypatch.setenv("XDG_STATE_HOME", str(state_home))
    monkeypatch.setenv("HOME", str(home))
    return config_home, state_home, home


def test_repository_paseo_source_has_valid_integrity_record():
    root, manifest, digest = setup._verify_paseo_plugin_source()

    assert root == setup.PASEO_PLUGIN_DIR.resolve()
    assert manifest["id"] == "cortex-acp"
    assert digest == setup._paseo_payload_sha256(root)


def test_paseo_integrity_ignores_dependency_and_generated_churn_but_covers_source(tmp_path):
    plugin = tmp_path / "paseo"
    (plugin / "client").mkdir(parents=True)
    (plugin / "package-lock.json").write_text("{}\n", encoding="utf-8")
    source = plugin / "client" / "provider.ts"
    source.write_text("export const value = 1;\n", encoding="utf-8")
    initial = setup._paseo_payload_sha256(plugin)

    for directory in ("node_modules", "build", "cache", "temp"):
        generated = plugin / directory
        generated.mkdir()
        (generated / "churn.txt").write_text("dependency or generated state\n", encoding="utf-8")
    assert setup._paseo_payload_sha256(plugin) == initial

    source.write_text("export const value = 2;\n", encoding="utf-8")
    assert setup._paseo_payload_sha256(plugin) != initial


def test_install_paseo_plugin_is_idempotent_and_preserves_config(tmp_path, monkeypatch):
    _config_home, _state_home, home = _isolate(tmp_path, monkeypatch)
    plugin = tmp_path / "paseo"
    plugin.mkdir()
    (plugin / "paseo-plugin.json").write_text(
        json.dumps({"id": "cortex-acp", "requirements": {"paseo": ">=0.9.0"}}), encoding="utf-8"
    )
    digest = setup._paseo_payload_sha256(plugin)
    (plugin / "plugin-integrity.json").write_text(json.dumps({"payload_sha256": digest}), encoding="utf-8")
    monkeypatch.setattr(setup, "PASEO_PLUGIN_DIR", plugin)
    daemon_home = tmp_path / "paseo-home"
    daemon_home.mkdir()
    config_path = daemon_home / "config.json"
    legacy_launcher = tmp_path / "legacy-cortex"
    legacy_launcher.write_text(
        f"#!/bin/sh\nexec /usr/bin/python3 -B {setup._ROOT / 'src' / 'cortex_acp.py'}\n",
        encoding="utf-8",
    )
    config_path.write_text(json.dumps({
        "pluginsEnabled": False,
        "keep": "value",
        "agents": {"providers": {
            "cortex": {"extends": "acp", "label": "Cortex", "command": [str(legacy_launcher)]},
            "other": {"extends": "acp", "label": "Other", "command": ["other-agent"]},
        }},
    }), encoding="utf-8")
    config_path.chmod(0o600)
    calls = []
    installed = {"value": False}

    def run(argv, **kwargs):
        calls.append(argv)
        if argv[1:4] == ["daemon", "status", "--json"]:
            return SimpleNamespace(returncode=0, stdout=json.dumps({"localDaemon": "running", "home": str(daemon_home)}), stderr="")
        if argv[1:4] == ["plugin", "ls", "--json"]:
            rows = ([{"id": "cortex-acp", "path": str(plugin), "status": "running"}] if installed["value"] else [])
            return SimpleNamespace(returncode=0, stdout=json.dumps(rows), stderr="")
        if argv[1:4] == ["plugin", "install", str(plugin)]:
            # Paseo loads the provider during install.  The durable launch
            # record must already exist so the compiled provider does not
            # evaluate its import.meta.url fallback on first load.
            launch = home / ".config" / "cortex" / "paseo-plugin-launch.json"
            assert launch.is_file()
            assert json.loads(launch.read_text(encoding="utf-8"))["argv"]
            installed["value"] = True
            return SimpleNamespace(returncode=0, stdout="", stderr="")
        if argv[1:3] == ["plugin", "reload"] or argv[1:2] == ["reload"]:
            return SimpleNamespace(returncode=0, stdout="", stderr="")
        if argv[1:4] == ["plugin", "reload", "cortex-acp"]:
            return SimpleNamespace(returncode=0, stdout="", stderr="")
        if argv[1:5] == ["plugin", "ls", "cortex-acp", "--json"]:
            return SimpleNamespace(returncode=0, stdout=json.dumps({"id": "cortex-acp", "path": str(plugin), "status": "running"}), stderr="")
        raise AssertionError(argv)

    first = setup.install_paseo_plugin(paseo_cli="paseo", runner=run)
    second = setup.install_paseo_plugin(paseo_cli="paseo", runner=run)
    assert ["paseo", "plugin", "reload", "cortex-acp"] in calls
    assert first["legacy_provider_migration"]["migrated"] is True
    assert second["legacy_provider_migration"] == {
        "migrated": False, "reason": "no_owned_legacy_provider"
    }
    assert {**first, "launch_config": {k: v for k, v in first["launch_config"].items() if k != "created"},
            "legacy_provider_migration": None} == {
        **second, "launch_config": {k: v for k, v in second["launch_config"].items() if k != "created"},
        "legacy_provider_migration": None,
    }
    launch = Path(home / ".config" / "cortex" / "paseo-plugin-launch.json")
    assert json.loads(launch.read_text(encoding="utf-8"))["argv"] == [
        "python3", "-B", str((setup._ROOT / "src" / "cortex_acp.py").resolve())
    ]
    assert json.loads(launch.read_text(encoding="utf-8"))["env"] == {
        "HOME": str(tmp_path / "home"),
        "XDG_CONFIG_HOME": str(tmp_path / "config"),
        "XDG_STATE_HOME": str(tmp_path / "state"),
    }
    assert launch.stat().st_mode & 0o777 == 0o600
    persisted = json.loads(config_path.read_text(encoding="utf-8"))
    assert persisted["pluginsEnabled"] is True
    assert persisted["keep"] == "value"
    assert "cortex" not in persisted["agents"]["providers"]
    assert "other" in persisted["agents"]["providers"]
    backups = list(daemon_home.glob("config.json.cortex-legacy-backup-*"))
    assert len(backups) == 1
    assert config_path.stat().st_mode & 0o777 == 0o600
    assert sum(call[1:4] == ["plugin", "install", str(plugin)] for call in calls) == 1


def test_legacy_generic_provider_migrates_only_when_owned_and_keeps_backup(tmp_path):
    config_path = tmp_path / "config.json"
    launcher = tmp_path / "cortex-acp"
    entrypoint = str((setup._ROOT / "src" / "cortex_acp.py").resolve())
    launcher.write_text(f"#!/bin/sh\nexec /usr/bin/python3 -B {entrypoint}\n", encoding="utf-8")
    config = {
        "pluginsEnabled": True,
        "agents": {"providers": {
            "cortex": {"extends": "acp", "label": "Cortex", "command": [str(launcher)]},
            "keep": {"extends": "acp", "label": "Keep", "command": ["keep-agent"]},
        }},
    }
    original = json.dumps(config, indent=2) + "\n"
    config_path.write_text(original, encoding="utf-8")
    config_path.chmod(0o600)

    result = setup._migrate_legacy_paseo_provider(config, config_path)

    assert result["migrated"] is True
    assert result["removed_provider"] == "cortex"
    backup = Path(result["backup"])
    assert backup.read_text(encoding="utf-8") == original
    assert backup.stat().st_mode & 0o777 == 0o600
    persisted = json.loads(config_path.read_text(encoding="utf-8"))
    assert "cortex" not in persisted["agents"]["providers"]
    assert "keep" in persisted["agents"]["providers"]


def test_legacy_generic_provider_migration_refuses_unrelated_launcher(tmp_path):
    config_path = tmp_path / "config.json"
    config = {"agents": {"providers": {
        "cortex": {"extends": "acp", "label": "Cortex", "command": ["/usr/bin/other-agent"]},
    }}}
    config_path.write_text(json.dumps(config), encoding="utf-8")

    result = setup._migrate_legacy_paseo_provider(config, config_path)

    assert result == {"migrated": False, "reason": "no_owned_legacy_provider"}
    assert json.loads(config_path.read_text(encoding="utf-8")) == config


def test_install_paseo_plugin_removes_new_launch_record_when_install_fails(tmp_path, monkeypatch):
    _config_home, _state_home, home = _isolate(tmp_path, monkeypatch)
    plugin = tmp_path / "paseo"
    plugin.mkdir()
    (plugin / "paseo-plugin.json").write_text(
        json.dumps({"id": "cortex-acp", "requirements": {"paseo": ">=0.9.0"}}), encoding="utf-8"
    )
    (plugin / "plugin-integrity.json").write_text(
        json.dumps({"payload_sha256": setup._paseo_payload_sha256(plugin)}), encoding="utf-8"
    )
    monkeypatch.setattr(setup, "PASEO_PLUGIN_DIR", plugin)
    daemon_home = tmp_path / "paseo-home"
    daemon_home.mkdir()
    config_path = daemon_home / "config.json"
    config_path.write_text(json.dumps({"pluginsEnabled": True}), encoding="utf-8")
    config_path.chmod(0o600)

    def run(argv, **kwargs):
        if argv[1:4] == ["daemon", "status", "--json"]:
            return SimpleNamespace(returncode=0, stdout=json.dumps({"localDaemon": "running", "home": str(daemon_home)}), stderr="")
        if argv[1:4] == ["plugin", "ls", "--json"]:
            return SimpleNamespace(returncode=0, stdout="[]", stderr="")
        if argv[1:4] == ["plugin", "install", str(plugin)]:
            return SimpleNamespace(returncode=1, stdout="", stderr="load failed")
        raise AssertionError(argv)

    with pytest.raises(SystemExit, match="Paseo command failed"):
        setup.install_paseo_plugin(paseo_cli="paseo", runner=run)
    launch_path = home / ".config" / "cortex" / "paseo-plugin-launch.json"
    assert not launch_path.exists()

    existing_launch = setup._write_paseo_launch_config()
    existing_body = Path(existing_launch["path"]).read_text(encoding="utf-8")
    with pytest.raises(SystemExit, match="Paseo command failed"):
        setup.install_paseo_plugin(paseo_cli="paseo", runner=run)
    assert Path(existing_launch["path"]).read_text(encoding="utf-8") == existing_body


def test_paseo_launch_config_upgrades_argv_only_record_for_daemon_restart(tmp_path, monkeypatch):
    config_home, state_home, home = _isolate(tmp_path, monkeypatch)
    launch_dir = home / ".config" / "cortex"
    launch_dir.mkdir(parents=True)
    launch = launch_dir / "paseo-plugin-launch.json"
    launch.write_text(json.dumps({
        "argv": ["python3", "-B", str((setup._ROOT / "src" / "cortex_acp.py").resolve())],
    }), encoding="utf-8")
    launch.chmod(0o600)

    # Use the real entrypoint expected by setup; this simulates an install
    # made before the daemon restart fix, with no shell-only wrapper present.
    monkeypatch.setattr(setup, "_ROOT", Path(__file__).resolve().parents[1])
    result = setup._write_paseo_launch_config()
    persisted = json.loads(launch.read_text(encoding="utf-8"))

    assert result["env"] == {
        "HOME": str(home),
        "XDG_CONFIG_HOME": str(config_home),
        "XDG_STATE_HOME": str(state_home),
    }
    assert persisted["env"]["HOME"] == str(home)


def test_paseo_launch_config_rejects_symlinked_private_ancestor(tmp_path, monkeypatch):
    _config_home, _state_home, home = _isolate(tmp_path, monkeypatch)
    real_config = tmp_path / "real-config"
    real_config.mkdir()
    (home / ".config").symlink_to(real_config, target_is_directory=True)

    with pytest.raises(SystemExit, match="symlinked or non-directory"):
        setup._write_paseo_launch_config()


def test_install_paseo_plugin_repairs_overly_readable_daemon_config(tmp_path, monkeypatch):
    _config_home, _state_home, home = _isolate(tmp_path, monkeypatch)
    plugin = tmp_path / "paseo"
    plugin.mkdir()
    (plugin / "paseo-plugin.json").write_text(
        json.dumps({"id": "cortex-acp", "requirements": {"paseo": ">=0.9.0"}}), encoding="utf-8"
    )
    (plugin / "plugin-integrity.json").write_text(
        json.dumps({"payload_sha256": setup._paseo_payload_sha256(plugin)}), encoding="utf-8"
    )
    monkeypatch.setattr(setup, "PASEO_PLUGIN_DIR", plugin)
    daemon_home = tmp_path / "paseo-home"
    daemon_home.mkdir()
    config_path = daemon_home / "config.json"
    config_path.write_text(json.dumps({"pluginsEnabled": True}), encoding="utf-8")
    config_path.chmod(0o644)

    def run(argv, **kwargs):
        if argv[1:4] == ["daemon", "status", "--json"]:
            return SimpleNamespace(returncode=0, stdout=json.dumps({"localDaemon": "running", "home": str(daemon_home)}), stderr="")
        if argv[1:4] == ["plugin", "ls", "--json"]:
            return SimpleNamespace(returncode=0, stdout=json.dumps([{"id": "cortex-acp", "path": str(plugin), "status": "running"}]), stderr="")
        if argv[1:4] == ["plugin", "reload", "cortex-acp"]:
            return SimpleNamespace(returncode=0, stdout="", stderr="")
        if argv[1:5] == ["plugin", "ls", "cortex-acp", "--json"]:
            return SimpleNamespace(returncode=0, stdout=json.dumps({"id": "cortex-acp", "path": str(plugin), "status": "running"}), stderr="")
        raise AssertionError(argv)

    setup.install_paseo_plugin(paseo_cli="paseo", runner=run)
    assert config_path.stat().st_mode & 0o777 == 0o600
    assert json.loads(config_path.read_text(encoding="utf-8")) == {"pluginsEnabled": True}
    assert (home / ".config" / "cortex" / "paseo-plugin-launch.json").stat().st_mode & 0o777 == 0o600


def test_init_storage_is_idempotent(tmp_path, monkeypatch):
    config_home, state_home, _home = _isolate(tmp_path, monkeypatch)
    first = setup.init_storage()
    assert Path(first["config_root"]).is_dir()
    assert Path(first["state_root"]).is_dir()
    second = setup.init_storage()
    assert second == first


def test_init_storage_never_creates_project_cortex_dir(tmp_path, monkeypatch):
    _isolate(tmp_path, monkeypatch)
    setup.init_storage()
    assert not (tmp_path / ".cortex").exists()


def test_add_connection_api_key_never_leaks_secret_to_return_value(tmp_path, monkeypatch):
    _isolate(tmp_path, monkeypatch)
    secret_file = tmp_path / "secret.txt"
    secret_file.write_bytes(b"fake-super-secret-key")
    result = setup.add_connection(
        name="work", engine="codex-acp", provider="openai", auth_mode=AuthMode.API_KEY,
        key_kind="openai_api_key", api_key_reader=lambda: secret_file.read_bytes(),
    )
    assert "fake-super-secret-key" not in str(result)
    assert result["state"] == "configured"
    assert result["connection_id"].startswith("cn_")


def test_add_connection_reuse_is_idempotent(tmp_path, monkeypatch):
    _isolate(tmp_path, monkeypatch)
    secret_file = tmp_path / "secret.txt"
    secret_file.write_bytes(b"fake-key")
    first = setup.add_connection(
        name="work", engine="codex-acp", provider="openai", auth_mode=AuthMode.API_KEY,
        key_kind="openai_api_key", api_key_reader=lambda: secret_file.read_bytes(),
    )
    second = setup.add_connection(
        name="work", engine="codex-acp", provider="openai", auth_mode=AuthMode.API_KEY,
        key_kind="openai_api_key", api_key_reader=lambda: secret_file.read_bytes(),
    )
    assert first["connection_id"] == second["connection_id"]


def test_add_connection_api_key_requires_reader(tmp_path, monkeypatch):
    _isolate(tmp_path, monkeypatch)
    with pytest.raises(SystemExit):
        setup.add_connection(name="work", engine="codex-acp", provider="openai", auth_mode=AuthMode.API_KEY,
                              key_kind="openai_api_key", api_key_reader=None)


def test_add_connection_native_account_prints_no_secret_instructions(tmp_path, monkeypatch):
    _isolate(tmp_path, monkeypatch)
    result = setup.add_connection(name="personal", engine="codex-acp", provider="openai",
                                   auth_mode=AuthMode.NATIVE_ACCOUNT)
    assert result["login_instructions"] is not None
    assert "CODEX_HOME=" in result["login_instructions"]


def test_validate_connection_cli_returns_only_safe_state(tmp_path, monkeypatch, capsys):
    _isolate(tmp_path, monkeypatch)
    conn = setup.add_connection(
        name="personal", engine="codex-acp", provider="openai", auth_mode=AuthMode.NATIVE_ACCOUNT,
        launch_command=("/owned/node", "/owned/codex-acp.js"),
    )
    assert setup.main(["validate-connection", "--connection", conn["connection_id"]]) == 0
    output = json.loads(capsys.readouterr().out)
    assert output == {
        "connection_id": conn["connection_id"],
        "engine": "codex-acp",
        "state": "authorization_required",
        "qualified": True,
    }
    assert "credential" not in json.dumps(output).lower()


def test_validate_connection_cli_unknown_id_fails_closed(tmp_path, monkeypatch):
    _isolate(tmp_path, monkeypatch)
    with pytest.raises(SystemExit, match="connection_unavailable"):
        setup.main(["validate-connection", "--connection", "cn_missing"])


def test_add_connection_cli_requires_verified_exact_install_record(tmp_path, monkeypatch):
    _isolate(tmp_path, monkeypatch)
    record = {
        "node_binary": "/owned/node",
        "adapter_entry": "/owned/adapter.js",
        "package_json": "/owned/package.json",
        "package_version": "0.78.0",
        "package_json_sha256": "0" * 64,
        "adapter_entry_sha256": "1" * 64,
    }
    record_path = tmp_path / "install.json"
    record_path.write_text(json.dumps(record), encoding="utf-8")

    captured = {}

    def fake_add_connection(**kwargs):
        captured.update(kwargs)
        return {"connection_id": "cn_test"}

    monkeypatch.setattr(setup, "add_connection", fake_add_connection)
    from cortex_runtime.engine import claude_install

    monkeypatch.setattr(claude_install, "verify_install", lambda install, environ=None: install)
    monkeypatch.setattr(claude_install, "launch_command_for", lambda install: (install.node_binary, install.adapter_entry))
    assert setup.main([
        "add-connection", "--name", "work", "--engine", "claude-agent-acp",
        "--provider", "anthropic", "--auth-mode", "native_account",
        "--install-record", str(record_path),
    ]) == 0
    assert captured["launch_command"] == ("/owned/node", "/owned/adapter.js")


def test_add_connection_cli_rejects_unknown_install_record_fields_before_mutation(tmp_path, monkeypatch):
    _isolate(tmp_path, monkeypatch)
    record_path = tmp_path / "install.json"
    record_path.write_text(json.dumps({"unexpected": True}), encoding="utf-8")
    with pytest.raises(SystemExit, match="unknown fields: unexpected"):
        setup.main([
            "add-connection", "--name", "work", "--engine", "claude-agent-acp",
            "--provider", "anthropic", "--auth-mode", "native_account",
            "--install-record", str(record_path),
        ])
    assert setup.status()["connections"] == []


def test_pin_route_persists_user_default_route(tmp_path, monkeypatch):
    _isolate(tmp_path, monkeypatch)
    secret_file = tmp_path / "secret.txt"
    secret_file.write_bytes(b"fake-key")
    conn = setup.add_connection(
        name="work", engine="codex-acp", provider="openai", auth_mode=AuthMode.API_KEY,
        key_kind="openai_api_key", api_key_reader=lambda: secret_file.read_bytes(),
    )
    result = setup.pin_route(connection_id=conn["connection_id"], model="gpt-5-codex")
    assert result["route_id"].startswith("route_") or result["route_id"]
    report = setup.status()
    assert report["pinned_route"] == result["route_id"]


def test_pin_route_rerun_same_model_is_idempotent(tmp_path, monkeypatch):
    _isolate(tmp_path, monkeypatch)
    secret_file = tmp_path / "secret.txt"
    secret_file.write_bytes(b"fake-key")
    conn = setup.add_connection(
        name="work", engine="codex-acp", provider="openai", auth_mode=AuthMode.API_KEY,
        key_kind="openai_api_key", api_key_reader=lambda: secret_file.read_bytes(),
    )
    first = setup.pin_route(connection_id=conn["connection_id"], model="gpt-5-codex")
    second = setup.pin_route(connection_id=conn["connection_id"], model="gpt-5-codex")
    assert first["route_id"] == second["route_id"]


def test_pin_route_unknown_connection_fails_closed(tmp_path, monkeypatch):
    _isolate(tmp_path, monkeypatch)
    with pytest.raises(SystemExit):
        setup.pin_route(connection_id="cn_doesnotexist", model="gpt-5-codex")


def test_pin_route_rejects_unqualified_gateway_connection(tmp_path, monkeypatch):
    _isolate(tmp_path, monkeypatch)
    conn = setup.add_connection(name="via-gateway", engine="codex-acp", provider="proxy",
                                 auth_mode=AuthMode.GATEWAY)
    with pytest.raises(SystemExit):
        setup.pin_route(connection_id=conn["connection_id"], model="gpt-5-codex")


_OBSERVED_CONFIG_OPTIONS = [
    dict(id="model", name="Model", category="model", type="select", currentValue="gpt-5-codex",
         options=[dict(value="gpt-5-codex", name="GPT-5 Codex")]),
    dict(id="reasoning_effort", name="Reasoning effort", category="thought_level", type="select",
         currentValue="medium",
         options=[dict(value="low", name="Low"), dict(value="medium", name="Medium"), dict(value="high", name="High")]),
]


def _write_observed_config_options(tmp_path, options=None):
    path = tmp_path / "observed-config-options.json"
    path.write_text(json.dumps(options if options is not None else _OBSERVED_CONFIG_OPTIONS), encoding="utf-8")
    return path


def test_record_observation_persists_real_supported_parameters(tmp_path, monkeypatch):
    """The core gap this closes: ``pin-route`` alone can never persist a
    real ``thought_level``/``reasoning_effort`` choice (its
    ``ManualDeclaration`` is only ever accepted with no parameters -- see
    ``catalog.discovery.build_catalog``'s "unsupported_manual_parameters"
    guard). ``record-observation`` goes through a real ``Observation``
    instead, so the saved route's ``supported_parameters`` are the
    engine's actual advertised effort values."""
    _isolate(tmp_path, monkeypatch)
    secret_file = tmp_path / "secret.txt"
    secret_file.write_bytes(b"fake-key")
    conn = setup.add_connection(
        name="work", engine="codex-acp", provider="openai", auth_mode=AuthMode.API_KEY,
        key_kind="openai_api_key", api_key_reader=lambda: secret_file.read_bytes(),
    )
    pinned = setup.pin_route(connection_id=conn["connection_id"], model="gpt-5-codex")
    assert pinned["route_id"]

    options_file = _write_observed_config_options(tmp_path)
    result = setup.record_observation(
        connection_id=conn["connection_id"], model="gpt-5-codex", config_options_file=options_file,
    )
    assert ["thought_level", "low"] in result["supported_parameters"]
    assert ["thought_level", "medium"] in result["supported_parameters"]
    assert ["thought_level", "high"] in result["supported_parameters"]
    # A real observation produces a different (richer) route id than the
    # empty-parameters ManualDeclaration pin-route left behind, and it is
    # now the pinned default.
    assert result["route_id"] != pinned["route_id"]
    report = setup.status()
    assert report["pinned_route"] == result["route_id"]


def test_record_observation_rerun_same_inputs_is_idempotent(tmp_path, monkeypatch):
    _isolate(tmp_path, monkeypatch)
    secret_file = tmp_path / "secret.txt"
    secret_file.write_bytes(b"fake-key")
    conn = setup.add_connection(
        name="work", engine="codex-acp", provider="openai", auth_mode=AuthMode.API_KEY,
        key_kind="openai_api_key", api_key_reader=lambda: secret_file.read_bytes(),
    )
    options_file = _write_observed_config_options(tmp_path)
    first = setup.record_observation(connection_id=conn["connection_id"], model="gpt-5-codex", config_options_file=options_file)
    second = setup.record_observation(connection_id=conn["connection_id"], model="gpt-5-codex", config_options_file=options_file)
    assert first["route_id"] == second["route_id"]


def test_record_observation_unknown_connection_fails_closed(tmp_path, monkeypatch):
    _isolate(tmp_path, monkeypatch)
    options_file = _write_observed_config_options(tmp_path)
    with pytest.raises(SystemExit):
        setup.record_observation(connection_id="cn_doesnotexist", model="gpt-5-codex", config_options_file=options_file)


def test_record_observation_rejects_model_the_engine_never_advertised(tmp_path, monkeypatch):
    _isolate(tmp_path, monkeypatch)
    secret_file = tmp_path / "secret.txt"
    secret_file.write_bytes(b"fake-key")
    conn = setup.add_connection(
        name="work", engine="codex-acp", provider="openai", auth_mode=AuthMode.API_KEY,
        key_kind="openai_api_key", api_key_reader=lambda: secret_file.read_bytes(),
    )
    options_file = _write_observed_config_options(tmp_path)
    with pytest.raises(SystemExit):
        setup.record_observation(connection_id=conn["connection_id"], model="a-model-never-advertised", config_options_file=options_file)


def test_record_observation_rejects_malformed_config_options(tmp_path, monkeypatch):
    _isolate(tmp_path, monkeypatch)
    secret_file = tmp_path / "secret.txt"
    secret_file.write_bytes(b"fake-key")
    conn = setup.add_connection(
        name="work", engine="codex-acp", provider="openai", auth_mode=AuthMode.API_KEY,
        key_kind="openai_api_key", api_key_reader=lambda: secret_file.read_bytes(),
    )
    options_file = _write_observed_config_options(tmp_path, options=[dict(id="model")])  # missing required fields
    with pytest.raises(SystemExit):
        setup.record_observation(connection_id=conn["connection_id"], model="gpt-5-codex", config_options_file=options_file)


def test_record_observation_cli_round_trip(tmp_path, monkeypatch, capsys):
    _isolate(tmp_path, monkeypatch)
    secret_file = tmp_path / "secret.txt"
    secret_file.write_bytes(b"fake-key")
    conn = setup.add_connection(
        name="work", engine="codex-acp", provider="openai", auth_mode=AuthMode.API_KEY,
        key_kind="openai_api_key", api_key_reader=lambda: secret_file.read_bytes(),
    )
    options_file = _write_observed_config_options(tmp_path)
    exit_code = setup.main([
        "record-observation", "--connection", conn["connection_id"], "--model", "gpt-5-codex",
        "--config-options-file", str(options_file),
    ])
    assert exit_code == 0
    printed = json.loads(capsys.readouterr().out)
    assert ["thought_level", "high"] in printed["supported_parameters"]


def test_status_on_untouched_environment_reports_no_connections(tmp_path, monkeypatch):
    _isolate(tmp_path, monkeypatch)
    report = setup.status()
    assert report["connections"] == []
    assert report["pinned_route"] is None


def test_print_client_recipes_never_writes_without_apply(tmp_path, monkeypatch, capsys):
    _isolate(tmp_path, monkeypatch)
    settings = tmp_path / "zed-settings.json"
    settings.write_text("{}\n", encoding="utf-8")
    exit_code = setup.main(["print-client-recipes", "--settings", str(settings)])
    assert exit_code == 0
    assert settings.read_text(encoding="utf-8") == "{}\n"
    out = capsys.readouterr().out
    assert "agent_servers" in out
    assert "codeg" in out.lower()


def test_cli_status_produces_json(tmp_path, monkeypatch, capsys):
    _isolate(tmp_path, monkeypatch)
    exit_code = setup.main(["status"])
    assert exit_code == 0
    out = capsys.readouterr().out
    import json

    data = json.loads(out)
    assert "connections" in data


def test_paseo_update_rejects_old_mcp_grants_before_mutating_daemon(tmp_path, monkeypatch):
    config, _state, home = _isolate(tmp_path, monkeypatch)
    directory = config / 'cortex'
    directory.mkdir(mode=0o700)
    path = directory / 'mcp-servers.json'
    path.write_text(json.dumps({'version':1,'servers':{'codebase-memory-mcp':{
        'command':'/bin/true','env':{'TOKEN':'never-print-this'}}}}))
    path.chmod(0o600)
    original=path.read_bytes()
    def forbidden(*args,**kwargs):
        pytest.fail('Paseo must not be called before registry validation')
    with pytest.raises(SystemExit,match='MCP configuration preflight failed') as caught:
        setup.install_paseo_plugin(paseo_cli='paseo',runner=forbidden)
    assert 'codebase-memory-mcp.allowed_tools' in str(caught.value)
    assert 'never-print-this' not in str(caught.value)
    assert path.read_bytes()==original
    assert not (home/'.config/cortex/paseo-plugin-launch.json').exists()
