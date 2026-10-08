"""A catalog refresh must outlive the short-lived ACP process that asked for it.

Paseo reads Cortex's models from an ACP process it stops about a second after
``session/new`` (SIGTERM, then SIGKILL). A real Claude refresh takes about
12 s (one effort probe per model). While the refresh ran on a thread of that
process it was killed every time, so Paseo's Refresh never saved a Claude
catalog and Haiku 5.5 could not appear (isolated dev Paseo, 2026-10-08).
These tests run the real ACP agent in a child process, SIGKILL it right after
``session/new`` and require a slow refresh to save its snapshot anyway.
Offline: every engine is ``tests/fake_engine_process.py``.
"""
from __future__ import annotations

import fcntl
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import textwrap
import time

from cortex_runtime.catalog import detached_refresh, store as catalog_store
from cortex_runtime.connections import registry

from engine_fakes import (
    default_scenario,
    fake_launch_command,
    isolated_environ,
    make_connection_record,
    new_connection_id,
    write_scenario,
)
from model_policy_fixtures import routing_fixture

SRC = Path(__file__).resolve().parents[1] / "src"
FIXTURES = Path(__file__).resolve().parent / "fixtures" / "catalog"

_REQUESTER = textwrap.dedent("""
    import os, sys, time
    sys.path.insert(0, sys.argv[1])
    from cortex_runtime.acp.agent import Agent

    class Runtime:
        def start_prompt(self, *args):
            raise AssertionError("no prompt")
        def cancel(self, session_id):
            return None

    class Transport:
        def send_response(self, message):
            pass
        def send_notification(self, message, timeout=None):
            return True

    agent = Agent(Runtime(), environ=os.environ)
    agent.bind_transport(Transport())
    agent.initialized = True
    agent.client_capabilities = {}
    result = agent._session_new(dict(cwd=sys.argv[2], mcpServers=[]))
    agent._post_session_new_notifications(result["sessionId"])
    print("session-new-answered", flush=True)
    time.sleep(60)
""")


def _setup(tmp_path, *, session_new_delay):
    environ = isolated_environ(tmp_path)
    root = Path(environ["XDG_CONFIG_HOME"]) / "cortex"
    root.mkdir(mode=0o700, exist_ok=True)
    (root / "routing.json").write_text(json.dumps(routing_fixture()))
    (root / "routing.json").chmod(0o600)
    options = json.loads((FIXTURES / "codex_acp_config_options.json").read_text())["configOptions"]
    scenario = default_scenario(config_options=options, session_new_delay=session_new_delay)
    record = make_connection_record(
        new_connection_id(), launch_command=fake_launch_command(write_scenario(tmp_path, scenario)),
        environ=environ)
    with registry.transaction(environ) as state:
        state.connections[record.id] = record
    project = tmp_path / "project"
    project.mkdir()
    return environ, record, project


def _wait_for_snapshot(record, environ, seconds):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        view = catalog_store.latest_connection_snapshot(record.id, environ)
        if view is not None:
            return view
        time.sleep(0.1)
    return None


def test_refresh_survives_the_acp_process_that_started_it(tmp_path):
    environ, record, project = _setup(tmp_path, session_new_delay=2.0)
    requester = subprocess.Popen(
        [sys.executable, "-B", "-c", _REQUESTER, str(SRC), str(project)],
        env=dict(environ), stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True)
    try:
        assert requester.stdout.readline().strip() == "session-new-answered"
        # Paseo stops its discovery process about a second after session/new;
        # the engine here is still answering session/new for another 1.5 s.
        time.sleep(0.5)
        requester.send_signal(signal.SIGKILL)
        requester.wait(10)
    finally:
        if requester.poll() is None:
            requester.kill()
            requester.wait()
    assert catalog_store.latest_connection_snapshot(record.id, environ) is None
    view = _wait_for_snapshot(record, environ, 20)
    assert view is not None, "the refresh died with the ACP process that started it"
    assert {entry.route.model for entry in view.entries} == {"model-a", "model-b"}


def test_run_returns_the_child_outcome_and_saves_the_snapshot(tmp_path):
    environ, record, project = _setup(tmp_path, session_new_delay=0)
    outcome = detached_refresh.run(record, str(project), environ=environ, wait=30)
    assert outcome.success and outcome.reason is None and outcome.snapshot_id
    view = catalog_store.load_connection_snapshot(record.id, outcome.snapshot_id, environ)
    assert {entry.route.model for entry in view.entries} == {"model-a", "model-b"}


def test_run_reports_an_unknown_connection_without_a_snapshot(tmp_path):
    environ, record, project = _setup(tmp_path, session_new_delay=0)
    with registry.transaction(environ) as state:
        del state.connections[record.id]
    outcome = detached_refresh.run(record, str(project), environ=environ, wait=30)
    assert not outcome.success and outcome.reason == "connection_not_found"
    assert catalog_store.latest_connection_snapshot(record.id, environ) is None


def test_a_running_refresh_is_not_started_twice(tmp_path):
    environ, record, project = _setup(tmp_path, session_new_delay=0)
    directory = catalog_store._snapshot_dir(record.id, environ)
    registry.ensure_private_dir(directory, root=registry.state_root(environ), environ=environ)
    descriptor = os.open(directory / detached_refresh.LOCK_NAME, os.O_RDWR | os.O_CREAT, 0o600)
    try:
        fcntl.flock(descriptor, fcntl.LOCK_EX)
        outcome = detached_refresh.refresh_locked(record.id, str(project), environ)
    finally:
        os.close(descriptor)
    assert outcome == dict(connection_id=record.id, success=False, reason="refresh_in_progress", snapshot_id=None)
    assert catalog_store.latest_connection_snapshot(record.id, environ) is None
