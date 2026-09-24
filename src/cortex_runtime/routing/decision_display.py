"""Compact user-facing summaries of recorded worker route decisions."""

from __future__ import annotations

from collections.abc import Mapping
from typing import Any

from ..catalog.labels import engine_short_name

# A worker card title is "Worker: <role> · <route> — <title>"; Paseo clips
# it with an ellipsis. The compact route text never exceeds this many
# characters, so the decision stays readable on a card.
CARD_SUMMARY_MAX = 80
# A model name is never shortened below this many characters plus "…".
_MIN_MODEL_CHARS = 12


def route_decision_summary(
    engine: str | None, model: str | None, detail: Mapping[str, Any] | None,
    *, compact: bool = False,
) -> str | None:
    """Render only the recorded route decision fields safe for user-facing status.

    Shows engine, model, effort, tier, the hint status when a hint was
    given, what decided the choice (``by usage``) and the chosen plan's
    usage (``usage 42%``). The tier reads ``light→standard`` when the model's
    tier differs from the task's: a fallback because no engine had a usable
    model in the task's tier, or an older row's cross-tier stand-in.
    ``compact`` (worker cards) shortens an ignored hint's reason and caps the
    text at :data:`CARD_SUMMARY_MAX`; the full reason stays in
    ``pipeline.md`` and the route audit. Older audit rows' ``balance`` counts
    are never shown: how often an engine ran says nothing about fit."""

    if not engine:
        return None
    detail = detail if isinstance(detail, Mapping) else {}
    model_label = None
    if model and model != "unknown":
        model_name = detail.get("model_name")
        model_label = str(model_name) if model_name else f"{model} (alias)"
    parts = [engine_short_name(str(engine))]
    if model_label:
        parts.append(model_label)
    effort = detail.get("effort")
    if effort:
        parts.append(str(effort))

    decision = detail.get("route_decision")
    hint_index = None
    if isinstance(decision, Mapping):
        tier = decision.get("tier")
        if tier:
            parts.append(_tier_label(decision, str(tier)))
        hint = decision.get("model_hint_status")
        if hint and hint != "auto":
            hint_index = len(parts)
            hint = str(hint)
            parts.append(f"hint {hint.split(':', 1)[0] if compact else hint}")
        decided_by = decision.get("decided_by")
        if decided_by:
            parts.append(f"by {decided_by}")
        parts.append(f"usage {_chosen_usage(decision)}")
    line = " · ".join(parts)
    if not compact or len(line) <= CARD_SUMMARY_MAX:
        return line
    # Too long for a card: drop the alias marker, then the hint, then
    # shorten the model name, so what decided it and the usage stay visible;
    # clip only as the last resort.
    if model_label and model_label.endswith(" (alias)"):
        parts[1] = model_label[: -len(" (alias)")]
    if hint_index is not None and len(" · ".join(parts)) > CARD_SUMMARY_MAX:
        del parts[hint_index]
    overflow = len(" · ".join(parts)) - CARD_SUMMARY_MAX
    if model_label and overflow > 0 and len(parts[1]) - overflow - 1 >= _MIN_MODEL_CHARS:
        parts[1] = parts[1][: len(parts[1]) - overflow - 1] + "…"
    line = " · ".join(parts)
    return line if len(line) <= CARD_SUMMARY_MAX else line[: CARD_SUMMARY_MAX - 1] + "…"


def _tier_label(decision: Mapping[str, Any], tier: str) -> str:
    """``tier``, or ``task→model`` when the routed model's tier differs.

    Rank version 3 rows recorded a cross-tier stand-in as the task's
    ``tier`` plus ``cross_tier.from_tier``; newer rows never promote a task
    that way, so only a fallback tier differs from ``role_tier``."""
    cross_tier = decision.get("cross_tier")
    if decision.get("chosen_cross_tier") and isinstance(cross_tier, Mapping) and cross_tier.get("from_tier"):
        return f"{tier}→{cross_tier['from_tier']}"
    role_tier = decision.get("role_tier")
    if role_tier and str(role_tier) != tier:
        return f"{role_tier}→{tier}"
    return tier


def _chosen_usage(decision: Mapping[str, Any]) -> str:
    """The chosen route's plan usage as recorded with its candidate."""
    chosen = decision.get("chosen")
    connection = decision.get("chosen_connection_id")
    for candidate in decision.get("candidates") or ():
        if (isinstance(candidate, Mapping) and candidate.get("route") == chosen
                and candidate.get("connection_id") == connection):
            used = candidate.get("used_percent")
            if isinstance(used, (int, float)) and not isinstance(used, bool):
                return f"{used:.0f}%"
            break
    return "unknown"
