"""T024: read_large_output MCP tool -- owner-scoped, chunked, and the real
``EngineSessionManagerAdapter`` persist-sink binding."""
from __future__ import annotations

import pytest

from cortex_runtime.contracts import StoreError
from cortex_runtime.delegation import contracts as delegation_contracts
from cortex_runtime.delegation import mcp_tools
from cortex_runtime.delegation.live_adapters import EngineSessionManagerAdapter
from cortex_runtime.recovery.large_output_store import persist_output
from cortex_runtime.store import Store

from test_recovery_capsule import _delegate, _provision_worker, _task


def test_read_large_output_is_advertised_for_both_roles():
    assert "read_large_output" in delegation_contracts.schemas_for("coordinator")
    assert "read_large_output" in delegation_contracts.schemas_for("worker")


def test_coordinator_reads_its_own_persisted_output(tmp_path):
    store, task_id, coordinator = _task(tmp_path)
    record = persist_output(
        store, task_id=task_id, owner_scope="coordinator", source_kind="terminal",
        data=b"coordinator terminal output", exit_code=0, signal=None, failure=None, truncated=False,
    )
    result = mcp_tools.dispatch(store, coordinator, "read_large_output", dict(large_output_id=record["id"]))
    assert result["chunk"] == "coordinator terminal output"
    assert result["exit_status"] == dict(exit_code=0, signal=None)


def test_worker_reads_its_own_assignment_scoped_output_never_coordinators(tmp_path):
    store, task_id, coordinator = _task(tmp_path)
    delegated = _delegate(store, coordinator)
    assignment_ref = delegated["assignment_ref"]
    worker_channel = _provision_worker(store, task_id, coordinator, assignment_ref)
    persist_output(
        store, task_id=task_id, owner_scope="coordinator", source_kind="terminal", data=b"coordinator-only",
    )
    worker_record = persist_output(
        store, task_id=task_id, owner_scope=assignment_ref, source_kind="terminal", data=b"worker own output",
    )
    result = mcp_tools.dispatch(store, worker_channel, "read_large_output", dict(large_output_id=worker_record["id"]))
    assert result["chunk"] == "worker own output"
    coordinator_record = persist_output(
        store, task_id=task_id, owner_scope="coordinator", source_kind="terminal", data=b"still coordinator-only",
    )
    with pytest.raises(StoreError, match="unknown_large_output_ref"):
        mcp_tools.dispatch(store, worker_channel, "read_large_output", dict(large_output_id=coordinator_record["id"]))


def test_unknown_large_output_id_is_refused_not_leaked(tmp_path):
    store, task_id, coordinator = _task(tmp_path)
    with pytest.raises(StoreError, match="unknown_large_output_ref"):
        mcp_tools.dispatch(store, coordinator, "read_large_output", dict(large_output_id="lo_" + "0" * 12))


# --- EngineSessionManagerAdapter real persist-sink binding ------------------


def test_adapter_persist_terminal_output_uses_recorded_session_meta(tmp_path):
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    adapter = EngineSessionManagerAdapter(lambda task_id: None, lambda task_id: "/tmp", store=store)
    adapter._session_meta["es_1"] = dict(
        task_id="t_" + "0" * 12, recipient="worker", assignment_ref="a_" + "1" * 12, attempt_id="xa_1",
    )
    # A real task_id is required by large_output_store's FK; create one.
    from cortex_runtime.bootstrap import bootstrap_task
    from cortex_runtime.portable.identity import SourceEvent
    result = bootstrap_task(store, SourceEvent(origin="cli", event_id="ev1"), "Build a page.", "owner-1")
    adapter._session_meta["es_1"]["task_id"] = result.task_id
    record_id = adapter._persist_terminal_output(
        "es_1", "term_1", b"worker terminal output", truncated=False, exit_code=0, signal=None, failure=None,
    )
    assert record_id is not None
    from cortex_runtime.recovery.large_output_store import read_output
    read = read_output(store, task_id=result.task_id, owner_scope="a_" + "1" * 12, record_id=record_id)
    assert read["chunk"] == "worker terminal output"


def test_adapter_persist_terminal_output_returns_none_without_recorded_session(tmp_path):
    store = Store(tmp_path / "private")
    adapter = EngineSessionManagerAdapter(lambda task_id: None, lambda task_id: "/tmp", store=store)
    record_id = adapter._persist_terminal_output(
        "es_unknown", "term_1", b"orphan output", truncated=False, exit_code=0, signal=None, failure=None,
    )
    assert record_id is None
