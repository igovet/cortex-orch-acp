"""Registry storage, locking, and safety-check tests (fake secrets only).

Every test sets XDG_CONFIG_HOME/XDG_STATE_HOME/HOME to a tmp_path so no real
user configuration under ~/.config/cortex or ~/.local/state/cortex is ever
touched.
"""
from __future__ import annotations

import json
import os
import stat

import pytest

from cortex_runtime.connections import onboarding, registry
from cortex_runtime.connections.model import AuthMode
from cortex_runtime.portable.errors import PortableError


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


def test_fresh_empty_registry_lists_no_connections(tmp_path, monkeypatch):
    _isolate(tmp_path, monkeypatch)
    state = registry.read_registry()
    assert state.connections == {}
    assert state.override_history == {}


def test_registry_root_and_secrets_dirs_are_created_owner_only(tmp_path, monkeypatch):
    _isolate(tmp_path, monkeypatch)
    registry.ensure_private_dir(registry.secrets_dir(), root=registry.config_root())
    info = registry.config_root().lstat()
    assert stat.S_IMODE(info.st_mode) == 0o700
    info = registry.secrets_dir().lstat()
    assert stat.S_IMODE(info.st_mode) == 0o700


def test_registry_file_is_written_0600(tmp_path, monkeypatch):
    _isolate(tmp_path, monkeypatch)
    onboarding.create_connection(
        name="work",
        engine="codex-acp",
        provider="openai",
        auth_mode=AuthMode.NATIVE_ACCOUNT,
    )
    info = registry.registry_path().lstat()
    assert stat.S_IMODE(info.st_mode) == 0o600


def test_registry_rejects_unknown_top_level_key(tmp_path, monkeypatch):
    _isolate(tmp_path, monkeypatch)
    registry.ensure_private_dir(registry.config_root(), root=registry.config_root())
    path = registry.registry_path()
    path.write_bytes(json.dumps({"schema_version": 1, "connections": {}, "unexpected": True}).encode())
    os.chmod(path, 0o600)
    with pytest.raises(PortableError) as excinfo:
        registry.read_registry()
    assert excinfo.value.code == "invalid_registry"


def test_registry_rejects_wrong_schema_version(tmp_path, monkeypatch):
    _isolate(tmp_path, monkeypatch)
    registry.ensure_private_dir(registry.config_root(), root=registry.config_root())
    path = registry.registry_path()
    path.write_bytes(json.dumps({"schema_version": 2, "connections": {}}).encode())
    os.chmod(path, 0o600)
    with pytest.raises(PortableError) as excinfo:
        registry.read_registry()
    assert excinfo.value.code == "invalid_registry"


def test_registry_rejects_symlinked_file(tmp_path, monkeypatch):
    _isolate(tmp_path, monkeypatch)
    registry.ensure_private_dir(registry.config_root(), root=registry.config_root())
    target = tmp_path / "elsewhere.json"
    target.write_text("{}")
    registry.registry_path().symlink_to(target)
    with pytest.raises(PortableError) as excinfo:
        registry.read_registry()
    assert excinfo.value.code == "unsafe_storage"


def test_registry_rejects_group_or_world_readable_file(tmp_path, monkeypatch):
    _isolate(tmp_path, monkeypatch)
    registry.ensure_private_dir(registry.config_root(), root=registry.config_root())
    path = registry.registry_path()
    path.write_bytes(json.dumps({"schema_version": 1, "connections": {}}).encode())
    os.chmod(path, 0o644)
    with pytest.raises(PortableError) as excinfo:
        registry.read_registry()
    assert excinfo.value.code == "unsafe_storage"


def test_registry_rejects_secret_file_that_is_symlinked(tmp_path, monkeypatch):
    _isolate(tmp_path, monkeypatch)
    record = onboarding.create_connection(
        name="work",
        engine="claude-agent-acp",
        provider="anthropic",
        auth_mode=AuthMode.API_KEY,
        key_kind="anthropic_api_key",
    )
    secret_path = registry.secret_path(record.id)
    outside = tmp_path / "outside-secret.txt"
    outside.write_text("fake-secret-value")
    secret_path.symlink_to(outside)
    with pytest.raises(PortableError) as excinfo:
        registry.read_owner_file(secret_path)
    assert excinfo.value.code == "unsafe_storage"


def test_registry_storage_paths_never_contain_a_cortex_component(tmp_path, monkeypatch):
    config_home = tmp_path / "project" / ".cortex" / "config"
    config_home.mkdir(parents=True)
    monkeypatch.setenv("XDG_CONFIG_HOME", str(config_home))
    monkeypatch.setenv("XDG_STATE_HOME", str(tmp_path / "state"))
    monkeypatch.setenv("HOME", str(tmp_path / "home"))
    with pytest.raises(PortableError) as excinfo:
        registry.config_root()
    assert excinfo.value.code == "connection_storage_under_project"


def test_registry_storage_rejects_a_root_nested_under_a_git_workspace_without_literal_cortex_component(tmp_path, monkeypatch):
    project = tmp_path / "myrepo"
    (project / ".git").mkdir(parents=True)
    config_home = project / "some_other_subdir" / "config"
    config_home.mkdir(parents=True)
    monkeypatch.setenv("XDG_CONFIG_HOME", str(config_home))
    monkeypatch.setenv("XDG_STATE_HOME", str(tmp_path / "state"))
    monkeypatch.setenv("HOME", str(tmp_path / "home"))
    with pytest.raises(PortableError) as excinfo:
        registry.config_root()
    assert excinfo.value.code == "connection_storage_under_project"


def test_ensure_private_dir_rejects_a_path_outside_the_declared_root(tmp_path, monkeypatch):
    _isolate(tmp_path, monkeypatch)
    outside = tmp_path / "elsewhere"
    with pytest.raises(PortableError) as excinfo:
        registry.ensure_private_dir(outside, root=registry.config_root())
    assert excinfo.value.code == "unsafe_storage"


def test_two_connections_same_engine_have_distinct_ids_and_state_dirs(tmp_path, monkeypatch):
    _isolate(tmp_path, monkeypatch)
    first = onboarding.create_connection(
        name="work",
        engine="codex-acp",
        provider="openai",
        auth_mode=AuthMode.NATIVE_ACCOUNT,
    )
    second = onboarding.create_connection(
        name="personal",
        engine="codex-acp",
        provider="openai-personal",
        auth_mode=AuthMode.NATIVE_ACCOUNT,
    )
    assert first.id != second.id
    first_dir = registry.connection_state_dir(first.id, first.engine)
    second_dir = registry.connection_state_dir(second.id, second.engine)
    assert first_dir != second_dir
    assert not str(first_dir).startswith(str(second_dir))
    assert not str(second_dir).startswith(str(first_dir))


def test_duplicate_credential_paths_detects_a_hand_tampered_collision(tmp_path, monkeypatch):
    _isolate(tmp_path, monkeypatch)
    first = onboarding.create_connection(
        name="work", engine="codex-acp", provider="openai-a", auth_mode=AuthMode.API_KEY, key_kind="openai_api_key",
    )
    second = onboarding.create_connection(
        name="other", engine="codex-acp", provider="openai-b", auth_mode=AuthMode.API_KEY, key_kind="openai_api_key",
    )
    with registry.transaction() as state:
        existing = state.connections[second.id]
        from cortex_runtime.connections.model import SecretFile
        tampered = SecretFile(path=str(registry.secret_path(first.id)))
        state.connections[second.id] = existing.__class__(
            id=existing.id, name=existing.name, engine=existing.engine, provider=existing.provider,
            auth_mode=existing.auth_mode, key_kind=existing.key_kind, endpoint=existing.endpoint,
            launch_command=existing.launch_command, credential_ref=tampered,
            approved_fingerprint=existing.approved_fingerprint, state=existing.state,
            qualified=existing.qualified, created_at=existing.created_at, updated_at=existing.updated_at,
        )
    tampered_state = registry.read_registry()
    collisions = registry.duplicate_credential_paths(tampered_state.connections)
    assert first.id in collisions
    assert second.id in collisions
