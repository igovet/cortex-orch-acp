"""T017: EngineSessionManagerAdapter capacity release against a REAL
``EngineSessionManager`` + ``tests/fake_engine_process.py`` subprocess --
never a real engine, but a real process lifecycle: confirmed exit frees
capacity, an unconfirmed (timeout_alive) termination stays ambiguous."""
from __future__ import annotations

import dataclasses
import threading
import time
import uuid

from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.delegation import capacity
from cortex_runtime.delegation.live_adapters import EngineSessionManagerAdapter
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.store import Store

from engine_fakes import default_scenario, fake_launch_command, isolated_environ, make_connection_record, new_connection_id, write_scenario


def _task(tmp_path):
    project = tmp_path / "project"
    project.mkdir()
    store = Store(project / ".cortex", project_root=project)
    result = bootstrap_task(store, SourceEvent(origin="cli", event_id=str(uuid.uuid4())), "Check in.", "owner-1")
    return store, result.task_id


def test_confirmed_process_exit_releases_capacity(tmp_path):
    store, task_id = _task(tmp_path)
    engine_dir = tmp_path / "engine"
    engine_dir.mkdir()
    environ = isolated_environ(engine_dir)
    scenario_path = write_scenario(engine_dir, default_scenario())
    connection_id = new_connection_id()
    record = make_connection_record(connection_id, launch_command=fake_launch_command(scenario_path), environ=environ)
    # The limit comes from the connection record itself -- no adapter-wide
    # ``capacity_limit`` override.
    record = dataclasses.replace(record, max_concurrent_sessions=1)

    adapter = EngineSessionManagerAdapter(
        lambda t: record, lambda t: str(tmp_path), environ=environ, call_timeout=10.0,
        store=store,
    )
    session_ref = adapter.open("shared_project_root", dict(task_id=task_id, attempt_id="xa_worker1"), "instructions")
    assert session_ref.startswith("es_")
    assert capacity.count_active(store, connection_id) == 1
    # A second attempt on the same connection queues (blocks) instead of
    # failing while the first reservation is still held (limit=1).
    results = []
    thread = threading.Thread(target=lambda: results.append(
        adapter.open("shared_project_root", dict(task_id=task_id, attempt_id="xa_worker2"), "instructions")
    ))
    thread.start()
    time.sleep(0.3)  # several backoff ticks
    assert not results  # still queued, never returned
    with store.connection() as db:
        queued = db.execute(
            "SELECT 1 FROM task_events WHERE task_id=? AND dedup_key='capacity_queued:xa_worker2'", (task_id,),
        ).fetchone()
    assert queued is not None  # queued once, no failure spam

    adapter.terminate_owned("xa_worker1")  # real SIGTERM/reap of the fake engine subprocess -- notifies the queued open
    thread.join(timeout=30)  # generous upper bound; returns as soon as the queued open() actually completes
    assert results and results[0].startswith("es_")  # worker2 proceeded once capacity freed
    adapter.terminate_owned("xa_worker2")
    with store.connection() as db:
        row = db.execute(
            "SELECT state, release_observation FROM capacity_reservations WHERE task_id=? AND attempt_id='xa_worker1'",
            (task_id,),
        ).fetchone()
    assert row["state"] == "released"
    assert row["release_observation"] == "process_exited"
    assert capacity.count_active(store, connection_id) == 0

    # Capacity is now reusable.
    session_ref2 = adapter.open("shared_project_root", dict(task_id=task_id, attempt_id="xa_worker3"), "instructions")
    assert session_ref2.startswith("es_")
    adapter.terminate_owned("xa_worker3")


def test_unconfirmed_termination_stays_ambiguous_and_holds_capacity(tmp_path):
    """Deterministic: force the adapter's post-shutdown observation check
    (``EngineProcess.wait_exited(0)``) to report "not yet confirmed exited",
    the same signal a real expired grace window with a still-alive process
    would produce (see ``test_engine_session_manager.py``'s
    ``test_process_exit_is_distinct_from_timeout`` for the underlying
    real-process semantics this reuses)."""
    store, task_id = _task(tmp_path)
    engine_dir = tmp_path / "engine"
    engine_dir.mkdir()
    environ = isolated_environ(engine_dir)
    scenario_path = write_scenario(engine_dir, default_scenario())
    connection_id = new_connection_id()
    record = make_connection_record(connection_id, launch_command=fake_launch_command(scenario_path), environ=environ)

    adapter = EngineSessionManagerAdapter(
        lambda t: record, lambda t: str(tmp_path), environ=environ, call_timeout=10.0,
        store=store, capacity_limit=1,
    )
    session_ref = adapter.open("shared_project_root", dict(task_id=task_id, attempt_id="xa_hang1"), "instructions")
    assert session_ref.startswith("es_")

    manager = adapter._managers["xa_hang1"]
    manager._process.wait_exited = lambda timeout=None: False  # force "not confirmed"
    adapter.terminate_owned("xa_hang1")

    with store.connection() as db:
        row = db.execute(
            "SELECT state, release_observation FROM capacity_reservations WHERE task_id=? AND attempt_id='xa_hang1'",
            (task_id,),
        ).fetchone()
    assert row["state"] == "ambiguous"
    assert row["release_observation"] == "timeout_alive"
    assert capacity.count_active(store, connection_id) == 1
