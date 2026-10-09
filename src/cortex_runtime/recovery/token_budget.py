"""T024: one shared, conservative token-size estimator plus route-level
context-budget planning.

**Single estimator.** Every module in this codebase that needs to turn a
byte count into an approximate token count (:mod:`recovery.capsule`'s
``size_estimate``, this module's :func:`plan_budget`, and any adapter-facing
budget check) calls :func:`estimate_tokens` -- never a second, independently
tuned constant. The ratio (:data:`BYTES_PER_TOKEN_ESTIMATE`) is a
conservative, documented guess (mixed English/JSON text averages somewhat
more than 4 bytes/token; using a *smaller* divisor overestimates token
count, which is the safe direction for a budget check that must never
under-count how much space something will consume) and is always rounded
**up** (``ceil``), never down -- undercounting a real cost is the one
mistake a conservative estim­ate must never make. Every consumer of this
number labels it ``"estimated"`` (see :data:`ESTIMATED_BASIS`) and never
presents it as an exact tokenizer count. When an adapter reports exact
usage via a real ``usage_update`` event (``portable.engine_contract
.UsageBasis.ENGINE_REPORTED``/``OBSERVED``), callers must prefer that exact
figure over this estimate -- this module never overrides a real reported
count.

**Budget planning.** :func:`plan_budget` accounts for *every* consumer of a
route's context window together -- role instructions, tool schemas, the
running chat/continuation history, a recovery capsule, a reserve for the
next expected tool output, and a reserve for the model's own response --
never only the capsule text (lead decision 3 / lines 33-34 of the task
card). It returns one of three decisions:

- ``"fits"`` -- every mandatory component plus both reserves fits under the
  route's context-window threshold with margin to spare.
- ``"needs_reference_mode"`` -- large *evidence* (not a mandatory
  constraint) should be delivered as an addressable reference (a
  ``large_outputs`` read, a ``read_source``/``read_report`` ref) instead of
  inlined, and doing so is projected to make the total fit.
- ``"overflow"`` -- even after reference-mode substitution, mandatory
  content (role instructions, exact user constraints, the capsule's
  critical sections) alone would not fit. This is always reported with the
  *concrete* limit that was exceeded and never silently resolved by
  truncating a constraint or by Cortex itself deciding to narrow scope --
  see ``docs/features/summaries-and-budgets/index.md`` and INVARIANTS.md: "no runtime-
  owned scope decisions".
- ``"unverified_window"`` -- mandatory content exceeds only the labeled
  default window because the route's real window is unknown (no catalog
  value and no engine ``usage_update.size`` observed yet). The default is a
  Cortex guess, not a limit of the engine, so the caller must not transmit
  an oversized recovery package: it records an explicit retryable preflight
  block until a real window is available rather than allowing an engine-side
  context-overflow error.
"""
from __future__ import annotations

import math
from dataclasses import dataclass, field

# Conservative bytes-per-token divisor, shared by every estimator in this
# codebase (see module docstring). Smaller than a typical real tokenizer's
# average bytes/token for English text -- so this estimate is always an
# over-count, never an under-count, of true token consumption.
BYTES_PER_TOKEN_ESTIMATE = 3.5

ESTIMATED_BASIS = "estimated"
ENGINE_REPORTED_BASIS = "engine_reported"
OBSERVED_BASIS = "observed"

# Conservative, clearly-labeled default context window used only when no
# evidenced ``catalog.model.CatalogEntry.context_window_tokens`` (or other
# real route metadata) is available for the resolved route. Deliberately
# small: a fabricated large window is the dangerous direction (it would
# let real overflow go undetected), while an over-conservative small
# default only pushes borderline assignments toward `needs_reference_mode`
# or an explicit, honest `overflow` a bit earlier than a real large-window
# engine would actually require.
DEFAULT_CONTEXT_WINDOW_TOKENS = 32_000

# Conservative fixed reserves, also labeled "estimated" -- never claimed to
# be an exact accounting of what any specific engine/model will actually
# consume for its own response or a subsequent tool result.
DEFAULT_RESPONSE_RESERVE_TOKENS = 2_000
DEFAULT_TOOL_OUTPUT_RESERVE_TOKENS = 4_000

# Never plan to use the full window: this fraction is the highest total
# (mandatory + reserves) ever considered "fits" without margin.
SAFETY_MARGIN_FRACTION = 0.9


def estimate_tokens(byte_length: int) -> int:
    """Conservative, always-rounded-up token estimate for ``byte_length``
    bytes of mixed English/JSON text. See module docstring."""
    if byte_length <= 0:
        return 0
    return math.ceil(byte_length / BYTES_PER_TOKEN_ESTIMATE)


def estimate_tokens_for_text(text: str) -> int:
    return estimate_tokens(len(text.encode("utf-8")))


@dataclass(frozen=True)
class BudgetComponent:
    """One accounted piece of context, in estimated (or exact) tokens."""

    name: str
    tokens: int
    basis: str  # "estimated" | "engine_reported" | "observed"
    mandatory: bool = True

    def __post_init__(self) -> None:
        if self.tokens < 0:
            raise ValueError(f"{self.name}: tokens must be >= 0")
        if self.basis not in (ESTIMATED_BASIS, ENGINE_REPORTED_BASIS, OBSERVED_BASIS):
            raise ValueError(f"{self.name}: unknown basis {self.basis!r}")


@dataclass(frozen=True)
class BudgetDecision:
    decision: str  # "fits" | "needs_reference_mode" | "overflow" | "unverified_window"
    context_window_tokens: int
    context_window_basis: str
    total_tokens: int
    mandatory_tokens: int
    reference_candidate_tokens: int
    components: tuple[BudgetComponent, ...]
    limit_name: str | None = None
    message: str | None = None


def context_window_for_route(context_window_tokens: int | None) -> tuple[int, str]:
    """Return ``(tokens, basis)`` for a route's context window: the real
    evidenced value from catalog/route metadata when present, otherwise the
    conservative labeled default (never a value Cortex invents per-model)."""
    if context_window_tokens is not None:
        if not isinstance(context_window_tokens, int) or context_window_tokens <= 0:
            raise ValueError("context_window_tokens must be a positive integer")
        return context_window_tokens, ENGINE_REPORTED_BASIS
    return DEFAULT_CONTEXT_WINDOW_TOKENS, ESTIMATED_BASIS


def plan_budget(
    *,
    context_window_tokens: int | None,
    role_instructions_bytes: int,
    tool_schemas_bytes: int,
    chat_history_bytes: int,
    capsule_bytes: int,
    reference_candidate_bytes: int = 0,
    response_reserve_tokens: int | None = None,
    tool_output_reserve_tokens: int | None = None,
    exact_usage_tokens: int | None = None,
    exact_usage_basis: str | None = None,
) -> BudgetDecision:
    """Plan one route's context budget across every mandatory consumer plus
    both reserves (see module docstring). ``reference_candidate_bytes`` is
    the size of large, *non-mandatory* evidence (e.g. a big worker report or
    terminal capture) that could instead be delivered as an addressable
    reference (see :mod:`recovery.large_output_store`) rather than inlined;
    it is never counted toward the mandatory total.
    """
    window_tokens, window_basis = context_window_for_route(context_window_tokens)
    response_reserve = response_reserve_tokens if response_reserve_tokens is not None else DEFAULT_RESPONSE_RESERVE_TOKENS
    tool_reserve = tool_output_reserve_tokens if tool_output_reserve_tokens is not None else DEFAULT_TOOL_OUTPUT_RESERVE_TOKENS

    components = [
        BudgetComponent("role_instructions", estimate_tokens(role_instructions_bytes), ESTIMATED_BASIS, mandatory=True),
        BudgetComponent("tool_schemas", estimate_tokens(tool_schemas_bytes), ESTIMATED_BASIS, mandatory=True),
        BudgetComponent("capsule", estimate_tokens(capsule_bytes), ESTIMATED_BASIS, mandatory=True),
        BudgetComponent("response_reserve", response_reserve, ESTIMATED_BASIS, mandatory=True),
        BudgetComponent("expected_tool_output_reserve", tool_reserve, ESTIMATED_BASIS, mandatory=True),
    ]
    if exact_usage_tokens is not None:
        # A real usage_update supersedes the estimated chat/continuation
        # history component -- never averaged with, or overridden by, the
        # estimate (see module docstring: "never overrides a real reported
        # count").
        components.append(BudgetComponent(
            "chat_history", exact_usage_tokens, exact_usage_basis or ENGINE_REPORTED_BASIS, mandatory=True,
        ))
    else:
        components.append(BudgetComponent(
            "chat_history", estimate_tokens(chat_history_bytes), ESTIMATED_BASIS, mandatory=True,
        ))

    mandatory_tokens = sum(c.tokens for c in components if c.mandatory)
    reference_tokens = estimate_tokens(reference_candidate_bytes)
    total_tokens = mandatory_tokens + reference_tokens
    usable_window = int(window_tokens * SAFETY_MARGIN_FRACTION)

    if total_tokens <= usable_window:
        return BudgetDecision(
            decision="fits", context_window_tokens=window_tokens, context_window_basis=window_basis,
            total_tokens=total_tokens, mandatory_tokens=mandatory_tokens,
            reference_candidate_tokens=reference_tokens, components=tuple(components),
        )
    if mandatory_tokens <= usable_window:
        return BudgetDecision(
            decision="needs_reference_mode", context_window_tokens=window_tokens, context_window_basis=window_basis,
            total_tokens=total_tokens, mandatory_tokens=mandatory_tokens,
            reference_candidate_tokens=reference_tokens, components=tuple(components),
            limit_name="context_window_tokens",
            message=(
                f"Estimated total {total_tokens} tokens exceeds the usable budget "
                f"{usable_window} of context_window_tokens={window_tokens} ({window_basis}); "
                f"the {reference_tokens}-token evidence candidate should be delivered as an "
                "addressable reference (see recovery.large_output_store) instead of inlined."
            ),
        )
    if window_basis == ESTIMATED_BASIS:
        return BudgetDecision(
            decision="unverified_window", context_window_tokens=window_tokens, context_window_basis=window_basis,
            total_tokens=total_tokens, mandatory_tokens=mandatory_tokens,
            reference_candidate_tokens=reference_tokens, components=tuple(components),
            limit_name="context_window_tokens",
            message=(
                f"Mandatory content is estimated at {mandatory_tokens} tokens, above the default "
                f"budget {usable_window} of context_window_tokens={window_tokens} ({window_basis}); "
                "this route has not reported its real context window yet, so the fit is unverified."
            ),
        )
    return BudgetDecision(
        decision="overflow", context_window_tokens=window_tokens, context_window_basis=window_basis,
        total_tokens=total_tokens, mandatory_tokens=mandatory_tokens,
        reference_candidate_tokens=reference_tokens, components=tuple(components),
        limit_name="context_window_tokens",
        message=(
            f"Mandatory content alone is estimated at {mandatory_tokens} tokens, exceeding the usable "
            f"budget {usable_window} of context_window_tokens={window_tokens} ({window_basis}) even before "
            "any optional evidence. This is not resolvable by reference-mode or truncation: report the "
            "concrete limit to the coordinator and ask it to split the assignment or change the execution "
            "approach -- Cortex never silently discards a mandatory requirement or narrows scope on its own."
        ),
    )
