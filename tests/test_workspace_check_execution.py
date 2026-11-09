"""Focused worker-check execution/evidence regressions; never native engine calls."""
import json
from pathlib import Path
import threading
from types import SimpleNamespace

import pytest

from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.contracts import StoreError
from cortex_runtime.delegation import mcp_tools
from cortex_runtime.orchestration import controller, worker_checks
from cortex_runtime.orchestration.state import TaskMachine, _record_execution_baseline
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.store import Store
from cortex_runtime.trusted_channel import resolve_channel
from runtime_scenarios import provision_fixture, publish_fixture_report
from test_workspace_safety_journeys import journey

pytestmark = pytest.mark.usefixtures("unjailed_gateway_command")


@pytest.fixture
def prepared(tmp_path, request, monkeypatch):
    project = tmp_path / "project"
    project.mkdir()
    marker = tmp_path / "blocked"
    argv = ["python3", "-B", "-c", f"from pathlib import Path; assert not Path({str(marker)!r}).exists()"]
    mode = getattr(request, "param", "gap")
    if isinstance(mode, tuple):
        mode, has_git = mode
        if has_git:
            from test_workspace_safety_journeys import git
            git(project, "init", "-q")
            from cortex_runtime.workspace_api.task_options import exclude_helpers
            exclude_helpers(project)
    if mode == "missing":
        argv = ["cortex-fixture-missing-executable"]
    elif mode == "slow":
        argv = ["python3", "-B", "-c", "import time; time.sleep(10)"]
    store = Store(project / ".cortex", project_root=project)
    task = bootstrap_task(store, SourceEvent(origin="checks", event_id="one"), "Implement", "owner")
    coordinator = resolve_channel(store, task.channel_id, task.secret)
    worker, assignment = provision_fixture(store, coordinator)
    machine = TaskMachine(store)
    with store.connection() as db:
        plan = dict(tasks=[], verification=[["python3", "-B", "-c", "print('required')"]])
        db.execute("UPDATE runtime_tasks SET plan_json=? WHERE task_id=?", (json.dumps(plan), task.task_id))
        _record_execution_baseline(db, task.task_id)
        db.commit()
    (project / "feature.py").write_text("pass\n")
    if mode in ("failed", "subdir-failed"):
        marker.touch()
        directory = project / "checks" if mode == "subdir-failed" else project
        directory.mkdir(exist_ok=True)
        receipt = mcp_tools.dispatch(store, worker, "command_run", dict(argv=argv, cwd=str(directory), request_key="initial"))
        assert receipt["exit_code"] != 0
        check = dict(name="Required check", outcome="failed", tool_call_ref=receipt["tool_call_ref"])
    elif mode == "withdrawn":
        receipt = mcp_tools.dispatch(store, worker, "command_run", dict(
            argv=["python3", "-B", "-c", "raise SystemExit(1)"], request_key="probe"))
        check = dict(name="Exploratory probe", outcome="failed", tool_call_ref=receipt["tool_call_ref"],
                     probe_withdrawn="Expected nonzero exploratory response, not a project requirement.")
    else:
        check = dict(name="Required check", outcome="not_run", command=argv, blocker="paid live check not requested by the owner" if mode == "policy" else "Worker omitted the check.")
    args = dict(report=publish_fixture_report(store, worker, "Worker evidence."),
                summary="Changed feature.", checks=[check], request_key="result")
    if mode in ("failed", "subdir-failed"):
        args["assignment_outcome"] = "failed"
    if mode == "missing":
        # Retained legacy results predate declaration-time availability validation.
        with monkeypatch.context() as legacy:
            legacy.setattr("cortex_runtime.orchestration.check_preflight.require_executable", lambda *a: None)
            mcp_tools.dispatch(store, worker, "result_submit", args)
    else:
        mcp_tools.dispatch(store, worker, "result_submit", args)
    reviewer, review_ref = provision_fixture(store, coordinator, key="review", role="code-reviewer")
    with store.connection() as db:
        db.execute("UPDATE runtime_nodes SET stage='review' WHERE assignment_ref=?", (review_ref,))
        db.execute("UPDATE runtime_tasks SET state='REVIEWING' WHERE task_id=?", (task.task_id,))
        db.commit()
    digest = machine.start_review(task.task_id, review_ref)
    review_checks = []
    if mode == "review-search":
        receipt = mcp_tools.dispatch(store, reviewer, "command_run", dict(
            argv=["grep", "-n", "pass", "feature.py"], request_key="search-callers"))
        review_checks = [dict(name="Search Python sources for multiply callers", outcome="passed",
                              tool_call_ref=receipt["tool_call_ref"])]
    mcp_tools.dispatch(store, reviewer, "result_submit", dict(
        report=publish_fixture_report(store, reviewer, "Reviewed."), summary="Reviewed.",
        checks=review_checks, verdict="APPROVE", request_key="review"))
    machine.move(task.task_id, "VERIFYING")
    runtime = SimpleNamespace(_store=store, _task_workspace_root=lambda _: str(project))
    observation = controller._verification_observation(
        runtime, task.task_id, machine.context(task.task_id), digest, None, threading.Event(), 12345)
    assert machine.record_verification(task.task_id, observation["checks"], digest)
    with store.connection() as db:
        target = worker_checks.targets(db, task.task_id, machine.context(task.task_id)["revision"])[0]
    return runtime, task.task_id, target, marker


@pytest.mark.parametrize("prepared", ["subdir-failed"], indirect=True)
def test_rerun_preserves_the_receipted_subdirectory(prepared):
    runtime, task_id, target, marker = prepared
    assert target["cwd"].endswith("/checks")
    marker.unlink()
    worker_checks.execute(runtime, task_id, target, "subdirectory")
    assert closures(runtime, task_id)
    assert executions(runtime, task_id)[-1]["cwd"] == target["cwd"]


def test_legacy_commandless_item_is_manual_and_not_automatically_launched(prepared, monkeypatch):
    runtime, task_id, target, _ = prepared
    with runtime._store.connection() as db:
        node = db.execute("SELECT result_json FROM runtime_nodes WHERE assignment_ref=?", (target["assignment"],)).fetchone()
        result = json.loads(node[0])
        result["checks"][0].pop("command")
        db.execute("UPDATE runtime_nodes SET result_json=? WHERE assignment_ref=?", (json.dumps(result), target["assignment"]))
        db.commit()
        mapped = worker_checks.targets(db, task_id, 1)[0]
    assert mapped["kind"] == "manual"
    monkeypatch.setattr(worker_checks, "execute", lambda *a, **k: pytest.fail("manual item launched"))
    worker_checks.automatic(runtime, task_id, threading.Event(), None)


@pytest.mark.parametrize("prepared", [(mode, has_git) for mode in ("gap", "missing", "failed", "withdrawn")
                                     for has_git in (False, True)], indirect=True)
def test_completed_review_actions_and_acceptance_with_or_without_git(prepared, tmp_path):
    from test_workspace_task_git import helper
    from cortex_runtime.workspace_api import WorkspaceAPI, task_options
    from cortex_runtime.workspace_api.storage import bind_execution_tx
    from test_workspace_api import create_command, query
    runtime, task_id, target, _ = prepared
    store = runtime._store
    worker_checks.automatic(runtime, task_id, threading.Event(), None)
    api = WorkspaceAPI(store, project_id="project-1", actor_id="local-owner")
    api.activate(request_id="activate")
    public_task = api.handle(create_command())["target"]
    assert task_options.prepare(store, public_task, {}) == store.project_root
    def finish(db):
        db.execute("INSERT INTO acp_host_sessions VALUES ('review-session',?,?,'{}','{}','[]','now',NULL)",
                   (store.project_root, task_id))
        execution = bind_execution_tx(db, public_task, task_id, "review-session")
        db.execute("UPDATE runtime_tasks SET state='COMPLETED',synthesis='Reviewed result',blocker=NULL WHERE task_id=?",
                   (task_id,))
        return execution
    result_id = "wr_" + store.immediate_write(finish)
    projected = api.handle(query("results.get", result_id))
    product = helper.product_read(store.project_root, tmp_path / "owner-state", api.project_id)
    actions = product["tasks"][public_task]["reviewActions"]
    assert actions["requestChanges"] is True
    if target["argv"][0] == "cortex-fixture-missing-executable" or target["outcome"] == "failed":
        assert actions["accept"] is False and actions["unblockAction"] == "rerun_checks"
        assert actions["reason"]
        with pytest.raises(helper.Refused, match="checks_required"):
            helper.task_action(store.project_root, api.project_id, tmp_path / "owner-state",
                               dict(action="accept", taskId=public_task, version=projected["version"],
                                    digest=projected["observedWorkspaceDigest"]))
        return
    else:
        assert actions["accept"] is True
    assert projected["acceptanceAvailable"] is True
    params = dict(action="accept", taskId=public_task, version=projected["version"],
                  digest=projected["observedWorkspaceDigest"], acknowledgeOpenChecks=True)
    if actions["acknowledgeOpenChecks"]:
        with pytest.raises(helper.Refused, match="acknowledgement_required"):
            helper.task_action(store.project_root, api.project_id, tmp_path / "owner-state",
                               dict(params, acknowledgeOpenChecks=False))
    with pytest.raises(helper.Refused, match="stale_result"):
        helper.task_action(store.project_root, api.project_id, tmp_path / "owner-state", dict(params, version="999"))
    helper.task_action(store.project_root, api.project_id, tmp_path / "owner-state", params)
    saved = helper.product_read(store.project_root, tmp_path / "owner-state", api.project_id)["tasks"][public_task]
    assert saved["status"] == "ready_to_merge" and saved["acceptance"]["digest"] == params["digest"]


def closures(runtime, task_id):
    with runtime._store.connection() as db:
        return worker_checks.closures(db, task_id, TaskMachine(runtime._store).context(task_id)["revision"])


def executions(runtime, task_id):
    with runtime._store.connection() as db:
        return [json.loads(r[0]) for r in db.execute("SELECT payload_json FROM runtime_events WHERE task_id=? "
                "AND type='CHECK_EXECUTION_FINISHED' ORDER BY id", (task_id,))]


def test_automatic_gap_executes_once_and_closes_only_from_real_output(prepared):
    runtime, task_id, target, marker = prepared
    worker_checks.automatic(runtime, task_id, threading.Event(), None)
    assert executions(runtime, task_id)[-1]["argv"] == target["argv"]
    assert closures(runtime, task_id)
    count = len(executions(runtime, task_id))
    worker_checks.automatic(runtime, task_id, threading.Event(), None)
    assert len(executions(runtime, task_id)) == count


@pytest.mark.parametrize("prepared", ["failed"], indirect=True)
def test_failed_rerun_stays_failed_then_exact_passing_receipt_closes(prepared):
    runtime, task_id, target, marker = prepared
    worker_checks.execute(runtime, task_id, target, "owner-failed")
    assert executions(runtime, task_id)[-1]["exit_code"] != 0
    assert not closures(runtime, task_id)
    marker.unlink()
    worker_checks.execute(runtime, task_id, target, "owner-passed")
    assert executions(runtime, task_id)[-1]["argv"] == target["argv"]
    assert closures(runtime, task_id)
    # A newer failed or interrupted run cannot be hidden by the previous pass.
    marker.touch()
    worker_checks.execute(runtime, task_id, target, "owner-failed-again")
    assert not closures(runtime, task_id)


@pytest.mark.parametrize("prepared", ["policy"], indirect=True)
def test_paid_disclosure_never_executes_implicitly(prepared, monkeypatch):
    runtime, task_id, target, _ = prepared
    def forbidden(*args, **kwargs):
        pytest.fail("policy-blocked command must not start")
    monkeypatch.setattr(controller, "_verification_observation", forbidden)
    worker_checks.automatic(runtime, task_id, threading.Event(), None)
    assert len(executions(runtime, task_id)) == 1
    with runtime._store.connection() as db:
        assert not any(c.get("technicalReason") for c in worker_checks.unresolved(db, task_id, 1).values())
    assert not closures(runtime, task_id)


@pytest.mark.parametrize("prepared", ["missing"], indirect=True)
def test_real_technical_blocker_is_retained_and_not_retried(prepared):
    runtime, task_id, target, _ = prepared
    worker_checks.automatic(runtime, task_id, threading.Event(), None)
    worker_checks.automatic(runtime, task_id, threading.Event(), None)
    assert len(executions(runtime, task_id)) == 2  # plan plus one failed exec attempt
    assert executions(runtime, task_id)[-1]["exit_code"] == 127
    with runtime._store.connection() as db:
        current = worker_checks.latest_attempts(db, task_id, 1)
        assert len(current) == 1
        reason = worker_checks.unresolved(db, task_id, 1)[(target["assignment"], target["index"])]
    assert reason == dict(outcome="failed")
    assert not closures(runtime, task_id)


@pytest.mark.parametrize("prepared", ["slow"], indirect=True)
def test_rerun_timeout_retains_failure_and_readable_reason(prepared, monkeypatch):
    runtime, task_id, target, _ = prepared
    monkeypatch.setattr(worker_checks, "RERUN_TIMEOUT_SECONDS", .01)
    worker_checks.execute(runtime, task_id, target, "deadline")
    run = executions(runtime, task_id)[-1]
    assert run["failure"] == "timeout" and run["complete"] is False
    with runtime._store.connection() as db:
        assert worker_checks.unresolved(db, task_id, 1)[(target["assignment"], target["index"])]["outcome"] == "failed"
    assert not closures(runtime, task_id)


@pytest.mark.parametrize("prepared", ["slow"], indirect=True)
def test_cancelled_rerun_kills_owned_check_and_never_closes(prepared):
    import time
    runtime, task_id, target, _ = prepared
    cancel = threading.Event()
    thread = threading.Thread(target=worker_checks.execute, args=(runtime, task_id, target, "cancelled"),
                              kwargs=dict(cancel=cancel))
    thread.start()
    try:
        deadline = time.monotonic() + 3
        while time.monotonic() < deadline:
            with runtime._store.connection() as db:
                if db.execute("SELECT count(*) FROM runtime_events WHERE task_id=? AND type='CHECK_EXECUTION_STARTED'",
                              (task_id,)).fetchone()[0] > 1:
                    break
            time.sleep(.01)
        else:
            pytest.fail("rerun did not start")
    finally:
        cancel.set()
        thread.join(timeout=10)
    assert not thread.is_alive()
    assert not closures(runtime, task_id)
    assert executions(runtime, task_id)[-1]["complete"] is False


@pytest.mark.parametrize("damage", ["output", "workspace", "epoch", "duplicate_start"])
def test_incomplete_stale_or_replayed_evidence_never_closes(prepared, damage):
    runtime, task_id, target, _ = prepared
    worker_checks.execute(runtime, task_id, target, "owner")
    assert closures(runtime, task_id)
    with runtime._store.connection() as db:
        if damage == "output":
            db.execute("UPDATE large_outputs SET truncated=1 WHERE source_ref=?", (executions(runtime, task_id)[-1]["command_ref"],))
        elif damage == "epoch":
            db.execute("UPDATE task_owners SET epoch=epoch+1 WHERE task_id=?", (task_id,))
        elif damage == "duplicate_start":
            db.execute("INSERT INTO runtime_events(task_id,type,actor,payload_json,created_at) "
                       "SELECT task_id,type,actor,payload_json,created_at FROM runtime_events "
                       "WHERE task_id=? AND type='CHECK_EXECUTION_STARTED' ORDER BY id DESC LIMIT 1", (task_id,))
        else:
            (Path(runtime._task_workspace_root(task_id)) / "feature.py").write_text("changed\n")
        db.commit()
    assert not closures(runtime, task_id)


def test_service_manual_check_does_not_block_acceptance(journey):
    j = journey
    task = j.create("technical-check")
    j.start(task)
    result = j.wait(lambda: j.result(task), "technical-check result did not finish")
    check = next(c for c in result["checks"] if c["name"] == "Required skipped check")
    assert check["outcome"] == "not_run"
    assert check["kind"] == "manual" and check["rerunnable"] is False
    assert result["verificationStatus"] == "passed"
    refused = j.call("machine.call", dict(op="product.task", projectId=j.project_id, params=dict(
        action="accept", taskId=task, digest=result["observedWorkspaceDigest"], version=result["version"])))
    assert refused["ok"] is True
    assert j.product()["tasks"][task]["acceptance"]["open"]["not_run"] == 0


def test_explicit_desktop_rerun_is_fresh_idempotent_and_stale_bound(journey):
    j = journey
    task = j.create("skipped-check")
    j.start(task)
    result = j.wait(lambda: j.result(task), "work did not finish")
    j.wait(lambda: j.ready(task), "work did not become ready")
    result = j.result(task)
    check = next(c for c in result["checks"] if c["name"] == "Required skipped check")
    assert check["outcome"] == "passed"
    before = j.checks(task)
    execution = j.query("tasks.get", task)["executionId"]
    params = dict(projectId=j.project_id, executionId=execution, action="rerun_check",
                  checkName=check["name"], checkIndex=0, resultVersion=result["version"],
                  digest=result["observedWorkspaceDigest"], requestId="typed-check")
    assert j.call("executions.control", params)["outcome"] == "recorded"
    assert len(j.checks(task)) == len(before) + 1
    latest = j.result(task)
    assert latest["checks"][0]["outcome"] == "passed"
    assert "skipped-check-executed" in latest["checks"][0]["output"]
    assert latest["checks"][0]["command"] == ["python3", "-B", "-c", "print('skipped-check-executed')"]
    # The exact same delivery must not repeat the subprocess.
    assert j.call("executions.control", params)["outcome"] == "recorded"
    assert len(j.checks(task)) == len(before) + 1
    with pytest.raises(AssertionError, match="acp_request_refused"):
        j.call("executions.control", dict(params, requestId="stale-check", digest="f" * 64))
    latest = j.result(task)
    assert j.call("executions.control", dict(params, requestId="runtime-check", checkIndex=1,
                  checkName="Runtime verification 1", resultVersion=latest["version"]))["outcome"] == "recorded"
    assert len(j.checks(task)) == len(before) + 2
    assert j.result(task)["checks"][-1]["outcome"] == "passed"
