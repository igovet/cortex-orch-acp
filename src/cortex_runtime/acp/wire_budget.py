"""Size bounds for what Cortex streams to an ACP host.

Paseo 0.9.2 keeps every provider event of a session in memory until the
session closes, so every byte Cortex sends is a byte the host daemon retains.
The durable task history (task events, reports) keeps the complete record;
these helpers bound only the on-wire projection.  Truncation is always
explicit: the kept value carries a marker naming how much was dropped and
where the full record lives.
"""
from __future__ import annotations

import json
from typing import Any, Mapping

#: Last-resort cap for one serialized ``session/update``.
SESSION_UPDATE_MAX_BYTES = 32 * 1024

#: Fields that identify an update or its state; never trimmed or dropped.
_PROTECTED_KEYS = frozenset({"sessionUpdate", "toolCallId", "status", "kind", "messageId", "sessionId"})
_TITLE_MAX_BYTES = 1024
_SHRINK_LIMITS = (8192, 2048, 512, 128, 32)
_MAX_DEPTH = 12


def encoded_size(value: Any) -> int:
    """UTF-8 bytes of ``value`` as compact JSON (the journal encoding)."""
    return len(json.dumps(value, ensure_ascii=False, separators=(",", ":"), default=str).encode("utf-8"))


def truncation_marker(dropped_bytes: int) -> str:
    return f" … [truncated {dropped_bytes} bytes — full record in task history]"


def clip_text(text: str, max_bytes: int) -> str:
    """Keep at most ``max_bytes`` UTF-8 bytes of ``text``, marker included."""
    raw = text.encode("utf-8")
    if len(raw) <= max_bytes:
        return text
    marker = truncation_marker(len(raw))
    keep = max(0, max_bytes - len(marker.encode("utf-8")))
    kept = raw[:keep].decode("utf-8", "ignore")
    return kept + truncation_marker(len(raw) - len(kept.encode("utf-8")))


def _shrink(value: Any, limit: int, depth: int = 0) -> Any:
    if isinstance(value, str):
        return clip_text(value, limit)
    if depth >= _MAX_DEPTH:
        return "[nested data truncated — full record in task history]"
    items_kept = max(4, limit // 16)
    if isinstance(value, Mapping):
        keys = [key for key in value if isinstance(key, str)]
        result = {key: _shrink(value[key], limit, depth + 1) for key in keys[:items_kept]}
        if len(keys) > items_kept:
            result["…"] = f"[{len(keys) - items_kept} more fields truncated — full record in task history]"
        return result
    if isinstance(value, (list, tuple)):
        result = [_shrink(item, limit, depth + 1) for item in value[:items_kept]]
        if len(value) > items_kept:
            result.append(f"[{len(value) - items_kept} more items truncated — full record in task history]")
        return result
    return value


def bound_value(value: Any, max_bytes: int) -> Any:
    """Return ``value`` unchanged when it fits, else a same-shaped reduction.

    Long strings are clipped with a marker, long lists and wide mappings keep
    their head plus a marker entry.  Short identifying values (paths, ids)
    therefore survive.  A value that still does not fit becomes a clipped
    JSON preview string.
    """
    if encoded_size(value) <= max_bytes:
        return value
    for limit in _SHRINK_LIMITS:
        candidate = _shrink(value, limit)
        if encoded_size(candidate) <= max_bytes:
            return candidate
    preview = json.dumps(value, ensure_ascii=False, separators=(",", ":"), default=str)
    budget = max_bytes
    while budget > 0:
        candidate = clip_text(preview, budget)
        if encoded_size(candidate) <= max_bytes:
            return candidate
        budget -= max(64, budget // 8)
    return ""


def cap_session_update(update: Mapping[str, Any], max_bytes: int = SESSION_UPDATE_MAX_BYTES) -> Mapping[str, Any]:
    """Enforce the per-update hard cap; return ``update`` itself when it fits.

    Identity and state fields (``sessionUpdate``, ``toolCallId``, ``status``,
    ``kind``, ``messageId``) are kept verbatim and the title is only clipped,
    so a truncated update still moves its card to the right state.
    """
    if encoded_size(update) <= max_bytes:
        return update
    base = {key: value for key, value in update.items() if key in _PROTECTED_KEYS}
    if isinstance(update.get("title"), str):
        base["title"] = clip_text(update["title"], _TITLE_MAX_BYTES)
    others = {key: value for key, value in update.items() if key not in base}
    for limit in _SHRINK_LIMITS:
        candidate = {**base, **{key: _shrink(value, limit) for key, value in others.items()}}
        if encoded_size(candidate) <= max_bytes:
            return candidate
    marker = "[update truncated — full record in task history]"
    fallback = dict(base)
    content = update.get("content")
    if isinstance(content, Mapping):
        fallback["content"] = dict(type="text", text=marker)
    elif content is not None or update.get("sessionUpdate") in {"tool_call", "tool_call_update"}:
        fallback["content"] = [dict(type="content", content=dict(type="text", text=marker))]
    return fallback


# Coordinator and engine tool cards: a per-task total budget.  Every distinct
# call used to be its own retained card, so a coordinator making N calls made
# the host keep N cards.  The first COORDINATOR_CARD_BUDGET distinct calls of
# a task get normal delta cards; later calls are counted on one aggregate
# "further tool calls" card instead.  A failed call past the budget still
# gets its own card, up to COORDINATOR_ERROR_CARD_BUDGET of them.  The
# aggregate card is created at once; later updates wait at least
# COORDINATOR_FOLDED_MIN_INTERVAL_SECONDS, doubling after each update, except
# the one at turn end, and there are at most COORDINATOR_FOLDED_MAX_UPDATES
# per task, creates included.  The durable task history keeps every call.
COORDINATOR_CARD_BUDGET = 150
COORDINATOR_ERROR_CARD_BUDGET = 50
#: Serialized bytes of own coordinator cards after which new calls are
#: folded even before COORDINATOR_CARD_BUDGET is reached.
COORDINATOR_CARD_BYTES_BUDGET = 4 * 1024 * 1024
#: Updates one own card may send, create included.  Past the cap a
#: non-terminal change is held back and merged into the card's terminal
#: update, which is always sent once; nothing is sent after it.
COORDINATOR_CARD_MAX_UPDATES = 10
COORDINATOR_FOLDED_MIN_INTERVAL_SECONDS = 30.0
COORDINATOR_FOLDED_MAX_UPDATES = 20
#: Card-id prefix of every coordinator/engine tool card the budget covers.
COORDINATOR_CARD_PREFIX = "call:coordinator:"
#: Card-id prefix of the aggregate card (outside COORDINATOR_CARD_PREFIX).
FOLDED_CARD_PREFIX = "call:cortex-folded:"


def folded_card_fields(count: int, failed: int, *, final: bool) -> dict[str, Any]:
    """Title and small rawOutput of the aggregate card for ``count`` calls."""
    title = f"{count} further tool call{'' if count == 1 else 's'} not shown individually"
    if failed:
        title += f" ({failed} failed)"
    title += " · full list in task history"
    if final:
        title += " · count no longer updated"
    return dict(title=title, raw_output=dict(furtherToolCalls=count, failed=failed, final=final))


def fold_replayed_tool_cards(history, session_id: str):
    """Yield ``session/load`` history with the live per-task card bound applied.

    Journals written before the budget can hold thousands of coordinator
    cards.  Updates of the first ``COORDINATOR_CARD_BUDGET +
    COORDINATOR_ERROR_CARD_BUDGET`` distinct coordinator cards are replayed
    as recorded; later cards are counted onto one aggregate card appended at
    the end; it names no source sequence and carries ``folded_cards``, since
    it summarizes recorded cards rather than repeating one.  A journal
    written under the budget never exceeds that number, so it replays
    unchanged.  Worker cards, text and other updates pass
    through.  The durable rows are never rewritten.
    """
    limit = COORDINATOR_CARD_BUDGET + COORDINATOR_ERROR_CARD_BUDGET
    shown: set[str] = set()
    folded: set[str] = set()
    failed: set[str] = set()
    for entry in history:
        update = entry.get("update") if entry.get("kind") == "update" else None
        card_id = update.get("toolCallId") if isinstance(update, Mapping) else None
        if isinstance(card_id, str) and card_id.startswith(COORDINATOR_CARD_PREFIX) and card_id not in shown:
            if card_id in folded or len(shown) >= limit:
                folded.add(card_id)
                if update.get("status") == "failed":
                    failed.add(card_id)
                continue
            shown.add(card_id)
        yield entry
    if folded:
        fields = folded_card_fields(len(folded), len(failed), final=False)
        yield dict(kind="update", source_sequences=[], folded_cards=len(folded), update=dict(
            sessionUpdate="tool_call", toolCallId=f"{FOLDED_CARD_PREFIX}replay:{session_id}",
            title=fields["title"], kind="other", status="completed", rawOutput=fields["raw_output"],
        ))
