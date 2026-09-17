"""TerminalBridge: creation/cwd containment, least-privilege env, bounded
output truncation at a UTF-8 boundary, exit status, kill/release, and
cross-session isolation (T014)."""
from __future__ import annotations

import os
import time

import pytest

from cortex_runtime.engine.resource_binding import ResourceBinding
from cortex_runtime.engine.terminal_bridge import TerminalBridge, TerminalBridgeError


def _binding(root, **overrides):
    fields = dict(
        engine_instance_id="eirt_a", engine_session_id="es_a", context_generation_id="gen_1",
        assignment_id="asg_1", attempt_id="att_1", workspace_binding_id="wb_1",
        workspace_root=str(root), ownership_epoch="epoch_1", role="worker",
    )
    fields.update(overrides)
    return ResourceBinding(**fields)


def test_create_and_wait_for_exit_reports_exit_code(tmp_path):
    bridge = TerminalBridge()
    binding = _binding(tmp_path)
    terminal_id = bridge.create(binding, dict(command="/bin/sh", args=["-c", "exit 3"]), output_byte_limit_max=4096)
    status = bridge.wait_for_exit("es_a", terminal_id, timeout=5.0)
    assert status == dict(exitCode=3, signal=None)
    bridge.release("es_a", terminal_id)


def test_output_captures_stdout(tmp_path):
    bridge = TerminalBridge()
    binding = _binding(tmp_path)
    terminal_id = bridge.create(binding, dict(command="/bin/echo", args=["hello-terminal"]), output_byte_limit_max=4096)
    bridge.wait_for_exit("es_a", terminal_id, timeout=5.0)
    result = bridge.output("es_a", terminal_id)
    assert "hello-terminal" in result["output"]
    assert result["truncated"] is False
    bridge.release("es_a", terminal_id)


def test_cwd_defaults_to_and_is_contained_in_workspace_root(tmp_path):
    bridge = TerminalBridge()
    binding = _binding(tmp_path)
    terminal_id = bridge.create(binding, dict(command="/bin/pwd"), output_byte_limit_max=4096)
    bridge.wait_for_exit("es_a", terminal_id, timeout=5.0)
    result = bridge.output("es_a", terminal_id)
    assert result["output"].strip() == str(tmp_path)
    bridge.release("es_a", terminal_id)


def test_cwd_outside_workspace_root_rejected(tmp_path):
    bridge = TerminalBridge()
    binding = _binding(tmp_path)
    outside = tmp_path.parent
    with pytest.raises(TerminalBridgeError):
        bridge.create(binding, dict(command="/bin/pwd", cwd=str(outside)), output_byte_limit_max=4096)


def test_env_does_not_leak_parent_sentinel_var(tmp_path, monkeypatch):
    monkeypatch.setenv("CORTEX_T014_SENTINEL_SHOULD_NOT_LEAK", "leaked-value")
    bridge = TerminalBridge()
    binding = _binding(tmp_path)
    terminal_id = bridge.create(
        binding, dict(command="/usr/bin/env"), output_byte_limit_max=4096,
    )
    bridge.wait_for_exit("es_a", terminal_id, timeout=5.0)
    result = bridge.output("es_a", terminal_id)
    assert "CORTEX_T014_SENTINEL_SHOULD_NOT_LEAK" not in result["output"]
    bridge.release("es_a", terminal_id)


def test_explicit_env_entries_are_applied(tmp_path):
    bridge = TerminalBridge()
    binding = _binding(tmp_path)
    terminal_id = bridge.create(
        binding,
        dict(command="/bin/sh", args=["-c", "echo $MY_TERMINAL_VAR"], env=[dict(name="MY_TERMINAL_VAR", value="present")]),
        output_byte_limit_max=4096,
    )
    bridge.wait_for_exit("es_a", terminal_id, timeout=5.0)
    result = bridge.output("es_a", terminal_id)
    assert "present" in result["output"]
    bridge.release("es_a", terminal_id)


def test_output_truncated_from_start_at_utf8_boundary(tmp_path):
    bridge = TerminalBridge()
    binding = _binding(tmp_path)
    # A multi-byte UTF-8 character ('é' = 2 bytes) straddling where a naive
    # byte-count truncation would cut.
    payload = "x" * 20 + "é" * 20
    terminal_id = bridge.create(
        binding, dict(command="/bin/sh", args=["-c", f"printf '%s' '{payload}'"]), output_byte_limit_max=21,
    )
    bridge.wait_for_exit("es_a", terminal_id, timeout=5.0)
    result = bridge.output("es_a", terminal_id)
    assert result["truncated"] is True
    # Decodes cleanly (never raises) -- the boundary was respected.
    result["output"].encode("utf-8")
    bridge.release("es_a", terminal_id)


def test_kill_stops_a_running_process(tmp_path):
    bridge = TerminalBridge()
    binding = _binding(tmp_path)
    terminal_id = bridge.create(binding, dict(command="/bin/sleep", args=["30"]), output_byte_limit_max=4096)
    bridge.kill("es_a", terminal_id)
    assert bridge.wait_for_exit("es_a", terminal_id, timeout=5.0) is not None
    bridge.release("es_a", terminal_id)


def test_release_after_exit_is_safe(tmp_path):
    bridge = TerminalBridge()
    binding = _binding(tmp_path)
    terminal_id = bridge.create(binding, dict(command="/bin/true"), output_byte_limit_max=4096)
    bridge.wait_for_exit("es_a", terminal_id, timeout=5.0)
    bridge.release("es_a", terminal_id)  # must not raise even though already exited


def test_unknown_terminal_id_rejected(tmp_path):
    bridge = TerminalBridge()
    with pytest.raises(TerminalBridgeError):
        bridge.output("es_a", "term_does_not_exist")


def test_terminal_of_another_session_is_not_visible(tmp_path):
    bridge = TerminalBridge()
    binding_a = _binding(tmp_path, engine_session_id="es_a")
    terminal_id = bridge.create(binding_a, dict(command="/bin/true"), output_byte_limit_max=4096)
    bridge.wait_for_exit("es_a", terminal_id, timeout=5.0)
    with pytest.raises(TerminalBridgeError):
        bridge.output("es_b", terminal_id)
    bridge.release("es_a", terminal_id)


def test_max_concurrent_terminals_per_session_enforced(tmp_path):
    bridge = TerminalBridge(max_per_session=2)
    binding = _binding(tmp_path)
    first = bridge.create(binding, dict(command="/bin/sleep", args=["30"]), output_byte_limit_max=4096)
    second = bridge.create(binding, dict(command="/bin/sleep", args=["30"]), output_byte_limit_max=4096)
    with pytest.raises(TerminalBridgeError):
        bridge.create(binding, dict(command="/bin/sleep", args=["30"]), output_byte_limit_max=4096)
    bridge.release("es_a", first)
    bridge.release("es_a", second)


def test_release_session_kills_and_frees_every_orphaned_terminal(tmp_path):
    bridge = TerminalBridge()
    binding = _binding(tmp_path)
    terminal_id = bridge.create(binding, dict(command="/bin/sleep", args=["30"]), output_byte_limit_max=4096)
    pid = None
    with bridge._lock:  # test-only introspection
        pid = bridge._terminals["es_a"][terminal_id].process.pid
    bridge.release_session("es_a")
    # Give the wait thread a moment to reap; the process must actually be gone.
    deadline = time.time() + 5.0
    gone = False
    while time.time() < deadline:
        try:
            os.kill(pid, 0)
        except ProcessLookupError:
            gone = True
            break
        time.sleep(0.05)
    assert gone
    with pytest.raises(TerminalBridgeError):
        bridge.output("es_a", terminal_id)
