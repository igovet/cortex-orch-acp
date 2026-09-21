"""T035 FINDING-12 closure: ``EngineSessionManagerAdapter._notify_coordinator_refusal``
is the seam ``engine.permissions.PermissionBridge`` uses to get its refusal
reason from the wire-less permission bridge into something the coordinator
model actually reads (a "progress" task event, rendered as continuation
text the next time the coordinator's own session is prompted -- see
``acp.orchestrating_runtime._render_continuation_text``). Unit-level: no
real engine subprocess, no ACP wire traffic -- see
``tests/test_engine_permissions.py`` for the ``PermissionBridge`` side of
this and ``tests/test_delegation_routing_dispatch.py`` for real fake-engine-
subprocess coverage of the rest of this adapter.
"""
from __future__ import annotations

import json
import uuid

from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.delegation.live_adapters import EngineSessionManagerAdapter
from cortex_runtime.engine.permissions import COORDINATOR_EDIT_REFUSAL_REASON
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.store import Store


def _bootstrap(tmp_path):
    project = tmp_path / "project"
    project.mkdir()
    store = Store(project / ".cortex", project_root=project)
    result = bootstrap_task(store, SourceEvent(origin="cli", event_id=str(uuid.uuid4())), "Do work.", "owner-1")
    return store, result.task_id


def _adapter(store):
    return EngineSessionManagerAdapter(
        connection_record_factory=lambda task_id: None,
        workspace_root_factory=lambda task_id: "/does/not/matter",
        store=store,
    )


def test_notify_coordinator_refusal_appends_a_progress_event(tmp_path):
    store, task_id = _bootstrap(tmp_path)
    adapter = _adapter(store)
    adapter._session_meta["es_a"] = dict(task_id=task_id, attempt_id="co_1", recipient="coordinator")

    adapter._notify_coordinator_refusal("es_a", COORDINATOR_EDIT_REFUSAL_REASON)

    with store.connection() as db:
        row = db.execute(
            "SELECT event_type, payload_json FROM task_events WHERE task_id=? AND event_type='progress'",
            (task_id,),
        ).fetchone()
    assert row is not None
    payload = json.loads(row["payload_json"])
    assert payload["note"] == COORDINATOR_EDIT_REFUSAL_REASON
    assert payload["kind"] == "coordinator_role_refusal"
    assert payload["message_key"] == "coordinator_mutation_refused"


def test_notify_coordinator_refusal_is_a_noop_for_an_unknown_session(tmp_path):
    """No matching ``_session_meta`` entry (e.g. the session never finished
    opening) -- best-effort, never raises."""
    store, task_id = _bootstrap(tmp_path)
    adapter = _adapter(store)
    adapter._notify_coordinator_refusal("es_unknown", "reason text")
    with store.connection() as db:
        row = db.execute("SELECT COUNT(*) AS n FROM task_events WHERE task_id=?", (task_id,)).fetchone()
    assert row["n"] == 0


def test_notify_coordinator_refusal_is_a_noop_without_a_store():
    adapter = EngineSessionManagerAdapter(
        connection_record_factory=lambda task_id: None,
        workspace_root_factory=lambda task_id: "/does/not/matter",
        store=None,
    )
    adapter._session_meta["es_a"] = dict(task_id="t1", attempt_id="co_1", recipient="coordinator")
    adapter._notify_coordinator_refusal("es_a", "reason text")  # must not raise
