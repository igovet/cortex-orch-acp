import json

import pytest

from cortex_runtime.store import Store
from cortex_runtime.delegation.work_graph import WorkGraph, WorkGraphError


def graph(tmp_path):
    store = Store(tmp_path / ".cortex")
    with store.connection() as db:
        db.execute("INSERT INTO tasks VALUES (?,?,?)", ("task-1", str(tmp_path), "2026-01-01T00:00:00Z"))
    return WorkGraph(store)


def test_requirements_are_revisioned_and_impact_is_queryable(tmp_path):
    g = graph(tmp_path)
    first = g.create_requirement("task-1", "build the API", criteria=["tests pass"], idempotency_key="r1")
    assert first["revision"] == 1
    assignment = g.add_assignment("task-1", "implementation", requirement_bindings={first["requirement_id"]: {"revision": 1}}, idempotency_key="a1")
    second = g.revise_requirement(first["requirement_id"], "build the API and document it", idempotency_key="r2")
    assert second["revision"] == 2
    assert g.requirement_impact("task-1", first["requirement_id"]) == [{
        "assignment_id": assignment["assignment_id"], "node_key": "implementation",
        "bound_revision": 1, "current_revision": 2,
    }]


def test_mutations_replay_exactly_and_conflicting_keys_fail(tmp_path):
    g = graph(tmp_path)
    result = g.create_requirement("task-1", "one", idempotency_key="same")
    with pytest.raises(WorkGraphError, match="idempotency_key_reused"):
        g.revise_requirement(result["requirement_id"], "two", idempotency_key="same")
    assert g.create_requirement("task-1", "one", idempotency_key="same") == result
    with pytest.raises(WorkGraphError, match="idempotency_request_changed"):
        g.create_requirement("task-1", "different", idempotency_key="same")


def test_predecessors_are_exact_and_cycles_are_rejected(tmp_path):
    g = graph(tmp_path)
    first = g.add_assignment("task-1", "research", idempotency_key="a1")
    with g.store.connection() as db:
        db.execute("CREATE TABLE candidate_staging (candidate_id TEXT PRIMARY KEY, task_id TEXT, status TEXT, content_digest TEXT)")
        db.execute("INSERT INTO candidate_staging VALUES ('c1','task-1','ready','digest')")
    g.bind_candidate(first["assignment_id"], "c1", idempotency_key="bind1")
    second = g.add_assignment("task-1", "implementation", predecessors=[{
        "assignment_id": first["assignment_id"], "candidate_ref": "candidate:c1"
    }], idempotency_key="a2")
    assert g.get_assignment(second["assignment_id"])["predecessors"] == [{
        "assignment_id": first["assignment_id"], "candidate_ref": "candidate:c1"
    }]
    with pytest.raises(WorkGraphError, match="inexact_predecessor_ref"):
        g.add_assignment("task-1", "bad", predecessors=[{"assignment_id": first["assignment_id"]}], idempotency_key="a3")
    with pytest.raises(WorkGraphError, match="unknown_predecessor"):
        g.add_assignment("task-1", "bad", predecessors=[{"assignment_id": "missing", "candidate_ref": "x"}], idempotency_key="a4")


def test_unknown_task_and_cross_task_predecessor_are_rejected(tmp_path):
    g = graph(tmp_path)
    with pytest.raises(WorkGraphError, match="unknown_task"):
        g.create_requirement("missing", "x", idempotency_key="r")
    with g.store.connection() as db:
        db.execute("INSERT INTO tasks VALUES (?,?,?)", ("task-2", str(tmp_path), "2026-01-01T00:00:00Z"))
    other = g.add_assignment("task-2", "other", idempotency_key="other")
    with pytest.raises(WorkGraphError, match="unknown_predecessor"):
        g.add_assignment("task-1", "bad", predecessors=[{"assignment_id": other["assignment_id"], "candidate_ref": "x"}], idempotency_key="cross")


def test_simple_assignment_is_one_node_without_forced_workflow(tmp_path):
    g = graph(tmp_path)
    result = g.add_assignment("task-1", "single-task", idempotency_key="single")
    assert result["predecessors"] == []
    assert result["status"] == "planned"


def test_attempts_are_durable_and_revision_requires_explicit_failure(tmp_path):
    g = graph(tmp_path)
    assignment = g.add_assignment("task-1", "implementation", idempotency_key="a")
    first = g.create_attempt(assignment["assignment_id"], idempotency_key="attempt-1")
    assert g.create_attempt(assignment["assignment_id"], idempotency_key="attempt-1") == first
    with pytest.raises(WorkGraphError, match="attempt_not_continuable"):
        g.create_attempt(assignment["assignment_id"], predecessor_attempt_id=first["attempt_id"], idempotency_key="attempt-2")
    g.fail_attempt(first["attempt_id"], "verification failed", idempotency_key="failure-1")
    assert g.get_assignment(assignment["assignment_id"])["status"] == "failed"
    second = g.create_attempt(assignment["assignment_id"], predecessor_attempt_id=first["attempt_id"], idempotency_key="attempt-2")
    assert second["attempt_no"] == 2
    assert g.get_assignment(assignment["assignment_id"])["status"] == "planned"
    with pytest.raises(WorkGraphError, match="attempt_not_continuable"):
        g.create_attempt(assignment["assignment_id"], predecessor_attempt_id=second["attempt_id"], idempotency_key="attempt-3")


def test_attempt_lifecycle_is_idempotent_and_report_is_not_acceptance(tmp_path):
    g = graph(tmp_path)
    assignment = g.add_assignment("task-1", "implementation", idempotency_key="a")
    attempt = g.create_attempt(assignment["assignment_id"], idempotency_key="attempt")
    running = g.transition_attempt(attempt["attempt_id"], "running", idempotency_key="running")
    assert g.transition_attempt(attempt["attempt_id"], "running", idempotency_key="running") == running
    assert g.get_assignment(assignment["assignment_id"])["status"] == "running"
    reported = g.transition_attempt(attempt["attempt_id"], "reported", idempotency_key="reported")
    assert reported["status"] == "reported"
    assert g.get_assignment(assignment["assignment_id"])["status"] == "reported"
    with pytest.raises(WorkGraphError, match="invalid_attempt_transition"):
        g.transition_attempt(attempt["attempt_id"], "failed", reason="late failure", idempotency_key="late")


def test_assignment_status_tracks_failure_and_a_continuation_attempt(tmp_path):
    g = graph(tmp_path)
    assignment = g.add_assignment("task-1", "implementation", idempotency_key="a")
    first = g.create_attempt(assignment["assignment_id"], idempotency_key="attempt-1")
    g.transition_attempt(first["attempt_id"], "running", idempotency_key="running-1")
    g.transition_attempt(
        first["attempt_id"], "failed", reason="cancelled",
        idempotency_key="failed-1",
    )
    assert g.get_assignment(assignment["assignment_id"])["status"] == "failed"

    second = g.create_attempt(
        assignment["assignment_id"], predecessor_attempt_id=first["attempt_id"],
        idempotency_key="attempt-2",
    )
    assert g.get_assignment(assignment["assignment_id"])["status"] == "planned"
    g.transition_attempt(second["attempt_id"], "running", idempotency_key="running-2")
    assert g.get_assignment(assignment["assignment_id"])["status"] == "running"
