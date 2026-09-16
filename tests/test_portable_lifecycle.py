import dataclasses
import inspect

import pytest

from cortex_runtime.portable import lifecycle
from cortex_runtime.portable.engine_contract import StopReason
from cortex_runtime.portable.errors import PortableError
from cortex_runtime.portable.lifecycle import (
    AcceptanceDecision,
    AssignmentCompletion,
    CancellationState,
    PublicationOutcome,
    PublicationStatus,
    ReconciliationRequired,
    ReleaseReason,
    TurnOutcome,
    accept_task,
    complete_assignment,
    confirm_cancellation,
    flag_uncertain_mutation,
    reconcile,
    release_resources,
    request_cancellation,
    turn_outcome_from_stop,
)


def test_turn_outcome_has_no_success_field():
    field_names = {item.name for item in dataclasses.fields(TurnOutcome)}
    assert "success" not in field_names
    outcome = turn_outcome_from_stop(StopReason.END_TURN, "es_" + "0" * 12)
    assert not hasattr(outcome, "success")


def test_publication_outcome_has_no_success_field():
    field_names = {item.name for item in dataclasses.fields(PublicationOutcome)}
    assert "success" not in field_names


def test_no_function_maps_turn_or_publication_outcome_to_task_success():
    """Scan lifecycle.py's public callables for any that accept a
    TurnOutcome/PublicationOutcome and return something resembling task
    success; none may exist."""
    for name, obj in vars(lifecycle).items():
        if name.startswith("_") or not inspect.isfunction(obj):
            continue
        try:
            signature = inspect.signature(obj)
        except (TypeError, ValueError):
            continue
        annotations = [str(parameter.annotation) for parameter in signature.parameters.values()]
        takes_outcome = any("TurnOutcome" in item or "PublicationOutcome" in item for item in annotations)
        assert not takes_outcome, f"{name} must not accept a TurnOutcome/PublicationOutcome"
    assert "success" not in dir(lifecycle) and not hasattr(lifecycle, "task_success_from_outcome")


def test_assignment_completion_requires_evidence():
    completion = complete_assignment("a_" + "0" * 12, ("ev_1",))
    assert isinstance(completion, AssignmentCompletion)

    with pytest.raises(PortableError) as excinfo:
        complete_assignment("a_" + "0" * 12, ())
    assert excinfo.value.code == "missing_evidence"


def test_publication_outcome_distinct_from_acceptance():
    published = PublicationOutcome(status=PublicationStatus.PUBLISHED, reference="r_" + "0" * 12)
    assert published.status == PublicationStatus.PUBLISHED

    with pytest.raises(PortableError):
        PublicationOutcome(status=PublicationStatus.PUBLISHED, reference=None)

    rejected = PublicationOutcome(status=PublicationStatus.REJECTED, reference=None, reason="malformed")
    assert rejected.reference is None


def test_task_acceptance_requires_coordinator_decision_and_evidence():
    accepted = accept_task("t_" + "0" * 12, AcceptanceDecision.ACCEPTED, ("r_" + "0" * 12,))
    assert accepted.decision == AcceptanceDecision.ACCEPTED

    with pytest.raises(PortableError) as excinfo:
        accept_task("t_" + "0" * 12, AcceptanceDecision.ACCEPTED, ())
    assert excinfo.value.code == "missing_evidence"


def test_cancellation_is_not_rollback_and_has_separate_states():
    requested = request_cancellation("es_" + "0" * 12)
    assert requested.state == CancellationState.CANCEL_REQUESTED
    confirmed = confirm_cancellation("es_" + "0" * 12)
    assert confirmed.state == CancellationState.CANCELLED
    assert requested.state != confirmed.state


def test_resource_release_is_separate_from_cancellation_and_session_close():
    release = release_resources("es_" + "0" * 12, ReleaseReason.SESSION_CLOSE)
    assert release.reason == ReleaseReason.SESSION_CLOSE
    # Releasing resources produces its own outcome type, never a
    # CancellationOutcome or TurnOutcome.
    assert not isinstance(release, (TurnOutcome,))


def test_uncertain_mutation_requires_reconciliation_before_retry():
    flagged = flag_uncertain_mutation("xa_" + "0" * 12, "write acknowledgement lost")
    assert isinstance(flagged, ReconciliationRequired)
    assert flagged.retry_eligible is False

    with pytest.raises(PortableError):
        ReconciliationRequired(subject_id="xa_" + "0" * 12, reason="x", retry_eligible=True)

    resolved = reconcile("xa_" + "0" * 12, "confirmed applied via replay")
    assert resolved.retry_eligible is True
