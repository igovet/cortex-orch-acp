"""D-T035-04 part 1: the coordinator's own instructions carry a compact
role -> allowed-routes table rendered from the route policy, so it can
choose a ``delegate`` call's optional ``engine``/``model``/``effort``
without a round trip.

Exercises ``acp.orchestrating_runtime.OrchestratingRuntime
._open_coordinator_session`` against the same in-process fakes
``test_orchestrating_runtime.py`` already uses (never a real engine)."""
from __future__ import annotations

import json
import threading

from cortex_runtime.acp.orchestrating_runtime import PromptOutcome
from cortex_runtime.connections import registry as connection_registry
from cortex_runtime.portable.engine_contract import StopReason

from engine_fakes import isolated_environ, make_connection_record, new_connection_id
from test_orchestrating_runtime import _make_runtime, _task


def _write_policy(tmp_path, roles=None, default=None):
    (tmp_path / ".cortex").mkdir(exist_ok=True)
    (tmp_path / ".cortex" / "route-policy.json").write_text(
        json.dumps({"roles": roles or {}, "default": default}), encoding="utf-8",
    )


def test_coordinator_instructions_carry_the_allowed_routes_table(tmp_path, monkeypatch):
    store, task_id = _task(tmp_path)
    # The instructions advertise connected routes, not every route written
    # in policy.  Use a separate user-scoped registry outside the project.
    environ_root = tmp_path.parent / (tmp_path.name + "-engine")
    environ_root.mkdir()
    environ = isolated_environ(environ_root)
    connection = make_connection_record(
        new_connection_id(), ("/usr/bin/python3", "-V"), environ=environ,
    )
    with connection_registry.transaction(environ) as state:
        state.connections[connection.id] = connection
    for key in ("HOME", "XDG_CONFIG_HOME", "XDG_STATE_HOME", "PATH"):
        monkeypatch.setenv(key, environ[key])
    _write_policy(tmp_path, roles={
        "worker-backend-dev": [
            {"engine": "codex-acp", "model": "gpt-5.6-terra", "efforts": ["medium", "high"], "default_effort": "medium"},
        ],
    })
    captured = {}

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        return PromptOutcome(StopReason.END_TURN, "Done.")

    def router(store, channel, session_ref, launch, instructions):
        if channel is not None and channel.role == "coordinator":
            captured["instructions"] = instructions
            return coordinator_behavior
        return None

    runtime, engine = _make_runtime(store, router)
    runtime.start_prompt(task_id, "s1", "Do something.", lambda update: None, threading.Event())

    assert "backend-dev: codex-acp/gpt-5.6-terra {high,medium} default=medium" in captured["instructions"]


def test_coordinator_instructions_unchanged_when_no_policy_file_exists(tmp_path, monkeypatch):
    # Isolated HOME: ``_open_coordinator_session`` falls back to the real
    # ``os.environ`` (matching ``configuration.route_policy
    # .ensure_default_policy``'s own precedent) when this task's own
    # project has no policy file -- so this test's "no policy anywhere"
    # case must not accidentally observe the real developer's own
    # ``~/.config/cortex/route-policy.json``.
    monkeypatch.setenv("HOME", str(tmp_path / "isolated-home"))
    monkeypatch.delenv("CORTEX_ROUTE_POLICY", raising=False)
    monkeypatch.delenv("XDG_CONFIG_HOME", raising=False)
    store, task_id = _task(tmp_path)
    captured = {}

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        return PromptOutcome(StopReason.END_TURN, "Done.")

    def router(store, channel, session_ref, launch, instructions):
        if channel is not None and channel.role == "coordinator":
            captured["instructions"] = instructions
            return coordinator_behavior
        return None

    runtime, engine = _make_runtime(store, router)
    runtime.start_prompt(task_id, "s1", "Do something.", lambda update: None, threading.Event())

    assert "Allowed worker routes" not in captured["instructions"]
