"""A sandboxed engine scans its whole project before it starts
(orchestration.sandbox.gateway_command). On a large tree on a cold HDD that
took minutes with nothing in the chat; a home directory as the project took
283 s before the launch was stopped. The scan now reports its progress, and
a sandboxed engine is never launched in the home directory or /."""
from __future__ import annotations

from types import SimpleNamespace

import pytest

from cortex_runtime.acp.orchestrating_runtime import OrchestratingRuntime
from cortex_runtime.engine.process import EngineProcess
from cortex_runtime.orchestration import sandbox
from cortex_runtime.portable.errors import PortableError

from engine_fakes import default_scenario, fake_launch_command, isolated_environ, make_connection_record, new_connection_id, write_scenario


def test_a_slow_scan_reports_progress_and_its_end(tmp_path, monkeypatch):
    root = tmp_path / "project"
    for index in range(30):
        (root / f"d{index}").mkdir(parents=True)
        (root / f"d{index}" / "file.txt").write_text("x")
    scratch = tmp_path / "scratch"
    scratch.mkdir()
    monkeypatch.setattr(sandbox, "SCAN_REPORT_AFTER_SECONDS", 0.0)
    monkeypatch.setattr(sandbox, "SCAN_REPORT_EVERY_SECONDS", 0.0)
    reports = []
    token = sandbox.SCAN_PROGRESS.set(lambda entries, seconds, done: reports.append((entries, done)))
    try:
        with sandbox.gateway_command(root, scratch, ["/bin/true"], ["."], system_root=True):
            pass
    finally:
        sandbox.SCAN_PROGRESS.reset(token)
    assert reports and not reports[0][1]
    assert reports[-1] == (60, True)
    assert [entries for entries, _ in reports] == sorted(entries for entries, _ in reports)


def test_a_fast_scan_reports_nothing(tmp_path):
    root = tmp_path / "project"
    root.mkdir()
    (root / "a.txt").write_text("x")
    scratch = tmp_path / "scratch"
    scratch.mkdir()
    reports = []
    token = sandbox.SCAN_PROGRESS.set(lambda *args: reports.append(args))
    try:
        with sandbox.gateway_command(root, scratch, ["/bin/true"], ["."], system_root=True):
            pass
    finally:
        sandbox.SCAN_PROGRESS.reset(token)
    assert reports == []


@pytest.mark.parametrize("where", ["home", "root"])
def test_a_sandboxed_engine_is_never_launched_in_home_or_root(tmp_path, where):
    environ = isolated_environ(tmp_path)
    record = make_connection_record(new_connection_id(), environ=environ,
                                    launch_command=fake_launch_command(write_scenario(tmp_path, default_scenario())))
    cwd = environ["HOME"] if where == "home" else "/"
    with pytest.raises(PortableError) as refused:
        EngineProcess(record, cwd, environ=environ, jail=dict(scopes=["."], readonly=[]))
    assert refused.value.code == "engine_cwd_is_home" and not refused.value.retry_eligible


def test_the_scan_card_opens_updates_and_closes():
    runtime = OrchestratingRuntime.__new__(OrchestratingRuntime)
    import threading
    runtime._lock = threading.Lock()
    cards = []
    runtime._task_tool_call_emitters = {"t1": cards.append}
    runtime._resolve_language = lambda task_id: "en"
    runtime._sandbox_scan_progress("t1", "a1", "as1", 1000, 3.4, False)
    runtime._sandbox_scan_progress("t1", "a1", "as1", 5000, 8.6, False)
    runtime._sandbox_scan_progress("t1", "a1", "as1", 9000, 12.2, True)
    assert [card["new"] for card in cards] == [True, False, False]
    assert cards[0]["title"] == "Preparing the sandbox (worker)"
    assert "1000 entries, 3 s" in cards[0]["text"] and "not a hang" in cards[0]["text"]
    assert cards[-1]["status"] == "completed" and "9000 entries in 12 s" in cards[-1]["text"]
    assert len({card["tool_call_id"] for card in cards}) == 1
