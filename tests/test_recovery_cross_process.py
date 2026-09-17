"""T023: real cross-process engine-liveness detection.

A real ``EngineSessionManager`` + ``tests/fake_engine_process.py``
subprocess (never a real engine -- see that module's docstring), launched
through one ``EngineSessionManagerAdapter`` instance and then looked up
through a *second*, independent adapter instance sharing only the same
``Store`` -- modeling a Cortex process restart, where the new process's own
adapter starts with empty in-memory dicts but the durable
``recovery.registry`` (pid/start-time/boot-id, see
``engine.process_identity``) survives on disk."""
from __future__ import annotations

import uuid

from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.delegation import capacity
from cortex_runtime.delegation.live_adapters import EngineSessionManagerAdapter
from cortex_runtime.engine import process_identity
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.recovery import registry as recovery_registry
from cortex_runtime.store import Store

from engine_fakes import default_scenario, fake_launch_command, isolated_environ, make_connection_record, new_connection_id, write_scenario


def _task(tmp_path):
    project = tmp_path / "project"
    project.mkdir()
    store = Store(project / ".cortex", project_root=project)
    result = bootstrap_task(store, SourceEvent(origin="cli", event_id=str(uuid.uuid4())), "Check in.", "owner-1")
    return store, result.task_id


def _adapter(tmp_path, store, environ, record):
    return EngineSessionManagerAdapter(
        lambda t: record, lambda t: str(tmp_path), environ=environ, call_timeout=10.0,
        store=store, capacity_limit=4,
    )


def test_alive_process_detected_and_confirmed_terminated_across_adapter_instances(tmp_path):
    store, task_id = _task(tmp_path)
    engine_dir = tmp_path / "engine"
    engine_dir.mkdir()
    environ = isolated_environ(engine_dir)
    scenario_path = write_scenario(engine_dir, default_scenario())
    connection_id = new_connection_id()
    record = make_connection_record(connection_id, launch_command=fake_launch_command(scenario_path), environ=environ)

    adapter1 = _adapter(tmp_path, store, environ, record)
    session_ref = adapter1.open("shared_project_root", dict(task_id=task_id, attempt_id="xa_old"), "instructions")
    assert session_ref.startswith("es_")

    # A durable registry row now exists, independent of adapter1's own
    # in-memory state.
    row = recovery_registry.lookup_process(store, "xa_old")
    assert row is not None
    assert recovery_registry.probe_liveness(row) == "alive"

    # Simulate a Cortex process restart: a brand-new adapter instance, no
    # in-memory record of "xa_old" at all, over the SAME store.
    adapter2 = _adapter(tmp_path, store, environ, record)
    assert "xa_old" not in adapter2._sessions
    assert adapter2.lookup_owned("xa_old") == "alive"

    adapter2.terminate_owned("xa_old")
    assert adapter2.lookup_owned("xa_old") == "absent"
    row_after = recovery_registry.lookup_process(store, "xa_old")
    assert row_after["terminated_at"] is not None


def test_absent_after_confirmed_termination_frees_capacity(tmp_path):
    store, task_id = _task(tmp_path)
    engine_dir = tmp_path / "engine"
    engine_dir.mkdir()
    environ = isolated_environ(engine_dir)
    scenario_path = write_scenario(engine_dir, default_scenario())
    connection_id = new_connection_id()
    record = make_connection_record(connection_id, launch_command=fake_launch_command(scenario_path), environ=environ)

    adapter1 = _adapter(tmp_path, store, environ, record)
    adapter1.open("shared_project_root", dict(task_id=task_id, attempt_id="xa_old2"), "instructions")
    assert capacity.count_active(store, connection_id) == 1

    adapter2 = _adapter(tmp_path, store, environ, record)
    adapter2.terminate_owned("xa_old2")
    assert capacity.count_active(store, connection_id) == 0


def test_pid_reuse_is_reported_absent_not_alive(tmp_path):
    """A registry row whose recorded start time no longer matches the
    live process at that pid (the kernel reused the pid for something
    else) must never be reported "alive"."""
    store, task_id = _task(tmp_path)
    engine_dir = tmp_path / "engine"
    engine_dir.mkdir()
    environ = isolated_environ(engine_dir)
    scenario_path = write_scenario(engine_dir, default_scenario())
    connection_id = new_connection_id()
    record = make_connection_record(connection_id, launch_command=fake_launch_command(scenario_path), environ=environ)

    adapter1 = _adapter(tmp_path, store, environ, record)
    adapter1.open("shared_project_root", dict(task_id=task_id, attempt_id="xa_reuse"), "instructions")
    row = recovery_registry.lookup_process(store, "xa_reuse")
    assert row is not None

    # Corrupt the recorded start_ticks to model "this pid now belongs to a
    # different process than the one that was actually launched" -- the
    # real process is still alive, but its own /proc start time will never
    # equal this bogus value.
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        db.execute(
            "UPDATE attempt_process_registry SET start_ticks=? WHERE attempt_id=?",
            ("999999999", "xa_reuse"),
        )
        db.commit()

    adapter2 = _adapter(tmp_path, store, environ, record)
    assert adapter2.lookup_owned("xa_reuse") == "absent"
    adapter1.terminate_owned("xa_reuse")  # real cleanup of the still-alive process


def test_unreadable_boot_id_reports_unknown(tmp_path, monkeypatch):
    store, task_id = _task(tmp_path)
    engine_dir = tmp_path / "engine"
    engine_dir.mkdir()
    environ = isolated_environ(engine_dir)
    scenario_path = write_scenario(engine_dir, default_scenario())
    connection_id = new_connection_id()
    record = make_connection_record(connection_id, launch_command=fake_launch_command(scenario_path), environ=environ)

    adapter1 = _adapter(tmp_path, store, environ, record)
    adapter1.open("shared_project_root", dict(task_id=task_id, attempt_id="xa_unknown"), "instructions")

    adapter2 = _adapter(tmp_path, store, environ, record)
    monkeypatch.setattr(process_identity, "read_boot_id", lambda: None)
    assert adapter2.lookup_owned("xa_unknown") == "unknown"
    monkeypatch.undo()
    adapter1.terminate_owned("xa_unknown")


def test_reboot_boot_id_change_reports_absent(tmp_path, monkeypatch):
    """A different current boot id than the one recorded at launch means
    the machine rebooted since -- every previously-recorded pid is gone,
    regardless of whether the number happens to be in use again."""
    store, task_id = _task(tmp_path)
    engine_dir = tmp_path / "engine"
    engine_dir.mkdir()
    environ = isolated_environ(engine_dir)
    scenario_path = write_scenario(engine_dir, default_scenario())
    connection_id = new_connection_id()
    record = make_connection_record(connection_id, launch_command=fake_launch_command(scenario_path), environ=environ)

    adapter1 = _adapter(tmp_path, store, environ, record)
    adapter1.open("shared_project_root", dict(task_id=task_id, attempt_id="xa_reboot"), "instructions")
    adapter1.terminate_owned("xa_reboot")  # real cleanup; registry row remains

    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        db.execute("UPDATE attempt_process_registry SET terminated_at=NULL WHERE attempt_id=?", ("xa_reboot",))
        db.commit()
    with store.connection() as db:
        row = db.execute("SELECT boot_id FROM attempt_process_registry WHERE attempt_id=?", ("xa_reboot",)).fetchone()
    assert row is not None

    adapter2 = _adapter(tmp_path, store, environ, record)
    monkeypatch.setattr(process_identity, "read_boot_id", lambda: "a-different-boot-id")
    assert adapter2.lookup_owned("xa_reboot") == "absent"
