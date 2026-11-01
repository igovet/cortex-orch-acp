"""Corrected invocations and sibling-only whitespace retain failures, not passes."""
import json
import subprocess
import sys
from unittest.mock import patch

import pytest

from cortex_runtime.contracts import StoreError
from cortex_runtime.delegation import evidence, mcp_tools
from cortex_runtime.trusted_channel import resolve_channel
from runtime_scenarios import provision_fixture
from test_check_history import _fixture, _node, _run, _submit, _tamper
from test_check_closure_fixes import BAD, GOOD, pair

pytestmark = pytest.mark.usefixtures("unjailed_gateway_command")


def corrected(failed, passed):
    return [
        dict(name="command check", outcome="failed", tool_call_ref=failed,
             command_correction=dict(corrected_by=passed, reason="Corrected my invocation.")),
        dict(name="command check", outcome="passed", tool_call_ref=passed),
    ]


@pytest.mark.parametrize("kind", ["probe", "argparse", "argparse_required"])
def test_corrected_command_is_superseded_history_never_a_pass(tmp_path, kind):
    from test_reviewer_handoff import reviewer
    from cortex_runtime.orchestration.reviewer_handoff import review_runtime_facts

    store, task, worker, assignment = _fixture(tmp_path, kind)
    bad, good = BAD, GOOD
    if kind.startswith("argparse"):
        (tmp_path / "check.py").write_text(
            "import argparse\np=argparse.ArgumentParser()\n"
            + ("p.add_argument('action')\np.parse_args()\n" if kind == "argparse_required" else
             "p.add_argument('action', nargs='?')\np.parse_args()\n"))
        bad = [sys.executable, "-B", "check.py", "--check"]
        good = [sys.executable, "-B", "check.py", "check"]
    failed = _run(store, worker, "bad", bad)["tool_call_ref"]
    # Even a previously presented/refused failure can be explicitly corrected.
    with pytest.raises(StoreError, match="open_failed_check"):
        mcp_tools.dispatch(store, worker, "result_submit", dict(
            report="# Result\nNot yet corrected.", summary="Observed",
            checks=[dict(name="command check", outcome="failed", tool_call_ref=failed)],
            request_key="first-result"))
    passed = _run(store, worker, "good", good)["tool_call_ref"]
    receipt = _submit(store, worker, corrected(failed, passed))
    assert receipt["node_status"] == "completed"
    assert receipt["checks"][0]["outcome"] == "failed"
    assert receipt["checks"][0]["superseded_by"] == passed
    assert "closed_by" not in receipt["checks"][0]
    retained = _node(store, task, assignment)["result"]["checks"]
    assert retained[0]["current_outcome"] == "superseded"
    with store.connection() as db:
        assert not evidence.open_failure(db, task.task_id, worker.attempt_id, retained)
        forged = [dict(retained[0], superseded_by="gateway:" + "0" * 32), retained[1]]
        assert evidence.open_failure(db, task.task_id, worker.attempt_id, forged)
        history = evidence.command_history(db, task.task_id, worker.attempt_id)
        assert [(r["ref"], r["status"]) for r in history] == [(failed, "failed"), (passed, "passed")]
        event = json.loads(db.execute(
            "SELECT payload_json FROM task_events WHERE task_id=? AND event_type='worker_result'",
            (task.task_id,)).fetchone()[0])
        assert event["checks"][0]["superseded_by"] == passed
        assert event["checks"][0]["outcome"] == "failed"
    coordinator = resolve_channel(store, task.channel_id, task.secret)
    _, review_ref = reviewer(store, task, coordinator)
    facts = review_runtime_facts(store, task.task_id, review_ref, tmp_path)
    assert facts["checks"][0]["current_outcome"] == "superseded"
    assert facts["checks"][0]["superseded_by"] == passed
    assert facts["checks"][0]["command_correction"]["reason"]


@pytest.mark.parametrize("case", [
    "real_failure", "different_name", "foreign", "missing_citation", "duplicate",
    "earlier_pass", "truncated", "missing_body", "later_failure", "later_unresolved",
    "different_profile", "runtime_verification",
])
def test_supersession_rejects_unproven_or_real_failures(tmp_path, case):
    store, task, worker, assignment = _fixture(tmp_path, case)
    bad = [sys.executable, "-B", "-c", "assert False"] if case == "real_failure" else BAD
    if case == "earlier_pass":
        passed = _run(store, worker, "early", GOOD)["tool_call_ref"]
        failed = _run(store, worker, "bad", bad)["tool_call_ref"]
    else:
        failed, passed = pair(store, worker, bad, GOOD)
    submitted = corrected(failed, passed)
    if case == "different_name":
        submitted[1]["name"] = "unrelated"
    elif case == "foreign":
        coordinator = resolve_channel(store, task.channel_id, task.secret)
        other, _ = provision_fixture(store, coordinator, key="other")
        passed = _run(store, other, "foreign", GOOD)["tool_call_ref"]
        submitted = corrected(failed, passed)
    elif case == "missing_citation":
        submitted.pop()
    elif case == "duplicate":
        submitted.append(dict(submitted[1]))
    elif case == "truncated":
        with store.connection() as db:
            db.execute("UPDATE large_outputs SET truncated=1 WHERE source_ref=?", (passed,))
            db.commit()
    elif case == "missing_body":
        with store.connection() as db:
            row = db.execute("SELECT filename FROM large_outputs WHERE source_ref=?", (failed,)).fetchone()
            body = store._task_directory(db, task.task_id) / "large_outputs" / row["filename"]
        body.unlink()
    elif case == "later_failure":
        _run(store, worker, "later", BAD)
    elif case == "later_unresolved":
        later = _run(store, worker, "later", GOOD)["tool_call_ref"]
        _tamper(store, later, lambda saved: saved.update(complete=False))
    elif case == "different_profile":
        _tamper(store, passed, lambda saved: saved["receipt"].update(profile="other"))
    original = evidence.assess_checks

    def assess(*args):
        result = original(*args)
        if case == "runtime_verification":
            result[0]["evidence_source"] = "runtime_verification"
        return result

    with patch.object(evidence, "assess_checks", assess):
        with pytest.raises(StoreError, match="invalid_check_supersession"):
            mcp_tools.dispatch(store, worker, "result_submit", dict(
                report="# Result\nObserved.", summary="Observed", checks=submitted, request_key="result"))
    assert _node(store, task, assignment)["status"] == "running"
    with store.connection() as db:
        assert not db.execute("SELECT 1 FROM task_events WHERE event_type='worker_result'").fetchone()


def whitespace_fixture(tmp_path, name, paths=("sibling/file.txt",)):
    store, task, worker, assignment = _fixture(tmp_path, name)
    coordinator = resolve_channel(store, task.channel_id, task.secret)
    _, sibling = provision_fixture(store, coordinator, key="sibling")
    with store.connection() as db:
        db.execute("UPDATE runtime_nodes SET scope_json=? WHERE assignment_ref=?",
                   (json.dumps(["own/**"]), assignment))
        db.execute("UPDATE runtime_nodes SET scope_json=? WHERE assignment_ref=?",
                   (json.dumps(["sibling/**"]), sibling))
        db.commit()
    subprocess.run(["git", "init", "-q", str(tmp_path)], check=True, capture_output=True)
    for path in paths:
        target = tmp_path / path
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text("baseline\n")
    subprocess.run(["git", "add", "--", *paths], cwd=tmp_path, check=True, capture_output=True)
    for path in paths:
        (tmp_path / path).write_text("changed  \n")
    return store, task, worker, assignment, sibling


@pytest.mark.parametrize("sibling_status", ["running", "completed"])
def test_whole_worktree_whitespace_only_in_sibling_scope_does_not_fail_worker(tmp_path, sibling_status):
    from test_reviewer_handoff import reviewer
    from cortex_runtime.orchestration.reviewer_handoff import review_runtime_facts

    store, task, worker, assignment, sibling = whitespace_fixture(tmp_path, sibling_status)
    with store.connection() as db:
        db.execute("UPDATE runtime_nodes SET status=? WHERE assignment_ref=?", (sibling_status, sibling))
        db.commit()
    failed = _run(store, worker, "whitespace", ["git", "diff", "--check"])
    assert failed["exit_code"] == 2
    receipt = _submit(store, worker, [
        dict(name="Whole-worktree whitespace", outcome="failed", tool_call_ref=failed["tool_call_ref"])])
    assert receipt["node_status"] == "completed"
    assert receipt["checks"][0]["outcome"] == "failed"
    assert "sibling/file.txt" in receipt["checks"][0]["not_attributable"]["paths"]
    retained = _node(store, task, assignment)["result"]["checks"]
    assert retained[0]["current_outcome"] == "not_attributable"
    with store.connection() as db:
        assert not evidence.open_failure(db, task.task_id, worker.attempt_id, retained)
        forged = [dict(retained[0], not_attributable={"paths": []})]
        assert evidence.open_failure(db, task.task_id, worker.attempt_id, forged)
    coordinator = resolve_channel(store, task.channel_id, task.secret)
    _, review_ref = reviewer(store, task, coordinator)
    facts = review_runtime_facts(store, task.task_id, review_ref, tmp_path)
    assert facts["checks"][0]["outcome"] == "failed"
    assert facts["checks"][0]["current_outcome"] == "not_attributable"


@pytest.mark.parametrize("case", [
    "own", "mixed", "overlap", "unknown", "read_only_sibling", "other_revision", "own_write",
    "truncated", "missing_body", "unknown_output", "runtime_verification",
])
def test_unattributable_whitespace_is_narrow_and_fail_closed(tmp_path, case):
    paths = ("own/file.txt",) if case == "own" else (
        ("sibling/file.txt", "own/file.txt") if case == "mixed" else (
            ("unreserved.txt",) if case == "unknown" else ("sibling/file.txt",)))
    store, task, worker, assignment, sibling = whitespace_fixture(tmp_path, case, paths)
    with store.connection() as db:
        if case == "overlap":
            db.execute("UPDATE runtime_nodes SET scope_json=? WHERE assignment_ref=?",
                       (json.dumps(["sibling/**"]), assignment))
        elif case == "read_only_sibling":
            db.execute("UPDATE runtime_nodes SET role='worker-code-reviewer',stage='review' WHERE assignment_ref=?", (sibling,))
        elif case == "other_revision":
            db.execute("UPDATE runtime_nodes SET revision=2 WHERE assignment_ref=?", (sibling,))
        db.commit()
    if case == "own_write":
        mcp_tools.dispatch(store, worker, "project_read", dict(path="sibling/file.txt"))
        mcp_tools.dispatch(store, worker, "project_edit", dict(
            path="sibling/file.txt", edits=[dict(old_string="changed", new_string="own change")],
            request_key="own-write"))
    failed = _run(store, worker, "whitespace", ["git", "diff", "--check"])["tool_call_ref"]
    if case == "truncated":
        with store.connection() as db:
            db.execute("UPDATE large_outputs SET truncated=1 WHERE source_ref=?", (failed,))
            db.commit()
    elif case == "missing_body":
        with store.connection() as db:
            row = db.execute("SELECT filename FROM large_outputs WHERE source_ref=?", (failed,)).fetchone()
            body = store._task_directory(db, task.task_id) / "large_outputs" / row["filename"]
        body.unlink()
    elif case == "unknown_output":
        _tamper(store, failed, lambda saved: saved["receipt"].update(output="unknown diagnostic"))
    original = evidence.assess_checks

    def assess(*args):
        result = original(*args)
        if case == "runtime_verification":
            result[0]["evidence_source"] = "runtime_verification"
        return result

    with patch.object(evidence, "assess_checks", assess):
        with pytest.raises(StoreError, match="open_failed_check"):
            mcp_tools.dispatch(store, worker, "result_submit", dict(
                report="# Result\nObserved.", summary="Observed", request_key="result",
                checks=[dict(name="Whole-worktree whitespace", outcome="failed", tool_call_ref=failed)]))
    assert _node(store, task, assignment)["status"] == "running"


def test_real_project_test_failure_cannot_be_superseded_by_another_selection(tmp_path):
    store, _, worker, _ = _fixture(tmp_path, "project-test")
    (tmp_path / "test_bad.py").write_text("def test_product():\n    assert False\n")
    (tmp_path / "test_good.py").write_text("def test_other():\n    assert True\n")
    prefix = [sys.executable, "-B", "-m", "pytest", "-q", "-o", "addopts="]
    failed, passed = pair(store, worker, prefix + ["test_bad.py"], prefix + ["test_good.py"])
    with pytest.raises(StoreError, match="invalid_check_supersession"):
        mcp_tools.dispatch(store, worker, "result_submit", dict(
            report="# Result\nObserved.", summary="Observed", checks=corrected(failed, passed),
            request_key="result"))


@pytest.mark.parametrize("kind", ["correction", "sibling"])
def test_publication_recovery_preserves_and_rederives_closure(tmp_path, kind):
    from cortex_runtime.orchestration.recovery import reconcile_results
    from cortex_runtime.orchestration.state import TaskMachine

    if kind == "sibling":
        store, task, worker, assignment, _ = whitespace_fixture(tmp_path, "recover")
        failed = _run(store, worker, "whitespace", ["git", "diff", "--check"])["tool_call_ref"]
        submitted = [dict(name="Whole-worktree whitespace", outcome="failed", tool_call_ref=failed)]
        key = "not_attributable"
    else:
        store, task, worker, assignment = _fixture(tmp_path, "recover")
        failed, passed = pair(store, worker)
        submitted = corrected(failed, passed)
        key = "superseded_by"
    with patch.object(TaskMachine, "record_result", side_effect=RuntimeError("fixture crash")):
        with pytest.raises(RuntimeError, match="fixture crash"):
            _submit(store, worker, submitted)
    reconcile_results(store, task.task_id)
    retained = _node(store, task, assignment)["result"]["checks"]
    assert _node(store, task, assignment)["status"] == "completed"
    assert retained[0]["outcome"] == "failed"
    assert retained[0][key]
    with store.connection() as db:
        assert not evidence.open_failure(db, task.task_id, worker.attempt_id, retained)


def test_schema_exposes_request_not_derived_closure():
    from cortex_runtime.orchestration.contracts import RESULT
    fields = RESULT["properties"]["checks"]["items"]["properties"]
    assert fields["command_correction"]["required"] == ["corrected_by", "reason"]
    assert "superseded_by" not in fields
    assert "not_attributable" not in fields
