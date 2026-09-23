"""T028: classify an observed engine/transport failure into one of a closed
set of durable classes, conservatively.

Every caller in this codebase that currently learns of a failed engine turn
(``acp.orchestrating_runtime.EngineTurnFailed.kind`` -- a short free-text
string from ``delegation.live_adapters``/``engine.session_manager``, itself
built from a JSON-RPC error message, an ``EngineEventKind.ERROR`` label, or
a raw process/transport signal) previously collapsed every one of those
into the single literal ``"engine_failure"`` before consulting T027's
routing/fallback gate (see ``routing.authorization.authorize_fallback`` and
its ``KNOWN_FALLBACK_ELIGIBLE_CLASSES``). That made a rate-limit response, an
auth-loss response, or a provider policy refusal from the *same* engine
process indistinguishable from a genuine engine crash -- and therefore, if
ever surfaced with the word "error" in it, silently fallback-eligible.

:func:`classify` is the single place that decides which of the eight T028
classes a failure belongs to, from bounded, conservative pattern matching
over the failure's own short kind/message text (never network access, never
unbounded regex backtracking -- every pattern here is a fixed substring
check). Unknown or ambiguous input is always ``INDETERMINATE``, never
silently ``ENGINE_FAILURE`` -- callers must never guess a class this module
was not confident about.

The three classes that must never be fallback-eligible
(``RATE_LIMITED``/``AUTHORIZATION_LOSS``/``PROVIDER_RESTRICTION``) are
deliberately *not* added to
``configuration.policy.KNOWN_FALLBACK_ELIGIBLE_CLASSES`` (only
``ENGINE_FAILURE``/``CAPACITY_UNAVAILABLE`` are members of that frozenset);
``routing.authorization.authorize_fallback`` already refuses any class
outside that set unconditionally (see its own docstring) -- passing this
module's classification result as the failure_class argument is therefore
enough to close the escalation gap, with no change needed to that gate
itself.
"""
from __future__ import annotations

# -- the closed class taxonomy (T028 lead decision 1) -----------------------
ENGINE_FAILURE = "engine_failure"
TRANSPORT_FAILURE = "transport_failure"
RATE_LIMITED = "rate_limited"
AUTHORIZATION_LOSS = "authorization_loss"
PROVIDER_RESTRICTION = "provider_restriction"
REFUSAL = "refusal"
CANCELLATION = "cancellation"
INDETERMINATE = "indeterminate"

ALL_CLASSES = frozenset({
    ENGINE_FAILURE, TRANSPORT_FAILURE, RATE_LIMITED, AUTHORIZATION_LOSS,
    PROVIDER_RESTRICTION, REFUSAL, CANCELLATION, INDETERMINATE,
})

# Never eligible for a T027 fallback attempt, regardless of what any
# fallback_policy allows -- mirrors configuration.policy.FALLBACK_HARD_DENYLIST
# for the classes this module can itself produce, plus the two additional
# classes T028 introduces that must equally never escalate.
NEVER_FALLBACK_ELIGIBLE = frozenset({
    RATE_LIMITED, AUTHORIZATION_LOSS, PROVIDER_RESTRICTION, REFUSAL, CANCELLATION, INDETERMINATE,
})

# Ordered (most specific first) bounded substring markers. Matching is
# case-insensitive and looks only at a short kind/message string (never at
# arbitrary engine stdout/stderr -- callers pass bounded, already-captured
# text). The first matching class wins; nothing here is a regex, so there is
# no backtracking-cost concern for an adversarial string.
_RATE_LIMIT_MARKERS = ("rate_limit", "rate limit", "too many requests", "429", "quota_exceeded", "quota exceeded",
                       "quota_exhausted")
_AUTH_LOSS_MARKERS = (
    "unauthorized", "auth_required", "auth required", "authentication_failed", "authentication failed",
    "token_expired", "token expired", "invalid_api_key", "invalid api key", "401", "403", "permission_denied_no_binding",
)
_PROVIDER_RESTRICTION_MARKERS = (
    "region_restricted", "region restricted", "model_not_available", "model not available", "model_unavailable",
    "provider_restricted", "provider restricted", "unsupported_region", "not_available_in_region",
)
_REFUSAL_MARKERS = ("refusal", "refused_by_model", "content_policy", "safety_refusal")
_CANCELLATION_MARKERS = ("cancelled", "canceled", "cancellation")
# ACP host-connection-level transport markers -- deliberately excludes
# "transport_eof"/"eof": this codebase already raises exactly that kind for
# a *downstream engine's own* transport closing (see
# engine.process.ProcessObservationKind.TRANSPORT_EOF /
# engine.session_manager's EngineEventKind.ERROR), which T027's existing
# behavior (and its own regression tests) already treats as a genuine
# ENGINE_FAILURE -- reachable-but-failed, not a distinct class. See
# _ENGINE_FAILURE_MARKERS below.
_TRANSPORT_MARKERS = ("broken_pipe", "broken pipe", "connection_reset", "connection reset", "writer_closed", "writer closed")
# Known, already-named engine-crash/unusable-session kinds this codebase
# itself raises (see delegation.live_adapters.EngineSessionManagerAdapter
# .prompt and engine.session_manager's own EngineEventKind.ERROR labels) --
# an explicit allowlist, not a catch-all, so an engine kind this module has
# never seen still falls through to INDETERMINATE rather than being assumed
# to be a genuine crash. ``transport_eof`` is included here (not in
# _TRANSPORT_MARKERS above) for the reason given there.
_ENGINE_FAILURE_MARKERS = (
    "poisoned", "timeout_alive", "engine_error", "process_exit", "nonzero_exit", "process_crashed", "transport_eof",
)


def _matches(text: str, markers: tuple[str, ...]) -> bool:
    lowered = text.lower()
    return any(marker in lowered for marker in markers)


def classify(kind: str | None, *, message: str | None = None, exit_code: int | None = None,
             signal: int | None = None) -> str:
    """Classify one observed failure into a member of :data:`ALL_CLASSES`.

    ``kind`` is the short machine-readable label already carried by this
    codebase's own failure objects (``EngineTurnFailed.kind``); ``message``
    is an optional additional short text (a JSON-RPC error message, ACP
    auth-required response body, or bounded stderr excerpt) consulted the
    same way. Conservative by construction: an input matching none of the
    known markers below -- including an empty/``None`` kind with no message
    -- is always :data:`INDETERMINATE`, never guessed as
    :data:`ENGINE_FAILURE`.

    A nonzero nonzero-process-exit / a delivered fatal signal (SIGKILL,
    SIGSEGV, ...) with no other identifying text is treated as
    :data:`ENGINE_FAILURE` -- an OS-level process death is exactly the kind
    of "engine itself failed while otherwise reachable" case T027's fallback
    gate is meant to cover; a *clean* exit (0) or no exit information at all
    never triggers this path.
    """
    haystack = " ".join(text for text in (kind, message) if text)
    if not haystack:
        if signal is not None or (exit_code is not None and exit_code != 0):
            return ENGINE_FAILURE
        return INDETERMINATE
    # Rate limit / auth loss / provider restriction / refusal / cancellation
    # are each checked before the generic engine-failure allowlist: a
    # message that happens to also contain the word "error" alongside an
    # explicit "rate_limit" marker must still classify as RATE_LIMITED, not
    # ENGINE_FAILURE.
    if _matches(haystack, _RATE_LIMIT_MARKERS):
        return RATE_LIMITED
    if _matches(haystack, _AUTH_LOSS_MARKERS):
        return AUTHORIZATION_LOSS
    if _matches(haystack, _PROVIDER_RESTRICTION_MARKERS):
        return PROVIDER_RESTRICTION
    if _matches(haystack, _REFUSAL_MARKERS):
        return REFUSAL
    if _matches(haystack, _CANCELLATION_MARKERS):
        return CANCELLATION
    if _matches(haystack, _TRANSPORT_MARKERS):
        return TRANSPORT_FAILURE
    if _matches(haystack, _ENGINE_FAILURE_MARKERS):
        return ENGINE_FAILURE
    if signal is not None or (exit_code is not None and exit_code != 0):
        return ENGINE_FAILURE
    return INDETERMINATE


# -- route unavailability (structured engine evidence only) -----------------
# What each pinned adapter itself puts in a JSON-RPC error's ``data`` when a
# turn or session is refused for a reason that is about the route (account,
# model), not the work. Read from the installed adapters, never guessed:
# codex-acp 1.13.0 sends ``data.codexErrorInfo`` (Codex's own enum, e.g.
# ``usageLimitExceeded`` for "You've hit your usage limit"), claude-agent-acp
# 0.81.0 sends ``data.errorKind`` (the Claude SDK's own error kind). Only
# these exact values count; free text is never matched, so an unrelated
# error can never read as an exhausted account.
QUOTA_EXHAUSTED = "quota_exhausted"
ROUTE_RATE_LIMITED = "rate_limited"
MODEL_UNAVAILABLE = "model_unavailable"

_CODEX_ROUTE_ERRORS = {
    "usageLimitExceeded": (QUOTA_EXHAUSTED, "connection"),
    "rateLimitExceeded": (ROUTE_RATE_LIMITED, "connection"),
}
_CLAUDE_ROUTE_ERRORS = {
    "rate_limit": (ROUTE_RATE_LIMITED, "connection"),
    "billing_error": (QUOTA_EXHAUSTED, "connection"),
    "account_on_hold": (QUOTA_EXHAUSTED, "connection"),
    "model_not_found": (MODEL_UNAVAILABLE, "model"),
}


def route_failure(error: BaseException) -> tuple[str, str, str] | None:
    """``(reason, scope, evidence)`` when ``error`` is an engine's own
    structured statement that this route cannot serve the work right now,
    else ``None``. ``scope`` is ``"connection"`` (the whole account) or
    ``"model"`` (this model on this account); ``evidence`` names the exact
    field and value that decided it (no engine prose, no credential)."""
    data = getattr(error, "data", None)
    if not isinstance(data, dict):
        return None
    codex = data.get("codexErrorInfo")
    if isinstance(codex, str) and codex in _CODEX_ROUTE_ERRORS:
        return (*_CODEX_ROUTE_ERRORS[codex], f"codexErrorInfo={codex}")
    kind = data.get("errorKind")
    if isinstance(kind, str) and kind in _CLAUDE_ROUTE_ERRORS:
        return (*_CLAUDE_ROUTE_ERRORS[kind], f"errorKind={kind}")
    return None


def is_fallback_eligible(failure_class: str) -> bool:
    """True only for :data:`ENGINE_FAILURE` -- the one class this module can
    itself produce that T027's ``KNOWN_FALLBACK_ELIGIBLE_CLASSES`` accepts
    (``CAPACITY_UNAVAILABLE``, the other member, is never produced by this
    classifier -- it is raised directly by ``delegation.provisioning`` from
    a capacity-limit observation, never from an engine failure)."""
    return failure_class == ENGINE_FAILURE
