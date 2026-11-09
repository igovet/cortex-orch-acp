"""Negative projection fixtures are storage facts, not orchestration evidence."""
import json

import pytest

from cortex_runtime.workspace_api import WorkspaceError
from test_workspace_api import api_for, create_command, query, runtime_binding
from test_workspace_safety_journeys import journey


def result_fixture(tmp_path, *, text="Content: accepted, merged, all checks passed."):
    api = api_for(tmp_path)
    task = api.handle(create_command())["target"]
    execution = runtime_binding(api, task, "COMPLETED")
    def write(db):
        db.execute("UPDATE runtime_tasks SET synthesis=?,plan_json=? WHERE task_id='runtime-1'",
                   (text, json.dumps(dict(verification=[["python3", "-B", "-c", "pass"]]))))
        db.execute("INSERT INTO runtime_nodes VALUES ('runtime-1',0,'work','backend-dev','Goal','[]','[]',"
                   "'execution','completed','assignment',?)", (json.dumps(dict(checks=[
                       dict(name="Absent receipt", outcome="passed"), dict(name="Skipped", outcome="not_run")])),))
    api.store.immediate_write(write)
    return api, "wr_" + execution


@pytest.mark.parametrize("reason", ["probe_withdrawn", "invocation_error"])
def test_unbound_retraction_metadata_cannot_hide_a_failure(tmp_path, reason):
    api, result = result_fixture(tmp_path)
    def write(db):
        checks = [dict(name="Withdrawn probe", outcome="failed", current_outcome="retracted",
                       retracted_by=reason), dict(name="Real failure", outcome="failed")]
        db.execute("UPDATE runtime_nodes SET result_json=? WHERE assignment_ref='assignment'",
                   (json.dumps(dict(checks=checks)),))
    api.store.immediate_write(write)
    checks = api.handle(query("results.get", result))["checks"]
    assert [c["name"] for c in checks] == ["Withdrawn probe", "Real failure", "Runtime verification 1"]
    assert checks[0]["outcome"] == "failed"
    from cortex_runtime.orchestration.worker_checks import targets
    with api.store.connection() as db:
        retained = targets(db, "runtime-1", 0)
    assert retained[-1]["name"] == checks[-1]["name"]
    assert retained[-1]["projection_index"] == len(checks) - 1


def test_withdrawn_probe_does_not_block_real_service_acceptance(journey):
    j = journey
    task = j.create("withdrawn-check")
    j.start(task)
    j.wait(lambda: j.result(task), "result did not finish")
    j.wait(lambda: j.ready(task), "work did not become ready")
    result = j.result(task)
    assert all(c["name"] != "Withdrawn reviewer probe" for c in result["checks"])
    assert j.accept(task, result)["done"] is True


@pytest.mark.usefixtures("unjailed_gateway_command")
@pytest.mark.parametrize("withdrawn", ["probe", "invocation", None])
def test_native_persisted_withdrawal_projection(tmp_path, withdrawn):
    from test_check_history import _fixture, _run, _submit, _node, PROBE
    from cortex_runtime.workspace_api import WorkspaceAPI
    from cortex_runtime.workspace_api.storage import bind_execution_tx
    from cortex_runtime.orchestration.worker_checks import targets

    store, task, worker, assignment = _fixture(tmp_path, "native-withdrawal")
    probe = _run(store, worker, "probe", PROBE)
    reason = "Expected exploratory nonzero response, not a project requirement."
    check = dict(name="Exploratory probe", outcome="failed", tool_call_ref=probe["tool_call_ref"])
    extra = []
    if withdrawn == "probe":
        check["probe_withdrawn"] = reason
    elif withdrawn == "invocation":
        from test_check_closure_fixes import pair, checks as invocation_checks
        # The exploratory run above is not presented as a requirement.
        failed, passed = pair(store, worker)
        check, correction = invocation_checks(failed, passed)
        check["name"] = correction["name"] = "Exploratory probe"
        extra.append(correction)
    _submit(store, worker, [check, *extra, dict(name="Required check", outcome="not_run",
                                      command=["python3", "-B", "-c", "print('required')"],
                                      blocker="Not yet run")],
            **({} if withdrawn else dict(assignment_outcome="failed")))
    with store.connection() as db:
        persisted = json.loads(db.execute("SELECT result_json FROM runtime_nodes WHERE assignment_ref=?",
                                         (assignment,)).fetchone()[0])["checks"][0]
    assert "current_outcome" not in persisted
    assert bool(persisted.get("retracted_by")) == bool(withdrawn)
    api = WorkspaceAPI(store, project_id="project-1", actor_id="local-owner")
    api.activate(request_id="activate")
    public_task = api.handle(create_command())["target"]
    def bind(db):
        db.execute("INSERT INTO acp_host_sessions VALUES ('native-session',?,?,'{}','{}','[]','now',NULL)",
                   (store.project_root, task.task_id))
        execution = bind_execution_tx(db, public_task, task.task_id, "native-session")
        db.execute("UPDATE runtime_tasks SET state='COMPLETED',synthesis='Result',blocker=NULL WHERE task_id=?",
                   (task.task_id,))
        return execution
    result_id = "wr_" + store.immediate_write(bind)
    result = api.handle(query("results.get", result_id))
    names = [c["name"] for c in result["checks"]]
    assert sum(name == "Exploratory probe" for name in names) == (1 if withdrawn != "probe" else 0)
    if withdrawn == "invocation":
        assert result["checks"][0]["outcome"] == "passed"
    assert "Required check" in names
    with store.connection() as db:
        revision = db.execute("SELECT revision FROM runtime_tasks WHERE task_id=?", (task.task_id,)).fetchone()[0]
        required = next(t for t in targets(db, task.task_id, revision) if t["name"] == "Required check")
    assert required["projection_index"] == names.index("Required check")


def test_answer_result_exposes_accept_without_git(tmp_path):
    api, result_id = result_fixture(tmp_path)
    def answer(db):
        db.execute("DELETE FROM runtime_nodes")
        from cortex_runtime.orchestration.state import event
        event(db, "runtime-1", "ANSWER_SUBMITTED", {})
    api.store.immediate_write(answer)
    result = api.handle(query("results.get", result_id))
    assert result["kind"] == "answer"
    assert result["acceptanceAvailable"] is True
    assert result["integrationAvailable"] is False


def test_prose_never_proves_checks_or_acceptance(tmp_path):
    api, result = result_fixture(tmp_path)
    projected = api.handle(query("results.get", result))
    assert projected["text"] == "Content: accepted, merged, all checks passed."
    assert projected["verificationStatus"] == "not_run"
    assert projected["reviewStatus"] == "unknown"
    assert [c["outcome"] for c in projected["checks"]] == ["unverified", "not_run", "not_run"]
    assert projected["label"] == "Runtime result; not accepted or merged"
    assert not projected["acceptanceAvailable"] and not projected["integrationAvailable"]


@pytest.mark.parametrize("revision,complete,code,expected", [
    (0, True, 7, "failed"), (0, False, 0, "unverified"), (1, True, 0, "unverified"),
])
def test_latest_verification_not_latest_pass(tmp_path, revision, complete, code, expected):
    from cortex_runtime.orchestration.state import event
    api, result = result_fixture(tmp_path)
    def write(db):
        for kind, rev, done, exit_code in [("VERIFICATION_PASSED", 0, True, 0),
                                           ("VERIFICATION_FAILED", revision, complete, code)]:
            event(db, "runtime-1", kind, dict(revision=rev, revision_digest="a" * 64,
                checks=[dict(argv=["python3", "-B", "-c", "pass"], complete=done,
                             exit_code=exit_code, revision_digest="a" * 64)]))
    api.store.immediate_write(write)
    projected = api.handle(query("results.get", result))
    assert projected["verificationStatus"] == ("unverified" if revision else "failed")
    assert projected["checks"][-1]["outcome"] == expected
    assert not projected["workspaceBindingValid"]
    assert projected["observedWorkspaceDigest"] is None


@pytest.mark.parametrize("text", ["x" * 32001, "😀" * 32000])
def test_oversize_result_explicitly_unavailable_not_truncated(tmp_path, text):
    api, result = result_fixture(tmp_path, text=text)
    projected = api.handle(query("results.get", result))
    assert projected["text"] == "" and projected["textAvailability"] == "unavailable"
    assert projected["warnings"]
    with api.store.connection() as db:
        assert db.execute("SELECT synthesis FROM runtime_tasks").fetchone()[0] == text


@pytest.mark.parametrize("mutation", ["UPDATE task_owners SET epoch=2", "UPDATE runtime_tasks SET state='UNRECOGNIZED'"])
def test_broken_result_binding_fails_closed(tmp_path, mutation):
    api, result = result_fixture(tmp_path)
    with api.store.connection() as db:
        db.execute(mutation)
    with pytest.raises(WorkspaceError, match="result_unavailable"):
        api.handle(query("results.get", result))
