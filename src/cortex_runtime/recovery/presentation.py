"""Present role and assignment text without heuristic line deletion.

The former compact mode guessed which lines were requirements by keywords.
That could silently discard an unmarked user constraint. Budget overflow is
reported by the caller; this module always preserves the full text.
"""
from __future__ import annotations

from dataclasses import dataclass

from .token_budget import estimate_tokens_for_text

@dataclass(frozen=True)
class Presentation:
    mode: str  # "full" | "compact"
    role_text: str
    brief_text: str
    acceptance_text: str
    estimated_tokens: int


def _full_presentation(role_text: str, brief_text: str, acceptance_text: str) -> Presentation:
    combined = "\n".join((role_text, brief_text, acceptance_text))
    return Presentation(
        mode="full", role_text=role_text, brief_text=brief_text, acceptance_text=acceptance_text,
        estimated_tokens=estimate_tokens_for_text(combined),
    )


def select_presentation(
    *, role_text: str, brief_text: str, acceptance_text: str, remaining_budget_tokens: int,
) -> Presentation:
    """Return the full text; callers handle a budget deficit explicitly."""
    return _full_presentation(role_text, brief_text, acceptance_text)
