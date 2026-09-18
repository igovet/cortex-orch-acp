"""Tests for scripts/cortex_uninstall.py (T031 reversible uninstall)."""
from __future__ import annotations

import subprocess
import sys
from pathlib import Path

import pytest

SCRIPTS = Path(__file__).resolve().parents[1] / "scripts"
sys.path.insert(0, str(SCRIPTS))
sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "plugins" / "cortex" / "scripts"))

import cortex_uninstall as uninstall  # noqa: E402
import cortex_acp_setup as setup  # noqa: E402
from cortex_runtime.connections.model import AuthMode  # noqa: E402
from cortex_runtime.store import Store  # noqa: E402


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


def _seed_cortex_state(tmp_path):
    secret_file = tmp_path / "secret.txt"
    secret_file.write_bytes(b"fake-key")
    conn = setup.add_connection(name="work", engine="codex-acp", provider="openai",
                                 auth_mode=AuthMode.API_KEY, key_kind="openai_api_key",
                                 api_key_reader=lambda: secret_file.read_bytes())
    setup.pin_route(connection_id=conn["connection_id"], model="gpt-5-codex")
    return conn


def test_default_is_dry_run_and_removes_nothing(tmp_path, monkeypatch):
    config_home, state_home, _home = _isolate(tmp_path, monkeypatch)
    _seed_cortex_state(tmp_path)
    report = uninstall.plan()
    assert config_home.joinpath("cortex").exists()
    assert any(e["scope"] == "user_config" and e["status"] == "owner_private" for e in report)
    exit_code = uninstall.main([])
    assert exit_code == 0
    assert config_home.joinpath("cortex", "connections.json").exists()


def test_remove_user_scope_requires_confirmation_flag(tmp_path, monkeypatch, capsys):
    _isolate(tmp_path, monkeypatch)
    _seed_cortex_state(tmp_path)
    exit_code = uninstall.main(["--remove-user-scope"])
    assert exit_code == 2
    config_root = setup.registry.config_root()
    assert (config_root / "connections.json").exists()


def test_remove_user_scope_deletes_only_cortex_config_and_state(tmp_path, monkeypatch):
    config_home, state_home, home = _isolate(tmp_path, monkeypatch)
    _seed_cortex_state(tmp_path)
    # Planted unrelated user file directly alongside Cortex's own config dir.
    unrelated = config_home / "some-other-app" / "settings.json"
    unrelated.parent.mkdir(parents=True)
    unrelated.write_text("{}", encoding="utf-8")
    exit_code = uninstall.main(["--remove-user-scope", "--i-confirm-removal"])
    assert exit_code == 0
    assert not (config_home / "cortex").exists()
    assert not (state_home / "cortex").exists()
    assert unrelated.exists()


def test_remove_user_scope_survives_engine_installs_credentials_and_history(tmp_path, monkeypatch):
    """DEP-06: confirmed user-scope removal deletes only $XDG_CONFIG_HOME/cortex
    and $XDG_STATE_HOME/cortex -- a real engine's own install/credential
    directories (outside Cortex's XDG scope -- see
    ``engine.codex_install._UNTRUSTED_HOME_RELATIVE_ROOTS`` and
    ``engine.claude_install``'s equivalent) and unrelated user history must
    survive untouched."""
    config_home, state_home, home = _isolate(tmp_path, monkeypatch)
    _seed_cortex_state(tmp_path)

    # Real engine install/credential locations (outside Cortex's own XDG
    # config/state roots).
    codeg_config = home / ".config" / "app.codeg" / "auth.json"
    codeg_config.parent.mkdir(parents=True)
    codeg_config.write_text('{"api_key": "fake-not-real"}', encoding="utf-8")
    zed_external_agent = home / ".local" / "share" / "zed" / "external_agents" / "codex-acp" / "binary"
    zed_external_agent.parent.mkdir(parents=True)
    zed_external_agent.write_bytes(b"not a real engine binary")
    # Unrelated user history file.
    bash_history = home / ".bash_history"
    bash_history.write_text("ls -la\ncd project\n", encoding="utf-8")

    exit_code = uninstall.main(["--remove-user-scope", "--i-confirm-removal"])
    assert exit_code == 0
    assert not (config_home / "cortex").exists()
    assert not (state_home / "cortex").exists()
    assert codeg_config.read_text() == '{"api_key": "fake-not-real"}'
    assert zed_external_agent.read_bytes() == b"not a real engine binary"
    assert bash_history.read_text() == "ls -la\ncd project\n"


def test_remove_project_store_never_touches_planted_user_files_or_git_branch(tmp_path, monkeypatch):
    _isolate(tmp_path, monkeypatch)
    project = tmp_path / "myproject"
    project.mkdir()
    subprocess.run(["git", "init", "-q"], cwd=project, check=True)
    (project / "README.md").write_text("hello\n", encoding="utf-8")
    subprocess.run(["git", "add", "README.md"], cwd=project, check=True)
    subprocess.run(["git", "-c", "user.email=a@b.c", "-c", "user.name=t", "commit", "-q", "-m", "init"],
                    cwd=project, check=True)
    subprocess.run(["git", "checkout", "-q", "-b", "feature/keep-me"], cwd=project, check=True)
    (project / "working-change.txt").write_text("wip\n", encoding="utf-8")
    # Planted fake "engine install" directory that must never be touched.
    fake_engine = project / "vendor" / "fake-engine-install"
    fake_engine.mkdir(parents=True)
    (fake_engine / "binary").write_bytes(b"not a real engine")

    store = Store(project / ".cortex", project_root=project)
    assert (project / ".cortex" / "cortex.sqlite3").exists()

    exit_code = uninstall.main(["--project", str(project), "--remove-project-store", "--i-confirm-removal"])
    assert exit_code == 0
    assert not (project / ".cortex").exists()
    assert (project / "README.md").exists()
    assert (project / "working-change.txt").exists()
    assert (fake_engine / "binary").read_bytes() == b"not a real engine"
    branch = subprocess.run(["git", "branch", "--show-current"], cwd=project, capture_output=True, text=True, check=True)
    assert branch.stdout.strip() == "feature/keep-me"


def test_remove_project_store_requires_explicit_project_flag(tmp_path, monkeypatch):
    _isolate(tmp_path, monkeypatch)
    exit_code = uninstall.main(["--remove-project-store", "--i-confirm-removal"])
    assert exit_code == 2


def test_remove_user_scope_on_fresh_environment_is_a_no_op(tmp_path, monkeypatch):
    _isolate(tmp_path, monkeypatch)
    exit_code = uninstall.main(["--remove-user-scope", "--i-confirm-removal"])
    assert exit_code == 0


def test_refuses_to_remove_a_symlinked_config_root(tmp_path, monkeypatch):
    config_home, _state_home, _home = _isolate(tmp_path, monkeypatch)
    _seed_cortex_state(tmp_path)
    real = config_home / "cortex"
    decoy_target = tmp_path / "decoy"
    decoy_target.mkdir()
    import shutil

    shutil.rmtree(real)
    real.symlink_to(decoy_target)
    with pytest.raises(SystemExit):
        uninstall.remove_user_scope()
    assert decoy_target.exists()
