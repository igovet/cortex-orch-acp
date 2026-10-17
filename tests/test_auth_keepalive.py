"""No-paid tests for scripts/cortex_auth_keepalive.py.

Every subprocess interaction is mocked; this suite never invokes a real
``claude``/``codex`` CLI binary and never reaches a paid model call.
"""
from __future__ import annotations

import importlib.util
import io
import json
import subprocess
import sys
from contextlib import redirect_stdout
from pathlib import Path

import pytest

SCRIPT_PATH = Path(__file__).resolve().parents[1] / "scripts" / "cortex_auth_keepalive.py"


def _load_module():
    spec = importlib.util.spec_from_file_location("cortex_auth_keepalive", SCRIPT_PATH)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


keepalive = _load_module()


def _fake_runner(returncode, *, raises=None):
    calls = []

    def runner(argv, **kwargs):
        calls.append((list(argv), kwargs))
        if raises is not None:
            raise raises
        return subprocess.CompletedProcess(argv, returncode=returncode)

    runner.calls = calls
    return runner


def test_default_engines_are_both_unsupported():
    # Investigation found no proven zero-cost refresh-forcing command for
    # either engine; the shipped defaults must not invent one.
    assert {spec.name for spec in keepalive.ENGINES} == {"claude", "codex"}
    for spec in keepalive.ENGINES:
        assert spec.command is None
        assert spec.unsupported_reason


def test_run_engine_unsupported_never_invokes_subprocess():
    spec = keepalive.EngineSpec(name="claude", command=None, unsupported_reason="no command")
    runner = _fake_runner(0)
    result = keepalive.run_engine(spec, runner=runner)
    assert result.status is keepalive.Status.UNSUPPORTED
    assert result.exit_code is None
    assert result.detail == "no command"
    assert runner.calls == []


def test_run_engine_success_reports_ok_and_discards_output():
    spec = keepalive.EngineSpec(name="codex", command=("codex", "login", "status"))
    runner = _fake_runner(0)
    result = keepalive.run_engine(spec, runner=runner)
    assert result.status is keepalive.Status.OK
    assert result.exit_code == 0
    argv, kwargs = runner.calls[0]
    assert argv == ["codex", "login", "status"]
    assert kwargs["stdout"] == subprocess.DEVNULL
    assert kwargs["stderr"] == subprocess.DEVNULL
    assert kwargs["stdin"] == subprocess.DEVNULL
    assert kwargs["check"] is False


def test_run_engine_nonzero_exit_reports_needs_login():
    spec = keepalive.EngineSpec(name="claude", command=("claude", "auth", "status"))
    runner = _fake_runner(1)
    result = keepalive.run_engine(spec, runner=runner)
    assert result.status is keepalive.Status.NEEDS_LOGIN
    assert result.exit_code == 1


def test_run_engine_timeout_reports_error_without_exit_code():
    spec = keepalive.EngineSpec(name="claude", command=("claude", "auth", "status"))
    runner = _fake_runner(0, raises=subprocess.TimeoutExpired(cmd="claude", timeout=30))
    result = keepalive.run_engine(spec, runner=runner, timeout_seconds=30)
    assert result.status is keepalive.Status.ERROR
    assert result.exit_code is None
    assert "timed out" in result.detail


def test_run_engine_missing_binary_reports_error():
    spec = keepalive.EngineSpec(name="codex", command=("codex", "login", "status"))
    runner = _fake_runner(0, raises=FileNotFoundError("codex"))
    result = keepalive.run_engine(spec, runner=runner)
    assert result.status is keepalive.Status.ERROR
    assert result.exit_code is None
    assert "FileNotFoundError" in result.detail


def test_overall_exit_code_ok_when_only_unsupported():
    results = keepalive.run_all()
    assert all(r.status is keepalive.Status.UNSUPPORTED for r in results)
    assert keepalive.overall_exit_code(results) == 0


def test_overall_exit_code_nonzero_on_needs_login():
    results = [
        keepalive.EngineResult(engine="claude", status=keepalive.Status.OK, exit_code=0, detail="ok"),
        keepalive.EngineResult(engine="codex", status=keepalive.Status.NEEDS_LOGIN, exit_code=1, detail="needs login"),
    ]
    assert keepalive.overall_exit_code(results) == 1


def test_overall_exit_code_nonzero_on_error():
    results = [
        keepalive.EngineResult(engine="claude", status=keepalive.Status.ERROR, exit_code=None, detail="boom"),
    ]
    assert keepalive.overall_exit_code(results) == 1


def test_render_is_sanitized_json_without_command_output():
    results = [
        keepalive.EngineResult(engine="claude", status=keepalive.Status.UNSUPPORTED, exit_code=None, detail="no command"),
    ]
    rendered = keepalive.render(results)
    payload = json.loads(rendered)
    assert payload == {
        "results": [
            {
                "engine": "claude",
                "status": "unsupported",
                "exit_code": None,
                "detail": "no command",
            }
        ]
    }


def test_main_prints_json_and_returns_zero_when_all_unsupported(monkeypatch):
    buffer = io.StringIO()
    with redirect_stdout(buffer):
        exit_code = keepalive.main([])
    assert exit_code == 0
    payload = json.loads(buffer.getvalue())
    engines = {entry["engine"] for entry in payload["results"]}
    assert engines == {"claude", "codex"}
    for entry in payload["results"]:
        assert entry["status"] == "unsupported"


def test_main_runs_only_mocked_engines_and_never_spawns_real_cli(monkeypatch):
    def forbidden_run(*args, **kwargs):  # pragma: no cover - defensive
        raise AssertionError("real subprocess.run must never be invoked by this test")

    monkeypatch.setattr(subprocess, "run", forbidden_run)
    # The default ENGINES are unsupported, so main() must not call subprocess
    # at all; this asserts that invariant directly.
    buffer = io.StringIO()
    with redirect_stdout(buffer):
        exit_code = keepalive.main([])
    assert exit_code == 0


def test_cli_help_does_not_crash(monkeypatch):
    with pytest.raises(SystemExit) as excinfo:
        keepalive.main(["--help"])
    assert excinfo.value.code == 0
