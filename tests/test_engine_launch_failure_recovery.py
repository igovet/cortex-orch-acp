"""P1 diagnosable launch failure (2026-09-25 audit follow-up): a coordinator
engine session failing to open (e.g. the engine's own child process could
not initialize its state sqlite db -- see docs/project/gotchas.md) happens
in ``acp.orchestrating_runtime._open_coordinator_session``, *before*
``orchestration.state.TaskMachine.create`` ever runs (see
``orchestration.controller.run``, the only caller of ``TaskMachine.create``,
reached only after ``_ensure_coordinator`` returns successfully). A retained
dev trace showed exactly that shape: a task id already durably bound
(``bootstrap.bootstrap_task``'s own ``tasks``/``task_orchestration`` rows)
but no ``runtime_tasks`` row at all.

This module covers, with no real engine subprocess and no paid model call
(same in-process fakes as ``tests/test_coordinator_self_notice_no_stall.py``):

1. The failure is tagged with a stage/task/attempt identity
   (``acp.orchestrating_runtime._open_coordinator_session``) and leaves a
   consistent, resumable state: ``task_orchestration`` provisioned, no
   ``runtime_tasks`` row yet.
2. A retry (a fresh ``start_prompt`` call for the same task id) recovers
   through the existing coordinator-replacement path and reaches exactly
   one ``task_orchestration`` row and exactly one ``runtime_tasks`` row --
   never a duplicated task.
"""
from __future__ import annotations

import threading
import uuid

import pytest

from cortex_runtime.acp.orchestrating_runtime import OrchestratingRuntime, PromptOutcome
from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.engine.retry_policy import RetryWindow
from cortex_runtime.portable.engine_contract import StopReason
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.store import Store

from orchestration_fakes import FakeConfigSnapshotSource, FakeSessionRuntime, FakeWorkspace


class FlakyCoordinatorOpenEngine(FakeSessionRuntime):
    """Same behavior as :class:`FakeSessionRuntime`, except the *next*
    coordinator ``open()`` call (``launch`` carries no ``assignment_id``)
    raises once, when armed, standing in for a real engine child process
    that failed to start (e.g. could not initialize its own state sqlite
    db) -- never for a worker open, and never more than the armed count."""

    def __init__(self, store, router, **kwargs):
        super().__init__(store, router, **kwargs)
        self.fail_next_coordinator_opens = 0
        self.failed_launch_attempt_ids = []

    def open(self, workspace_binding_id, launch, instructions):
        if self.fail_next_coordinator_opens > 0 and not launch.get("assignment_id"):
            self.fail_next_coordinator_opens -= 1
            self.failed_launch_attempt_ids.append(launch.get("attempt_id"))
            raise RuntimeError("engine child process could not initialize its own state sqlite db")
        return super().open(workspace_binding_id, launch, instructions)


def _task(tmp_path):
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    result = bootstrap_task(store, SourceEvent(origin="cli", event_id=str(uuid.uuid4())), "Build a page.", "owner-1")
    return store, result.task_id


def _make_runtime(store, router, **kwargs):
    engine = FlakyCoordinatorOpenEngine(store, router)
    runtime = OrchestratingRuntime(
        store, engine, FakeWorkspace(), FakeConfigSnapshotSource(),
        allow_unqualified_delivery=True, wait_timeout_seconds=kwargs.pop("wait_timeout_seconds", 30.0),
        **kwargs,
    )
    # These tests cover the failure the runtime gives up on; its transient
    # retries are covered by tests/test_engine_retry_window.py.
    runtime._engine_retry_window = RetryWindow(window_seconds=0)
    return runtime, engine


def _simple_router():
    def coordinator_behavior(store, channel, session_ref, message, call_index):
        return PromptOutcome(StopReason.END_TURN, "Understood.")

    def router(store, channel, session_ref, launch, instructions):
        return coordinator_behavior if channel and channel.role == "coordinator" else None

    return router


def _orchestration_row(store, task_id):
    with store.connection() as db:
        return db.execute("SELECT * FROM task_orchestration WHERE task_id=?", (task_id,)).fetchone()


def _runtime_task_row(store, task_id):
    with store.connection() as db:
        return db.execute("SELECT * FROM runtime_tasks WHERE task_id=?", (task_id,)).fetchone()


def test_engine_launch_failure_is_tagged_and_leaves_consistent_recoverable_state(tmp_path):
    store, task_id = _task(tmp_path)
    runtime, engine = _make_runtime(store, _simple_router())
    engine.fail_next_coordinator_opens = 1

    with pytest.raises(RuntimeError) as failure:
        runtime.start_prompt(task_id, "s1", "Hello.", lambda _t: True, threading.Event())

    # acp.agent's crash-diagnostic wiring reads exactly these attributes --
    # see acp.agent._write_crash_diagnostic and its callers.
    assert failure.value.cortex_launch_stage == "engine_launch"
    assert failure.value.cortex_launch_task_id == task_id
    assert failure.value.cortex_launch_attempt_id is not None
    assert failure.value.cortex_launch_attempt_id == engine.failed_launch_attempt_ids[-1]

    # Bound task id, provisioned coordinator attempt -- but the engine
    # session never opened, so orchestration.state.TaskMachine.create
    # (only reached from orchestration.controller.run, itself only called
    # after _ensure_coordinator returns) never ran either.
    orchestration = _orchestration_row(store, task_id)
    assert orchestration is not None
    assert orchestration["state"] == "provisioning"
    assert _runtime_task_row(store, task_id) is None


def test_retry_after_engine_launch_failure_recovers_without_duplicating_the_task(tmp_path):
    store, task_id = _task(tmp_path)
    runtime, engine = _make_runtime(store, _simple_router())
    engine.fail_next_coordinator_opens = 1

    with pytest.raises(RuntimeError):
        runtime.start_prompt(task_id, "s1", "Hello.", lambda _t: True, threading.Event())

    # A fresh process would construct a brand-new OrchestratingRuntime with
    # no in-memory cache at all; using the same instance (its coordinator
    # cache was never populated for task_id, since open() raised before
    # that assignment) exercises the identical _ensure_coordinator
    # "no cached session" path a real restart takes.
    reason = runtime.start_prompt(task_id, "s1", "Hello again.", lambda _t: True, threading.Event())
    assert reason == StopReason.END_TURN

    with store.connection() as db:
        orchestration_rows = db.execute(
            "SELECT * FROM task_orchestration WHERE task_id=?", (task_id,),
        ).fetchall()
        runtime_task_rows = db.execute(
            "SELECT * FROM runtime_tasks WHERE task_id=?", (task_id,),
        ).fetchall()
        task_rows = db.execute("SELECT * FROM tasks WHERE id=?", (task_id,)).fetchall()

    assert len(task_rows) == 1
    assert len(orchestration_rows) == 1
    # Reached a normal end-of-turn state (never stuck "provisioning") --
    # the exact label is orchestration.controller's own, not this test's
    # concern; only "exactly one row, no longer stuck mid-launch" is.
    assert orchestration_rows[0]["state"] not in ("provisioning", None)
    assert len(runtime_task_rows) == 1

    # The recovered attempt is a genuinely new one, not a silent reuse of
    # the attempt whose engine session never actually opened.
    assert orchestration_rows[0]["coordinator_attempt_ref"] not in engine.failed_launch_attempt_ids


def test_retry_still_publishes_the_initial_pipeline_once(tmp_path):
    """Regression for the companion consistency bug this same failure shape
    exposed: ``_run``'s initial-pipeline publish used to be gated on
    ``not pre_existing`` (``task_orchestration`` already existing), but
    that row is committed *before* the engine session opens -- so a first
    turn that fails at engine-launch left ``pre_existing`` reading True on
    every subsequent retry and the pipeline was never published at all.
    ``pipeline_bootstrap.publish_initial_pipeline`` is idempotent on its
    own (the task's ``reports`` row), so this must still publish exactly
    once, on the retry that actually succeeds."""
    store, task_id = _task(tmp_path)
    runtime, engine = _make_runtime(store, _simple_router())
    engine.fail_next_coordinator_opens = 1

    with pytest.raises(RuntimeError):
        runtime.start_prompt(task_id, "s1", "Hello.", lambda _t: True, threading.Event())

    with store.connection() as db:
        assert db.execute(
            "SELECT COUNT(*) AS n FROM reports WHERE task_id=? AND kind='pipeline'", (task_id,),
        ).fetchone()["n"] == 0

    runtime.start_prompt(task_id, "s1", "Hello again.", lambda _t: True, threading.Event())

    with store.connection() as db:
        assert db.execute(
            "SELECT COUNT(*) AS n FROM reports WHERE task_id=? AND kind='pipeline'", (task_id,),
        ).fetchone()["n"] == 1
