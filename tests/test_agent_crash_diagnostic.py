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


def test_writes_launch_stage_and_identities_when_supplied(tmp_path):
    """P1 diagnosable launch failure: a coordinator engine-launch crash
    (see acp.orchestrating_runtime._open_coordinator_session) carries a
    task_id already durably bound but no runtime_tasks row -- the
    diagnostic must name the stage and every identity it was actually
    given, plus this running process's own package version, so an
    operator never has to correlate a bare traceback by hand."""
    state_home = tmp_path / "isolated-state"
    env = {"HOME": str(tmp_path / "isolated-home"), "XDG_STATE_HOME": str(state_home)}

    _write_crash_diagnostic(
        env, _captured_crash(), stage="engine_launch",
        task_id="t_abc123", session_id="s_def456", attempt_id="co_ghi789",
    )

    files = list((state_home / "cortex" / "agent-crashes").glob("*.log"))
    assert len(files) == 1
    diagnostic = files[0].read_text(encoding="utf-8")
    assert "stage: engine_launch" in diagnostic
    assert "task_id: t_abc123" in diagnostic
    assert "session_id: s_def456" in diagnostic
    assert "attempt_id: co_ghi789" in diagnostic
    assert "runtime_package_version:" in diagnostic
    assert "LookupError" in diagnostic


def test_omitted_launch_context_defaults_to_prompt_stage_and_no_identity_lines(tmp_path):
    """Every pre-existing caller (no stage/identities supplied) keeps the
    same header shape it always had -- just the added package version and
    an honest 'prompt' default stage, never a fabricated identity."""
    state_home = tmp_path / "isolated-state"
    env = {"HOME": str(tmp_path / "isolated-home"), "XDG_STATE_HOME": str(state_home)}

    _write_crash_diagnostic(env, _captured_crash())

    diagnostic = next((state_home / "cortex" / "agent-crashes").glob("*.log")).read_text(encoding="utf-8")
    assert "stage: prompt" in diagnostic
    assert "task_id:" not in diagnostic
    assert "session_id:" not in diagnostic
    assert "attempt_id:" not in diagnostic
    assert "runtime_package_version:" in diagnostic
