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


# --- explicit-``environ`` isolation (regression: onboarding.create_connection
#     previously derived credential_ref via canonical_credential_ref calls
#     that silently dropped the caller's own ``environ`` and fell back to
#     the real process os.environ -- see onboarding._derive_credential_ref)
# -----------------------------------------------------------------------

def test_create_connection_honors_a_custom_environ_even_when_process_environ_differs(tmp_path, monkeypatch):
    """A distinct, unrelated real-process HOME/XDG (set on the actual
    ``os.environ`` via monkeypatch) must never be used: every filesystem
    path this connection's credential_ref derives to must live under the
    *caller-supplied* ``environ`` dict, not the process environment."""
    real_home = tmp_path / "real-process-home"
    real_config = real_home / "config"
    real_state = real_home / "state"
    for path in (real_home, real_config, real_state):
        path.mkdir(parents=True)
    monkeypatch.setenv("HOME", str(real_home))
    monkeypatch.setenv("XDG_CONFIG_HOME", str(real_config))
    monkeypatch.setenv("XDG_STATE_HOME", str(real_state))

    caller_home = tmp_path / "caller-supplied-home"
    caller_config = caller_home / "config"
    caller_state = caller_home / "state"
    for path in (caller_home, caller_config, caller_state):
        path.mkdir(parents=True)
    caller_environ = dict(HOME=str(caller_home), XDG_CONFIG_HOME=str(caller_config), XDG_STATE_HOME=str(caller_state))

    record = onboarding.create_connection(
        name="work", engine="codex-acp", provider="openai", auth_mode=AuthMode.API_KEY,
        key_kind="openai_api_key", environ=caller_environ)
    record = onboarding.set_api_key(record.id, reader=lambda: b"fake-secret-key", environ=caller_environ)

    assert str(caller_config) in record.credential_ref.path
    assert str(real_config) not in record.credential_ref.path
    assert not (real_config / "cortex").exists()
    assert (caller_config / "cortex" / "secrets" / record.id).exists()


def test_native_account_credential_ref_honors_a_custom_environ(tmp_path, monkeypatch):
    real_home = tmp_path / "real-process-home"
    real_home.mkdir()
    monkeypatch.setenv("HOME", str(real_home))
    monkeypatch.delenv("XDG_CONFIG_HOME", raising=False)
    monkeypatch.delenv("XDG_STATE_HOME", raising=False)

    caller_home = tmp_path / "caller-home"
    caller_home.mkdir()
    caller_environ = dict(HOME=str(caller_home))

    record = onboarding.create_connection(
        name="native", engine="codex-acp", provider="openai", auth_mode=AuthMode.NATIVE_ACCOUNT,
        environ=caller_environ)
    assert str(caller_home) in record.credential_ref.path
    assert str(real_home) not in record.credential_ref.path
    assert not (real_home / ".local").exists()


# --- $HOME-is-a-Git-repository exception + Cortex-root .gitignore --------

def _git_init(path):
    import subprocess
    subprocess.run(["git", "init", "-q", str(path)], check=True)


def test_home_as_git_repo_is_allowed_and_gets_a_gitignore(tmp_path):
    home = tmp_path / "home"
    home.mkdir()
    _git_init(home)
    environ = dict(HOME=str(home))  # no XDG override -> $HOME/.config, $HOME/.local/state

    config_root = registry.config_root(environ)
    state_root = registry.state_root(environ)
    registry.ensure_private_dir(config_root, root=config_root, environ=environ)
    registry.ensure_private_dir(state_root, root=state_root, environ=environ)

    assert (config_root / ".gitignore").read_bytes() == b"*\n"
    assert (state_root / ".gitignore").read_bytes() == b"*\n"
    assert stat.S_IMODE((config_root / ".gitignore").lstat().st_mode) == 0o600


def test_other_enclosing_git_repo_containing_xdg_config_home_still_rejected(tmp_path):
    home = tmp_path / "home"
    home.mkdir()
    project = tmp_path / "some-project"
    (project / "xdg-config").mkdir(parents=True)
    _git_init(project)
    environ = dict(HOME=str(home), XDG_CONFIG_HOME=str(project / "xdg-config"))
    with pytest.raises(PortableError) as excinfo:
        registry.config_root(environ)
    assert excinfo.value.code == "connection_storage_under_project"


def test_home_global_acp_session_marker_does_not_block_user_configuration(tmp_path):
    home = tmp_path / "home"
    (home / ".cortex").mkdir(parents=True)
    environ = dict(HOME=str(home))
    assert registry.config_root(environ) == home / ".config" / "cortex"
    assert registry.state_root(environ) == home / ".local" / "state" / "cortex"


def test_cortex_marker_ancestor_still_rejected_even_under_home(tmp_path):
    home = tmp_path / "home"
    (home / ".cortex" / "xdg-config").mkdir(parents=True)
    _git_init(home)  # HOME is also a git repo, but the .cortex marker still rejects
    environ = dict(HOME=str(home), XDG_CONFIG_HOME=str(home / ".cortex" / "xdg-config"))
    with pytest.raises(PortableError) as excinfo:
        registry.config_root(environ)
    assert excinfo.value.code == "connection_storage_under_project"


def test_existing_different_gitignore_is_never_overwritten(tmp_path):
    home = tmp_path / "home"
    (home / ".config" / "cortex").mkdir(parents=True)
    (home / ".config" / "cortex" / ".gitignore").write_bytes(b"not-a-star\n")
    os.chmod(home / ".config" / "cortex" / ".gitignore", 0o600)
    environ = dict(HOME=str(home))
    config_root = registry.config_root(environ)
    with pytest.raises(PortableError) as excinfo:
        registry.ensure_private_dir(config_root, root=config_root, environ=environ)
    assert excinfo.value.code == "unsafe_storage"
    # Untouched: still the original content.
    assert (home / ".config" / "cortex" / ".gitignore").read_bytes() == b"not-a-star\n"


def test_gitignore_write_is_idempotent_across_repeated_calls(tmp_path):
    home = tmp_path / "home"
    home.mkdir()
    environ = dict(HOME=str(home))
    config_root = registry.config_root(environ)
    registry.ensure_private_dir(config_root, root=config_root, environ=environ)
    registry.ensure_private_dir(config_root, root=config_root, environ=environ)  # no error, no rewrite
    assert (config_root / ".gitignore").read_bytes() == b"*\n"


# --- F1 regression (review fingerprint 3dbb6a37): a relative HOME/XDG_* -----
# used to make _assert_not_nested_in_project_workspace mis-walk (Path('').
# anchor for a relative path silently skips the first real path component),
# so e.g. HOME="projectdir" with projectdir/.cortex present was never
# rejected. Every one of these must now fail closed with a validation
# error, before the ancestor walk is even reached.

def test_relative_home_is_rejected_before_any_ancestor_walk(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    (tmp_path / "projectdir" / ".cortex").mkdir(parents=True)
    environ = dict(HOME="projectdir")  # relative, resolves against cwd
    with pytest.raises(PortableError) as excinfo:
        registry.config_root(environ)
    assert excinfo.value.code == "relative_storage_root"
    with pytest.raises(PortableError) as excinfo:
        registry.state_root(environ)
    assert excinfo.value.code == "relative_storage_root"


def test_relative_xdg_config_home_is_rejected(tmp_path):
    environ = dict(HOME=str(tmp_path / "home"), XDG_CONFIG_HOME="relative/config")
    with pytest.raises(PortableError) as excinfo:
        registry.config_root(environ)
    assert excinfo.value.code == "relative_storage_root"


def test_relative_xdg_state_home_is_rejected(tmp_path):
    environ = dict(HOME=str(tmp_path / "home"), XDG_STATE_HOME="relative/state")
    with pytest.raises(PortableError) as excinfo:
        registry.state_root(environ)
    assert excinfo.value.code == "relative_storage_root"


def test_ancestor_walker_itself_rejects_a_relative_path(tmp_path):
    with pytest.raises(PortableError) as excinfo:
        registry._assert_not_nested_in_project_workspace(
            __import__("pathlib").Path("relative/sub/dir"), dict(HOME=str(tmp_path)))
    assert excinfo.value.code == "relative_storage_root"


def test_absolute_home_and_xdg_still_work_after_the_f1_fix(tmp_path):
    home = tmp_path / "home"
    config_home = tmp_path / "config"
    state_home = tmp_path / "state"
    for path in (home, config_home, state_home):
        path.mkdir()
    environ = dict(HOME=str(home), XDG_CONFIG_HOME=str(config_home), XDG_STATE_HOME=str(state_home))
    assert registry.config_root(environ) == config_home / "cortex"
    assert registry.state_root(environ) == state_home / "cortex"


def test_noop_transaction_preserves_inode_and_missing_registry(tmp_path, monkeypatch):
    _isolate(tmp_path, monkeypatch)
    with registry.transaction():
        pass
    assert not registry.registry_path().exists()
    registry.write_registry(registry.RegistryState({}, {}))
    before = registry.registry_path().stat()
    with registry.transaction() as state:
        assert state.connections == {}
    after = registry.registry_path().stat()
    assert (after.st_ino, after.st_mtime_ns) == (before.st_ino, before.st_mtime_ns)


def test_concurrent_noop_transactions_and_readers_are_safe(tmp_path, monkeypatch):
    from concurrent.futures import ThreadPoolExecutor
    from threading import Barrier
    _isolate(tmp_path, monkeypatch)
    registry.write_registry(registry.RegistryState({}, {}))
    barrier = Barrier(4)
    before = registry.registry_path().stat()
    def transact():
        barrier.wait(timeout=5)
        for _ in range(100):
            with registry.transaction() as state:
                assert state.connections == {}
    def read():
        barrier.wait(timeout=5)
        for _ in range(300):
            assert registry.read_registry().connections == {}
    with ThreadPoolExecutor(max_workers=4) as pool:
        futures = [pool.submit(operation) for operation in (transact, transact, read, read)]
        for future in futures:
            future.result(timeout=20)
    assert registry.registry_path().stat().st_ino == before.st_ino


def test_owner_read_reopens_an_inode_unlinked_by_atomic_replace(tmp_path, monkeypatch):
    path = tmp_path / "private"
    path.write_bytes(b"old")
    path.chmod(0o600)
    original_open = os.open
    descriptors = []
    def replacing_open(target, flags, *args, **kwargs):
        fd = original_open(target, flags, *args, **kwargs)
        descriptors.append(fd)
        if len(descriptors) == 1:
            replacement = tmp_path / "replacement"
            replacement.write_bytes(b"new")
            replacement.chmod(0o600)
            os.replace(replacement, path)
        return fd
    monkeypatch.setattr(registry.os, "open", replacing_open)
    assert registry.read_owner_file(path) == b"new"
    assert len(descriptors) == 2
    for fd in descriptors:
        with pytest.raises(OSError):
            os.fstat(fd)


def test_owner_read_replacement_retry_is_bounded(tmp_path, monkeypatch):
    path = tmp_path / "private"
    path.write_bytes(b"initial")
    path.chmod(0o600)
    original_open = os.open
    opened = []
    def replacing_open(target, flags, *args, **kwargs):
        fd = original_open(target, flags, *args, **kwargs)
        opened.append(fd)
        replacement = tmp_path / "replacement"
        replacement.write_bytes(b"new")
        replacement.chmod(0o600)
        os.replace(replacement, path)
        return fd
    monkeypatch.setattr(registry.os, "open", replacing_open)
    with pytest.raises(PortableError, match="unsafe_storage"):
        registry.read_owner_file(path)
    assert len(opened) == 8
    for fd in opened:
        with pytest.raises(OSError):
            os.fstat(fd)


def test_registry_rejects_hardlinked_file(tmp_path, monkeypatch):
    _isolate(tmp_path, monkeypatch)
    registry.write_registry(registry.RegistryState({}, {}))
    os.link(registry.registry_path(), tmp_path / "hardlink")
    assert registry.registry_path().stat().st_nlink == 2
    with pytest.raises(PortableError, match="unsafe_storage"):
        registry.read_registry()


@pytest.mark.parametrize("unsafe", ["mode", "owner"])
def test_unlinked_inode_never_bypasses_other_safety_checks(tmp_path, monkeypatch, unsafe):
    from types import SimpleNamespace
    path = tmp_path / "private"
    path.write_bytes(b"untrusted")
    path.chmod(0o600)
    original_fstat = os.fstat
    def unsafe_fstat(fd):
        info = original_fstat(fd)
        return SimpleNamespace(st_nlink=0,
                               st_mode=info.st_mode | (0o044 if unsafe == "mode" else 0),
                               st_uid=info.st_uid + (1 if unsafe == "owner" else 0))
    monkeypatch.setattr(registry.os, "fstat", unsafe_fstat)
    with pytest.raises(PortableError, match="unsafe_storage"):
        registry.read_owner_file(path)


@pytest.mark.parametrize("unsafe", ["symlink", "hardlink", "mode"])
def test_reopened_replacement_is_validated_not_trusted(tmp_path, monkeypatch, unsafe):
    path = tmp_path / "private"
    path.write_bytes(b"old")
    path.chmod(0o600)
    target = tmp_path / "untrusted"
    target.write_bytes(b"untrusted")
    target.chmod(0o600)
    original_open = os.open
    opens = []
    def replacing_open(name, flags, *args, **kwargs):
        fd = original_open(name, flags, *args, **kwargs)
        opens.append(fd)
        if len(opens) == 1:
            replacement = tmp_path / "replacement"
            if unsafe == "symlink":
                replacement.symlink_to(target)
            elif unsafe == "hardlink":
                os.link(target, replacement)
            else:
                replacement.write_bytes(b"untrusted")
                replacement.chmod(0o644)
            os.replace(replacement, path)
        return fd
    monkeypatch.setattr(registry.os, "open", replacing_open)
    with pytest.raises(PortableError, match="unsafe_storage"):
        registry.read_owner_file(path)


def test_changed_transaction_commits_but_exception_never_commits(tmp_path, monkeypatch):
    _isolate(tmp_path, monkeypatch)
    with registry.transaction() as state:
        state.override_history["test"] = []
    assert registry.read_registry().override_history == {"test": []}
    before = registry.registry_path().read_bytes()
    with pytest.raises(RuntimeError, match="abort"):
        with registry.transaction() as state:
            state.override_history["other"] = []
            raise RuntimeError("abort")
    assert registry.registry_path().read_bytes() == before
