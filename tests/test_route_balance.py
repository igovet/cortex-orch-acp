"""Bounded audit history and parallel route reservation for balance ranking."""
from __future__ import annotations

import uuid
from collections import Counter
from concurrent.futures import ThreadPoolExecutor

from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.routing import balance
from cortex_runtime.routing import store as routing_store
from cortex_runtime.store import Store

from test_auto_route_delegation import _setup


def _store(tmp_path):
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    task = bootstrap_task(store, SourceEvent(origin="cli", event_id=str(uuid.uuid4())), "Work.", "owner")
    return store, task.task_id


def _audit(store, task_id, attempt_id, engine, tier, *, role="worker-backend-dev"):
    routing_store.record_attempt_audit(
        store, attempt_id=attempt_id, task_id=task_id, role=role, engine=engine,
        connection_id="cn_test", model="model", params_fingerprint="fp",
        config_revision_id=None, decision="authorized",
        detail={"route_decision": {"tier": tier}} if tier else {},
    )


def test_recent_counts_use_newest_worker_rows_and_selected_tier(tmp_path):
    store, task_id = _store(tmp_path)
    _audit(store, task_id, "old", "codex-acp", "standard")
    _audit(store, task_id, "deep", "claude-agent-acp", "deep")
    _audit(store, task_id, "coordinator", "claude-agent-acp", "standard", role="coordinator")
    _audit(store, task_id, "new-1", "claude-agent-acp", "standard")
    _audit(store, task_id, "new-2", "codex-acp", "standard")
    assert balance.recent_engine_attempts(store, "standard", window=2) == {
        "new-2": "codex-acp", "new-1": "claude-agent-acp",
    }
    assert balance.recent_engine_counts(store, "standard", window=2) == {
        "codex-acp": 1, "claude-agent-acp": 1,
    }
    assert balance.recent_engine_counts(store, "standard", scan=1) == {"codex-acp": 1}
    assert balance.recent_engine_counts(store, "deep") == {"claude-agent-acp": 1}


def test_parallel_route_reservations_count_pending_workers(tmp_path, monkeypatch):
    from cortex_runtime.orchestration import state

    store, channel, provisioning, adapter, *_rest = _setup(tmp_path)
    project = _rest[-2]
    monkeypatch.setattr(state, "attempt_routing", lambda *_: state.AttemptRouting(False, "normal", "auto", None, None))

    def resolve(index):
        launch = {
            "task_id": channel.task_id, "attempt_id": f"parallel-{index}",
            "assignment_id": f"parallel-assignment-{index}", "role_id": "worker-backend-dev",
        }
        adapter._resolve_runtime_route(launch, str(project))
        return launch["route_decision"]

    with ThreadPoolExecutor(max_workers=6) as pool:
        decisions = list(pool.map(resolve, range(6)))
    assert Counter(d["chosen"].split("/", 1)[0] for d in decisions) == {
        "codex-acp": 5, "claude-agent-acp": 1,
    }
    assert any(d["balance"]["inflight"] for d in decisions[1:])
    assert len(adapter._route_inflight) == 6
