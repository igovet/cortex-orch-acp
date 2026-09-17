"""Tests for scripts/cortex_acp_setup.py (T031 first-run setup)."""
from __future__ import annotations

import sys
from pathlib import Path

import pytest

SCRIPTS = Path(__file__).resolve().parents[1] / "scripts"
sys.path.insert(0, str(SCRIPTS))
sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "plugins" / "cortex" / "scripts"))

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


def test_add_connection_api_key_requires_reader():
    with pytest.raises(SystemExit):
        setup.add_connection(name="work", engine="codex-acp", provider="openai", auth_mode=AuthMode.API_KEY,
                              key_kind="openai_api_key", api_key_reader=None)


def test_add_connection_native_account_prints_no_secret_instructions(tmp_path, monkeypatch):
    _isolate(tmp_path, monkeypatch)
    result = setup.add_connection(name="personal", engine="codex-acp", provider="openai",
                                   auth_mode=AuthMode.NATIVE_ACCOUNT)
    assert result["login_instructions"] is not None
    assert "CODEX_HOME=" in result["login_instructions"]


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
