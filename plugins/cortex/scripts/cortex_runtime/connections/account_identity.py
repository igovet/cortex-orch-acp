"""Best-effort account identity for one connection's own display label.

T035 (account label task): the ``provider`` control's first selector must
show *which account* is authenticated, not the operator-chosen connection
nickname -- a nickname says nothing about whose subscription is billed. The
account identity is read from the connection's own isolated credential
directory (never the ambient user environment), and only ever an identity
*claim* (an email/display string) is returned -- never a token, never a
key, and never anything read while resolving it.

Security invariant, enforced by construction, not by convention: every
function here either returns ``str | None`` or raises nothing at all. Any
failure -- missing credential, unreadable file, malformed token, a CLI that
errors, times out, or hangs -- collapses to ``None`` inside a narrow
``try/except Exception`` around the one step that touches the secret, so a
credential value can never reach a caller, a log line, or an exception
message (including inside a traceback): the local name holding it never
outlives that except block, and nothing here calls a logger with it.
"""
from __future__ import annotations

import base64
import json
import os
import shutil
import subprocess

from . import environment, registry
from .model import ConnectionRecord

# Bounds every external CLI call this module makes: these binaries are not
# ours and must be assumed capable of hanging (per task instructions).
_CLI_TIMEOUT_SECONDS = 5.0

_CODEX_ENGINE = "codex-acp"
_CLAUDE_ENGINE = "claude-agent-acp"


def resolve_account_identity(record: ConnectionRecord, environ: dict[str, str] | None = None) -> str | None:
    """The signed-in account's email/display identity for ``record``, read
    from *its own* isolated credential directory, or ``None`` when it
    cannot be determined for any reason. Never raises."""
    try:
        if record.engine == _CODEX_ENGINE:
            return _codex_account_identity(record, environ)
        if record.engine == _CLAUDE_ENGINE:
            return _claude_account_identity(record, environ)
        return None
    except Exception:
        # Defense in depth: every path below already catches its own
        # failures narrowly, but nothing may ever escape this function.
        return None


def _codex_account_identity(record: ConnectionRecord, environ: dict[str, str] | None) -> str | None:
    try:
        directory = registry.connection_state_dir(record.id, record.engine, environ)
        raw = registry.read_owner_file(directory / "auth.json")
    except Exception:
        return None
    if raw is None:
        return None
    return _email_from_codex_auth_json(raw)


def _email_from_codex_auth_json(raw: bytes) -> str | None:
    """Decode the ``email`` claim out of ``tokens.id_token``'s JWT payload.

    Plain base64 decoding of the unverified middle segment -- this only
    ever displays an identity, it never claims to have verified a
    signature. ``raw``/the token/the decoded claims are local to this one
    function and are discarded (never logged, never re-raised) the moment
    a failure occurs.
    """
    try:
        auth = json.loads(raw)
        tokens = auth.get("tokens") if isinstance(auth, dict) else None
        id_token = tokens.get("id_token") if isinstance(tokens, dict) else None
        if not isinstance(id_token, str) or id_token.count(".") != 2:
            return None
        segment = id_token.split(".")[1]
        segment += "=" * (-len(segment) % 4)
        claims = json.loads(base64.urlsafe_b64decode(segment.encode("ascii")))
        email = claims.get("email") if isinstance(claims, dict) else None
        return email if isinstance(email, str) and email else None
    except Exception:
        # Never re-raise: a malformed/adversarial token must never surface
        # its own bytes (or a fragment of them) in a traceback.
        return None


def _claude_account_identity(record: ConnectionRecord, environ: dict[str, str] | None) -> str | None:
    base_environ = os.environ if environ is None else environ
    path_value = base_environ.get("PATH", "")
    binary = shutil.which("claude", path=path_value) if path_value else shutil.which("claude")
    if not binary:
        return None
    try:
        # This connection's own isolated CLAUDE_CONFIG_DIR, never the
        # ambient one -- build_process_env re-derives/re-verifies it fresh
        # from the registry rather than trusting the caller-held record.
        process_env = environment.build_process_env(record, {"PATH": path_value}, environ=environ)
    except Exception:
        return None
    try:
        result = subprocess.run(
            [binary, "auth", "status"],
            env=process_env,
            stdin=subprocess.DEVNULL,
            capture_output=True,
            text=True,
            timeout=_CLI_TIMEOUT_SECONDS,
            check=False,
        )
    except Exception:
        return None
    if result.returncode != 0:
        return None
    return _email_from_claude_auth_status(result.stdout)


def _email_from_claude_auth_status(stdout: str) -> str | None:
    try:
        payload = json.loads(stdout)
        email = payload.get("email") if isinstance(payload, dict) else None
        return email if isinstance(email, str) and email else None
    except Exception:
        return None
