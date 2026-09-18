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
    assert checks_by_name["session/new:no_model_option_without_connections"]["status"] == "pass"
    assert report["versions"]["protocol_version"] == 1
    assert report["versions"]["cortex_plugin_version"] != "unknown"


def test_smoke_with_fixture_connection_shows_model_option(tmp_path):
    proc, report = _run_smoke(tmp_path, "--fixture-connection")
    assert report is not None, f"no report written; stderr={proc.stderr}"
    _assert_all_checks_passed(report)
    assert proc.returncode == 0
    checks_by_name = {check["name"]: check for check in report["checks"]}
    assert checks_by_name["session/new:configOptions_with_fixture"]["status"] == "pass"
    option_ids = {opt["id"] for opt in checks_by_name["session/new:configOptions_with_fixture"]["detail"]}
    assert option_ids == {"provider", "model", "thought_level", "language"}


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
