"""acp.pipeline_bootstrap: the runtime publishes a task's initial pipeline
report itself, through the coordinator's own create_draft/write_report
store operations, with no model turn spent on it.

No real engine, no paid call, no network I/O -- everything here runs
against a tmp_path Store and the same in-process fakes
test_orchestrating_runtime.py uses.
"""
from __future__ import annotations

import threading
import uuid

from cortex_runtime.acp import pipeline_bootstrap
from cortex_runtime.acp.orchestrating_runtime import OrchestratingRuntime, PromptOutcome
from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.portable.engine_contract import StopReason
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.store import PIPELINE_PLACEHOLDERS, Store
from cortex_runtime.trusted_channel import issue_channel_tx, resolve_channel

from orchestration_fakes import FakeConfigSnapshotSource, FakeSessionRuntime, FakeWorkspace


def _task(tmp_path):
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    result = bootstrap_task(store, SourceEvent(origin="cli", event_id=str(uuid.uuid4())), "Build a page.", "owner-1")
    return store, result.task_id


def _coordinator_channel(store, task_id, attempt_id="co_test"):
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        try:
            epoch = db.execute("SELECT epoch FROM task_owners WHERE task_id=?", (task_id,)).fetchone()["epoch"]
            channel_id, secret = issue_channel_tx(db, task_id, "coordinator", epoch, attempt_id=attempt_id)
            db.commit()
        except BaseException:
            db.rollback()
            raise
    return resolve_channel(store, channel_id, secret)


def _pipeline_body(store, task_id):
    with store.connection() as db:
        report = db.execute(
            "SELECT filename FROM reports WHERE task_id=? AND kind='pipeline'", (task_id,),
        ).fetchone()
        if report is None:
            return None
        task_dir = store._task_directory(db, task_id)
    return (task_dir / report["filename"]).read_text(encoding="utf-8")


def test_publish_creates_pipeline_report(tmp_path):
    store, task_id = _task(tmp_path)
    channel = _coordinator_channel(store, task_id)
    pipeline_bootstrap.publish_initial_pipeline(
        store, task_id, channel, "Build a page.", dict(engine="codex-acp", model="gpt-5", effort=None),
    )
    with store.connection() as db:
        row = db.execute("SELECT 1 FROM reports WHERE task_id=? AND kind='pipeline'", (task_id,)).fetchone()
    assert row is not None


def test_published_pipeline_validates_and_has_no_placeholder_markers(tmp_path):
    store, task_id = _task(tmp_path)
    channel = _coordinator_channel(store, task_id)
    pipeline_bootstrap.publish_initial_pipeline(store, task_id, channel, "Build a page.", None)
    body = _pipeline_body(store, task_id)
    assert body is not None
    assert "Build a page." in body
    for marker in PIPELINE_PLACEHOLDERS:
        assert marker.decode() not in body
    assert "<!--" not in body and "-->" not in body
    # A second real coordinator-owned edition must still be publishable
    # against the same pipeline report (proves write_report itself accepted
    # this edition as a valid, well-formed prior edition to build on).
    with store.connection() as db:
        report_id = db.execute(
            "SELECT id FROM reports WHERE task_id=? AND kind='pipeline'", (task_id,),
        ).fetchone()["id"]
        editions = db.execute("SELECT author FROM editions WHERE report_id=?", (report_id,)).fetchall()
    assert len(editions) == 1
    assert editions[0]["author"] == "runtime"


def test_second_first_turn_publishes_nothing_new(tmp_path):
    store, task_id = _task(tmp_path)
    channel = _coordinator_channel(store, task_id)
    pipeline_bootstrap.publish_initial_pipeline(store, task_id, channel, "Build a page.", None)
    with store.connection() as db:
        report_id = db.execute(
            "SELECT id FROM reports WHERE task_id=? AND kind='pipeline'", (task_id,),
        ).fetchone()["id"]
        editions_before = db.execute(
            "SELECT COUNT(*) AS n FROM editions WHERE report_id=?", (report_id,),
        ).fetchone()["n"]
    # A second "first turn" -- for example a recovery/replacement session --
    # resolves a *different* coordinator channel for the same task, exactly
    # as coordinator_recovery.recover would; the durable reports row (not
    # an in-process flag) is what must stop a second publication.
    channel2 = _coordinator_channel(store, task_id, attempt_id="co_recovered")
    pipeline_bootstrap.publish_initial_pipeline(store, task_id, channel2, "Build a page.", None)
    with store.connection() as db:
        editions_after = db.execute(
            "SELECT COUNT(*) AS n FROM editions WHERE report_id=?", (report_id,),
        ).fetchone()["n"]
        reports_count = db.execute(
            "SELECT COUNT(*) AS n FROM reports WHERE task_id=? AND kind='pipeline'", (task_id,),
        ).fetchone()["n"]
    assert editions_after == editions_before == 1
    assert reports_count == 1


def test_publication_failure_is_recorded_and_never_raises(tmp_path):
    import dataclasses

    store, task_id = _task(tmp_path)
    channel = _coordinator_channel(store, task_id)
    # A channel whose principal thread was never bound to any task: exactly
    # the shape ``store.Store.call``'s ``_resolve_thread`` rejects with
    # ``task_not_bound`` -- a genuine publication failure, not a scripted
    # exception, exercised the same way ``trusted=True`` in-process calls
    # actually fail.
    broken = dataclasses.replace(channel, principal_thread_id="unbound-thread-" + uuid.uuid4().hex)
    pipeline_bootstrap.publish_initial_pipeline(store, task_id, broken, "Build a page.", None)  # must not raise
    with store.connection() as db:
        report = db.execute("SELECT 1 FROM reports WHERE task_id=? AND kind='pipeline'", (task_id,)).fetchone()
        observability = db.execute(
            "SELECT category, outcome, basis FROM observability_events WHERE task_id=?", (task_id,),
        ).fetchall()
        progress = db.execute(
            "SELECT event_type FROM task_events WHERE task_id=? AND event_type='progress'", (task_id,),
        ).fetchall()
    assert report is None
    assert any(row["category"] == "pipeline_bootstrap" and row["outcome"] == "failed" for row in observability)
    assert len(progress) >= 1


def test_start_prompt_publishes_initial_pipeline_with_no_model_turn(tmp_path):
    """The coordinator's own scripted behavior never touches create_draft/
    write_report -- yet the task's initial pipeline exists, valid and
    placeholder-free, after its very first turn. Proves publication is the
    runtime's own work, not a model turn."""
    store, task_id = _task(tmp_path)

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        return PromptOutcome(StopReason.END_TURN, "ok")

    def router(store, channel, session_ref, launch, instructions):
        return coordinator_behavior if channel is not None and channel.role == "coordinator" else None

    engine = FakeSessionRuntime(store, router)
    runtime = OrchestratingRuntime(
        store, engine, FakeWorkspace(), FakeConfigSnapshotSource(),
        allow_unqualified_delivery=True, wait_timeout_seconds=5.0,
    )
    updates = []
    cancel_event = threading.Event()
    result = runtime.start_prompt(task_id, "s1", "Build a page.", updates.append, cancel_event)

    assert result == StopReason.END_TURN
    body = _pipeline_body(store, task_id)
    assert body is not None
    assert "Build a page." in body
    for marker in PIPELINE_PLACEHOLDERS:
        assert marker.decode() not in body
    # Exactly one coordinator engine turn happened -- the runtime never
    # spent a second one publishing the pipeline.
    coordinator_session_ref = runtime._coordinator_sessions[task_id]
    assert sum(1 for ref in engine.prompt_calls if ref == coordinator_session_ref) == 3


def test_second_start_prompt_does_not_republish(tmp_path):
    store, task_id = _task(tmp_path)

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        return PromptOutcome(StopReason.END_TURN, "ok")

    def router(store, channel, session_ref, launch, instructions):
        return coordinator_behavior if channel is not None and channel.role == "coordinator" else None

    engine = FakeSessionRuntime(store, router)
    runtime = OrchestratingRuntime(
        store, engine, FakeWorkspace(), FakeConfigSnapshotSource(),
        allow_unqualified_delivery=True, wait_timeout_seconds=5.0,
    )
    cancel_event = threading.Event()
    runtime.start_prompt(task_id, "s1", "Build a page.", lambda text: None, cancel_event)
    with store.connection() as db:
        report_id = db.execute(
            "SELECT id FROM reports WHERE task_id=? AND kind='pipeline'", (task_id,),
        ).fetchone()["id"]
        editions_before = db.execute(
            "SELECT COUNT(*) AS n FROM editions WHERE report_id=?", (report_id,),
        ).fetchone()["n"]
    runtime.start_prompt(task_id, "s1", "A follow-up.", lambda text: None, threading.Event())
    with store.connection() as db:
        editions_after = db.execute(
            "SELECT COUNT(*) AS n FROM editions WHERE report_id=?", (report_id,),
        ).fetchone()["n"]
    assert editions_before == editions_after
    assert editions_before >= 2
    assert _pipeline_body(store,task_id).startswith("Cortex draft ID:")
