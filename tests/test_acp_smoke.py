"""End-to-end tests for ``scripts/cortex_acp_smoke.py`` (T011).

Runs the real smoke harness as a subprocess -- never imports its internals
directly -- from a tmp cwd outside this repository (see
``tests/conftest.py``'s T009-incident guard for why: the harness itself
also refuses to use the repo checkout as an ACP project root). Every run
uses only fake fixture data and never reaches a paid model.
"""
from __future__ import annotations

import json
import subprocess
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
SMOKE_SCRIPT = REPO_ROOT / "scripts" / "cortex_acp_smoke.py"


def _run_smoke(tmp_path, *extra_args):
    report_path = tmp_path / "smoke-report.json"
    proc = subprocess.run(
        [sys.executable, "-B", str(SMOKE_SCRIPT), "--report", str(report_path), *extra_args],
        cwd=str(tmp_path), capture_output=True, text=True, timeout=60)
    return proc, json.loads(report_path.read_text()) if report_path.exists() else None


def _assert_all_checks_passed(report):
    failed = [check for check in report["checks"] if check["status"] == "fail"]
    assert not failed, f"smoke harness reported failures: {failed}"
    assert report["all_passed"] is True


def test_smoke_without_connections_exits_clean(tmp_path):
    proc, report = _run_smoke(tmp_path)
    assert report is not None, f"no report written; stderr={proc.stderr}"
    _assert_all_checks_passed(report)
    assert proc.returncode == 0
    # No connection registered -> no model config option at all.
    checks_by_name = {check["name"]: check for check in report["checks"]}
    assert checks_by_name["initialize:image_prompt_capability"]["status"] == "pass"
    assert checks_by_name["session/new:no_model_option_without_connections"]["status"] == "pass"
    access = checks_by_name["session/new:access_mode_option_shape"]["detail"]
    assert access["currentValue"] == "development"
    assert {option["value"] for option in access["options"]} == {"development", "guarded", "auto"}
    assert report["versions"]["protocol_version"] == 1
    assert report["versions"]["cortex_version"] != "unknown"


def test_smoke_with_fixture_connection_shows_model_option(tmp_path):
    proc, report = _run_smoke(tmp_path, "--fixture-connection")
    assert report is not None, f"no report written; stderr={proc.stderr}"
    _assert_all_checks_passed(report)
    assert proc.returncode == 0
    checks_by_name = {check["name"]: check for check in report["checks"]}
    assert checks_by_name["session/new:configOptions_with_fixture"]["status"] == "pass"
    option_ids = {opt["id"] for opt in checks_by_name["session/new:configOptions_with_fixture"]["detail"]}
    assert option_ids == {"provider", "model", "thought_level", "language", "access_mode"}


def test_smoke_never_creates_a_task_for_cortex_status(tmp_path):
    proc, report = _run_smoke(tmp_path)
    assert report is not None, f"no report written; stderr={proc.stderr}"
    checks_by_name = {check["name"]: check for check in report["checks"]}
    assert checks_by_name["cortex_status:no_task_created"]["status"] == "pass"
    assert checks_by_name["cortex_status:no_task_created"]["detail"] == 0


def test_smoke_normal_prompt_fails_closed_no_hidden_provider(tmp_path):
    proc, report = _run_smoke(tmp_path)
    assert report is not None, f"no report written; stderr={proc.stderr}"
    checks_by_name = {check["name"]: check for check in report["checks"]}
    detail = checks_by_name["normal_prompt:orchestration_unavailable"]["detail"]
    assert detail["code"] == -32001
    assert detail["message"] == "orchestration_unavailable"
    assert checks_by_name["normal_prompt:orchestration_unavailable"]["status"] == "pass"


def test_smoke_report_written_to_requested_path(tmp_path):
    report_path = tmp_path / "custom-report.json"
    proc = subprocess.run(
        [sys.executable, "-B", str(SMOKE_SCRIPT), "--report", str(report_path)],
        cwd=str(tmp_path), capture_output=True, text=True, timeout=60)
    assert proc.returncode == 0
    assert report_path.exists()
    # Nothing protocol-related leaks onto this process's own stdout/stderr
    # capture beyond the harness's own diagnostics (stdout must stay empty:
    # the report goes to the file, not stdout).
    assert proc.stdout == ""


def test_smoke_never_touches_real_user_cortex_config(tmp_path, monkeypatch):
    """Guard: the harness must never read/create the real
    ``~/.config/cortex``/``~/.local/state/cortex`` -- point HOME at an
    empty tmp dir with no cortex config, and confirm nothing appears in it
    after a run (the harness must have used its own internal tmp HOME,
    never this one)."""
    fake_real_home = tmp_path / "fake-real-home"
    fake_real_home.mkdir()
    report_path = tmp_path / "report.json"
    env = {"HOME": str(fake_real_home), "PATH": __import__("os").environ.get("PATH", "")}
    proc = subprocess.run(
        [sys.executable, "-B", str(SMOKE_SCRIPT), "--report", str(report_path)],
        cwd=str(tmp_path), capture_output=True, text=True, timeout=60, env=env)
    assert proc.returncode == 0
    assert not (fake_real_home / ".config" / "cortex").exists()
    assert not (fake_real_home / ".local" / "state" / "cortex").exists()


def test_session_new_stays_fast_with_no_connections_configured(tmp_path):
    """Regression guard: a real run (no ``--fixture-connection``, the exact
    scenario ``test_smoke_never_touches_real_user_cortex_config`` exercises)
    must finish well inside the harness's own 10s per-message read timeout
    -- not a flaky wall-clock assertion, but a bound generous enough that
    only a genuine stall (an unhandled failure deep in ``session/new`` that
    silently drops the follow-up notification, a live external call on the
    session-creation path, etc.) could ever trip it; a merely slow but
    healthy machine still finishes in well under one second (see the
    measured ~20ms in the fix's own investigation notes)."""
    report_path = tmp_path / "report.json"
    env = {"HOME": str(tmp_path / "smoke-home"), "PATH": __import__("os").environ.get("PATH", "")}
    Path(env["HOME"]).mkdir()
    import time
    start = time.monotonic()
    proc = subprocess.run(
        [sys.executable, "-B", str(SMOKE_SCRIPT), "--report", str(report_path)],
        cwd=str(tmp_path), capture_output=True, text=True, timeout=60, env=env)
    elapsed = time.monotonic() - start
    assert proc.returncode == 0, f"stderr={proc.stderr}"
    assert elapsed < 5.0, f"session/new (and the rest of the smoke run) took {elapsed:.1f}s, expected < 5s"


def test_smoke_failure_prints_reason_to_stderr_without_polluting_stdout(tmp_path, capsys):
    """The second defect (see the task this fix closes): a failing run
    (exit 1) must never be silent -- the actual failing check(s) must be
    printed to stderr even when the caller never opens the --report file,
    while stdout (which the harness itself never writes protocol data to,
    unlike the agent's own stdout-purity contract -- see
    test_smoke_report_written_to_requested_path) stays untouched.

    Drives ``main()`` in-process (never a subprocess) with ``run_smoke``
    replaced by a scripted failure -- no real engine/session/connection is
    touched at all, this only exercises the harness's own report/stderr
    plumbing."""
    sys.path.insert(0, str(REPO_ROOT / "scripts"))
    import cortex_acp_smoke as smoke_module
    import importlib
    importlib.reload(smoke_module)

    def _fake_run_smoke(*, fixture_connection, report, keep_temp=False):
        report.record("session/new:has_session_id", False)
        raise smoke_module.SmokeFailure("scripted failure: no stdout line arrived in time")

    smoke_module.run_smoke = _fake_run_smoke
    report_path = tmp_path / "report.json"
    exit_code = smoke_module.main(["--report", str(report_path)])
    captured = capsys.readouterr()
    assert exit_code == 1
    assert captured.out == ""
    assert "session/new:has_session_id" in captured.err or "harness:fatal" in captured.err
    assert "scripted failure" in captured.err
    assert report_path.exists()


def test_smoke_removes_its_temporary_directories(tmp_path):
    """Every run used to leave its project and HOME directories behind
    (561 pairs, 16.6 GB under /var/tmp by 2026-10-15)."""
    import os
    root = Path(os.environ["TMPDIR"])
    proc, report = _run_smoke(tmp_path)
    assert proc.returncode == 0
    assert not [entry for entry in os.listdir(root) if entry.startswith("cortex-acp-smoke-")]
    proc, report = _run_smoke(tmp_path, "--keep-temp")
    kept = [entry for entry in os.listdir(root) if entry.startswith("cortex-acp-smoke-")]
    assert len(kept) == 2 and "kept" in proc.stderr
