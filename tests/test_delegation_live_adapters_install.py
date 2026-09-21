"""EngineSessionManagerAdapter.open re-verifies a connection's own
launch_command before spawning a real engine, for both codex-acp and
claude-agent-acp (T035 follow-up: "a connection's launch command is
trusted verbatim, never re-verified" -- this is exactly how a wrong entry
point, a library module with no stdin listener instead of the package's
declared bin, reached live testing).

Every install here is a synthetic, owned fixture built fresh under
``tmp_path`` (see ``engine_fakes.build_codex_npm_install``/
``build_claude_npm_install``) -- never a reference to a real installed
engine, and no real node/npm engine is ever spawned by these tests: the
whole point is that a drifted/wrong-entry install is refused *before* any
process is started.
"""
from __future__ import annotations

import pytest

from cortex_runtime.delegation.live_adapters import EngineSessionManagerAdapter
from cortex_runtime.portable.errors import PortableError

from engine_fakes import (
    _resolved_node_binary,
    build_claude_npm_install,
    build_codex_npm_install,
    default_scenario,
    fake_launch_command,
    isolated_environ,
    make_connection_record,
    new_connection_id,
    write_scenario,
)


def _adapter(record, tmp_path, environ):
    return EngineSessionManagerAdapter(
        lambda task_id: record, lambda task_id: str(tmp_path), environ=environ, call_timeout=5.0,
    )


def _open_launch(attempt_suffix: str) -> dict:
    return dict(task_id="t_" + "1" * 12, attempt_id="xa_" + attempt_suffix)


# --- unresolvable (this repository's own fake-engine doubles) never blocks -


def test_open_launches_unchanged_when_launch_command_is_not_install_shaped(tmp_path):
    """The pre-existing, still-supported case: a launch_command that isn't
    install-shaped at all (every fake-engine test double in this suite)
    resolves to install=None and launches exactly as before this feature
    existed -- proven here directly (not just "the rest of the suite still
    passes")."""
    environ = isolated_environ(tmp_path)
    scenario_path = write_scenario(tmp_path, default_scenario())
    connection_id = new_connection_id()
    record = make_connection_record(connection_id, launch_command=fake_launch_command(scenario_path), environ=environ)
    adapter = _adapter(record, tmp_path, environ)

    assert adapter._resolve_launch_install(record) is None


# --- resolvable but drifted: fail closed, for both engines -----------------


def test_open_fails_closed_for_codex_acp_wrong_entry_point(tmp_path):
    environ = isolated_environ(tmp_path)
    scenario_path = write_scenario(tmp_path, default_scenario())
    install_root = tmp_path / "install"
    build_codex_npm_install(install_root, scenario_path)
    wrong_entry = install_root / "dist" / "other.js"
    wrong_entry.write_text("// never named by package.json's own bin field\n", encoding="utf-8")
    launch_command = (_resolved_node_binary(), str(wrong_entry))
    connection_id = new_connection_id()
    record = make_connection_record(connection_id, launch_command=launch_command, engine="codex-acp", environ=environ)
    adapter = _adapter(record, tmp_path, environ)

    with pytest.raises(PortableError) as excinfo:
        adapter.open("shared_project_root", _open_launch("aaaaaaaaaaaa"), "instructions")
    assert excinfo.value.code == "engine_install_drifted"
    assert not adapter._managers  # refused before a manager/process was ever created


def test_open_fails_closed_for_claude_agent_acp_wrong_entry_point(tmp_path):
    """The exact live-qualification bug report: package.json declares
    "bin": {"claude-agent-acp": "dist/index.js"}, but launch_command points
    at a different file under the same package (e.g. dist/lib.js, a
    library module with no stdin listener) -- open() must refuse before
    spawning it, not just codex_install's own pre-existing check."""
    environ = isolated_environ(tmp_path)
    scenario_path = write_scenario(tmp_path, default_scenario())
    install_root = tmp_path / "install"
    build_claude_npm_install(install_root, scenario_path)
    wrong_entry = install_root / "dist" / "lib.js"
    wrong_entry.write_text("// a library module, never a runnable entry point\n", encoding="utf-8")
    launch_command = (_resolved_node_binary(), str(wrong_entry))
    connection_id = new_connection_id()
    record = make_connection_record(
        connection_id, launch_command=launch_command, engine="claude-agent-acp", environ=environ,
    )
    adapter = _adapter(record, tmp_path, environ)

    with pytest.raises(PortableError) as excinfo:
        adapter.open("shared_project_root", _open_launch("bbbbbbbbbbbb"), "instructions")
    assert excinfo.value.code == "engine_install_drifted"
    assert not adapter._managers


# --- resolvable and verified: launches, and the verified install's own ----
# --- cross-check inside EngineProcess is exercised end to end -------------


def test_open_launches_through_a_verified_codex_install(tmp_path):
    environ = isolated_environ(tmp_path)
    scenario_path = write_scenario(tmp_path, default_scenario(
        prompt_updates=[dict(sessionUpdate="agent_message_chunk", content=dict(type="text", text="ok"))],
        prompt_stop_reason="end_turn",
    ))
    launch_command, _binary = build_codex_npm_install(
        tmp_path / "install", scenario_path, private_adapter_shape=True,
    )
    connection_id = new_connection_id()
    record = make_connection_record(connection_id, launch_command=launch_command, engine="codex-acp", environ=environ)
    adapter = _adapter(record, tmp_path, environ)
    try:
        session_id = adapter.open("shared_project_root", _open_launch("cccccccccccc"), "instructions")
        assert session_id
        assert adapter._managers  # a real manager/process was created this time
    finally:
        for manager in list(adapter._managers.values()):
            manager.shutdown(grace=2.0)
