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
                          "eof_no_exit" | "hang" | "silent_until_cancel" |
                          "client_request" | "malformed_then_normal",
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
import platform
import stat
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


# --- realistic on-disk npm install layouts (T035 follow-up: --------------
# --- engine.install_resolution.resolve_verified_install) -------------------
#
# These build a directory layout close enough to a real pinned
# codex-acp/claude-agent-acp install (package.json + dist/index.js, per
# docs/acp/codex-acp-adapter.md and docs/acp/claude-agent-acp-adapter.md)
# for ``install_resolution.resolve_verified_install`` to actually build and
# verify a real install from the returned ``(node_binary, adapter_entry)``
# launch_command alone -- but the "adapter entry" is a thin Python wrapper
# that execs this module's own ``fake_engine_process.py`` against a fixed
# scenario file, never a real Node/codex-acp/claude-agent-acp process. Kept
# here (not duplicated per test file) since three different test modules
# need the identical layout.


def _resolved_node_binary() -> str:
    """This interpreter's own real, canonical (no symlink component) path
    -- codex_install/claude_install's own ``verify_install`` rejects a
    ``node_binary`` whose ``.resolve()`` differs from itself, and
    ``sys.executable`` is frequently a symlink (e.g. ``/usr/bin/python3``
    -> ``/usr/bin/python3.12``)."""
    return str(Path(sys.executable).resolve())


def _write_executable_file(path: Path, content: bytes = b"#!/bin/sh\nexit 0\n") -> None:
    path.write_bytes(content)
    path.chmod(path.stat().st_mode | stat.S_IXUSR)


def write_wrapper_adapter_entry(path: Path, scenario_path: Path) -> None:
    """A launchable ``adapter_entry`` file that, run with no arguments
    beyond its own path (the exact ``(node_binary, adapter_entry)`` shape a
    real install's own launch_command always has), execs the real
    ``fake_engine_process.py`` against ``scenario_path`` -- so
    ``sys.executable`` + this file alone is a complete, ACP-speaking,
    exactly-2-element launch_command."""
    path.write_text(
        "#!/usr/bin/env python3\n"
        "import runpy, sys\n"
        f"sys.argv = [sys.argv[0], {str(scenario_path)!r}]\n"
        f"runpy.run_path({str(FAKE_ENGINE_SCRIPT)!r}, run_name='__main__')\n",
        encoding="utf-8",
    )
    path.chmod(path.stat().st_mode | stat.S_IXUSR)


def build_claude_npm_install(
    root: Path, scenario_path: Path, *, package_version: str = "0.78.0", bin_entry: str = "dist/index.js",
) -> tuple[str, str]:
    """A realistic claude-agent-acp npm install under ``root``; returns the
    ``(node_binary, adapter_entry)`` launch_command tuple for its own
    package.json-declared ``bin_entry`` (deliberately overridable, so a
    test can build a package.json declaring ``dist/index.js`` while
    pointing ``launch_command`` at a *different* file under the same
    package -- the "wrong entry point" scenario this whole feature guards
    against)."""
    root.mkdir(parents=True, exist_ok=True)
    dist = root / "dist"
    dist.mkdir()
    adapter_entry = dist / "index.js"
    write_wrapper_adapter_entry(adapter_entry, scenario_path)
    (root / "package.json").write_text(json.dumps({
        "name": "@agentclientprotocol/claude-agent-acp",
        "version": package_version,
        "bin": {"claude-agent-acp": bin_entry},
    }), encoding="utf-8")
    return (_resolved_node_binary(), str(adapter_entry))


def build_codex_npm_install(
    root: Path, scenario_path: Path, *, package_version: str = "1.12.0", bundled_codex_version: str = "0.154.0-fixture",
    codex_debug_models_body: str | None = None,
) -> tuple[tuple[str, str], Path]:
    """A realistic codex-acp npm install under ``root`` (package.json +
    dist/index.js adapter entry, plus a bundled ``@openai/codex`` package
    and this host's own real platform/vendor binary path -- computed via
    ``engine.install_resolution``'s own platform table, so this layout is
    exactly what that production code expects to find). Returns
    ``((node_binary, adapter_entry), bundled_codex_binary_path)``;
    ``codex_debug_models_body`` (a Python source fragment) is written as the
    vendored ``codex`` "binary"'s own executable body -- ``codex_debug_
    models.fetch_effort_ceiling`` runs ``<bundled_codex_binary> debug
    models`` and parses its stdout, so a fixture that ignores argv and
    always prints the same JSON is sufficient."""
    from cortex_runtime.engine import install_resolution

    root.mkdir(parents=True, exist_ok=True)
    dist = root / "dist"
    dist.mkdir()
    adapter_entry = dist / "index.js"
    write_wrapper_adapter_entry(adapter_entry, scenario_path)
    (root / "package.json").write_text(json.dumps({
        "name": "@agentclientprotocol/codex-acp",
        "version": package_version,
        "bin": {"codex-acp": "dist/index.js"},
    }), encoding="utf-8")

    codex_pkg_dir = root / "node_modules" / "@openai" / "codex"
    codex_pkg_dir.mkdir(parents=True)
    (codex_pkg_dir / "package.json").write_text(
        json.dumps({"name": "@openai/codex", "version": bundled_codex_version}), encoding="utf-8",
    )

    triple_key = (platform.system(), platform.machine())
    target_triple = install_resolution._CODEX_TARGET_TRIPLE_BY_PLATFORM[triple_key]
    platform_package_name = install_resolution._CODEX_PLATFORM_PACKAGE[target_triple]
    vendor_bin_dir = root / "node_modules" / platform_package_name / "vendor" / target_triple / "bin"
    vendor_bin_dir.mkdir(parents=True)
    binary_name = "codex.exe" if platform.system() == "Windows" else "codex"
    bundled_codex_binary = vendor_bin_dir / binary_name
    body = codex_debug_models_body or "import sys; sys.exit(0)"
    _write_executable_file(bundled_codex_binary, f"#!/usr/bin/env python3\n{body}\n".encode("utf-8"))

    return (_resolved_node_binary(), str(adapter_entry)), bundled_codex_binary
