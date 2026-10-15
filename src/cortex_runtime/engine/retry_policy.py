"""T028: bounded retry policy for operations this runtime might repeat
after an uncertain outcome (lost reply, transport failure, crash).

Every existing retry-shaped path in this codebase already follows the
contract this module names explicitly:

- ``acp.orchestrating_runtime.OrchestratingRuntime._attempt_worker_route_fallback``
  reconciles the *old* attempt (T023's worker-lost primitives -- confirmed/
  ambiguous capacity release, channel revocation) *before* ever opening a
  replacement session on the fallback route; see its own docstring.
- ``recovery.worker_reconciliation.reconcile`` checks for an already-
  published ``worker_result`` event (the durable "did this already mutate
  externally" ledger) before writing any ``recovery_worker_lost`` failure,
  and re-checks it again *inside the same transaction* as that write (see
  ``_write_recovery_failure_if_still_lost``) so a result that lands
  concurrently always wins.
- ``delegation.deliveries``/``delegation.provisioning`` use an idempotency
  key (never a bare re-send) for every durable publication.

:func:`decide` is the single place a *new* retry-shaped call site should
consult to decide whether it may retry at all, named the same way those
existing call sites already behave: an operation that could not possibly
have mutated anything external yet (never dispatched, never sent) is always
safely repeatable; one that already may have (a prompt already sent, a
tool already executed, a result already published) requires the caller to
have positively reconciled the actual external outcome first (an explicit
``reconciled=True``) -- without that, this module always says "do not
retry, stay indeterminate", and the caller records an
:mod:`engine.failure_classification` ``INDETERMINATE`` outcome rather than
ever claiming exactly-once for a mutation whose fate it cannot actually
observe.

:func:`bounded_backoff` is a small, capped exponential backoff generator
(the same doubling-with-ceiling shape
``delegation.live_adapters._reserve_capacity_blocking`` already uses for
its own bounded capacity-queue wait) for the caller's own retry loop; this
module never sleeps or loops itself.
"""
from __future__ import annotations

from dataclasses import dataclass
from typing import Iterator

# Operations this runtime ever considers retrying, named for readability at
# call sites -- an arbitrary string is also accepted by :func:`decide` (this
# is documentation, not an enforced enum), but every current call site uses
# one of these.
CAPACITY_RESERVATION = "capacity_reservation"       # never mutates externally: reserve() is atomic and idempotent-safe
WORKER_SESSION_OPEN = "worker_session_open"         # opening a fresh engine session: no prior external effect yet
ENGINE_PROMPT_TURN = "engine_prompt_turn"           # a prompt already sent may have executed tools/produced output
PUBLICATION = "publication"                         # write_report / worker_result: an idempotency-keyed durable write

# Never safely repeatable without an explicit, positive reconciliation of
# the actual external outcome first (see module docstring).
_REQUIRES_RECONCILIATION = frozenset({ENGINE_PROMPT_TURN, PUBLICATION})

DEFAULT_MAX_ATTEMPTS = 3
DEFAULT_BACKOFF_START_SECONDS = 0.05
DEFAULT_BACKOFF_MAX_SECONDS = 2.0


@dataclass(frozen=True)
class RetryDecision:
    may_retry: bool
    reason: str


def decide(operation: str, *, attempt_number: int, max_attempts: int = DEFAULT_MAX_ATTEMPTS,
           reconciled: bool = False) -> RetryDecision:
    """Whether ``operation`` may be retried for its ``attempt_number``th
    attempt (1-based: the *next* attempt about to be made).

    ``reconciled`` must be ``True`` only when the caller has already
    positively confirmed (via the actual durable ledger/lookup_owned/
    publication-idempotency check appropriate to ``operation`` -- never
    guessed) that the previous attempt's external effect either definitely
    did not apply, or definitely did and its result is already durably
    captured (so a retry would be redundant, never a second application).
    An operation outside :data:`_REQUIRES_RECONCILIATION` never needs this
    (it could not have mutated anything external yet).
    """
    if attempt_number > max_attempts:
        return RetryDecision(False, "max_attempts_exhausted")
    if operation in _REQUIRES_RECONCILIATION and not reconciled:
        return RetryDecision(False, "reconciliation_required_but_not_performed")
    return RetryDecision(True, "safely_repeatable" if operation not in _REQUIRES_RECONCILIATION else "reconciled")


def bounded_backoff(*, start: float = DEFAULT_BACKOFF_START_SECONDS,
                     ceiling: float = DEFAULT_BACKOFF_MAX_SECONDS) -> Iterator[float]:
    """An unbounded generator of capped, doubling backoff delays (the
    caller bounds the number of iterations itself, e.g. via
    :data:`DEFAULT_MAX_ATTEMPTS`/``decide``'s own ``max_attempts``) --
    never sleeps itself."""
    delay = start
    while True:
        yield delay
        delay = min(delay * 2, ceiling)


# -- transient-failure retry window -----------------------------------------
# An engine that failed to start, or whose session died before it did
# anything, is retried without bothering the user: the chat shows each retry
# as it happens. The wait between retries doubles from the first delay but
# never exceeds RETRY_MAX_DELAY_SECONDS; the work stops only once the
# failures have kept repeating for RETRY_WINDOW_SECONDS. A failure more than
# that long after the previous one starts a new streak.
RETRY_FIRST_DELAY_SECONDS = 5.0
RETRY_MAX_DELAY_SECONDS = 60.0
RETRY_WINDOW_SECONDS = 600.0


@dataclass(frozen=True)
class RetryWindow:
    first_delay_seconds: float = RETRY_FIRST_DELAY_SECONDS
    max_delay_seconds: float = RETRY_MAX_DELAY_SECONDS
    window_seconds: float = RETRY_WINDOW_SECONDS


def default_window() -> RetryWindow:
    """The window the runtime uses, read from the module constants at call
    time (the test suite shortens them)."""
    return RetryWindow(RETRY_FIRST_DELAY_SECONDS, RETRY_MAX_DELAY_SECONDS, RETRY_WINDOW_SECONDS)


@dataclass(frozen=True)
class WindowDecision:
    may_retry: bool
    retry_number: int        # 1-based: the retry about to be made (or refused)
    delay_seconds: float     # wait before that retry; 0 when refused
    elapsed_seconds: float   # since the first failure of the streak


def streak_start(failure_times, window: RetryWindow = RetryWindow()) -> int:
    """Index in the ascending ``failure_times`` where the current streak
    begins: the earliest failure reached without a gap longer than the
    window."""
    start = len(failure_times) - 1
    while start > 0 and failure_times[start] - failure_times[start - 1] <= window.window_seconds:
        start -= 1
    return max(start, 0)


def windowed_retry(failure_times, now: float, window: RetryWindow = RetryWindow()) -> WindowDecision:
    """Whether to retry after the latest of ``failure_times`` (ascending,
    the latest failure included; any one clock, the same as ``now``)."""
    if not failure_times:
        return WindowDecision(True, 1, window.first_delay_seconds, 0.0)
    streak = failure_times[streak_start(failure_times, window):]
    retry_number = len(streak)
    elapsed = max(0.0, now - streak[0])
    if elapsed >= window.window_seconds:
        return WindowDecision(False, retry_number, 0.0, elapsed)
    delay = min(window.first_delay_seconds * 2 ** (retry_number - 1), window.max_delay_seconds)
    return WindowDecision(True, retry_number, delay, elapsed)
