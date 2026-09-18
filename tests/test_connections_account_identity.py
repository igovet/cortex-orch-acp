"""``connections.account_identity`` -- read-only account display identity.

T035 (account-label follow-up): the first selector must show the real
account, never the operator-chosen nickname. Every credential used here is
a fake, locally-generated fixture -- never a real token, never a paid call.

The load-bearing invariant under test: no secret substring (an id_token, an
access/refresh token, or an API key) may ever appear in the resolved
label, in a log line, or in a raised error -- failures collapse to
``None`` instead.
"""
from __future__ import annotations

import base64
import json
import logging
import os
import stat
from pathlib import Path

import pytest

from cortex_runtime.connections import account_identity, onboarding, registry
from cortex_runtime.connections.model import AuthMode
from cortex_runtime.catalog.labels import build_provider_label

FAKE_ACCESS_TOKEN = "fake-access-token-do-not-log-ZzZ999"
FAKE_REFRESH_TOKEN = "fake-refresh-token-do-not-log-YyY888"


def _isolate(tmp_path, monkeypatch):
    config_home = tmp_path / "config"
    state_home = tmp_path / "state"
    home = tmp_path / "home"
    for path in (config_home, state_home, home):
        path.mkdir()
    monkeypatch.setenv("XDG_CONFIG_HOME", str(config_home))
    monkeypatch.setenv("XDG_STATE_HOME", str(state_home))
    monkeypatch.setenv("HOME", str(home))
    return dict(XDG_CONFIG_HOME=str(config_home), XDG_STATE_HOME=str(state_home), HOME=str(home))


def _b64url(data: dict) -> str:
    raw = json.dumps(data).encode("utf-8")
    return base64.urlsafe_b64encode(raw).rstrip(b"=").decode("ascii")


def _fake_id_token(email="igovet+2@gmail.com", **claims) -> str:
    header = _b64url({"alg": "none", "typ": "JWT"})
    payload = _b64url({"email": email, "email_verified": True, "name": "Test User", **claims})
    signature = base64.urlsafe_b64encode(b"fake-signature-not-real").rstrip(b"=").decode("ascii")
    return f"{header}.{payload}.{signature}"


def _native_codex_connection(environ, *, name="codex-live"):
    return onboarding.create_connection(
        name=name, engine="codex-acp", provider="openai", auth_mode=AuthMode.NATIVE_ACCOUNT, environ=environ)


def _write_codex_auth_json(record, environ, payload: dict) -> None:
    directory = registry.connection_state_dir(record.id, record.engine, environ)
    path = directory / "auth.json"
    registry.atomic_write_private(path, json.dumps(payload).encode("utf-8"), directory=directory)


# --- codex-acp: id_token JWT payload claim ---------------------------------

def test_codex_resolves_email_from_isolated_auth_json(tmp_path, monkeypatch):
    environ = _isolate(tmp_path, monkeypatch)
    record = _native_codex_connection(environ)
    _write_codex_auth_json(record, environ, {
        "tokens": {
            "id_token": _fake_id_token(),
            "access_token": FAKE_ACCESS_TOKEN,
            "refresh_token": FAKE_REFRESH_TOKEN,
        }
    })
    identity = account_identity.resolve_account_identity(record, environ)
    assert identity == "igovet+2@gmail.com"


def test_codex_falls_back_to_none_when_no_credential_yet(tmp_path, monkeypatch):
    environ = _isolate(tmp_path, monkeypatch)
    record = _native_codex_connection(environ)  # never logged in: no auth.json
    assert account_identity.resolve_account_identity(record, environ) is None


def test_codex_falls_back_to_none_on_malformed_id_token(tmp_path, monkeypatch):
    environ = _isolate(tmp_path, monkeypatch)
    record = _native_codex_connection(environ)
    _write_codex_auth_json(record, environ, {"tokens": {"id_token": "not-a-jwt"}})
    assert account_identity.resolve_account_identity(record, environ) is None


def test_codex_falls_back_to_none_when_claim_missing(tmp_path, monkeypatch):
    environ = _isolate(tmp_path, monkeypatch)
    record = _native_codex_connection(environ)
    header = _b64url({"alg": "none"})
    payload = _b64url({"sub": "no-email-here"})
    signature = base64.urlsafe_b64encode(b"sig").rstrip(b"=").decode("ascii")
    _write_codex_auth_json(record, environ, {"tokens": {"id_token": f"{header}.{payload}.{signature}"}})
    assert account_identity.resolve_account_identity(record, environ) is None


def test_codex_falls_back_to_none_on_unreadable_permissions(tmp_path, monkeypatch):
    environ = _isolate(tmp_path, monkeypatch)
    record = _native_codex_connection(environ)
    directory = registry.connection_state_dir(record.id, record.engine, environ)
    path = directory / "auth.json"
    path.write_bytes(json.dumps({"tokens": {"id_token": _fake_id_token()}}).encode("utf-8"))
    os.chmod(path, 0o644)  # group/other-readable -- read_owner_file must refuse it
    assert account_identity.resolve_account_identity(record, environ) is None


# --- claude-agent-acp: bounded CLI call, own isolated CLAUDE_CONFIG_DIR ----

_FAKE_CLAUDE_SCRIPT = """#!/bin/sh
if [ "$1" = "auth" ] && [ "$2" = "status" ]; then
  printf '%s' '{{"loggedIn": true, "email": "{email}", "configDirectory": "'"$CLAUDE_CONFIG_DIR"'"}}'
  exit 0
fi
exit 1
"""

_FAKE_CLAUDE_SCRIPT_FAILS = """#!/bin/sh
exit 1
"""

_FAKE_CLAUDE_SCRIPT_HANGS = """#!/bin/sh
sleep 30
"""


def _install_fake_claude(tmp_path, monkeypatch, script_body, environ):
    bin_dir = tmp_path / "fakebin"
    bin_dir.mkdir(exist_ok=True)
    script = bin_dir / "claude"
    script.write_text(script_body)
    script.chmod(script.stat().st_mode | stat.S_IEXEC | stat.S_IXGRP | stat.S_IXOTH)
    new_path = f"{bin_dir}:{os.environ.get('PATH', '')}"
    monkeypatch.setenv("PATH", new_path)
    environ["PATH"] = new_path
    return environ


def _claude_connection(environ, *, name="claude-live"):
    return onboarding.create_connection(
        name=name, engine="claude-agent-acp", provider="anthropic", auth_mode=AuthMode.NATIVE_ACCOUNT, environ=environ)


def test_claude_resolves_email_from_own_isolated_config_dir(tmp_path, monkeypatch):
    environ = _isolate(tmp_path, monkeypatch)
    record = _claude_connection(environ)
    environ = _install_fake_claude(tmp_path, monkeypatch, _FAKE_CLAUDE_SCRIPT.format(email="igovet@gmail.com"), environ)
    identity = account_identity.resolve_account_identity(record, environ)
    assert identity == "igovet@gmail.com"


def test_claude_falls_back_to_none_when_binary_missing(tmp_path, monkeypatch):
    environ = _isolate(tmp_path, monkeypatch)
    record = _claude_connection(environ)
    monkeypatch.setenv("PATH", str(tmp_path / "empty-bin"))
    environ["PATH"] = str(tmp_path / "empty-bin")
    assert account_identity.resolve_account_identity(record, environ) is None


def test_claude_falls_back_to_none_when_cli_fails(tmp_path, monkeypatch):
    environ = _isolate(tmp_path, monkeypatch)
    record = _claude_connection(environ)
    environ = _install_fake_claude(tmp_path, monkeypatch, _FAKE_CLAUDE_SCRIPT_FAILS, environ)
    assert account_identity.resolve_account_identity(record, environ) is None


def test_claude_falls_back_to_none_when_cli_hangs_past_the_timeout(tmp_path, monkeypatch):
    environ = _isolate(tmp_path, monkeypatch)
    record = _claude_connection(environ)
    environ = _install_fake_claude(tmp_path, monkeypatch, _FAKE_CLAUDE_SCRIPT_HANGS, environ)
    monkeypatch.setattr(account_identity, "_CLI_TIMEOUT_SECONDS", 0.3)
    assert account_identity.resolve_account_identity(record, environ) is None


# --- no secret substring ever leaks into a label, a log line, or an error -

def test_no_secret_substring_leaks_into_label_log_or_error(tmp_path, monkeypatch, caplog):
    environ = _isolate(tmp_path, monkeypatch)
    record = _native_codex_connection(environ)
    _write_codex_auth_json(record, environ, {
        "tokens": {
            "id_token": _fake_id_token(),
            "access_token": FAKE_ACCESS_TOKEN,
            "refresh_token": FAKE_REFRESH_TOKEN,
        }
    })
    secrets = (FAKE_ACCESS_TOKEN, FAKE_REFRESH_TOKEN)

    caplog.set_level(logging.DEBUG)
    identity = account_identity.resolve_account_identity(record, environ)
    label = build_provider_label(record.engine, identity)

    for secret in secrets:
        assert secret not in label
        assert secret not in caplog.text

    # Also exercise the deliberately-broken paths: a truncated/garbage
    # id_token still carrying the fake secrets nearby must never raise
    # (and if it somehow did, the secret must not be in the message).
    _write_codex_auth_json(record, environ, {
        "tokens": {"id_token": "garbage", "access_token": FAKE_ACCESS_TOKEN, "refresh_token": FAKE_REFRESH_TOKEN}
    })
    try:
        broken_identity = account_identity.resolve_account_identity(record, environ)
    except Exception as exc:  # pragma: no cover -- must never happen
        message = str(exc)
        for secret in secrets:
            assert secret not in message
        raise
    assert broken_identity is None
    for secret in secrets:
        assert secret not in caplog.text


def test_resolve_account_identity_never_raises_for_unsupported_engine(tmp_path, monkeypatch):
    environ = _isolate(tmp_path, monkeypatch)
    record = onboarding.create_connection(
        name="work", engine="codex-acp", provider="openai", auth_mode=AuthMode.API_KEY,
        key_kind="openai_api_key", environ=environ)
    record = onboarding.set_api_key(record.id, reader=lambda: b"fake-test-secret-key", environ=environ)
    # api_key mode: no id_token to decode, must gracefully fall back.
    assert account_identity.resolve_account_identity(record, environ) is None
