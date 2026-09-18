"""Portable lifecycle validators.

Every function here is a small, independent pure validator: it accepts
plain arguments and returns a plain outcome dataclass, or raises
``PortableError``.  There is intentionally no workflow/orchestrator class
in this module and no function anywhere in this package that maps a
``TurnOutcome`` or ``PublicationOutcome`` to overall task success -- that
mapping is a coordinator decision, never a derived/automatic one (see
``tests/test_portable_lifecycle.py`` for an explicit check that no such
function exists).
"""
from __future__ import annotations

from dataclasses import dataclass, fields
from enum import Enum

from .engine_contract import StopReason
from .errors import PortableError, PortableErrorCategory


@dataclass(frozen=True)
class TurnOutcome:
    """The bounded, observed outcome of one engine turn.

    Deliberately has no ``success`` field: a turn ending is a fact about the
    engine's stream, not a judgment about whether the assignment succeeded.
    """

    stop_reason: StopReason
    engine_session_id: str
    context_generation_id: str | None = None

    def __post_init__(self) -> None:
        if not isinstance(self.stop_reason, StopReason):
            raise PortableError(
                category=PortableErrorCategory.VALIDATION,
                code="invalid_field",
                remediation="'stop_reason' must be a StopReason member.",
                retry_eligible=False,
                field="stop_reason",
            )
        if not isinstance(self.engine_session_id, str) or not self.engine_session_id:
            raise PortableError(
                category=PortableErrorCategory.VALIDATION,
                code="invalid_field",
                remediation="'engine_session_id' must be a non-empty string.",
                retry_eligible=False,
                field="engine_session_id",
            )


def turn_outcome_from_stop(stop_reason: StopReason, engine_session_id: str, context_generation_id: str | None = None) -> TurnOutcome:
    """Build a TurnOutcome; the only supported way to record a turn ending."""
    return TurnOutcome(stop_reason=stop_reason, engine_session_id=engine_session_id, context_generation_id=context_generation_id)


@dataclass(frozen=True)
class AssignmentCompletion:
    """An assignment completion claim, always carrying non-empty evidence."""

    assignment_id: str
    evidence_refs: tuple[str, ...]

    def __post_init__(self) -> None:
        if not isinstance(self.assignment_id, str) or not self.assignment_id:
            raise PortableError(
                category=PortableErrorCategory.VALIDATION,
                code="invalid_field",
                remediation="'assignment_id' must be a non-empty string.",
                retry_eligible=False,
                field="assignment_id",
            )
        if not isinstance(self.evidence_refs, tuple) or not self.evidence_refs or not all(isinstance(item, str) and item for item in self.evidence_refs):
            raise PortableError(
                category=PortableErrorCategory.VALIDATION,
                code="missing_evidence",
                remediation="Assignment completion requires at least one non-empty evidence reference.",
                retry_eligible=False,
                field="evidence_refs",
            )


def complete_assignment(assignment_id: str, evidence_refs: tuple[str, ...]) -> AssignmentCompletion:
    """Record an assignment completion; fails closed without evidence."""
    return AssignmentCompletion(assignment_id=assignment_id, evidence_refs=tuple(evidence_refs))


class PublicationStatus(str, Enum):
    PUBLISHED = "published"
    REJECTED = "rejected"


@dataclass(frozen=True)
class PublicationOutcome:
    """The outcome of publishing an artifact/report; distinct from acceptance."""

    status: PublicationStatus
    reference: str | None
    reason: str | None = None

    def __post_init__(self) -> None:
        if not isinstance(self.status, PublicationStatus):
            raise PortableError(
                category=PortableErrorCategory.VALIDATION,
                code="invalid_field",
                remediation="'status' must be a PublicationStatus member.",
                retry_eligible=False,
                field="status",
            )
        if self.status == PublicationStatus.PUBLISHED and not self.reference:
            raise PortableError(
                category=PortableErrorCategory.VALIDATION,
                code="missing_publication_reference",
                remediation="A 'published' outcome requires a non-empty reference.",
                retry_eligible=False,
                field="reference",
            )
        if self.status == PublicationStatus.REJECTED and not self.reason:
            raise PortableError(
                category=PortableErrorCategory.VALIDATION,
                code="missing_rejection_reason",
                remediation="A 'rejected' outcome requires a non-empty reason.",
                retry_eligible=False,
                field="reason",
            )


class AcceptanceDecision(str, Enum):
    ACCEPTED = "accepted"
    REJECTED = "rejected"


@dataclass(frozen=True)
class TaskAcceptance:
    """A coordinator-owned acceptance decision, always evidence-backed.

    This is the only portable path to marking a task accepted; there is no
    automatic derivation from a ``TurnOutcome`` or ``PublicationOutcome``.
    """

    task_id: str
    decision: AcceptanceDecision
    evidence_refs: tuple[str, ...]

    def __post_init__(self) -> None:
        if not isinstance(self.task_id, str) or not self.task_id:
            raise PortableError(
                category=PortableErrorCategory.VALIDATION,
                code="invalid_field",
                remediation="'task_id' must be a non-empty string.",
                retry_eligible=False,
                field="task_id",
            )
        if not isinstance(self.decision, AcceptanceDecision):
            raise PortableError(
                category=PortableErrorCategory.VALIDATION,
                code="invalid_field",
                remediation="'decision' must be an AcceptanceDecision member.",
                retry_eligible=False,
                field="decision",
            )
        if not isinstance(self.evidence_refs, tuple) or not self.evidence_refs or not all(isinstance(item, str) and item for item in self.evidence_refs):
            raise PortableError(
                category=PortableErrorCategory.VALIDATION,
                code="missing_evidence",
                remediation="Task acceptance requires at least one non-empty coordinator-reviewed evidence reference.",
                retry_eligible=False,
                field="evidence_refs",
            )


def accept_task(task_id: str, decision: AcceptanceDecision, evidence_refs: tuple[str, ...]) -> TaskAcceptance:
    """The only portable function that records task acceptance.

    Callers pass an explicit coordinator ``decision``; this function never
    infers acceptance from a turn or publication outcome.
    """
    return TaskAcceptance(task_id=task_id, decision=decision, evidence_refs=tuple(evidence_refs))


class CancellationState(str, Enum):
    """Cancellation is a request-then-confirm state machine, not a rollback."""

    CANCEL_REQUESTED = "cancel_requested"
    CANCELLED = "cancelled"


@dataclass(frozen=True)
class CancellationOutcome:
    """Records cancellation progress. Resource release is tracked separately."""

    engine_session_id: str
    state: CancellationState

    def __post_init__(self) -> None:
        if not isinstance(self.engine_session_id, str) or not self.engine_session_id:
            raise PortableError(
                category=PortableErrorCategory.VALIDATION,
                code="invalid_field",
                remediation="'engine_session_id' must be a non-empty string.",
                retry_eligible=False,
                field="engine_session_id",
            )
        if not isinstance(self.state, CancellationState):
            raise PortableError(
                category=PortableErrorCategory.VALIDATION,
                code="invalid_field",
                remediation="'state' must be a CancellationState member.",
                retry_eligible=False,
                field="state",
            )


def request_cancellation(engine_session_id: str) -> CancellationOutcome:
    return CancellationOutcome(engine_session_id=engine_session_id, state=CancellationState.CANCEL_REQUESTED)


def confirm_cancellation(engine_session_id: str) -> CancellationOutcome:
    return CancellationOutcome(engine_session_id=engine_session_id, state=CancellationState.CANCELLED)


@dataclass(frozen=True)
class ReconciliationRequired:
    """An uncertain mutation's outcome; retry is not eligible until reconciled."""

    subject_id: str
    reason: str
    retry_eligible: bool = False

    def __post_init__(self) -> None:
        if not isinstance(self.subject_id, str) or not self.subject_id:
            raise PortableError(
                category=PortableErrorCategory.VALIDATION,
                code="invalid_field",
                remediation="'subject_id' must be a non-empty string.",
                retry_eligible=False,
                field="subject_id",
            )
        if not isinstance(self.reason, str) or not self.reason:
            raise PortableError(
                category=PortableErrorCategory.VALIDATION,
                code="invalid_field",
                remediation="'reason' must be a non-empty string.",
                retry_eligible=False,
                field="reason",
            )
        if self.retry_eligible:
            raise PortableError(
                category=PortableErrorCategory.VALIDATION,
                code="invalid_field",
                remediation="An uncertain mutation is never retry-eligible until explicitly reconciled.",
                retry_eligible=False,
                field="retry_eligible",
            )


def flag_uncertain_mutation(subject_id: str, reason: str) -> ReconciliationRequired:
    """The only portable path for an uncertain-outcome mutation."""
    return ReconciliationRequired(subject_id=subject_id, reason=reason)


@dataclass(frozen=True)
class Reconciled:
    """Marks a previously uncertain mutation resolved; retry may resume."""

    subject_id: str
    resolution: str
    retry_eligible: bool = True

    def __post_init__(self) -> None:
        if not isinstance(self.subject_id, str) or not self.subject_id:
            raise PortableError(
                category=PortableErrorCategory.VALIDATION,
                code="invalid_field",
                remediation="'subject_id' must be a non-empty string.",
                retry_eligible=False,
                field="subject_id",
            )
        if not isinstance(self.resolution, str) or not self.resolution:
            raise PortableError(
                category=PortableErrorCategory.VALIDATION,
                code="invalid_field",
                remediation="'resolution' must be a non-empty string.",
                retry_eligible=False,
                field="resolution",
            )


def reconcile(subject_id: str, resolution: str) -> Reconciled:
    return Reconciled(subject_id=subject_id, resolution=resolution)


class ReleaseReason(str, Enum):
    SESSION_CLOSE = "session_close"
    HISTORY_TRIM = "history_trim"
    EXPLICIT = "explicit"


@dataclass(frozen=True)
class ResourceRelease:
    """Release of engine-side resources; distinct from session close/history.

    A session close and a history retention decision each produce their own
    outcome type; this dataclass only ever records that resources tied to
    ``engine_session_id`` were released, and why.
    """

    engine_session_id: str
    reason: ReleaseReason

    def __post_init__(self) -> None:
        if not isinstance(self.engine_session_id, str) or not self.engine_session_id:
            raise PortableError(
                category=PortableErrorCategory.VALIDATION,
                code="invalid_field",
                remediation="'engine_session_id' must be a non-empty string.",
                retry_eligible=False,
                field="engine_session_id",
            )
        if not isinstance(self.reason, ReleaseReason):
            raise PortableError(
                category=PortableErrorCategory.VALIDATION,
                code="invalid_field",
                remediation="'reason' must be a ReleaseReason member.",
                retry_eligible=False,
                field="reason",
            )


def release_resources(engine_session_id: str, reason: ReleaseReason) -> ResourceRelease:
    return ResourceRelease(engine_session_id=engine_session_id, reason=reason)


# ---------------------------------------------------------------------------
# Explicit non-goal: no function maps a TurnOutcome/PublicationOutcome to
# task success. Keep this frozenset in sync with any dataclass fields above
# named "success"; tests/test_portable_lifecycle.py asserts it stays empty.
_OUTCOME_TYPES_FOR_SUCCESS_FIELD_CHECK = (TurnOutcome, PublicationOutcome)


def _assert_no_success_field() -> None:
    for outcome_type in _OUTCOME_TYPES_FOR_SUCCESS_FIELD_CHECK:
        for item in fields(outcome_type):
            if item.name == "success":
                raise AssertionError(f"{outcome_type.__name__} must not gain a 'success' field")


_assert_no_success_field()
