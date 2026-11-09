"""Executable declarations use the actual verification launch seam; no engines."""
import contextlib
import subprocess

import pytest

from cortex_runtime.contracts import StoreError
from cortex_runtime.orchestration import controller
from cortex_runtime.orchestration.check_preflight import probe, require_executable


@pytest.fixture
def launch(monkeypatch):
    calls = []
    @contextlib.contextmanager
    def direct(root, scratch, argv):
        calls.append((str(root), argv))
        yield argv, ()
    monkeypatch.setattr(controller, "_verification_command", direct)
    return calls


def test_probe_is_not_a_check_and_never_executes_the_declared_program(tmp_path, launch):
    script = tmp_path / "check"
    marker = tmp_path / "ran"
    script.write_text("#!/bin/sh\ntouch " + str(marker) + "\n")
    script.chmod(0o700)
    assert probe(tmp_path, [str(script)])["available"] is True
    assert not marker.exists()
    assert launch[0][0] == str(tmp_path)


def test_env_shaped_program_and_loader_arguments_are_never_executed(tmp_path, launch):
    marker = tmp_path / "ran"
    env_program = tmp_path / "env"
    env_program.write_text("#!/bin/sh\ntouch " + str(marker) + "\n")
    env_program.chmod(0o700)
    assert probe(tmp_path, [str(env_program), "LD_PRELOAD=/no-library", "/bin/sh"])["available"]
    assert not marker.exists()
    assert str(env_program) not in launch[0][1][:3]
    assert "LD_PRELOAD=/no-library" not in launch[0][1]


def test_missing_command_gets_private_install_or_question_guidance(tmp_path, launch):
    result = probe(tmp_path, ["cortex-fixture-no-such-check"])
    assert result["available"] is False
    assert "project-private" in result["message"]
    assert "ask_question" in result["message"]
    with pytest.raises(StoreError, match="check_executable_unavailable"):
        require_executable(tmp_path, [["cortex-fixture-no-such-check"]])


def test_wrappers_do_not_hide_a_missing_program(tmp_path, launch):
    assert not probe(tmp_path, ["timeout", "3", "cortex-fixture-no-such-check"])["available"]
    assert not probe(tmp_path, ["env", "PATH=/no-tools", "python3", "-V"])["available"]
    assert not probe(tmp_path, ["env", "PATH=/no-tools", "cd"])["available"]


def test_plan_refuses_missing_program_before_recording_or_dispatch(tmp_path, launch):
    from cortex_runtime.bootstrap import bootstrap_task
    from cortex_runtime.orchestration.state import TaskMachine
    from cortex_runtime.portable.identity import SourceEvent
    from cortex_runtime.store import Store
    from cortex_runtime.trusted_channel import resolve_channel
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    task = bootstrap_task(store, SourceEvent(origin="preflight", event_id="plan"), "Implement", "owner")
    owner = resolve_channel(store, task.channel_id, task.secret)
    TaskMachine(store).create(task.task_id, "Implement")
    plan = dict(tasks=[dict(id="work", role="fullstack-dev", objective="Implement", scope=["src"], dependencies=[], model="auto")],
                verification=[["cortex-fixture-no-such-check"]])
    with pytest.raises(StoreError, match="check_executable_unavailable"):
        TaskMachine(store).submit_plan(owner, plan, "plan")
    with store.connection() as db:
        assert db.execute("SELECT state FROM runtime_tasks WHERE task_id=?", (task.task_id,)).fetchone()[0] == "PLANNING"
        assert db.execute("SELECT count(*) FROM runtime_nodes WHERE task_id=?", (task.task_id,)).fetchone()[0] == 0


def test_declaration_and_result_refusal_leave_worker_available_for_questions(tmp_path, launch):
    from cortex_runtime.bootstrap import bootstrap_task
    from cortex_runtime.portable.identity import SourceEvent
    from cortex_runtime.store import Store
    from cortex_runtime.trusted_channel import resolve_channel
    from cortex_runtime.delegation import mcp_tools
    from runtime_scenarios import provision_fixture
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    task = bootstrap_task(store, SourceEvent(origin="preflight", event_id="one"), "Implement", "owner")
    owner = resolve_channel(store, task.channel_id, task.secret)
    worker, _ = provision_fixture(store, owner)
    offered = {t["name"] for t in mcp_tools.tool_definitions("worker", role_id="worker-fullstack-dev")}
    assert "check_preflight" in offered
    reply = mcp_tools.dispatch(store, worker, "check_preflight", {"argv": ["cortex-fixture-no-such-check"]})
    assert reply["available"] is False
    with pytest.raises(StoreError, match="check_executable_unavailable"):
        mcp_tools.dispatch(store, worker, "result_submit", dict(
            report="# Result\\n\\nNo work claimed.", summary="Blocked check", request_key="missing",
            checks=[dict(name="Missing", outcome="not_run", command=["cortex-fixture-no-such-check"], blocker="Tool missing")]))
    assert mcp_tools.dispatch(store, worker, "check_preflight", {"argv": ["/bin/sh"]})["available"]


def test_probe_failure_is_not_inferred_available(tmp_path, launch, monkeypatch):
    monkeypatch.setattr(subprocess, "run", lambda *a, **k: (_ for _ in ()).throw(subprocess.TimeoutExpired("probe", 1)))
    result = probe(tmp_path, ["python3"])
    assert result["available"] is False
    assert result["reason"] == "probe_unavailable"
    with pytest.raises(StoreError, match="check_preflight_unavailable"):
        require_executable(tmp_path, [["python3"]])


@pytest.mark.real_verification_sandbox
def test_probe_uses_the_same_final_launch_as_verification(tmp_path, monkeypatch):
    from sandbox_fakes import unjailed_gateway_argv
    from cortex_runtime.orchestration import sandbox
    # Exercise the production environment/cwd builder; only replace the final
    # namespace launch, exactly as the fake-engine service fixture does.
    monkeypatch.setattr(sandbox, "compact_argv", unjailed_gateway_argv)
    tools = tmp_path / ".venv/bin"
    tools.mkdir(parents=True)
    program = tools / "project-check"
    program.write_text("#!/bin/sh\nexit 99\n")
    program.chmod(0o700)
    assert probe(tmp_path, ["project-check"])["available"]
    assert probe(tmp_path, ["python3", "-B", "-c", "pass"])["available"]
    assert not probe(tmp_path, ["cortex-fixture-no-such-check"])["available"]


def test_relative_program_is_resolved_from_the_check_directory(tmp_path, launch):
    program = tmp_path / "check"
    program.write_text("#!/bin/sh\nexit 99\n")
    program.chmod(0o700)
    assert probe(tmp_path, ["./check"])["available"]
    assert probe(tmp_path, ["env", "PATH=.", "check"])["available"]
    assert not probe(tmp_path, ["./absent"])["available"]


def test_no_declared_program_is_not_available(tmp_path, launch):
    for argv in ([], ["env", "PATH=/usr/bin"], ["timeout", "3"]):
        assert not probe(tmp_path, argv)["available"]


@pytest.mark.parametrize("interrupted", [False, True])
def test_compacted_probe_closes_only_owned_descriptors(tmp_path, launch, monkeypatch, interrupted):
    import os
    from cortex_runtime.orchestration import sandbox
    held = os.open("/dev/null", os.O_RDONLY)
    extra = os.open("/dev/null", os.O_RDONLY)
    def compact(command, fds):
        return command, (held, extra)
    monkeypatch.setattr(sandbox, "compact_argv", compact)
    @contextlib.contextmanager
    def prepared(root, scratch, argv):
        yield argv, (held,)
    monkeypatch.setattr(controller, "_verification_command", prepared)
    if interrupted:
        monkeypatch.setattr(subprocess, "run", lambda *a, **k: (_ for _ in ()).throw(subprocess.TimeoutExpired("probe", 1)))
    try:
        assert probe(tmp_path, ["/bin/sh"])["available"] is not interrupted
        os.fstat(held)
        with pytest.raises(OSError):
            os.fstat(extra)
    finally:
        os.close(held)
        try:
            os.close(extra)
        except OSError:
            pass


@pytest.mark.parametrize("exit_code", [1, 126, 127])
def test_failed_probe_launch_is_not_a_missing_program(tmp_path, launch, monkeypatch, exit_code):
    monkeypatch.setattr(subprocess, "run", lambda *a, **k: subprocess.CompletedProcess(a[0], exit_code))
    result = probe(tmp_path, ["/bin/sh"])
    assert result["available"] is False
    assert result["reason"] == "probe_unavailable"
    with pytest.raises(StoreError, match="check_preflight_unavailable"):
        require_executable(tmp_path, [["/bin/sh"]])
