"""Worker-side requirement lookup across delegation identity namespaces.

The coordinator's delegation event uses its deterministic idempotency key;
the provisioned worker channel uses ``attempt_id``.  These tests model the
durable rows produced by the real provisioning path and exercise the actual
worker MCP publication gate, rather than testing only the helper in isolation.
"""
from __future__ import annotations

import json
import uuid

import pytest

from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.contracts import StoreError
from cortex_runtime.delegation import events as delegation_events
from cortex_runtime.delegation import mcp_tools
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.store import Store
from cortex_runtime.trusted_channel import issue_channel_tx, resolve_channel
from cortex_runtime.ownership import current_owner_epoch


def _task(tmp_path):
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    result = bootstrap_task(
        store, SourceEvent(origin="cli", event_id=str(uuid.uuid4())),
        "Build a page.", "owner-1",
    )
    coordinator = resolve_channel(store, result.channel_id, result.secret)
    return store, result.task_id, coordinator


def _worker(store, task_id, coordinator, attempt_id):
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        epoch = current_owner_epoch(db, task_id)
        channel_id, secret = issue_channel_tx(
            db, task_id, "worker", epoch, parent_channel_id=coordinator.channel_id,
            assignment_id="a_provisioned", attempt_id=attempt_id,
        )
        db.commit()
    return resolve_channel(store, channel_id, secret)


def _worker_report(store, worker):
    from pathlib import Path

    key = "report-" + uuid.uuid4().hex
    draft = store.call(
        "create_draft", {"template": "general", "request_key": key},
        worker.principal_thread_id, worker.parent_principal_thread_id, trusted=True,
    )
    path = Path(draft["draft_path"])
    marker = path.read_bytes().split(b"\n\n", 1)[0] + b"\n\n"
    path.write_bytes(marker + b"Worker result.\n")
    written = store.call(
        "write_report",
        {"title": "Worker report", "summary": "Result.", "author": "worker",
         "draft_id": draft["draft_id"], "request_key": key},
        worker.principal_thread_id, worker.parent_principal_thread_id, trusted=True,
    )
    return written["report_id"]


def _provisioned_request(store, task_id, coordinator, worker, plan_assignment_ref, *, general=False):
    delegation_ref = "d" * 64 if not general else "g" * 64
    now = store._now()
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        db.execute(
            "INSERT INTO delegation_provisioning "
            "(idempotency_key,task_id,assignment_id,attempt_id,role_id,step,status,"
            "owner_epoch,created_at,updated_at) VALUES (?,?,?,?,?,?,?,?,?,?)",
            (delegation_ref, task_id, plan_assignment_ref, worker.attempt_id,
             "worker-frontend-dev", "ready", "ready", 1, now, now),
        )
        db.commit()
    delegation_events.append_event(
        store, task_id, "delegation_requested", "delegate:" + delegation_ref,
        {
            "title": "Implement page", "brief": "Build and verify the page.",
            "role": "frontend-dev", "acceptance": "The page passes.",
            "plan_assignment_ref": plan_assignment_ref,
            "structured_requirement_state": "structured",
        }, origin=coordinator.channel_id, assignment_ref=delegation_ref,
    )
    return delegation_ref




