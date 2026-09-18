"""Single redaction choke point for anything written to logs/observations/
diagnostics (T029).

``engine.logs.EngineLogs`` (the JSONL observation sink and the bounded
stderr ring -- see that module) and ``gateway.diagnostics`` are the two
existing free-text sinks in this runtime; both route through the functions
below instead of formatting a raw string/mapping themselves. No prompt,
report, or source body is ever logged here (never call these on a content
body -- log a ref + digest + size instead, as every other durable evidence
path in this runtime already does).

Three independent redaction passes, applied to every string value:

1. URL userinfo (``scheme://user:pass@host`` -> ``scheme://[REDACTED]@host``).
2. ``Bearer``/``token``-prefixed credentials.
3. ``key=value``/``key: value`` pairs whose key name looks credential-shaped
   (reusing ``connections.environment.DENYLIST_PATTERN``'s vocabulary).
4. Any remaining long opaque token-shaped run (24+ base64/hex-alphabet
   characters, and containing at least one digit -- a plain English/
   snake_case identifier such as an error code or event name never
   matches, so "Keep ids, status, error category and counts" holds; this
   runtime's own short generated ids, e.g. ``t_``/``eirt_`` + 12 hex chars,
   are well under the 24-character floor) -- collapsed to a stable short
   digest so a repeated value is still recognizable as "the same secret"
   across log lines without ever exposing it.

A command line is never logged verbatim: :func:`scrub_command` keeps only
``argv[0]`` (the fixed, non-secret launch binary path) plus a digest and
count of the full argv.
"""
from __future__ import annotations

import hashlib
import re
from typing import Any, Mapping

from ..connections.environment import DENYLIST_PATTERN

MAX_TEXT_CHARS = 400
MAX_DEPTH = 3

_URL_CREDENTIALS = re.compile(r"([a-zA-Z][a-zA-Z0-9+.-]*://)[^/@\s]+@")
_BEARER = re.compile(r"(?i)\b(?:bearer|token)\s+[A-Za-z0-9._~+/=-]{8,}")
_KEY_VALUE_SECRET = re.compile(
    r"(?i)\b([A-Za-z0-9_]*(?:api[_-]?key|token|secret|password|authorization)[A-Za-z0-9_]*)\s*[=:]\s*\S+"
)
_LONG_OPAQUE_TOKEN = re.compile(r"\b(?=[A-Za-z0-9_\-]*\d)[A-Za-z0-9_\-]{24,}\b")


def _digest(value: str) -> str:
    return hashlib.sha256(value.encode("utf-8", errors="replace")).hexdigest()[:16]


def scrub_text(value: Any, *, max_chars: int = MAX_TEXT_CHARS) -> str:
    """Redact credential-shaped substrings from one free-text value and
    bound its length. Idempotent: scrubbing already-scrubbed text is a
    no-op (the redaction markers themselves are not secret-shaped)."""
    text = value if isinstance(value, str) else str(value)
    original_len = len(text)
    text = _URL_CREDENTIALS.sub(r"\1[REDACTED]@", text)
    text = _BEARER.sub("[REDACTED_TOKEN]", text)
    text = _KEY_VALUE_SECRET.sub(lambda m: f"{m.group(1)}=[REDACTED]", text)
    text = _LONG_OPAQUE_TOKEN.sub(lambda m: f"[REDACTED:{_digest(m.group(0))}]", text)
    if len(text) > max_chars:
        text = text[:max_chars] + f"...[truncated_from:{original_len}_chars]"
    return text


_PATHISH = re.compile(r"(?:[A-Za-z]:)?(?:/[^\s'\"]+)+")


def scrub_display_text(value: Any, *, max_chars: int = MAX_TEXT_CHARS) -> str:
    """:func:`scrub_text` for a string whose whole purpose is to be shown
    to the user who owns the project -- a tool call's title, for instance.

    Identical in every respect but one: an absolute filesystem path is
    left intact. ``_LONG_OPAQUE_TOKEN`` cannot tell a credential from a
    directory name, and a workspace directory is exactly the shape it
    matches, so a title reading "Read file '/project/.cortex/workspaces/
    coordinator_7093afe9d3ff3a61/work/calc.py'" came out with the path
    destroyed -- unreadable, and unusable as the link it exists to be.

    The narrowing is deliberate and bounded: a path in the user's own
    project, shown back to that same user, is not a credential, and the
    other three rules (URL userinfo, bearer tokens, key=value secrets)
    still apply to the path and to everything around it. Never use this
    for anything durable, exported, or sent anywhere but this user's own
    client -- that is what :func:`scrub_text` is for.
    """
    text = value if isinstance(value, str) else str(value)
    original_len = len(text)
    text = _URL_CREDENTIALS.sub(r"\1[REDACTED]@", text)
    text = _BEARER.sub("[REDACTED_TOKEN]", text)
    text = _KEY_VALUE_SECRET.sub(lambda m: f"{m.group(1)}=[REDACTED]", text)

    pieces: list[str] = []
    last = 0
    for match in _PATHISH.finditer(text):
        pieces.append(_LONG_OPAQUE_TOKEN.sub(lambda m: f"[REDACTED:{_digest(m.group(0))}]", text[last:match.start()]))
        pieces.append(match.group(0))
        last = match.end()
    pieces.append(_LONG_OPAQUE_TOKEN.sub(lambda m: f"[REDACTED:{_digest(m.group(0))}]", text[last:]))
    text = "".join(pieces)

    if len(text) > max_chars:
        text = text[:max_chars] + f"...[truncated_from:{original_len}_chars]"
    return text


def scrub_command(command: Any) -> dict:
    """Reduce a full ``argv`` list to a non-sensitive shape: the fixed
    launch binary path (``argv[0]``, never secret -- see
    ``engine.process.EngineProcess``, which refuses anything but an
    absolute, connection-pinned path) plus a digest and count of the
    complete argv (arguments, unlike argv[0], may carry connection-specific
    paths/flags and are never logged verbatim)."""
    if not isinstance(command, (list, tuple)) or not command:
        return dict(argv0=None, argv_hash=None, argc=0)
    argv0 = command[0] if isinstance(command[0], str) else str(command[0])
    joined = "\x1f".join(str(part) for part in command)
    return dict(argv0=argv0, argv_hash=_digest(joined), argc=len(command))


def scrub_url(value: Any) -> str:
    return scrub_text(value)


_COMMAND_KEYS = frozenset({"command", "argv", "launch_command"})


def scrub_mapping(fields: Mapping[str, Any], *, _depth: int = 0) -> dict:
    """Redact every value in a flat-ish field mapping before it is logged.

    IDs/status/error-category/count fields (short strings, ints, bools,
    None) pass through unchanged -- only free text and command lines are
    ever rewritten -- so diagnosis-relevant identity/outcome fields survive
    while secrets/private bodies never reach a log file.
    """
    if _depth > MAX_DEPTH:
        return dict(truncated_depth=True)
    result: dict[str, Any] = {}
    for key, value in fields.items():
        if key in _COMMAND_KEYS and isinstance(value, (list, tuple)):
            result[key] = scrub_command(value)
        elif isinstance(value, str):
            if DENYLIST_PATTERN.search(key):
                result[key] = "[REDACTED]"
            else:
                result[key] = scrub_text(value)
        elif isinstance(value, Mapping):
            result[key] = scrub_mapping(value, _depth=_depth + 1)
        elif isinstance(value, (list, tuple)):
            result[key] = [
                scrub_mapping(item, _depth=_depth + 1) if isinstance(item, Mapping)
                else scrub_text(item) if isinstance(item, str)
                else item
                for item in value
            ]
        else:
            result[key] = value
    return result
