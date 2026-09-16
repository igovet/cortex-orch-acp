"""Onboarding lifecycle tests: create, set_api_key, native login, validate,
remove, and resolve_connection_for_route (fake credentials only)."""
from __future__ import annotations

import inspect
import stat

import pytest

from cortex_runtime.connections import onboarding, registry
from cortex_runtime.connections.model import (
    AuthMode,
    ConnectionState,
    EngineNativeDir,
    OsKeyring,
    SecretFile,
)
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


def _native_connection(tmp_path, monkeypatch, engine="codex-acp"):
    _isolate(tmp_path, monkeypatch)
    record = onboarding.create_connection(
        name="work",
        engine=engine,
        provider="openai" if engine == "codex-acp" else "anthropic",
        auth_mode=AuthMode.NATIVE_ACCOUNT,
    )
    native_dir = registry.connection_state_dir(record.id, engine)
    return record, native_dir


def test_create_connection_derives_credential_ref_under_cortex_roots(tmp_path, monkeypatch):
    _isolate(tmp_path, monkeypatch)
    record = onboarding.create_connection(
        name="work", engine="codex-acp", provider="openai", auth_mode=AuthMode.API_KEY, key_kind="openai_api_key",
    )
    assert isinstance(record.credential_ref, SecretFile)
    assert record.credential_ref.path == str(registry.secret_path(record.id))

    native = onboarding.create_connection(
        name="native", engine="codex-acp", provider="openai-native", auth_mode=AuthMode.NATIVE_ACCOUNT,
    )
    assert isinstance(native.credential_ref, EngineNativeDir)
    assert native.credential_ref.path == str(registry.connection_state_dir(native.id, native.engine))


def test_create_connection_has_no_path_or_credential_ref_parameter():
    signature = inspect.signature(onboarding.create_connection)
    assert "credential_ref" not in signature.parameters
    for name in signature.parameters:
        assert "path" not in name.lower()


def test_create_connection_duplicate_rejected_without_reuse(tmp_path, monkeypatch):
    _isolate(tmp_path, monkeypatch)
    kwargs = dict(name="work", engine="codex-acp", provider="openai", auth_mode=AuthMode.NATIVE_ACCOUNT)
    onboarding.create_connection(**kwargs)
    with pytest.raises(PortableError) as excinfo:
        onboarding.create_connection(**kwargs)
    assert excinfo.value.code == "connection_already_exists"


def test_create_connection_duplicate_reused_with_reuse_true(tmp_path, monkeypatch):
    _isolate(tmp_path, monkeypatch)
    kwargs = dict(name="work", engine="codex-acp", provider="openai", auth_mode=AuthMode.NATIVE_ACCOUNT)
    first = onboarding.create_connection(**kwargs)
    second = onboarding.create_connection(reuse=True, **kwargs)
    assert first.id == second.id


def test_unsupported_engine_key_kind_combo_rejected(tmp_path, monkeypatch):
    _isolate(tmp_path, monkeypatch)
    with pytest.raises(PortableError) as excinfo:
        onboarding.create_connection(
            name="wrong", engine="claude-agent-acp", provider="anthropic",
            auth_mode=AuthMode.API_KEY, key_kind="openai_api_key",
        )
    assert excinfo.value.code == "unsupported_auth_mode"


def test_gateway_still_registers_but_is_never_qualified(tmp_path, monkeypatch):
    _isolate(tmp_path, monkeypatch)
    record = onboarding.create_connection(
        name="gw", engine="codex-acp", provider="custom-gateway", auth_mode=AuthMode.GATEWAY,
    )
    assert record.qualified is False
    assert isinstance(record.credential_ref, EngineNativeDir)


def test_set_api_key_has_no_argv_or_string_secret_parameter():
    signature = inspect.signature(onboarding.set_api_key)
    parameters = list(signature.parameters.values())
    assert parameters[0].name == "connection_id"
    assert parameters[1].name == "reader"
    for name in signature.parameters:
        if name in ("connection_id", "environ"):
            continue
        assert name == "reader", "only a callable reader may supply the secret"


def _api_key_connection(tmp_path, monkeypatch, *, engine="claude-agent-acp", provider="anthropic", key_kind="anthropic_api_key"):
    _isolate(tmp_path, monkeypatch)
    record = onboarding.create_connection(
        name="work", engine=engine, provider=provider, auth_mode=AuthMode.API_KEY, key_kind=key_kind,
    )
    return record, registry.secret_path(record.id)


def test_set_api_key_writes_atomically_and_state_becomes_configured(tmp_path, monkeypatch):
    record, secret_path = _api_key_connection(tmp_path, monkeypatch)
    assert record.state == ConnectionState.AUTHORIZATION_REQUIRED

    updated = onboarding.set_api_key(record.id, lambda: b"fake-anthropic-key-0001")
    assert updated.state == ConnectionState.CONFIGURED
    info = secret_path.lstat()
    assert stat.S_IMODE(info.st_mode) == 0o600
    assert secret_path.read_bytes() == b"fake-anthropic-key-0001"


def test_set_api_key_rejects_empty_secret(tmp_path, monkeypatch):
    record, _secret_path = _api_key_connection(tmp_path, monkeypatch)
    with pytest.raises(PortableError) as excinfo:
        onboarding.set_api_key(record.id, lambda: b"")
    assert excinfo.value.code == "empty_secret"


def test_set_api_key_refuses_to_write_when_credential_ref_is_tampered(tmp_path, monkeypatch):
    record, _secret_path = _api_key_connection(tmp_path, monkeypatch)
    outside_target = tmp_path / "outside-secret"
    with registry.transaction() as state:
        existing = state.connections[record.id]
        tampered = SecretFile(path=str(outside_target))
        state.connections[record.id] = existing.__class__(
            id=existing.id, name=existing.name, engine=existing.engine, provider=existing.provider,
            auth_mode=existing.auth_mode, key_kind=existing.key_kind, endpoint=existing.endpoint,
            launch_command=existing.launch_command, credential_ref=tampered,
            approved_fingerprint=existing.approved_fingerprint, state=existing.state,
            qualified=existing.qualified, created_at=existing.created_at, updated_at=existing.updated_at,
        )
    with pytest.raises(PortableError) as excinfo:
        onboarding.set_api_key(record.id, lambda: b"fake-should-never-be-written")
    assert excinfo.value.code == "connection_invalid"
    assert not outside_target.exists()


def test_mark_native_login_pending_returns_instructions_naming_env_var_and_dir_no_secret(tmp_path, monkeypatch):
    record, native_dir = _native_connection(tmp_path, monkeypatch)
    instructions = onboarding.mark_native_login_pending(record.id)
    assert "CODEX_HOME" in instructions
    assert str(native_dir) in instructions
    assert "fake" not in instructions.lower()
    updated = onboarding.validate(record.id)
    assert updated.state == ConnectionState.AUTHORIZATION_REQUIRED


def test_validate_native_marker_missing_is_authorization_required(tmp_path, monkeypatch):
    record, native_dir = _native_connection(tmp_path, monkeypatch)
    assert onboarding.validate(record.id).state == ConnectionState.AUTHORIZATION_REQUIRED


def test_validate_native_marker_present_is_configured(tmp_path, monkeypatch):
    record, native_dir = _native_connection(tmp_path, monkeypatch)
    native_dir.mkdir(mode=0o700, exist_ok=True)
    (native_dir / "auth.json").write_text('{"fake": "not-a-real-token"}')
    assert onboarding.validate(record.id).state == ConnectionState.CONFIGURED


def test_deleting_native_marker_reverts_state_and_resolve_raises(tmp_path, monkeypatch):
    record, native_dir = _native_connection(tmp_path, monkeypatch)
    native_dir.mkdir(mode=0o700, exist_ok=True)
    marker = native_dir / "auth.json"
    marker.write_text('{"fake": "not-a-real-token"}')
    assert onboarding.validate(record.id).state == ConnectionState.CONFIGURED

    marker.unlink()
    with pytest.raises(PortableError) as excinfo:
        onboarding.resolve_connection_for_route(record.id)
    assert excinfo.value.code == "connection_requires_reauthorization"
    assert excinfo.value.category.value == "authorization"


def test_resolve_connection_for_route_never_substitutes_another_connection(tmp_path, monkeypatch):
    first, _first_dir = _native_connection(tmp_path, monkeypatch)
    second_record = onboarding.create_connection(
        name="working", engine="codex-acp", provider="openai-2", auth_mode=AuthMode.NATIVE_ACCOUNT,
    )
    second_dir = registry.connection_state_dir(second_record.id, second_record.engine)
    second_dir.mkdir(mode=0o700, exist_ok=True)
    (second_dir / "auth.json").write_text('{"fake": "value"}')
    onboarding.validate(second_record.id)

    with pytest.raises(PortableError) as excinfo:
        onboarding.resolve_connection_for_route(first.id)
    assert excinfo.value.code == "connection_requires_reauthorization"


def test_resolve_connection_for_route_unknown_id_is_unavailable(tmp_path, monkeypatch):
    _isolate(tmp_path, monkeypatch)
    with pytest.raises(PortableError) as excinfo:
        onboarding.resolve_connection_for_route("cn_000000000000")
    assert excinfo.value.code == "connection_unavailable"


def test_resolve_connection_for_route_configured_returns_the_record(tmp_path, monkeypatch):
    record, native_dir = _native_connection(tmp_path, monkeypatch)
    native_dir.mkdir(mode=0o700, exist_ok=True)
    (native_dir / "auth.json").write_text('{"fake": "value"}')
    resolved = onboarding.resolve_connection_for_route(record.id)
    assert resolved.id == record.id
    assert resolved.state == ConnectionState.CONFIGURED


def test_remove_deletes_registry_entry_and_cortex_secret_but_never_native_dir_contents(tmp_path, monkeypatch):
    record, secret_path = _api_key_connection(tmp_path, monkeypatch)
    onboarding.set_api_key(record.id, lambda: b"fake-secret-99")
    assert secret_path.exists()

    onboarding.remove(record.id)
    assert not secret_path.exists()
    assert registry.read_registry().connections == {}


def test_remove_never_touches_native_dir_contents(tmp_path, monkeypatch):
    record, native_dir = _native_connection(tmp_path, monkeypatch)
    native_dir.mkdir(mode=0o700, exist_ok=True)
    marker = native_dir / "auth.json"
    marker.write_text('{"fake": "value"}')
    onboarding.remove(record.id)
    assert marker.exists()
    assert marker.read_text() == '{"fake": "value"}'


def test_remove_refuses_to_unlink_a_tampered_non_canonical_secret_path(tmp_path, monkeypatch):
    record, secret_path = _api_key_connection(tmp_path, monkeypatch)
    onboarding.set_api_key(record.id, lambda: b"fake-secret-99")
    outside_target = tmp_path / "unrelated-owned-file"
    outside_target.write_text("do-not-delete-me")
    with registry.transaction() as state:
        existing = state.connections[record.id]
        tampered = SecretFile(path=str(outside_target))
        state.connections[record.id] = existing.__class__(
            id=existing.id, name=existing.name, engine=existing.engine, provider=existing.provider,
            auth_mode=existing.auth_mode, key_kind=existing.key_kind, endpoint=existing.endpoint,
            launch_command=existing.launch_command, credential_ref=tampered,
            approved_fingerprint=existing.approved_fingerprint, state=existing.state,
            qualified=existing.qualified, created_at=existing.created_at, updated_at=existing.updated_at,
        )
    onboarding.remove(record.id)
    assert outside_target.exists()
    assert secret_path.exists()  # the real, canonical secret file is untouched too


def test_os_keyring_without_module_is_unavailable(tmp_path, monkeypatch):
    _isolate(tmp_path, monkeypatch)
    import sys
    assert "keyring" not in sys.modules
    record = onboarding.create_connection(
        name="work", engine="claude-agent-acp", provider="anthropic",
        auth_mode=AuthMode.API_KEY, key_kind="claude_oauth_token", credential_kind="os_keyring",
    )
    assert isinstance(record.credential_ref, OsKeyring)
    assert record.credential_ref.service == "cortex"
    assert record.credential_ref.account == record.id
    assert record.state == ConnectionState.UNAVAILABLE
    with pytest.raises(PortableError) as excinfo:
        onboarding.resolve_connection_for_route(record.id)
    assert excinfo.value.code == "connection_unavailable"
