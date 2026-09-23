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
4. Any remaining long opaque alphanumeric run (24+ base64/hex-alphabet
   characters, containing at least one digit, and with no identifier
   separators) -- collapsed to a stable short digest.  Hyphenated and
   underscored names are ordinary project or business identifiers until an
   explicit credential form above identifies them as sensitive.  The marker
   calls the value a hidden secret rather than presenting a digest as an
   unexplained identifier.

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
_SECRET_CLAIM = re.compile(
    r"(?i)\b((?:api[_ -]?key|token|secret|password|authorization|credential)\s+(?:is|was))\s+\S+"
)
_LONG_OPAQUE_TOKEN = re.compile(r"\b(?=[A-Za-z0-9]*\d)[A-Za-z0-9]{24,}\b")
_LONG_LEXICAL_FIELD = re.compile(r"\S{4097,}")


class StreamingRedactor:
    """Redact a text stream without exposing a secret split across chunks.

    Text ending in a lexical word run is retained until a delimiter or the
    end of the stream.  This is the smallest useful holdback for the token
    and credential key/value expressions above: a chunk containing the first
    half of a credential therefore cannot be emitted before its second half
    arrives.  All released text is scrubbed as a complete unit.  An
    overlong lexical field is replaced by a deterministic bounded marker so
    a transport partition cannot change the result or grow the holdback.
    """

    _MAX_PENDING = 4096

    def __init__(self, *, max_chars: int = MAX_TEXT_CHARS) -> None:
        self.max_chars = max_chars
        self._pending = ""
        self._discarding = False
        self._overflowing = False
        self._overflow_hash = hashlib.sha256()
        self._overflow_len = 0
        self._overflow_allowed = True
        self._overflow_has_digit = False

    @staticmethod
    def _hold_start(text: str) -> int:
        # Keep the entire current whitespace-delimited lexical field. A
        # secret may be split at every possible character boundary, so no
        # prefix of that field is safe to emit until its delimiter arrives.
        # ``sha 256`` has an internal delimiter, so retain the partial label
        # (``sha 2``/``sha 25``) as well as the already handled ``sha `` form.
        partial_sha = re.search(
            r"(?i)(?:^|\s)sha(?:\s+2(?:5(?:6)?)?)?$", text,
        )
        if partial_sha:
            return partial_sha.start() + (
                1 if text[partial_sha.start():].startswith(" ") else 0
            )
        digest_prefix = re.search(
            r"(?is)(?:sha[- ]?256|hash|digest)\s*:\s*[a-f0-9]*$", text,
        )
        if digest_prefix:
            return digest_prefix.start()
        digest_field = re.search(
            r"(?is)(?:sha[- ]?256|hash|digest)\s*:\s*[a-f0-9]{64}\s*$", text,
        )
        if digest_field:
            return digest_field.start()
        digest_complete = re.search(
            r"(?is)(?:sha[- ]?256|hash|digest)\s*:\s*[a-f0-9]{64}\b", text,
        )
        if digest_complete:
            return digest_complete.start()
        if text and text[-1].isspace():
            stripped = text.rstrip()
            field = re.search(r"(?:^|\s)([^\s]+)$", stripped)
            if field:
                token = field.group(1)
                if (re.fullmatch(r"(?i)(?:bearer|token)", token)
                        or re.fullmatch(r"(?i)sha", token)
                        or re.search(r"(?i)(?:api[_-]?key|token|secret|password|authorization)\s*[=:]$", stripped)
                        or re.search(r"(?i)(?:sha[- ]?256|hash|digest)\s*:$", stripped)):
                    return field.start(1)
            return len(text)
        match = re.search(r"\s+[^\s]*$", text)
        if not match:
            start = 0
        else:
            start = match.start() + 1
        prefix = text[:start].rstrip()
        # Bearer/token credentials and SHA-256 claims use two lexical fields;
        # retain their prefix together with the value so the scrubber sees
        # the complete expression and accepted-final digest checks still
        # receive the exact candidate text.
        if re.search(r"(?i)(?:bearer|token)$", prefix):
            start = prefix.rfind(" ") + 1
        elif re.search(r"(?i)sha[- ]?256\s*:$", prefix):
            start = prefix.rfind(" ") + 1
        elif re.search(r"(?i)(?:hash|digest)\s*:$", prefix):
            start = prefix.rfind(" ") + 1
        elif re.search(
            r"(?i)(?:api[_-]?key|token|secret|password|authorization)\s*[=:][^\s]*$",
            prefix,
        ):
            start = prefix.rfind(" ") + 1
        return start

    def push(self, value: Any) -> str:
        text = value if isinstance(value, str) else str(value)
        if not text:
            return ""
        output: list[str] = []
        offset = 0
        while offset < len(text):
            if self._overflowing:
                boundary = re.search(r"\s", text[offset:])
                field = text[offset:] if boundary is None else text[offset:offset + boundary.start()]
                self._feed_overflow(field)
                offset += len(field)
                if boundary is None:
                    return "".join(output)
                output.append(self._finish_overflow())
                output.append(boundary.group(0))
                offset += boundary.end() - boundary.start()
                continue
            if self._discarding:
                # An overlong lexical field has already been represented by
                # one neutral marker.  Discard only through its next lexical
                # boundary, then resume normal processing for the remainder
                # of the same transport update.
                boundary = re.search(r"\s", text[offset:])
                if boundary is None:
                    return "".join(output)
                output.append(boundary.group(0))
                offset += boundary.end()
                self._discarding = False
                continue

            # Never append an entire provider update to the holdback.  A
            # single update may be megabytes long, while the only unsafe
            # suffix we need to retain is bounded by _MAX_PENDING.
            room = self._MAX_PENDING - len(self._pending)
            take = min(len(text) - offset, max(1, room + 1))
            self._pending += text[offset:offset + take]
            offset += take

            hold = self._hold_start(self._pending)
            if hold:
                released, self._pending = self._pending[:hold], self._pending[hold:]
                output.append(self._scrub_released(released))

            if len(self._pending) <= self._MAX_PENDING:
                continue

            # The retained suffix is one lexical field (or a labelled digest
            # claim) with no safe boundary yet.  Once it crosses the bound,
            # fail closed with one deterministic marker and ignore only that
            # field until its delimiter.  This makes a large one-shot update
            # and the same text split over many updates observationally equal.
            if re.search(r"\s", self._pending):
                output.append("[REDACTED_STREAM]")
                # A labelled digest or a compound credential expression is
                # deliberately failed closed as one bounded marker.  It is
                # not safe to hash a multi-field claim as if it were one
                # opaque token.
                trailing = re.search(r"\s+$", self._pending)
                if trailing:
                    output.append(trailing.group(0))
                self._discarding = trailing is None
                self._pending = ""
            else:
                self._start_overflow(self._pending)
                self._pending = ""
            continue
        return "".join(output)

    def flush(self, *, preserve_labeled_digests: bool = False) -> str:
        if self._overflowing:
            result = self._finish_overflow()
            self._overflowing = False
            self._pending = ""
            self._discarding = False
            return result
        pending, self._pending = self._pending, ""
        self._discarding = False
        return self._scrub(pending, preserve_labeled_digests=preserve_labeled_digests) if pending else ""

    def _start_overflow(self, value: str) -> None:
        self._overflowing = True
        self._overflow_hash = hashlib.sha256()
        self._overflow_len = 0
        self._overflow_allowed = True
        self._overflow_has_digit = False
        self._feed_overflow(value)

    def _feed_overflow(self, value: str) -> None:
        if not value:
            return
        self._overflow_hash.update(value.encode("utf-8", errors="replace"))
        self._overflow_len += len(value)
        self._overflow_allowed = (
            self._overflow_allowed
            and bool(re.fullmatch(r"[A-Za-z0-9_-]+", value))
        )
        self._overflow_has_digit = self._overflow_has_digit or any(char.isdigit() for char in value)

    def _finish_overflow(self) -> str:
        digest = self._overflow_hash.hexdigest()[:16]
        result = (
            f"[HIDDEN_SECRET:{digest}]"
            if self._overflow_len >= 24 and self._overflow_allowed and self._overflow_has_digit
            else "[REDACTED_STREAM]"
        )
        self._overflowing = False
        return result

    def _scrub(
        self,
        value: str,
        *,
        preserve_labeled_digests: bool = False,
        max_chars: int | None = None,
    ) -> str:
        """Scrub a released field while retaining a labelled SHA-256 value.

        A labelled digest is evidence for the accepted-final guard, not a
        credential. Keeping its exact 64 hex characters lets that guard make
        the comparison after streaming; unlabelled long numeric runs still
        use the ordinary opaque-token redaction rule.
        """
        labelled = re.match(
            r"(?is)^(.*?\b(?:sha[- ]?256|hash|digest)\s*:\s*)([a-f0-9]{64})(?=$|[\s.,)\]:])(.*)$",
            value,
        )
        if labelled and preserve_labeled_digests:
            return (
                scrub_text(labelled.group(1), max_chars=max_chars or self.max_chars)
                + labelled.group(2)
                + scrub_text(labelled.group(3) or "", max_chars=max_chars or self.max_chars)
            )
        return scrub_text(value, max_chars=max_chars or self.max_chars)

    def _scrub_released(self, value: str) -> str:
        """Scrub text that is known to end at a lexical boundary.

        A complete transport update can contain a lexical field much larger
        than the holdback.  Normalize those fields before ordinary redaction
        so the result does not depend on whether the provider split the same
        field across updates.  The substitution prevents a released prefix
        from becoming an unbounded retained object merely because its final
        delimiter happened to arrive in this update.
        """
        value = _LONG_LEXICAL_FIELD.sub("[REDACTED_STREAM]", value)
        return self._scrub(value, max_chars=max(self.max_chars, len(value) * 2 + 1000))


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
    text = _SECRET_CLAIM.sub(lambda m: f"{m.group(1)} [REDACTED]", text)
    text = _LONG_OPAQUE_TOKEN.sub(lambda m: f"[HIDDEN_SECRET:{_digest(m.group(0))}]", text)
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
    text = _SECRET_CLAIM.sub(lambda m: f"{m.group(1)} [REDACTED]", text)

    pieces: list[str] = []
    last = 0
    for match in _PATHISH.finditer(text):
        pieces.append(_LONG_OPAQUE_TOKEN.sub(lambda m: f"[HIDDEN_SECRET:{_digest(m.group(0))}]", text[last:match.start()]))
        pieces.append(match.group(0))
        last = match.end()
    pieces.append(_LONG_OPAQUE_TOKEN.sub(lambda m: f"[HIDDEN_SECRET:{_digest(m.group(0))}]", text[last:]))
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
            if (DENYLIST_PATTERN.search(key)
                    or re.search(r"(?i)(?:credential|api[_-]?key|token|secret|password|authorization)", key)):
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
