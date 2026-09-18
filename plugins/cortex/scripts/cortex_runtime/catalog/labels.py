"""Safe route labels: ``Engine · Connection name · Model``.

Never exposes an endpoint, credential, or other sensitive detail. Every
component is first Unicode-normalized (NFKC) and stripped of invisible
control/format characters (Unicode category ``Cf``, which covers bidi
override/isolate marks, zero-width joiners/non-joiners, the word joiner,
and the BOM) so a hidden-character bypass cannot slip past any check below.

``connection_name`` is validated with an **allowlist**, not a denylist: a
denylist of "URL/host/IP-shaped" patterns is an open-ended, always-catchable
game (see the F3 review round, which found IPv6 zone ids, mapped IPv4
addresses, spaced colons, and a bare Unicode combining mark all slipping
past an earlier denylist). Only Unicode letters/digits (categories ``L*``/
``N*``), a plain space, the fixed punctuation set ``- _ ( ) # + & , '``, and
a colon immediately followed by a space (so "Team: research" is a label but
"host:8080" is not) are accepted; a combining mark (category ``M*``) is only
accepted directly after a non-ASCII base character (so scripts such as
Devanagari keep working) and never at the start or stacked onto an ASCII
base (which is exactly how a hidden combining character can forge a
lookalike hostname). Anything else -- including ``.``, ``/``, ``\\``, ``@``,
``%``, ``[``, ``]``, ``=``, ``?``, ``*``, and every other symbol -- is
rejected outright (``unsafe_connection_label``) rather than sanitized and
displayed, because a truncated/escaped endpoint is still a leaked endpoint.
A handful of denylist-style checks (URL scheme, bare/bracketed IPv6,
hostname-shaped, host:port, punycode) are kept purely as redundant defense
in depth; the allowlist above is what actually enforces the invariant.

``engine``/``model`` are never security boundaries in the same sense (they
are adapter- and catalog-supplied, not a user-typed connection name), so
they are only sanitized, never rejected: invisible/control characters and a
combining mark stacked on an ASCII base are stripped, and the result is
length-capped. A model id containing a literal dot (e.g. "model-1.5") is
legitimate and stays intact.

Equal labels within one snapshot are disambiguated by appending a short
suffix derived from the route id, never by silently merging or dropping an
entry.
"""
from __future__ import annotations

import re
import unicodedata
from typing import Iterable

from ..portable.errors import PortableError, PortableErrorCategory

_SEPARATOR = " · "
_MAX_COMPONENT = 64
_MAX_TOTAL = 200
_ELLIPSIS = "…"

_WHITESPACE = re.compile(r"\s+")

# --- redundant denylist-style checks (defense in depth only) ---------------
_IPV4 = re.compile(r"^\d{1,3}(\.\d{1,3}){3}$")
_HOSTNAME = re.compile(
    r"^(?=.{1,253}$)[A-Za-z0-9]([A-Za-z0-9-]{0,61}[A-Za-z0-9])?"
    r"(\.[A-Za-z0-9]([A-Za-z0-9-]{0,61}[A-Za-z0-9])?){1,}$"
)
_IPV6_BARE = re.compile(r"^[0-9A-Fa-f]{0,4}(:[0-9A-Fa-f]{0,4}){2,7}$")
_IPV6_BRACKETED = re.compile(r"^\[[0-9A-Fa-f:]+\](:\d{1,5})?$")
_HOST_PORT = re.compile(r"^[A-Za-z0-9]([A-Za-z0-9.-]{0,251}[A-Za-z0-9])?:\d{2,5}(/\S*)?$")
_PUNYCODE_LABEL = re.compile(r"(^|\.)xn--", re.IGNORECASE)

# --- allowlist (the real gate) ----------------------------------------------
_ALLOWED_PUNCTUATION = frozenset("-_()#+&,'")

# T035 (F4 review round): the ``provider`` control's own label is
# ``<short engine name> · <connection name>`` -- never the raw adapter
# package id (``claude-agent-acp``/``codex-acp``), which is an
# implementation detail the user never typed and should not have to read;
# the *connection name* is what actually tells them which account is in
# use, so it is never abbreviated. This is the one place a new engine's
# short name is added; ``build_connection_label`` is the only caller. An
# engine id with no entry here falls back to itself unchanged -- never a
# fabricated guess -- so nothing breaks before this map is updated.
ENGINE_SHORT_NAMES: dict[str, str] = {
    "claude-agent-acp": "claude",
    "codex-acp": "codex",
}


def engine_short_name(engine: str) -> str:
    """The short, user-facing engine name (e.g. ``claude``, ``codex``) for
    display -- never used for engine identity/storage/routing, which always
    keeps the full adapter package id (see :data:`ENGINE_SHORT_NAMES`)."""
    return ENGINE_SHORT_NAMES.get(engine, engine)


def _unsafe_label(field_name: str, value: str) -> PortableError:
    return PortableError(
        category=PortableErrorCategory.VALIDATION,
        code="unsafe_connection_label",
        remediation=(
            "A connection's display name may only contain letters, digits, "
            "spaces, and the punctuation - _ ( ) # + & , ' (plus a colon "
            "immediately followed by a space); it must never be, or "
            "resemble, a URL, host, IPv4/IPv6 address (including zone ids "
            "or mapped forms), host:port pair, punycode/internationalized "
            "domain label, or embedded credential. Rename the connection to "
            "a plain human label instead."
        ),
        retry_eligible=False,
        field=field_name,
    )


def _normalize(value: str) -> str:
    """NFKC-normalize, then drop C0/C1 controls and Unicode ``Cf`` (format,
    including bidi override/isolate marks, zero-width joiners, word joiner,
    and BOM) characters -- always applied before any safety check or
    sanitization so a hidden/lookalike character can never bypass either."""
    normalized = unicodedata.normalize("NFKC", value)
    kept = []
    for char in normalized:
        code = ord(char)
        if code <= 0x1F or 0x7F <= code <= 0x9F:
            continue
        if unicodedata.category(char) == "Cf":
            continue
        kept.append(char)
    return "".join(kept)


def _strip_stray_combining_marks(value: str) -> str:
    """Drop a combining mark (category ``M*``) at the start of ``value`` or
    stacked directly onto an ASCII base character; keep one following a
    non-ASCII base (needed for scripts such as Devanagari)."""
    result: list[str] = []
    for char in value:
        if unicodedata.category(char)[0] == "M":
            previous = result[-1] if result else None
            if previous is None or ord(previous) < 128:
                continue
        result.append(char)
    return "".join(result)


def _connection_name_allowed(cleaned: str) -> bool:
    length = len(cleaned)
    for index, char in enumerate(cleaned):
        category = unicodedata.category(char)
        if category[0] == "M":
            if index == 0:
                return False
            if ord(cleaned[index - 1]) < 128:
                return False
            continue
        if char == ":":
            if index + 1 >= length or cleaned[index + 1] != " ":
                return False
            continue
        if category[0] in ("L", "N"):
            continue
        if char == " ":
            continue
        if char in _ALLOWED_PUNCTUATION:
            continue
        return False
    return True


def assert_safe_connection_name(name: str) -> None:
    if not isinstance(name, str) or not name:
        raise _unsafe_label("connection_name", name)
    normalized = _normalize(name)
    stripped = normalized.strip()
    if not stripped:
        raise _unsafe_label("connection_name", name)
    # Redundant denylist-style checks (defense in depth only): the allowlist
    # below is what actually enforces the invariant, but these catch a few
    # shapes at the full-string level before the per-character scan.
    if "://" in normalized or "@" in normalized:
        raise _unsafe_label("connection_name", name)
    if _IPV4.match(stripped) or _HOSTNAME.match(stripped):
        raise _unsafe_label("connection_name", name)
    if _IPV6_BARE.match(stripped) or _IPV6_BRACKETED.match(stripped):
        raise _unsafe_label("connection_name", name)
    if _HOST_PORT.match(stripped):
        raise _unsafe_label("connection_name", name)
    if _PUNYCODE_LABEL.search(stripped):
        raise _unsafe_label("connection_name", name)
    if not _connection_name_allowed(normalized):
        raise _unsafe_label("connection_name", name)


def _sanitize_component(value: str, field_name: str) -> str:
    if not isinstance(value, str) or not value:
        raise PortableError(
            category=PortableErrorCategory.VALIDATION,
            code="invalid_field",
            remediation=f"'{field_name}' must be a non-empty string.",
            retry_eligible=False,
            field=field_name,
        )
    cleaned = _normalize(value)
    cleaned = _strip_stray_combining_marks(cleaned)
    cleaned = _WHITESPACE.sub(" ", cleaned).strip()
    if not cleaned:
        raise PortableError(
            category=PortableErrorCategory.VALIDATION,
            code="invalid_field",
            remediation=f"'{field_name}' has no displayable characters after sanitization.",
            retry_eligible=False,
            field=field_name,
        )
    if len(cleaned) > _MAX_COMPONENT:
        cleaned = cleaned[: _MAX_COMPONENT - 1].rstrip() + _ELLIPSIS
    return cleaned


def build_connection_label(engine: str, connection_name: str) -> str:
    """``<short engine name> · Connection name`` -- the ``provider``
    control's own label (T035; F4): the short, user-facing engine name
    (:func:`engine_short_name`, e.g. ``claude``/``codex``), never the raw
    adapter package id -- same sanitization/safety rules as
    :func:`build_label`, just without a model component."""
    assert_safe_connection_name(connection_name)
    engine_component = _sanitize_component(engine_short_name(engine), "engine")
    name_component = _sanitize_component(connection_name, "connection_name")
    label = _SEPARATOR.join((engine_component, name_component))
    if len(label) > _MAX_TOTAL:
        label = label[: _MAX_TOTAL - 1].rstrip() + _ELLIPSIS
    return label


def build_model_label(model: str) -> str:
    """Just ``Model`` -- the ``model`` control's own label (T035): the
    ``provider`` control already shows ``Engine · Connection name``, so the
    ``model`` control must not repeat that prefix (repeating it made every
    entry render identically once the host truncated the shared prefix --
    see FINDING-04). Sanitized/length-capped the same as ``build_label``'s
    model component."""
    return _sanitize_component(model, "model")


def build_label(engine: str, connection_name: str, model: str) -> str:
    """``Engine · Connection name · Model``, sanitized and length-bounded."""
    assert_safe_connection_name(connection_name)
    engine_component = _sanitize_component(engine, "engine")
    name_component = _sanitize_component(connection_name, "connection_name")
    model_component = _sanitize_component(model, "model")
    label = _SEPARATOR.join((engine_component, name_component, model_component))
    if len(label) > _MAX_TOTAL:
        label = label[: _MAX_TOTAL - 1].rstrip() + _ELLIPSIS
    return label


def disambiguate_labels(entries: Iterable[tuple[str, str]]) -> dict[str, str]:
    """Map ``route_id -> label``, appending a route-id suffix on collision.

    ``entries`` is an iterable of ``(route_id, label)`` pairs (one per
    catalog entry, in any order). Only labels that collide within this
    snapshot are suffixed; unique labels pass through unchanged.
    """
    pairs = list(entries)
    counts: dict[str, int] = {}
    for _, label in pairs:
        counts[label] = counts.get(label, 0) + 1
    result: dict[str, str] = {}
    for route_id, label in pairs:
        if counts[label] > 1:
            suffix = route_id[3:11] if route_id.startswith("xr_") else route_id[:8]
            result[route_id] = f"{label} (#{suffix})"
        else:
            result[route_id] = label
    return result
