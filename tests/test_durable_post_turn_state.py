"""Durable coordinator end-turn classification regressions.

These tests use the in-process ACP fakes only.  They cover the boundary where
the coordinator's process has no live worker future but the durable delegation
record still says work is unresolved: that state must get one reconciliation
turn and then remain blocked, never become idle or accepted.
"""
from __future__ import annotations

import json
import threading
import uuid

from cortex_runtime.acp.orchestrating_runtime import OrchestratingRuntime, PromptOutcome
from cortex_runtime.acp import questions_store
from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.delegation import events as delegation_events
from cortex_runtime.portable.engine_contract import StopReason
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.store import Store

from orchestration_fakes import FakeConfigSnapshotSource, FakeSessionRuntime, FakeWorkspace


def _task(tmp_path):
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    result = bootstrap_task(
        store, SourceEvent(origin="test", event_id=str(uuid.uuid4())),
        "Build a page.", "owner-1",
    )
    return store, result.task_id


def _runtime(store, router):
    engine = FakeSessionRuntime(store, router)
    runtime = OrchestratingRuntime(
        store, engine, FakeWorkspace(), FakeConfigSnapshotSource(),
        allow_unqualified_delivery=True, wait_timeout_seconds=0.02,
        max_no_progress_continuations=2,
    )
    return runtime, engine






