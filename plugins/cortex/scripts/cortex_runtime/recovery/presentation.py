"""T024: a compact role/assignment presentation variant for a genuinely
constrained context *budget* -- selected purely by remaining budget
(:mod:`recovery.token_budget`), never by an assumed model "capability"
(task card line 36 / out-of-scope line 59: "Do not claim weak-model
qualification from a strong model with a small artificial window alone").
A small window is a budget fact about the *route actually resolved*; it is
never treated as evidence about what the underlying model can or cannot
reason about.

Two presentation modes:

- ``"full"`` -- the role's full source text plus the coordinator's full
  brief/acceptance text, unmodified (this is what every non-budget-limited
  turn already gets).
- ``"compact"`` -- every *critical* line is preserved **verbatim**
  (headings recognized as containing an exact constraint/rule: see
  :data:`_CRITICAL_MARKERS`), narrative/explanatory prose between critical
  lines is dropped, and the assignment's ``acceptance`` criteria are always
  kept verbatim in full (an acceptance criterion is itself a constraint --
  it is never summarized or reworded). This never invents, paraphrases, or
  drops a single exact rule/constraint line -- it only removes prose that
  carries no independently-checkable requirement.

Selection (:func:`select_presentation`) is a pure function of the
estimated token cost of the ``"full"`` presentation against the caller's
remaining budget (already computed by
:func:`recovery.token_budget.plan_budget`) -- never a lookup keyed by
engine/model name.
"""
from __future__ import annotations

from dataclasses import dataclass

from .token_budget import estimate_tokens_for_text

# A line is "critical" (kept verbatim in compact mode) when it starts with
# one of these case-insensitive markers, or is itself a Markdown list item
# (``-``/``*``/numbered) -- list items are exactly where a role/brief states
# discrete, individually-checkable rules/constraints in this codebase's
# authored role Markdown (see portable/roles fixtures and docs/acp).
_CRITICAL_MARKERS = (
    "must", "never", "always", "required", "constraint", "acceptance",
    "do not", "forbidden", "exactly", "only",
)


def _is_critical_line(line: str) -> bool:
    stripped = line.strip()
    if not stripped:
        return False
    lowered = stripped.lower()
    if stripped[0] in ("-", "*") or (stripped[0].isdigit() and "." in stripped[:4]):
        return True
    return any(marker in lowered for marker in _CRITICAL_MARKERS)


def _compact_text(text: str) -> str:
    kept = [line for line in text.splitlines() if _is_critical_line(line)]
    return "\n".join(kept)


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


def _compact_presentation(role_text: str, brief_text: str, acceptance_text: str) -> Presentation:
    compact_role = _compact_text(role_text)
    compact_brief = _compact_text(brief_text)
    # Acceptance criteria are never summarized/reworded -- always verbatim.
    combined = "\n".join((compact_role, compact_brief, acceptance_text))
    return Presentation(
        mode="compact", role_text=compact_role, brief_text=compact_brief, acceptance_text=acceptance_text,
        estimated_tokens=estimate_tokens_for_text(combined),
    )


def select_presentation(
    *, role_text: str, brief_text: str, acceptance_text: str, remaining_budget_tokens: int,
) -> Presentation:
    """Return the ``"full"`` presentation when it fits ``remaining_budget_tokens``,
    otherwise the ``"compact"`` one (which never drops a critical
    rule/constraint line or any acceptance text -- see module docstring).
    Selection is purely budget-driven, never keyed by engine/model name."""
    full = _full_presentation(role_text, brief_text, acceptance_text)
    if full.estimated_tokens <= remaining_budget_tokens:
        return full
    return _compact_presentation(role_text, brief_text, acceptance_text)
