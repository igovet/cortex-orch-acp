"""Real runtime result emission and read-only replay, also consumed by the ACP bridge fixture."""
import json
import sys
import tempfile
import threading
from pathlib import Path

import pytest

# Also runnable by the no-paid plugin bridge test, from the checkout root.
sys.path[:0] = [str(Path(__file__).resolve().parents[1] / "src"), str(Path(__file__).resolve().parent)]
from cortex_runtime.acp import protocol
from cortex_runtime.acp.orchestrating_runtime import OrchestratingRuntime
from cortex_runtime.acp.replay_store import append_host_update, replay_session, terminal_worker_card_reconciliation
from cortex_runtime.acp.sessions import create_session, bind_task
from cortex_runtime.delegation.events import append_event
from test_runtime_gateway import _role_worker
from test_orchestrating_runtime import _make_runtime


def runtime_fixture(root, case):
    root.mkdir()
    store, worker, _ = _role_worker(root, "backend-dev")
    task = worker.task_id
    with store.connection() as db:
        ref = db.execute("SELECT idempotency_key FROM delegation_provisioning WHERE attempt_id=?",
                         (worker.attempt_id,)).fetchone()[0]
    failed = case.startswith("failed")
    cancelled = case == "cancelled"
    checks = [
        {"name": "read", "outcome": "failed"},
        {"name": "counts", "outcome": "not_run", "blocker": "read failed"},
        {"name": "total", "outcome": "not_run", "blocker": "counts unavailable"},
    ] if failed or case == "issues" else [
        {"name": "earlier", "outcome": "failed", "closed_by": "gateway:exact-later"},
        {"name": "rerun", "outcome": "passed"},
    ]
    if case.startswith("failed_cancel"):
        checks = checks[:1]
    if case == "issues":
        checks[0]["outcome"] = "unverified"
    result = {"summary": "DETAIL_BODY_MUST_STAY_IN_WORKER_TAB", "checks": checks,
              **({"assignment_outcome": "failed"} if failed else {})}
    with store.connection() as db:
        db.execute("insert into runtime_tasks(task_id,state,revision,goal,created_at,updated_at) values(?,?,?,?,?,?) "
                   "on conflict(task_id) do nothing", (task, "EXECUTING", 1, "Read", store._now(), store._now()))
        db.execute("UPDATE runtime_nodes SET status=?,result_json=? WHERE task_id=? AND assignment_ref=?",
                   ("running" if cancelled else "failed" if failed else "completed",
                    None if cancelled else json.dumps(result), task, ref))
        db.commit()
    if not cancelled:
        append_event(store, task, "worker_result", "result", result, origin="runtime",
                     assignment_ref=worker.attempt_id)
    runtime, _ = _make_runtime(store, lambda *_: None)
    create_session(store, "fixture", str(root), {}, {}, [])
    bind_task(store, "fixture", task)
    updates = []
    def emit(fields):
        fields = dict(fields)
        cid, new = fields.pop("tool_call_id"), fields.pop("new")
        frame = protocol.tool_call("fixture", cid, fields.pop("title"), fields.pop("kind"),
                                   fields.pop("status"), **fields) if new else protocol.tool_call_update("fixture", cid, **fields)
        update = frame["params"]["update"]
        updates.append(update)
        append_host_update(store, "fixture", update)
    runtime._worker_card_panel_mode[ref] = True
    runtime._worker_card_title[ref] = "Worker: backend-dev · codex · sol · medium — Read inventory"
    runtime._worker_card_emitters[ref] = emit
    try:
        runtime._emit_worker_card(emit, ref, status="pending" if case == "failed_cancel_early" else "in_progress")
        # The live defect: result published while worker turn unwinds. This
        # actual mirror used to hardcode completed, ignoring the failed node.
        if case == "failed_cancel_persist":
            # Reconnected runtime: no original emitter. Only the existing
            # owning session receives the correct persisted terminal update.
            runtime._worker_card_emitters.pop(ref)
            runtime._cancel_assignment(task, ref, "replanning")
            updates = [e["update"] for e in replay_session(store, "fixture") if e["kind"] == "update"]
        if case == "failed_cancel_early" or cancelled:
            # RECOVERING cancels every old assignment, even a failed node whose
            # result is committed but whose worker turn has not unwound yet.
            runtime._cancel_assignment(task, ref, "user stop" if cancelled else "replanning")
        if case == "failed_direct":
            runtime._emit_result_worker_card(task, emit, ref, result_payload=result)
        else:
            runtime._mirror_durable_worker_result_card(task, ref)
        if case == "failed_cancel_late":
            runtime._cancel_assignment(task, ref, "user stop")
        # A separately claimed cancellation event must obey the same evidence.
        from cortex_runtime.delegation.events import claim_unconsumed
        runtime._emit_assignment_tool_calls(task, emit, claim_unconsumed(store, task, "fixture"))
        history = [dict(kind="update", update=u) for u in updates]
        assert terminal_worker_card_reconciliation(store, task, history) == []
        # An old persisted clean Completed must be corrected from the node.
        old = history if cancelled else [dict(kind="update", update={**updates[0], "status": "completed"})]
        if case.startswith("failed_cancel"):
            # Actual pre-fix retained shape: pending -> ACP failed with the
            # cleanup title/string, no result metadata. Reconcile read-only.
            old = [history[0], dict(kind="update", update={
                "sessionUpdate": "tool_call_update", "toolCallId": "worker:" + ref,
                "status": "failed", "title": runtime._worker_card_title[ref] + " · Cancelled",
                "rawOutput": "Cancelled: replanning"})]
        repairs = terminal_worker_card_reconciliation(store, task, old)
        return {"live": updates, "replay": [*(e["update"] for e in old), *repairs], "ref": ref,
                "label": "cancelled" if cancelled else "failed" if failed else
                         "completed with issues" if case == "issues" else "completed",
                "issues": None if cancelled else len(checks) if failed or case == "issues" else 0}
    finally:
        runtime.shutdown()


@pytest.mark.parametrize("case,status,count", [("failed", "failed", 3), ("failed_direct", "failed", 3), ("issues", "completed", 3),
    ("clean", "completed", 0), ("failed_cancel_early", "failed", 1), ("failed_cancel_late", "failed", 1), ("failed_cancel_persist", "failed", 1)])
def test_runtime_result_and_legacy_replay_keep_committed_outcome(tmp_path, case, status, count):
    fixture = runtime_fixture(tmp_path / case, case)
    for key in ("live", "replay"):
        last = {}
        for update in fixture[key]:
            last.update(update)
        assert last["status"] == status
        assert last["rawOutput"] == {"cortexWorkerOutcome": {"status": status, "openIssues": count}}
        assert "DETAIL_BODY" not in json.dumps(last)
        assert ("Completed with issues" in last.get("title", "")) == (case == "issues")
        assert " · Cancelled" not in last.get("title", "")


def test_user_cancellation_without_a_result_stays_cancelled(tmp_path):
    fixture = runtime_fixture(tmp_path / "cancelled", "cancelled")
    for key in ("live", "replay"):
        state = {}
        for update in fixture[key]:
            state.update(update)
        assert state["status"] == "failed"  # ACP has no cancelled status.
        assert state["title"].endswith(" · Cancelled")
        assert "cortexWorkerOutcome" not in state.get("rawOutput", "")


@pytest.mark.parametrize("store_state", ["missing", "none"])
@pytest.mark.parametrize("has_title", [True, False])
@pytest.mark.parametrize("emission", ["result", "cancelled"])
def test_worker_card_emission_without_store_keeps_portable_fallback(store_state, has_title, emission):
    runtime = OrchestratingRuntime.__new__(OrchestratingRuntime)
    runtime._lock = threading.RLock()
    runtime._worker_action_log = {}
    runtime._worker_action_refs = {}
    runtime._worker_card_sent = {}
    runtime._worker_card_terminal = {}
    runtime._emitted_worker_cards = set()
    runtime._worker_final_response_seen = set()
    base = "Worker: backend-dev — Read inventory"
    runtime._worker_card_title = {"assignment": base} if has_title else {}
    if store_state == "none":
        runtime._store = None
    cards = []
    assert runtime._mirror_durable_worker_result_card(
        "task", "assignment", emit_tool_call=cards.append,
    ) is False
    assert cards == []
    if emission == "result":
        runtime._emit_result_worker_card("task", cards.append, "assignment", entry="Result: published")
    else:
        runtime._emit_cancelled_worker_card("task", cards.append, "assignment", "user stop")
    assert len(cards) == 1
    card = cards[0]
    assert card["new"] is True
    assert card["tool_call_id"] == "worker:assignment"
    assert card["status"] == ("completed" if emission == "result" else "failed")
    assert card["title"] == (
        base + (" · Cancelled" if emission == "cancelled" else "")
        if has_title else "Worker assignment"
    )
    # No store/result payload means no durable verdict or issue count to invent.
    assert "cortexWorkerOutcome" not in json.dumps(card)
    assert "openIssues" not in json.dumps(card)


@pytest.mark.parametrize("helper", ["_worker_card_outcome", "_mirror_durable_worker_result_card"])
def test_store_coding_errors_are_not_hidden_by_card_fallback(helper):
    class BrokenStore:
        def connection(self):
            raise AttributeError("store implementation bug")

    runtime = OrchestratingRuntime.__new__(OrchestratingRuntime)
    runtime._store = BrokenStore()
    runtime._lock = threading.RLock()
    runtime._worker_card_terminal = {}
    with pytest.raises(AttributeError, match="store implementation bug"):
        getattr(runtime, helper)("task", "assignment")


if __name__ == "__main__":
    with tempfile.TemporaryDirectory(prefix="cortex-outcomes-") as directory:
        print(json.dumps({case: runtime_fixture(Path(directory) / case, case)
                          for case in ("failed", "issues", "clean", "failed_cancel_early", "failed_cancel_late", "failed_cancel_persist", "cancelled")}))
