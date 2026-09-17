"""Test-only fake downstream ACP engine + scenario-driven spawn helpers.

``fake_engine_main.py``-equivalent logic lives directly in this module as
``FAKE_ENGINE_SOURCE`` (a standalone stdlib script executed by an absolute
``python3`` path as a connection's ``launch_command`` -- see
``spawn_fake_engine_record``). It is intentionally standalone (it must run
as a real subprocess with no import of ``cortex_runtime`` or ``tests``) and
clearly separate from any real-engine qualification: nothing here claims to
be codex-acp/claude-agent-acp behavior, only a scripted collaborator for
transport/lifecycle/correlation tests (see T012 task card's "controllable
fake ACP engine ... keep it clearly separate from real-engine
qualification").

Scenario schema (JSON file, path given as argv[1]):

    {
      "agent_capabilities": {"loadSession": bool, "sessionCapabilities": {"resume": bool}},
      "session_id": "<wire sessionId returned by session/new>",
      "config_options": [ {"id": str, "currentValue": str, "options": [...]}, ... ],
      "configure_behavior": "accept" | "mismatch" | "error" | "hang",
      "prompt_updates": [ {"delay": float, "sessionUpdate": str, ...} ],
      "prompt_stop_reason": "end_turn" | "cancelled" | ...,
      "prompt_behavior": "normal" | "ignore_cancel" | "late_reply" | "crash" |
                          "eof_no_exit" | "hang" | "client_request" | "malformed_then_normal",
      "client_request": {"method": str, "params": {}},
      "malformed_mode": null | "non_json" | "batch" | "oversize" | "nesting_bomb" | "invalid_utf8",
      "env_dump_path": "<abs path>",
      "replay_updates": [ {"sessionUpdate": "user_message_chunk", ...}, ... ]
    }
"""
from __future__ import annotations

import datetime
import json
import os
import sys
import uuid
from pathlib import Path

from cortex_runtime.connections import registry as connection_registry
from cortex_runtime.connections.model import AuthMode, ConnectionRecord, ConnectionState, EngineNativeDir

ROOT = Path(__file__).resolve().parents[1]
FAKE_ENGINE_SCRIPT = Path(__file__).resolve().parent / "fake_engine_process.py"


def isolated_environ(tmp_path: Path) -> dict[str, str]:
    """An explicit, self-contained environ mapping under ``tmp_path``.

    Passed explicitly as ``environ=`` to ``make_connection_record``,
    ``EngineProcess``, and ``EngineSessionManager`` -- never relies on
    ``os.environ``/monkeypatch ordering. ``build_process_env`` (and every
    registry call it makes) now accepts and honors this same mapping
    directly (see the ``connections/environment.py`` ``environ`` parameter
    added for this task), so passing this dict is sufficient on its own to
    keep every registry/state/secret path off the real user's
    ``~/.config/cortex``/``~/.local/state/cortex``, independent of whatever
    ``os.environ`` itself happens to contain.
    """
    home = tmp_path / "home"
    config_home = home / "config"
    state_home = home / "state"
    for path in (home, config_home, state_home):
        path.mkdir(exist_ok=True)
    return {
        "HOME": str(home),
        "XDG_CONFIG_HOME": str(config_home),
        "XDG_STATE_HOME": str(state_home),
        "PATH": os.environ.get("PATH", "/usr/bin:/bin"),
    }


def write_scenario(tmp_path: Path, scenario: dict) -> Path:
    path = tmp_path / f"scenario-{os.getpid()}-{id(scenario)}.json"
    path.write_text(json.dumps(scenario), encoding="utf-8")
    return path


def fake_launch_command(scenario_path: Path) -> tuple[str, ...]:
    """The ``launch_command`` tuple a ``ConnectionRecord`` should carry to
    run the fake engine (absolute python3 path, per T012's launch
    contract: only ``ConnectionRecord.launch_command`` is ever executed)."""
    return (sys.executable, "-B", str(FAKE_ENGINE_SCRIPT), str(scenario_path))


def make_connection_record(connection_id: str, launch_command: tuple[str, ...], *, engine: str = "codex-acp", environ: dict[str, str] | None = None) -> ConnectionRecord:
    """A minimal, registry-canonical ``ConnectionRecord`` for a fake engine.

    ``credential_ref`` is resolved against the *same* explicit ``environ``
    that will later be passed to ``EngineProcess``/``EngineSessionManager``
    (see ``isolated_environ``), so ``build_process_env``'s re-verification
    against that identical mapping succeeds.
    """
    now = datetime.datetime.now(datetime.timezone.utc).isoformat(timespec="microseconds")
    credential_ref = EngineNativeDir(path=str(connection_registry.connection_state_dir(connection_id, engine, environ)))
    return ConnectionRecord(
        id=connection_id, name="fake-engine", engine=engine, provider="fake",
        auth_mode=AuthMode.NATIVE_ACCOUNT, key_kind=None, endpoint=None,
        launch_command=launch_command, credential_ref=credential_ref,
        approved_fingerprint="fp", state=ConnectionState.CONFIGURED, qualified=False,
        created_at=now, updated_at=now,
    )


def new_connection_id() -> str:
    return "cn_" + uuid.uuid4().hex[:12]


def default_scenario(**overrides) -> dict:
    scenario = dict(
        agent_capabilities=dict(loadSession=False, sessionCapabilities=dict(resume=False)),
        session_id="sess-1",
        config_options=[
            dict(id="model", currentValue="gpt-x", options=[dict(value="gpt-x", name="GPT X")]),
        ],
        configure_behavior="accept",
        prompt_updates=[
            dict(delay=0.0, sessionUpdate="agent_message_chunk", content=dict(type="text", text="hello ")),
            dict(delay=0.0, sessionUpdate="agent_message_chunk", content=dict(type="text", text="world")),
        ],
        prompt_stop_reason="end_turn",
        prompt_behavior="normal",
    )
    scenario.update(overrides)
    return scenario
