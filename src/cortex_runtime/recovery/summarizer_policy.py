"""T024: independent summarizer selection, adapter capability findings, and
the bounded-retry deterministic-fallback loop guard.

**Working model vs. summarizer are independent choices** (task card step 1
/ lead decision 1). A route's working model (``ExecutionRoute.model``) is
never conflated with a separate summarizer model: this module only ever
*exposes* a distinct summarizer configuration when a specific adapter has a
verifiable, static-dist-evidenced configuration path for one -- otherwise
it documents the summarization as engine-owned (the adapter/engine performs
its own internal compaction/summarization, opaque to Cortex) and callers
rely on the durable, derived T021 capsule instead of any model-authored
digest for exact state.

**Static adapter findings** (see :data:`ADAPTER_SUMMARIZER_SUPPORT`):

- ``codex-acp`` (pinned 1.12.0): no separate summarizer configuration
  option exists anywhere in its observed ``configOptions`` surface (T013's
  ``codex_option_mapping`` maps only ``model``/``reasoning_effort``/``mode``
  from *observed* ``configOptions`` -- never a ``summarizer`` entry; no such
  option is emitted by the bundled adapter's ``createSessionConfigOptions``
  per T013's static dist reading). Compaction is entirely engine-owned
  (codex-acp's own internal ``contextCompaction``, see
  ``delegation.generations`` module docstring) -- Cortex never selects or
  configures *how* it summarizes, only observes *that* it happened.
- ``claude-agent-acp`` (pinned 0.77.0/0.78.0): same conclusion -- no
  ``summarizer`` configOption is exposed by either pinned version's static
  dist (``dist/context-compaction.js``'s ``ContextCompactionLifecycle`` runs
  its own internal compaction with no separate summarizer selection
  surface). Engine-owned, same as codex-acp.

Because `portable.engine_contract.EngineCapability.SUMMARIZER_SELECTION`
exists as a *portable concept* (a future adapter could expose one) but is
wired to **no current adapter**, :func:`summarizer_capability` always
returns ``supported=False`` for both pinned engines today -- never
fabricated as available. ``configuration.resolution._DEPENDENT_SETTING_NAMES``
already lists ``"summarizer"`` as a recognized *dependent setting name* (so
an explicit caller-supplied value round-trips validation) but
``configuration/config_options.py`` deliberately never surfaces it as a
selectable UI option -- this module is the single source of truth for why.

**Bounded fallback loop guard.** :func:`resolve_summary` never retries an
unavailable/invalid summarizer more than :data:`MAX_RETRIES` times, and
never spends more than :data:`MAX_RETRY_TOKEN_BUDGET` estimated tokens
across all attempts (using the shared estimator from
:mod:`recovery.token_budget`) before giving up and returning the
deterministic, prose-free capsule fallback. It never escalates to a
different/more expensive route on its own (task card line 32 / lead
decision 2: "never escalate to another/more expensive route silently") --
the caller decides that, out of band, if it wants to.
"""
from __future__ import annotations

from dataclasses import dataclass
from typing import Any, Callable, Mapping

from . import summary_validation
from .token_budget import estimate_tokens_for_text

MAX_RETRIES = 2
MAX_RETRY_TOKEN_BUDGET = 4_000


@dataclass(frozen=True)
class AdapterSummarizerSupport:
    engine: str
    supported: bool
    reason: str


_ENGINE_OWNED_REASON = (
    "No distinct summarizer configuration option is exposed by this adapter's "
    "static dist code (see module docstring); summarization/compaction is "
    "entirely engine-owned. Cortex relies on the deterministic T021 recovery "
    "capsule for exact state, never on this engine's internal summarizer."
)

ADAPTER_SUMMARIZER_SUPPORT: dict[str, AdapterSummarizerSupport] = {
    "codex-acp": AdapterSummarizerSupport("codex-acp", False, _ENGINE_OWNED_REASON),
    "claude-agent-acp": AdapterSummarizerSupport("claude-agent-acp", False, _ENGINE_OWNED_REASON),
    "opencode": AdapterSummarizerSupport("opencode", False, _ENGINE_OWNED_REASON),
}

_UNKNOWN_ENGINE_REASON = (
    "Unknown engine: no static dist evidence has been reviewed for a separate "
    "summarizer configuration path; never assumed supported without evidence."
)


def summarizer_capability(engine: str) -> AdapterSummarizerSupport:
    """Static, evidence-based summarizer-configurability finding for
    ``engine``. Never returns ``supported=True`` without a real, cited
    adapter configuration path (see module docstring); an unrecognized
    engine name is conservatively ``supported=False``."""
    return ADAPTER_SUMMARIZER_SUPPORT.get(engine, AdapterSummarizerSupport(engine, False, _UNKNOWN_ENGINE_REASON))


@dataclass(frozen=True)
class SummaryResolution:
    """Result of :func:`resolve_summary`: either a validated semantic
    summary, or the deterministic prose-free fallback."""

    used_deterministic_fallback: bool
    summary: Mapping[str, Any] | None
    attempts: int
    reason: str | None = None
    escalated: bool = False  # always False -- see module docstring


def resolve_summary(
    produce_candidate: Callable[[], Any],
    *,
    known_refs: set[str] | frozenset[str] = frozenset(),
    authoritative: Mapping[str, Any] | None = None,
    max_retries: int = MAX_RETRIES,
    max_retry_token_budget: int = MAX_RETRY_TOKEN_BUDGET,
) -> SummaryResolution:
    """Attempt to obtain one valid semantic summary from ``produce_candidate``
    (a zero-arg callable returning raw candidate text/JSON, or raising any
    exception to signal "unavailable this attempt").

    Bounded by *both* an attempt count (``max_retries`` + the first attempt)
    and a cumulative estimated-token spend across every attempted candidate
    (``max_retry_token_budget``) -- whichever bound is hit first stops the
    loop. On exhaustion (or on the very first attempt already exceeding the
    per-call token budget) this returns the deterministic fallback
    (``used_deterministic_fallback=True``, ``summary=None``) -- never
    prose, never a partially-trusted candidate, and never a signal to try a
    different/more expensive route (``escalated`` is always ``False``).
    """
    spent_tokens = 0
    attempts = 0
    last_reason: str | None = None
    for attempt_number in range(max_retries + 1):
        if spent_tokens >= max_retry_token_budget:
            last_reason = "retry_token_budget_exhausted"
            break
        attempts = attempt_number + 1
        try:
            candidate = produce_candidate()
        except Exception as error:  # noqa: BLE001 - any producer failure is "unavailable"
            last_reason = f"summarizer_unavailable:{type(error).__name__}"
            continue
        candidate_text = candidate if isinstance(candidate, str) else str(candidate)
        spent_tokens += estimate_tokens_for_text(candidate_text)
        result = summary_validation.validate_summary(candidate, known_refs=known_refs, authoritative=authoritative)
        if result.valid:
            return SummaryResolution(used_deterministic_fallback=False, summary=result.summary, attempts=attempts)
        last_reason = f"summary_invalid:{','.join(result.violations) or 'unknown'}"
    else:
        last_reason = last_reason or "retries_exhausted"
    return SummaryResolution(
        used_deterministic_fallback=True, summary=None, attempts=attempts,
        reason=last_reason or "no_attempt_made",
    )
