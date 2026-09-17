"""Per-connection process environment isolation (fake credentials only).

Includes regression tests for the T006 independent-review findings
(fingerprint 3f743b68):
  F1: a caller could point credential_ref at an arbitrary path (e.g. inside
      a project's .cortex, or a project worktree) -- now structurally
      impossible: create_connection never accepts a path.
  F2: two connections could share the same credential path, so
      build_process_env returned one account's key for the other -- now
      detected and rejected before any read.
  F3: extra_env was denylist-only, case-sensitive, and incomplete -- now
      removed entirely and replaced by base_env's fixed exact allowlist
      plus a narrow, validated channel_credentials parameter.
"""
from __future__ import annotations

import pytest

from cortex_runtime.connections import environment, onboarding, registry
from cortex_runtime.connections.model import AuthMode, EngineNativeDir, SecretFile
from cortex_runtime.portable.errors import PortableError

FAKE_ANTHROPIC_KEY = b"fake-anthropic-key-AAA111"
FAKE_OPENAI_KEY = b"fake-openai-key-BBB222"


def _isolate(tmp_path, monkeypatch):
    config_home = tmp_path / "config"
    state_home = tmp_path / "state"
    home = tmp_path / "home"
    for path in (config_home, state_home, home):
        path.mkdir()
    monkeypatch.setenv("XDG_CONFIG_HOME", str(config_home))
    monkeypatch.setenv("XDG_STATE_HOME", str(state_home))
    monkeypatch.setenv("HOME", str(home))


def _api_key_connection(tmp_path, monkeypatch, *, engine, provider, key_kind, secret):
    record = onboarding.create_connection(
        name=f"{provider}-connection", engine=engine, provider=provider,
        auth_mode=AuthMode.API_KEY, key_kind=key_kind,
    )
    return onboarding.set_api_key(record.id, lambda: secret)


def test_two_connections_same_engine_have_disjoint_dirs_env_and_secrets(tmp_path, monkeypatch):
    _isolate(tmp_path, monkeypatch)
    first = _api_key_connection(tmp_path, monkeypatch, engine="codex-acp", provider="openai-work", key_kind="openai_api_key", secret=FAKE_OPENAI_KEY)
    second = _api_key_connection(tmp_path, monkeypatch, engine="codex-acp", provider="openai-personal", key_kind="openai_api_key", secret=b"fake-openai-key-CCC333")

    assert first.id != second.id
    env1 = environment.build_process_env(first, base_env={})
    env2 = environment.build_process_env(second, base_env={})
    assert env1["CODEX_HOME"] != env2["CODEX_HOME"]
    assert env1["CODEX_API_KEY"] == "fake-openai-key-BBB222"
    assert env2["CODEX_API_KEY"] == "fake-openai-key-CCC333"
    assert first.credential_ref.path != second.credential_ref.path


def test_process_env_contains_only_this_connections_own_secret(tmp_path, monkeypatch):
    _isolate(tmp_path, monkeypatch)
    target = _api_key_connection(tmp_path, monkeypatch, engine="claude-agent-acp", provider="anthropic-target", key_kind="anthropic_api_key", secret=b"fake-target-secret-ZZZ")

    polluted_base_env = {
        "PATH": "/usr/bin",
        "HOME": "/home/fakeuser",
        "ANTHROPIC_API_KEY": "fake-OTHER-anthropic-key",
        "OPENAI_API_KEY": "fake-other-openai-key",
        "CODEX_API_KEY": "fake-other-codex-key",
        "CLAUDE_CODE_OAUTH_TOKEN": "fake-other-oauth-token",
        "AWS_SECRET_ACCESS_KEY": "fake-aws-secret",
        "GITHUB_TOKEN": "fake-github-token",
        "MY_APP_PASSWORD": "fake-app-password",
    }
    env = environment.build_process_env(target, base_env=polluted_base_env)

    values = list(env.values())
    assert "fake-target-secret-ZZZ" in values
    for key, value in polluted_base_env.items():
        if key in ("PATH", "HOME"):
            continue
        assert value not in values, f"leaked polluted base_env value for {key}"
    assert env["ANTHROPIC_API_KEY"] == "fake-target-secret-ZZZ"


def test_base_env_is_an_exact_fixed_allowlist_not_a_denylist(tmp_path, monkeypatch):
    """F3 regression: base_env passthrough is allowlist-only, case-sensitive."""
    _isolate(tmp_path, monkeypatch)
    record = _api_key_connection(tmp_path, monkeypatch, engine="codex-acp", provider="openai-work", key_kind="openai_api_key", secret=FAKE_OPENAI_KEY)
    polluted_base_env = {
        "PATH": "/usr/bin",
        "LD_PRELOAD": "/tmp/evil.so",
        "PYTHONPATH": "/tmp/evil",
        "NODE_OPTIONS": "--require /tmp/evil.js",
        "npm_config_registry": "http://attacker.example.com",
        "GIT_ASKPASS": "/tmp/evil-askpass",
        "SSH_AUTH_SOCK": "/tmp/evil.sock",
        "http_proxy": "http://attacker.example.com",
        "https_proxy": "http://attacker.example.com",
        "openai_api_key": "fake-lowercase-leak",
        "anthropic_api_key": "fake-lowercase-leak-2",
        "aws_secret_access_key": "fake-lowercase-leak-3",
        "my_custom_secret": "fake-custom-leak",
        "XDG_CONFIG_HOME": "/tmp/evil-config",
        "SOME_SERVICE_BASE_URL": "http://attacker.example.com",
    }
    env = environment.build_process_env(record, base_env=polluted_base_env)
    for key in polluted_base_env:
        if key == "PATH":
            continue
        assert key not in env, f"{key} leaked through base_env even though it is not in the fixed allowlist"
    for key, value in polluted_base_env.items():
        if key == "PATH":
            continue
        assert value not in env.values()


def test_denylist_is_defense_in_depth_case_insensitive(tmp_path, monkeypatch):
    from cortex_runtime.connections.environment import DENYLIST_PATTERN
    for name in ("openai_api_key", "OPENAI_API_KEY", "OpenAI_Api_Key", "my_token", "MY_TOKEN"):
        assert DENYLIST_PATTERN.search(name), f"{name} should match the case-insensitive denylist"


def test_path_entries_are_validated_empty_and_relative_dropped(tmp_path, monkeypatch):
    _isolate(tmp_path, monkeypatch)
    record = _api_key_connection(tmp_path, monkeypatch, engine="codex-acp", provider="openai-work", key_kind="openai_api_key", secret=FAKE_OPENAI_KEY)
    env = environment.build_process_env(record, base_env={"PATH": "/usr/bin::relative/dir:/usr/local/bin:"})
    assert env["PATH"] == "/usr/bin:/usr/local/bin"


def test_env_allowlist_only_includes_documented_minimal_keys(tmp_path, monkeypatch):
    _isolate(tmp_path, monkeypatch)
    record = _api_key_connection(tmp_path, monkeypatch, engine="codex-acp", provider="openai-work", key_kind="openai_api_key", secret=FAKE_OPENAI_KEY)
    base_env = {
        "PATH": "/usr/bin", "HOME": "/home/x", "LANG": "en_US.UTF-8", "SHELL": "/bin/bash",
        "XDG_RUNTIME_DIR": "/run/user/1000", "RANDOM_APP_VAR": "value",
    }
    env = environment.build_process_env(record, base_env=base_env)
    assert "SHELL" not in env
    assert "XDG_RUNTIME_DIR" not in env
    assert "RANDOM_APP_VAR" not in env
    assert env["PATH"] == "/usr/bin"
    assert env["LANG"] == "en_US.UTF-8"


def test_channel_credentials_set_exactly_three_names_when_passed(tmp_path, monkeypatch):
    _isolate(tmp_path, monkeypatch)
    record = _api_key_connection(tmp_path, monkeypatch, engine="codex-acp", provider="openai-work", key_kind="openai_api_key", secret=FAKE_OPENAI_KEY)
    channel = ("ch_" + "a" * 12, "fake-channel-secret-token-1234567890", "/home/user/project")
    env = environment.build_process_env(record, base_env={}, channel_credentials=channel)
    assert env["CORTEX_CHANNEL_ID"] == channel[0]
    assert env["CORTEX_CHANNEL_SECRET"] == channel[1]
    assert env["CORTEX_CHANNEL_PROJECT_ROOT"] == channel[2]


def test_channel_credentials_absent_when_not_passed(tmp_path, monkeypatch):
    _isolate(tmp_path, monkeypatch)
    record = _api_key_connection(tmp_path, monkeypatch, engine="codex-acp", provider="openai-work", key_kind="openai_api_key", secret=FAKE_OPENAI_KEY)
    env = environment.build_process_env(record, base_env={})
    assert "CORTEX_CHANNEL_ID" not in env
    assert "CORTEX_CHANNEL_SECRET" not in env
    assert "CORTEX_CHANNEL_PROJECT_ROOT" not in env


def test_channel_credentials_reject_malformed_values(tmp_path, monkeypatch):
    _isolate(tmp_path, monkeypatch)
    record = _api_key_connection(tmp_path, monkeypatch, engine="codex-acp", provider="openai-work", key_kind="openai_api_key", secret=FAKE_OPENAI_KEY)
    with pytest.raises(PortableError) as excinfo:
        environment.build_process_env(record, base_env={}, channel_credentials=("not-a-channel-id", "fake-secret-1234567890abcdef", "/home/x"))
    assert excinfo.value.code == "invalid_field"
    with pytest.raises(PortableError):
        environment.build_process_env(record, base_env={}, channel_credentials=("ch_" + "a" * 12, "fake-secret-1234567890abcdef", "relative/path"))


def test_extra_env_parameter_no_longer_exists(tmp_path, monkeypatch):
    import inspect
    signature = inspect.signature(environment.build_process_env)
    assert "extra_env" not in signature.parameters
    assert "channel_credentials" in signature.parameters


def test_mcp_allowlist_is_not_forwarded_wholesale_into_process_env(tmp_path, monkeypatch):
    _isolate(tmp_path, monkeypatch)
    record = _api_key_connection(tmp_path, monkeypatch, engine="codex-acp", provider="openai-work", key_kind="openai_api_key", secret=FAKE_OPENAI_KEY)
    env = environment.build_process_env(record, base_env={}, mcp_allowlist=("server-a", "server-b"))
    assert "server-a" not in env.values()
    assert not any("mcp" in key.lower() for key in env)


def test_mcp_allowlist_must_be_a_tuple_of_strings(tmp_path, monkeypatch):
    _isolate(tmp_path, monkeypatch)
    record = _api_key_connection(tmp_path, monkeypatch, engine="codex-acp", provider="openai-work", key_kind="openai_api_key", secret=FAKE_OPENAI_KEY)
    with pytest.raises(PortableError) as excinfo:
        environment.build_process_env(record, base_env={}, mcp_allowlist=["server-a"])
    assert excinfo.value.code == "invalid_field"


def test_native_account_connection_isolation_var_points_at_its_own_dir(tmp_path, monkeypatch):
    _isolate(tmp_path, monkeypatch)
    record = onboarding.create_connection(name="work", engine="codex-acp", provider="openai", auth_mode=AuthMode.NATIVE_ACCOUNT)
    env = environment.build_process_env(record, base_env={})
    assert env["CODEX_HOME"] == str(registry.connection_state_dir(record.id, "codex-acp"))
    assert "CODEX_API_KEY" not in env


def test_describe_process_env_returns_names_only(tmp_path, monkeypatch):
    _isolate(tmp_path, monkeypatch)
    record = _api_key_connection(tmp_path, monkeypatch, engine="codex-acp", provider="openai-work", key_kind="openai_api_key", secret=FAKE_OPENAI_KEY)
    env = environment.build_process_env(record, base_env={"PATH": "/usr/bin"})
    names = environment.describe_process_env(env)
    assert "CODEX_API_KEY" in names
    for name in names:
        assert isinstance(name, str)
    assert "fake-openai-key-BBB222" not in names


# --- F1 regression: no caller-supplied path can escape into a project ---

def test_f1_secret_file_path_can_never_be_pointed_inside_a_project_cortex(tmp_path, monkeypatch):
    project = tmp_path / "project"
    (project / ".cortex").mkdir(parents=True)
    _isolate(tmp_path, monkeypatch)
    import inspect
    assert "path" not in "".join(inspect.signature(onboarding.create_connection).parameters)
    record = onboarding.create_connection(name="work", engine="codex-acp", provider="openai", auth_mode=AuthMode.API_KEY, key_kind="openai_api_key")
    assert not str(record.credential_ref.path).startswith(str(project))
    assert ".cortex" not in record.credential_ref.path


def test_f1_engine_native_dir_can_never_be_pointed_inside_a_project_worktree(tmp_path, monkeypatch):
    project = tmp_path / "project"
    (project / ".git").mkdir(parents=True)
    _isolate(tmp_path, monkeypatch)
    record = onboarding.create_connection(name="work", engine="codex-acp", provider="openai", auth_mode=AuthMode.NATIVE_ACCOUNT)
    assert not str(record.credential_ref.path).startswith(str(project))


def test_f1_tampered_registry_ref_pointing_into_a_project_is_invalid_and_env_build_raises(tmp_path, monkeypatch):
    project = tmp_path / "project"
    (project / ".cortex").mkdir(parents=True)
    _isolate(tmp_path, monkeypatch)
    record = onboarding.create_connection(name="work", engine="codex-acp", provider="openai", auth_mode=AuthMode.API_KEY, key_kind="openai_api_key")
    evil_path = project / ".cortex" / "evil_secret"
    with registry.transaction() as state:
        existing = state.connections[record.id]
        tampered = SecretFile(path=str(evil_path))
        state.connections[record.id] = existing.__class__(
            id=existing.id, name=existing.name, engine=existing.engine, provider=existing.provider,
            auth_mode=existing.auth_mode, key_kind=existing.key_kind, endpoint=existing.endpoint,
            launch_command=existing.launch_command, credential_ref=tampered,
            approved_fingerprint=existing.approved_fingerprint, state=existing.state,
            qualified=existing.qualified, created_at=existing.created_at, updated_at=existing.updated_at,
        )
    assert onboarding.validate(record.id).state.value == "invalid"
    with pytest.raises(PortableError):
        onboarding.set_api_key(record.id, lambda: b"fake-secret-should-never-write")
    assert not evil_path.exists()
    tampered_record = registry.read_registry().connections[record.id]
    with pytest.raises(PortableError):
        environment.build_process_env(tampered_record, base_env={})
    assert not evil_path.exists()


# --- F2 regression: two connections can never share a credential path ---

def test_f2_hand_tampered_shared_secret_path_makes_both_connections_invalid(tmp_path, monkeypatch):
    _isolate(tmp_path, monkeypatch)
    first = _api_key_connection(tmp_path, monkeypatch, engine="codex-acp", provider="openai-a", key_kind="openai_api_key", secret=b"fake-key-A")
    second = onboarding.create_connection(name="b", engine="codex-acp", provider="openai-b", auth_mode=AuthMode.API_KEY, key_kind="openai_api_key")

    with registry.transaction() as state:
        existing = state.connections[second.id]
        shared = SecretFile(path=str(registry.secret_path(first.id)))
        state.connections[second.id] = existing.__class__(
            id=existing.id, name=existing.name, engine=existing.engine, provider=existing.provider,
            auth_mode=existing.auth_mode, key_kind=existing.key_kind, endpoint=existing.endpoint,
            launch_command=existing.launch_command, credential_ref=shared,
            approved_fingerprint=existing.approved_fingerprint, state=existing.state,
            qualified=existing.qualified, created_at=existing.created_at, updated_at=existing.updated_at,
        )

    updated_first = onboarding.validate(first.id)
    updated_second = onboarding.validate(second.id)
    assert updated_first.state.value == "invalid"
    assert updated_second.state.value == "invalid"

    fresh_first = registry.read_registry().connections[first.id]
    fresh_second = registry.read_registry().connections[second.id]
    with pytest.raises(PortableError):
        environment.build_process_env(fresh_first, base_env={})
    with pytest.raises(PortableError):
        environment.build_process_env(fresh_second, base_env={})


# --- T012 follow-up: build_process_env must honor an explicit `environ` ---

def test_build_process_env_honors_explicit_environ_over_os_environ(tmp_path, monkeypatch):
    """A caller-supplied ``environ`` mapping -- not ``os.environ`` -- must
    drive every registry/config/state resolution inside ``build_process_env``.

    ``os.environ``'s own ``HOME`` is monkeypatched to a *different* tmp
    directory than the one named in the explicit ``environ=`` mapping; only
    the explicit mapping's tmp root may ever be touched.
    """
    wrong_home = tmp_path / "wrong-home-must-stay-empty"
    wrong_home.mkdir()
    monkeypatch.setenv("HOME", str(wrong_home))
    monkeypatch.delenv("XDG_CONFIG_HOME", raising=False)
    monkeypatch.delenv("XDG_STATE_HOME", raising=False)

    right_home = tmp_path / "right-home"
    right_config = right_home / "config"
    right_state = right_home / "state"
    for path in (right_home, right_config, right_state):
        path.mkdir()
    explicit_environ = {"HOME": str(right_home), "XDG_CONFIG_HOME": str(right_config), "XDG_STATE_HOME": str(right_state)}

    record = onboarding.create_connection(
        name="explicit-environ-connection", engine="codex-acp", provider="openai-explicit",
        auth_mode=AuthMode.NATIVE_ACCOUNT, environ=explicit_environ,
    )

    env = environment.build_process_env(record, base_env={}, environ=explicit_environ)
    assert env["CODEX_HOME"].startswith(str(right_state))

    # Nothing was ever created under the monkeypatched os.environ HOME.
    assert list(wrong_home.iterdir()) == []

    # The real registry/state directories exist only under the explicit
    # environ's own tmp root.
    assert (right_config / "cortex" / "connections.json").is_file()
    assert (right_state / "cortex" / "connections" / record.id).is_dir()
