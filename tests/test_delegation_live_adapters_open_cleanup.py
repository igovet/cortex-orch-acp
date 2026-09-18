"""EXT-01 #8 regression: a failure part-way through
``EngineSessionManagerAdapter.open`` -- install verification, ``initialize``,
or ``session/new`` -- must never leak a capacity slot or an orphan process.
Before this fix, ``open`` only registered the manager/session/reservation
into its own bookkeeping *after* every one of these steps succeeded, so a
failure at any of them left the just-reserved capacity slot held forever
(never released, never even marked ambiguous) and, for the latter two, a
real already-spawned engine process running with nothing able to find and
stop it (``terminate_owned`` depends on exactly that bookkeeping).

Each of the three fault points below asserts the *real* state afterward --
not just that ``open`` raised -- against a real ``tests/fake_engine_process.py``
subprocess (never a real engine): the reservation row's own state/
observation in ``capacity_reservations``, and the spawned OS process (where
one was spawned) actually gone.
"""
from __future__ import annotations

import os
import uuid

import pytest

from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.delegation import capacity
from cortex_runtime.delegation.live_adapters import EngineSessionManagerAdapter
from cortex_runtime.engine.client import EngineClient
from cortex_runtime.portable.errors import PortableError
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.store import Store

from engine_fakes import (
    _resolved_node_binary,
    build_codex_npm_install,
    default_scenario,
    fake_launch_command,
    isolated_environ,
    make_connection_record,
    new_connection_id,
    write_scenario,
)


def _task(tmp_path):
    project = tmp_path / "project"
    project.mkdir()
    store = Store(project / ".cortex", project_root=project)
    result = bootstrap_task(store, SourceEvent(origin="cli", event_id=str(uuid.uuid4())), "Check in.", "owner-1")
    return store, result.task_id


def _reservation_row(store, task_id, attempt_id):
    with store.connection() as db:
        return db.execute(
            "SELECT state, release_observation FROM capacity_reservations WHERE task_id=? AND attempt_id=?",
            (task_id, attempt_id),
        ).fetchone()


def _pid_gone(pid: int) -> bool:
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return True
    except OSError:
        return True
    return False


# --- fault point 1: install verification (never spawns a process) ---------


def test_install_verification_failure_releases_capacity_and_spawns_nothing(tmp_path):
    """A drifted/wrong-entry install is refused by ``_resolve_launch_install``
    before ``EngineSessionManager`` is even constructed -- no process ever
    exists, so the reservation must be released outright (never left
    ``reserved``, never ``ambiguous`` -- there is nothing to reconcile)."""
    store, task_id = _task(tmp_path)
    environ = isolated_environ(tmp_path)
    scenario_path = write_scenario(tmp_path, default_scenario())
    install_root = tmp_path / "install"
    build_codex_npm_install(install_root, scenario_path)
    wrong_entry = install_root / "dist" / "other.js"
    wrong_entry.write_text("// never named by package.json's own bin field\n", encoding="utf-8")
    launch_command = (_resolved_node_binary(), str(wrong_entry))
    connection_id = new_connection_id()
    record = make_connection_record(connection_id, launch_command=launch_command, engine="codex-acp", environ=environ)
    adapter = EngineSessionManagerAdapter(
        lambda t: record, lambda t: str(tmp_path), environ=environ, call_timeout=5.0,
        store=store, capacity_limit=1,
    )

    with pytest.raises(PortableError) as excinfo:
        adapter.open("shared_project_root", dict(task_id=task_id, attempt_id="xa_install_fault"), "instructions")
    assert excinfo.value.code == "engine_install_drifted"

    assert not adapter._managers  # never registered -- refused before construction
    assert not adapter._sessions
    assert not adapter._reservations
    row = _reservation_row(store, task_id, "xa_install_fault")
    assert row["state"] == "released"
    assert row["release_observation"] == "never_started"
    assert capacity.count_active(store, connection_id) == 0

    # Capacity is immediately reusable -- nothing was leaked.
    reservation_id = capacity.reserve(store, connection_id, task_id, "xa_after_install_fault", "worker")
    assert reservation_id is not None


def _fake_adapter(store, tmp_path):
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
    return adapter, connection_id


def _raise_for(method_to_fail, pids):
    """A monkeypatch replacement for ``EngineClient.call`` that records the
    real spawned process's pid on every call (so the test can independently
    confirm it later exits), then raises for exactly one method (the fault
    point under test) and otherwise defers to the real implementation."""
    original_call = EngineClient.call

    def _call(self, method, params=None, *, timeout=30.0):
        pids.append(self._process.process.pid)
        if method == method_to_fail:
            raise RuntimeError(f"injected {method_to_fail} failure")
        return original_call(self, method, params, timeout=timeout)

    return _call


# --- fault point 2: initialize (process already spawned) ------------------


def test_initialize_failure_stops_the_process_and_releases_capacity(tmp_path, monkeypatch):
    store, task_id = _task(tmp_path)
    adapter, connection_id = _fake_adapter(store, tmp_path)
    pids: list[int] = []
    monkeypatch.setattr(EngineClient, "call", _raise_for("initialize", pids))

    with pytest.raises(RuntimeError, match="injected initialize failure"):
        adapter.open("shared_project_root", dict(task_id=task_id, attempt_id="xa_init_fault"), "instructions")

    assert pids, "the real engine process must have actually spawned"
    assert not adapter._managers  # never registered -- open() failed before reaching it
    assert not adapter._sessions
    assert not adapter._reservations
    assert _pid_gone(pids[0])  # no orphan process left behind

    row = _reservation_row(store, task_id, "xa_init_fault")
    assert row["state"] == "released"
    assert row["release_observation"] == "process_exited"
    assert capacity.count_active(store, connection_id) == 0


# --- fault point 3: session/new (process already spawned, initialize ok) --


def test_session_new_failure_stops_the_process_and_releases_capacity(tmp_path, monkeypatch):
    store, task_id = _task(tmp_path)
    adapter, connection_id = _fake_adapter(store, tmp_path)
    pids: list[int] = []
    monkeypatch.setattr(EngineClient, "call", _raise_for("session/new", pids))

    with pytest.raises(RuntimeError, match="injected session/new failure"):
        adapter.open("shared_project_root", dict(task_id=task_id, attempt_id="xa_session_new_fault"), "instructions")

    assert pids
    assert not adapter._managers
    assert not adapter._sessions
    assert not adapter._reservations
    assert _pid_gone(pids[0])

    row = _reservation_row(store, task_id, "xa_session_new_fault")
    assert row["state"] == "released"
    assert row["release_observation"] == "process_exited"
    assert capacity.count_active(store, connection_id) == 0
