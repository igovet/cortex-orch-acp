"""Regression coverage for best-effort durable prompt crash diagnostics."""
from __future__ import annotations

from cortex_runtime.acp.agent import _write_crash_diagnostic


def _captured_crash():
    try:
        raise LookupError("crash marker for traceback")
    except LookupError as exc:
        return exc


def test_writes_exception_type_and_traceback_without_environment_secrets(tmp_path):
    state_home = tmp_path / "isolated-state"
    env = {
        "HOME": str(tmp_path / "isolated-home"),
        "XDG_STATE_HOME": str(state_home),
        "API_TOKEN": "credential-value-do-not-write",
        "PASSWORD": "secret-looking-value-do-not-write",
    }

    _write_crash_diagnostic(env, _captured_crash())

    files = list((state_home / "cortex" / "agent-crashes").glob("*.log"))
    assert len(files) == 1
    diagnostic = files[0].read_text(encoding="utf-8")
    assert "LookupError" in diagnostic
    assert "crash marker for traceback" in diagnostic
    assert "Traceback (most recent call last)" in diagnostic
    assert "credential-value-do-not-write" not in diagnostic
    assert "secret-looking-value-do-not-write" not in diagnostic


def test_unwritable_diagnostic_location_is_best_effort(tmp_path):
    state_home_blocker = tmp_path / "state-home-is-a-file"
    state_home_blocker.write_text("block directory creation", encoding="utf-8")
    env = {
        "HOME": str(tmp_path / "isolated-home"),
        "XDG_STATE_HOME": str(state_home_blocker),
    }

    _write_crash_diagnostic(env, _captured_crash())


def test_missing_xdg_state_home_uses_explicit_temporary_home(tmp_path):
    home = tmp_path / "isolated-home"
    env = {"HOME": str(home)}

    _write_crash_diagnostic(env, _captured_crash())

    files = list((home / ".local" / "state" / "cortex" / "agent-crashes").glob("*.log"))
    assert len(files) == 1
