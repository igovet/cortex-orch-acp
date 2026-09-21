import pytest

from cortex_runtime.delegation.context_packet import ContextPacketError, build_context_packet
from cortex_runtime.delegation.work_graph import WorkGraph
from cortex_runtime.store import Store


def _store(tmp_path):
    store = Store(tmp_path / ".cortex")
    with store.connection() as db:
        db.execute("INSERT INTO tasks VALUES (?,?,?)", ("task-1", str(tmp_path), "2026-01-01T00:00:00Z"))
    return store


def test_packet_contains_current_graph_and_data_trust_boundary(tmp_path):
    store = _store(tmp_path)
    graph = WorkGraph(store)
    req = graph.create_requirement("task-1", "ship the API", criteria=["tests pass"], idempotency_key="r")
    assignment = graph.add_assignment(
        "task-1", "implementation",
        requirement_bindings={req["requirement_id"]: {"revision": 1}}, idempotency_key="a",
    )
    attempt = graph.create_attempt(assignment["assignment_id"], idempotency_key="attempt")
    packet = build_context_packet(
        store, "task-1", attempt_id=attempt["attempt_id"],
        revision_feedback=["Fix the failing endpoint"],
        open_questions=[{"text": "Is the legacy route still required?", "source": "coordinator"}],
    )
    body = packet.as_dict()
    assert body["requirements"][0]["statement"] == "ship the API"
    assert body["assignment"]["assignment_id"] == assignment["assignment_id"]
    assert body["attempt"]["attempt_id"] == attempt["attempt_id"]
    assert body["revision_feedback"][0]["trust"] == "data"
    assert body["open_questions"][0]["authority"] == "evidence_only"
    assert body["trust_boundary"]["data_blocks"] == ["predecessor_reports", "revision_feedback", "open_questions"]
    assert packet.packet_digest == build_context_packet(store, "task-1", attempt_id=attempt["attempt_id"], revision_feedback=["Fix the failing endpoint"], open_questions=[{"text": "Is the legacy route still required?", "source": "coordinator"}]).packet_digest


def test_packet_rejects_cross_task_report_reference(tmp_path):
    store = _store(tmp_path)
    with store.connection() as db:
        db.execute("INSERT INTO tasks VALUES (?,?,?)", ("task-2", str(tmp_path), "2026-01-01T00:00:00Z"))
        db.execute("INSERT INTO reports VALUES (?,?,?,?,?)", ("r-other", "task-2", "report", "r-other.md", "2026-01-01T00:00:00Z"))
        db.execute("INSERT INTO editions VALUES (?,?,?,?,?,?,?,?)", (1, "r-other", "title", "summary", "worker", "2026-01-01T00:00:00Z", "digest", 12))
    with pytest.raises(ContextPacketError, match="report_task_mismatch"):
        build_context_packet(store, "task-1", selected_report_refs=["r-other"])


def test_packet_does_not_accept_unbound_candidate_path(tmp_path):
    store = _store(tmp_path)
    with pytest.raises(ContextPacketError, match="invalid_candidate_ref"):
        build_context_packet(store, "task-1", input_candidate_ref="/tmp/candidate")
