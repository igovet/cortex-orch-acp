"""Trust for endpoint/launch-command/provider overrides (fake credentials only)."""
from __future__ import annotations

import pytest

from cortex_runtime.connections import onboarding, trust
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


def _connection(tmp_path, monkeypatch, *, endpoint="https://gateway.example.com", launch_command=("codex-acp", "serve")):
    _isolate(tmp_path, monkeypatch)
    return onboarding.create_connection(
        name="work",
        engine="codex-acp",
        provider="openai",
        auth_mode=AuthMode.API_KEY,
        key_kind="openai_api_key",
        endpoint=endpoint,
        launch_command=launch_command,
    )


def test_legit_declared_route_matching_approved_values_passes(tmp_path, monkeypatch):
    record = _connection(tmp_path, monkeypatch)
    resolved = trust.resolve_project_route(record.id, "https://gateway.example.com", ("codex-acp", "serve"), "openai")
    assert resolved.id == record.id


def test_malicious_project_override_of_endpoint_is_rejected(tmp_path, monkeypatch):
    record = _connection(tmp_path, monkeypatch)
    with pytest.raises(PortableError) as excinfo:
        trust.resolve_project_route(record.id, "https://attacker.example.com", ("codex-acp", "serve"), "openai")
    assert excinfo.value.code == "untrusted_project_override"
    assert excinfo.value.category.value == "authorization"
    assert excinfo.value.retry_eligible is False


def test_malicious_project_override_of_launch_command_is_rejected(tmp_path, monkeypatch):
    record = _connection(tmp_path, monkeypatch)
    with pytest.raises(PortableError) as excinfo:
        trust.resolve_project_route(record.id, "https://gateway.example.com", ("rm", "-rf", "/"), "openai")
    assert excinfo.value.code == "untrusted_project_override"


def test_malicious_project_override_of_provider_is_rejected(tmp_path, monkeypatch):
    record = _connection(tmp_path, monkeypatch)
    with pytest.raises(PortableError) as excinfo:
        trust.resolve_project_route(record.id, "https://gateway.example.com", ("codex-acp", "serve"), "attacker-provider")
    assert excinfo.value.code == "untrusted_project_override"


def test_unknown_connection_id_is_unavailable(tmp_path, monkeypatch):
    _isolate(tmp_path, monkeypatch)
    with pytest.raises(PortableError) as excinfo:
        trust.resolve_project_route("cn_000000000000", "https://gateway.example.com", None, "openai")
    assert excinfo.value.code == "connection_unavailable"


def test_approve_override_requires_explicit_approved_by_user_true(tmp_path, monkeypatch):
    record = _connection(tmp_path, monkeypatch)
    with pytest.raises(PortableError) as excinfo:
        trust.approve_override(record.id, "https://new.example.com", ("codex-acp", "serve"), "openai", approved_by_user=False)
    assert excinfo.value.code == "untrusted_project_override"


def test_approve_override_updates_fingerprint_and_records_history(tmp_path, monkeypatch):
    record = _connection(tmp_path, monkeypatch)
    original_fingerprint = record.approved_fingerprint

    updated = trust.approve_override(record.id, "https://new.example.com", ("codex-acp", "serve"), "openai", approved_by_user=True)
    assert updated.approved_fingerprint != original_fingerprint
    assert updated.endpoint == "https://new.example.com"

    history = trust.override_history(record.id)
    assert len(history) == 1
    entry = history[0]
    assert entry["previous_fingerprint"] == original_fingerprint
    assert entry["fingerprint"] == updated.approved_fingerprint
    assert entry["approved_by_user"] is True
    assert "approved_at" in entry

    # The now-approved new endpoint resolves cleanly.
    resolved = trust.resolve_project_route(record.id, "https://new.example.com", ("codex-acp", "serve"), "openai")
    assert resolved.endpoint == "https://new.example.com"
    # The old endpoint is no longer trusted without a fresh approval.
    with pytest.raises(PortableError):
        trust.resolve_project_route(record.id, "https://gateway.example.com", ("codex-acp", "serve"), "openai")


def test_fingerprint_is_deterministic_and_order_independent_of_call_site(tmp_path, monkeypatch):
    first = trust.fingerprint("codex-acp", "openai", "https://gateway.example.com", ("codex-acp", "serve"))
    second = trust.fingerprint("codex-acp", "openai", "https://gateway.example.com", ("codex-acp", "serve"))
    third = trust.fingerprint("codex-acp", "openai", "https://gateway.example.com", ("codex-acp", "serve", "--extra"))
    assert first == second
    assert first != third
