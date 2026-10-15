"""Explicit invocation retractions preserve failure evidence; outside reruns bind."""
import json
from unittest.mock import patch

import pytest

from cortex_runtime.contracts import StoreError
from cortex_runtime.delegation import evidence, mcp_tools
from cortex_runtime.orchestration.state import TaskMachine
from cortex_runtime.trusted_channel import resolve_channel
from runtime_scenarios import provision_fixture
from test_check_history import _fixture, _node, _run, _submit, _tamper, TOGGLE

pytestmark = pytest.mark.usefixtures("unjailed_gateway_command")
BAD = ["python3", "-B", "-c", "import re; re.search('(', 'subject')"]
GOOD = ["python3", "-B", "-c", "import re; re.search('subject', 'subject')"]
REASON = "My standalone probe used an unbalanced regex; the corrected pattern matches the subject."


def checks(failed, passed):
    return [
        dict(name="ad-hoc probe", outcome="failed", tool_call_ref=failed,
             invocation_error=dict(corrected_by=passed, reason=REASON)),
        dict(name="ad-hoc probe", outcome="passed", tool_call_ref=passed),
    ]


def pair(store, worker, bad=BAD, good=GOOD):
    failed = _run(store, worker, "mistake", bad)
    passed = _run(store, worker, "correction", good)
    assert (failed["exit_code"], passed["exit_code"]) == (1, 0)
    return failed["tool_call_ref"], passed["tool_call_ref"]


@pytest.mark.parametrize("bad,good", [
    (BAD, GOOD),
    (["python3", "-B", "-c", "assert ("], ["python3", "-B", "-c", "assert True"]),
    (["python3", "-B", "-c", "items=['a', 'b']; assert len(items)==3"],
     ["python3", "-B", "-c", "items=['a', 'b']; assert len(items)==2"]),
    (["python3", "-I", "-S", "-c", "items=[2, 1]; assert sorted(items)==[2, 1]; assert sum(items)==3"],
     ["python3", "-I", "-S", "-c", "items=[2, 1]; assert sorted(items)==[1, 2]; assert sum(items)==3"]),
])
def test_self_retraction_is_visible_and_never_a_pass(tmp_path, bad, good):
    from test_reviewer_handoff import reviewer
    from cortex_runtime.orchestration.reviewer_handoff import review_runtime_facts
    store, task, worker, assignment = _fixture(tmp_path, "retracted")
    failed, passed = pair(store, worker, bad, good)
    receipt = _submit(store, worker, checks(failed, passed))
    assert receipt["node_status"] == "completed"
    assert receipt["checks"][0]["outcome"] == "failed"
    assert receipt["checks"][0]["retracted_by"] == passed
    assert "closed_by" not in receipt["checks"][0]
    retained = _node(store, task, assignment)["result"]["checks"]
    assert retained[0]["invocation_error"]["reason"] == REASON
    assert retained[0]["evidence_strength"] == "verified"
    assert retained[0]["current_outcome"] == "retracted"
    with store.connection() as db:
        current = evidence.current_outcomes(db, task.task_id, worker.attempt_id, retained)
        assert current[0]["outcome"] == "retracted"
        assert not evidence.open_failure(db, task.task_id, worker.attempt_id, retained)
        forged = [dict(retained[0], retracted_by="gateway:" + "0" * 32), retained[1]]
        assert evidence.open_failure(db, task.task_id, worker.attempt_id, forged)
        event = json.loads(db.execute(
            "SELECT payload_json FROM task_events WHERE task_id=? AND event_type='worker_result'",
            (task.task_id,)).fetchone()[0])
        assert event["checks"][0]["outcome"] == "failed"
        assert event["checks"][0]["evidence"] == "observed"
        assert event["checks"][0]["retracted_by"] == passed
        history = evidence.command_history(db, task.task_id, worker.attempt_id)
        assert [(r["ref"], r["status"]) for r in history] == [(failed, "failed"), (passed, "passed")]
    coordinator = resolve_channel(store, task.channel_id, task.secret)
    _, review_ref = reviewer(store, task, coordinator)
    facts = review_runtime_facts(store, task.task_id, review_ref, tmp_path)
    assert facts["checks"][0]["current_outcome"] == "retracted"
    assert facts["checks"][0]["outcome"] == "failed"
    assert facts["checks"][0]["retracted_by"] == passed
    assert facts["checks"][0]["invocation_error"]["reason"] == REASON


@pytest.mark.parametrize("case", [
    "foreign_failure", "foreign_correction", "claimed_failure", "claimed_correction",
    "earlier_correction", "duplicate_correction", "missing_citation", "different_name",
    "incomplete", "truncated", "later_failure", "real_failure", "empty_reason",
    "unsupported_error", "different_profile", "changed_assertion", "test_runner", "missing_failure_ref",
    "lost_failure_body", "lost_correction_body", "later_correction_failed",
])
def test_retraction_refuses_unsafe_evidence_before_publication(tmp_path, case):
    store, task, worker, assignment = _fixture(tmp_path, case)
    bad = ["python3", "-B", "-c", "assert False"] if case == "real_failure" else BAD
    if case == "unsupported_error":
        bad = ["python3", "-B", "-c", "raise ValueError('bad probe')"]
    good = GOOD
    if case == "changed_assertion":
        bad = ["python3", "-B", "-c", "import re; re.search('subject', 'subject'); assert False"]
        good = ["python3", "-B", "-c", "import re; re.search('subject', 'subject'); assert True"]
    if case == "test_runner":
        bad = ["python3", "-B", "-c", "import unittest,re; re.search('(', 'subject')"]
        good = ["python3", "-B", "-c", "import unittest,re; re.search('subject', 'subject')"]
    if case == "earlier_correction":
        passed = _run(store, worker, "early", GOOD)["tool_call_ref"]
        failed = _run(store, worker, "mistake", BAD)["tool_call_ref"]
    else:
        failed, passed = pair(store, worker, bad=bad, good=good)
    if case.startswith("foreign"):
        coordinator = resolve_channel(store, task.channel_id, task.secret)
        other, _ = provision_fixture(store, coordinator, key="other")
        if case == "foreign_failure":
            failed = _run(store, other, "mistake", BAD)["tool_call_ref"]
        else:
            passed = _run(store, other, "correction", GOOD)["tool_call_ref"]
    submitted = checks(failed, passed)
    if case == "missing_failure_ref":
        submitted[0]["tool_call_ref"] = "gateway:" + "0" * 32
    if case == "claimed_failure":
        submitted[0].pop("tool_call_ref")
    elif case == "claimed_correction":
        submitted[1].pop("tool_call_ref")
    elif case == "duplicate_correction":
        submitted.append(dict(submitted[1]))
    elif case == "missing_citation":
        submitted.pop()
    elif case == "different_name":
        submitted[1]["name"] = "different check"
    elif case == "incomplete":
        _tamper(store, passed, lambda saved: saved.update(complete=False))
    elif case == "truncated":
        with store.connection() as db:
            db.execute("UPDATE large_outputs SET truncated=1 WHERE source_ref=?", (passed,))
            db.commit()
    elif case == "later_failure":
        _run(store, worker, "later", BAD)
    elif case in {"lost_failure_body", "lost_correction_body"}:
        ref = failed if case == "lost_failure_body" else passed
        with store.connection() as db:
            output = db.execute("SELECT filename FROM large_outputs WHERE source_ref=?", (ref,)).fetchone()
            body = store._task_directory(db, task.task_id) / "large_outputs" / output["filename"]
        body.unlink()
    elif case == "later_correction_failed":
        later = _run(store, worker, "later-correction", GOOD)["tool_call_ref"]
        with store.connection() as db:
            db.execute("UPDATE large_outputs SET exit_code=1 WHERE source_ref=?", (later,))
            db.commit()
        _tamper(store, later, lambda saved: saved["receipt"].update(exit_code=1))
    elif case == "empty_reason":
        submitted[0]["invocation_error"]["reason"] = " "
    elif case == "different_profile":
        _tamper(store, passed, lambda saved: saved["receipt"].update(profile="other"))
    args = dict(report="# Result\nObserved.", summary="Observed", checks=submitted, request_key="result")
    with store.connection() as db:
        reports_before = db.execute("SELECT COUNT(*) FROM reports").fetchone()[0]
    with pytest.raises(StoreError, match="invalid_check_retraction|invalid_arguments"):
        mcp_tools.dispatch(store, worker, "result_submit", args)
    assert _node(store, task, assignment)["status"] == "running"
    with store.connection() as db:
        assert db.execute("SELECT COUNT(*) FROM task_events WHERE event_type='worker_result'").fetchone()[0] == 0
        assert db.execute("SELECT COUNT(*) FROM runtime_requests WHERE operation='result_submit'").fetchone()[0] == 0
        assert db.execute("SELECT COUNT(*) FROM reports").fetchone()[0] == reports_before


def test_runtime_verification_failure_cannot_be_retracted(tmp_path):
    from test_reviewer_verification_dedupe import fixture, observe, command
    runtime, task, machine, _, worker, assignment, digest = fixture(tmp_path)
    store = runtime._store
    with store.connection() as db:
        db.execute("UPDATE runtime_tasks SET plan_json=? WHERE task_id=?",
                   (json.dumps(dict(verification=[BAD])), task.task_id))
        db.commit()
    observed = observe(runtime, task, machine, digest)
    assert observed["checks"][0]["exit_code"] == 1
    failed = command(store, worker, BAD)["tool_call_ref"]
    passed = command(store, worker, GOOD, key="corrected")["tool_call_ref"]
    with store.connection() as db:
        assert evidence.assess_check(db, task.task_id, worker.attempt_id, "failed", failed)["strength"] == "verified"
    with pytest.raises(StoreError, match="invalid_check_retraction"):
        mcp_tools.dispatch(store, worker, "result_submit", dict(
            report="# Review\nInvocation mistake.", summary="Observed", verdict="APPROVE",
            checks=checks(failed, passed), request_key="result"))
    assert _node(store, task, assignment)["status"] == "running"


def test_retraction_survives_publication_interruption(tmp_path):
    from cortex_runtime.orchestration.recovery import reconcile_results
    store, task, worker, assignment = _fixture(tmp_path, "recovery")
    failed, passed = pair(store, worker)
    with patch.object(TaskMachine, "record_result", side_effect=RuntimeError("interruption")):
        with pytest.raises(RuntimeError, match="interruption"):
            _submit(store, worker, checks(failed, passed))
    reconcile_results(store, task.task_id)
    node = _node(store, task, assignment)
    assert node["status"] == "completed"
    assert node["result"]["checks"][0]["retracted_by"] == passed


@pytest.mark.parametrize("different", [False, True])
def test_outside_command_rerun_has_full_identity_and_exact_closure(tmp_path, monkeypatch, different):
    from cortex_runtime.orchestration import consent
    from cortex_runtime.orchestration.gateway import verified_check
    monkeypatch.setattr(consent, "request", lambda *args, **kwargs: True)
    store, task, worker, assignment = _fixture(tmp_path, "outside")
    def outside(key, argv):
        return mcp_tools.dispatch(store, worker, "outside_command",
                                  dict(argv=argv, cwd=str(tmp_path), request_key=key))
    failed = outside("failure", TOGGLE)
    (tmp_path / "fixed.flag").write_text("fixed")
    argv = TOGGLE + ["other"] if different else TOGGLE
    passed = outside("rerun", argv)
    assert (failed["exit_code"], passed["exit_code"]) == (1, 0)
    assert passed["argv"] == argv and passed["cwd"] == str(tmp_path)
    assert passed["profile"] == "outside_confirmed" and passed["complete"] is True
    with store.connection() as db:
        assert verified_check(db, task.task_id, worker.attempt_id, failed["tool_call_ref"], "failed")
        assert verified_check(db, task.task_id, worker.attempt_id, passed["tool_call_ref"])
        assessed = evidence.assess_check(db, task.task_id, worker.attempt_id, "passed", passed["tool_call_ref"])
        assert assessed["identity"] == ("outside_command", tuple(argv), str(tmp_path), "outside_confirmed")
        saved = json.loads(db.execute("SELECT result_json FROM runtime_requests WHERE operation='outside_command' "
                                     "AND request_key='rerun'").fetchone()[0])
        assert saved["receipt"]["argv"] == argv and saved["receipt"]["cwd"] == str(tmp_path)
    receipt = _submit(store, worker, [
        dict(name="outside check", outcome="failed", tool_call_ref=failed["tool_call_ref"]),
        dict(name="outside check", outcome="passed", tool_call_ref=passed["tool_call_ref"]),
    ], **({"assignment_outcome": "failed"} if different else {}))
    assert ("closed_by" in receipt["checks"][0]) is not different
    assert receipt["node_status"] == ("failed" if different else "completed")


def test_different_argv_without_explicit_retraction_still_fails(tmp_path):
    store, _, worker, _ = _fixture(tmp_path, "unchanged")
    failed, passed = pair(store, worker)
    submitted = checks(failed, passed)
    submitted[0].pop("invocation_error")
    with pytest.raises(StoreError, match="open_failed_check"):
        mcp_tools.dispatch(store, worker, "result_submit", dict(
            report="# Result\nObserved.", summary="Observed", checks=submitted, request_key="result"))


def test_assertion_extraction_typo_is_retracted_only_with_all_other_assertions_unchanged(tmp_path):
    store, task, worker, assignment = _fixture(tmp_path, "extraction")
    (tmp_path / "examples.txt").write_text(
        "`deny-write: .claude/settings.json`\n`deny-write: .claude/settings.local.json`\n`deny-write: .claude`")
    source = ("import re; from pathlib import Path; text=Path('examples.txt').read_text(); "
              "directives=re.findall(r'`(deny-write: \\.claude[^`]+)`', text); "
              "assert len(directives)==3, directives; assert directives[-1]=='deny-write: .claude'")
    bad = ["python3", "-B", "-c", source]
    good = ["python3", "-B", "-c", source.replace("[^`]+", "[^`]*")]
    failed, passed = pair(store, worker, bad, good)
    submitted = checks(failed, passed)
    submitted[0]["invocation_error"]["reason"] = (
        "My extraction regex wrongly required a character after the directory name; "
        "only that literal was corrected and every assertion was preserved.")
    receipt = _submit(store, worker, submitted)
    assert receipt["node_status"] == "completed"
    assert receipt["checks"][0]["outcome"] == "failed"
    assert receipt["checks"][0]["retracted_by"] == passed


def test_known_outside_run_no_longer_makes_command_run_closure_ambiguous(tmp_path, monkeypatch):
    from cortex_runtime.orchestration import consent
    monkeypatch.setattr(consent, "request", lambda *args, **kwargs: True)
    store, _, worker, _ = _fixture(tmp_path, "outside-between")
    failed = _run(store, worker, "fail")["tool_call_ref"]
    (tmp_path / "fixed.flag").write_text("fixed")
    passed = _run(store, worker, "pass")["tool_call_ref"]
    outside = mcp_tools.dispatch(store, worker, "outside_command", dict(
        argv=GOOD, cwd=str(tmp_path), request_key="outside"))
    assert outside["exit_code"] == 0
    receipt = _submit(store, worker, [
        dict(name="unit tests", outcome="failed", tool_call_ref=failed),
        dict(name="unit tests", outcome="passed", tool_call_ref=passed)])
    assert receipt["checks"][0]["closed_by"] == passed
    assert receipt["node_status"] == "completed"


def test_synthesis_shows_retracted_failure_without_counting_it_as_passed(tmp_path):
    import threading
    from types import SimpleNamespace
    from cortex_runtime.orchestration import controller, synthesis
    from cortex_runtime.orchestration.state import _record_execution_baseline
    from test_reviewer_handoff import reviewer
    store, task, worker, assignment = _fixture(tmp_path, "synthesis")
    machine = TaskMachine(store)
    coordinator = resolve_channel(store, task.channel_id, task.secret)
    with store.connection() as db:
        node = db.execute("SELECT node_id FROM runtime_nodes WHERE assignment_ref=?", (assignment,)).fetchone()[0]
        db.execute("UPDATE runtime_tasks SET plan_json=? WHERE task_id=?",
                   (json.dumps(dict(tasks=[dict(id=node)], verification=[GOOD])), task.task_id))
        _record_execution_baseline(db, task.task_id)
        db.commit()
    failed, passed = pair(store, worker)
    _submit(store, worker, checks(failed, passed))
    review, review_ref = reviewer(store, task, coordinator)
    with store.connection() as db:
        db.execute("UPDATE runtime_tasks SET state='REVIEWING' WHERE task_id=?", (task.task_id,))
        db.commit()
    mcp_tools.dispatch(store, review, "result_submit", dict(
        report="# Review\nObserved.", summary="Independent review", verdict="APPROVE", checks=[], request_key="review-result"))
    machine.move(task.task_id, "VERIFYING")
    digest = machine.context(task.task_id)["nodes"][-1]["result"]["project_digest"]
    runtime = SimpleNamespace(_store=store, _task_workspace_root=lambda _: str(tmp_path))
    observed = controller._verification_observation(
        runtime, task.task_id, machine.context(task.task_id), digest, None, threading.Event(), 123)
    assert machine.record_verification(task.task_id, observed["checks"], digest)
    with store.connection() as db:
        row = db.execute("SELECT * FROM runtime_tasks WHERE task_id=?", (task.task_id,)).fetchone()
        text = synthesis.compose(db, row, "en")
    assert text is not None
    assert '"current_outcome": "retracted"' in text
    assert '"submitted_outcome": "failed"' in text
    assert failed in text and passed in text and REASON in text
    # Missing correction output later reopens the failure at synthesis, rather
    # than trusting the stored retraction marker.
    with store.connection() as db:
        output = db.execute("SELECT filename FROM large_outputs WHERE source_ref=?", (passed,)).fetchone()
        body = store._task_directory(db, task.task_id) / "large_outputs" / output["filename"]
    body.unlink()
    with store.connection() as db:
        row = db.execute("SELECT * FROM runtime_tasks WHERE task_id=?", (task.task_id,)).fetchone()
        assert synthesis.compose(db, row, "en") is None


@pytest.mark.parametrize("first,second", [
    ("command_run", "outside_command"), ("outside_command", "command_run"),
])
@pytest.mark.parametrize("case", ["exact", "different_argv", "different_cwd", "later_failure", "later_unresolved"])
def test_cross_operation_exact_closure(tmp_path, monkeypatch, first, second, case):
    from cortex_runtime.orchestration import consent
    monkeypatch.setattr(consent, "request", lambda *args, **kwargs: True)
    store, task, worker, assignment = _fixture(tmp_path, "cross-" + case)
    def run(operation, key, argv=TOGGLE, cwd=None):
        return mcp_tools.dispatch(store, worker, operation, dict(
            argv=argv, cwd=cwd or str(tmp_path), request_key=key))
    failed = run(first, "failure")
    (tmp_path / "fixed.flag").write_text("fixed")
    other = tmp_path / "other"
    other.mkdir()
    (other / "fixed.flag").write_text("fixed")
    passed = run(second, "passing", TOGGLE + ["other"] if case == "different_argv" else TOGGLE,
                 str(other) if case == "different_cwd" else None)
    assert (failed["exit_code"], passed["exit_code"]) == (1, 0)
    if case == "later_failure":
        (tmp_path / "fixed.flag").unlink()
        assert run(second, "late")["exit_code"] == 1
    elif case == "later_unresolved":
        later = run(second, "late")
        with store.connection() as db:
            db.execute("UPDATE runtime_requests SET result_json=json_set(result_json, '$.complete', json('false')) "
                       "WHERE operation=? AND request_key='late'", (second,))
            db.commit()
    submitted = [
        dict(name="exact check", outcome="failed", tool_call_ref=failed["tool_call_ref"]),
        dict(name="exact check", outcome="passed", tool_call_ref=passed["tool_call_ref"]),
    ]
    receipt = _submit(store, worker, submitted,
                      **({} if case == "exact" else {"assignment_outcome": "failed"}))
    assert receipt["node_status"] == ("completed" if case == "exact" else "failed")
    retained = _node(store, task, assignment)["result"]["checks"]
    assert retained[0]["outcome"] == "failed"
    assert retained[0].get("closed_by") == (passed["tool_call_ref"] if case == "exact" else None)
    assert "retracted_by" not in retained[0]
    with store.connection() as db:
        current = evidence.current_outcomes(db, task.task_id, worker.attempt_id, retained)
        assert current[0]["outcome"] == ("passed" if case == "exact" else "failed")
        history = evidence.command_history(db, task.task_id, worker.attempt_id)
        assert [(r["ref"], r["status"]) for r in history[:2]] == [
            (failed["tool_call_ref"], "failed"), (passed["tool_call_ref"], "passed")]


@pytest.mark.parametrize("bad,good", [
    ("items=[1, 2]; assert len(items)==3", "items=[1, 2, 3]; assert len(items)==3"),
    ("items=[1, 2]; assert len(items)==3", "items=[1, 2]; assert True"),
    ("items=[1, 2]; assert len(items)==3; assert sum(items)==3",
     "items=[1, 2]; assert len(items)==2"),
    ("assert 1==2; assert 2==3", "assert 1==1; assert 2==2"),
    ("from pathlib import Path; assert len(Path.cwd().name)==999",
     "from pathlib import Path; assert len(Path.cwd().name)>0"),
    ("import unittest; assert len([1, 2])==3", "import unittest; assert len([1, 2])==2"),
    ("import pytest; assert len([1, 2])==3", "import pytest; assert len([1, 2])==2"),
    ("import subprocess; subprocess.run(['true']); assert len([1, 2])==3",
     "import subprocess; subprocess.run(['true']); assert len([1, 2])==2"),
    ("exec('assert False')", "exec('assert True')"),
])
def test_expectation_retraction_never_weakens_product_checks(tmp_path, bad, good):
    store, _, worker, _ = _fixture(tmp_path, "unsafe-expectation")
    failed, passed = pair(store, worker, ["python3", "-B", "-c", bad], ["python3", "-B", "-c", good])
    with pytest.raises(StoreError, match="invalid_check_retraction"):
        mcp_tools.dispatch(store, worker, "result_submit", dict(
            report="# Result\nObserved.", summary="Observed", checks=checks(failed, passed), request_key="result"))


def test_expectation_retraction_requires_source_bound_complete_assertion_output():
    argv = ["python3", "-B", "-c", "items=[1, 2]; assert len(items)==3"]
    output = 'Traceback (most recent call last):\n  File "<string>", line 1, in <module>\nAssertionError\n'
    assert evidence._probe_invocation_error(argv, output, len(output.encode())) == "python_expectation"
    assert evidence._probe_invocation_error(argv, output, len(output.encode()) + 1) is None
    for other in ("AssertionError\n", output.replace("AssertionError", "ValueError")):
        assert evidence._probe_invocation_error(argv, other, len(other.encode())) is None
    for command in (["python3", "-m", "pytest"], ["ruff", "check", "."], ["make", "check"],
                    ["python3", "probe.py"], ["python3", "-O", "-c", argv[-1]]):
        assert evidence._probe_invocation_error(command, output, len(output.encode())) is None


def test_command_matching_preserves_same_operation_profile_boundaries():
    argv = ("python3", "-m", "pytest")
    restricted = ("command_run", argv, "/project", "restricted")
    development = ("command_run", argv, "/project", "development")
    outside = ("outside_command", argv, "/project", "outside_confirmed")
    assert not evidence._same_command(restricted, development)
    assert evidence._same_command(restricted, outside)
    assert evidence._same_command(outside, development)
    assert not evidence._same_command(None, outside)
    assert not evidence._same_command(("command_run", argv, None, "restricted"), outside)


def test_literal_probe_limits_and_dynamic_shapes_fail_closed():
    import ast
    for source in ("items=[1, 2]; items.append(3); assert len(items)==4",
                   "len=1; assert len==2", "x=1; x=2; assert x==3",
                   "assert len([1, 2])>2", "assert False", "assert len(1)==2",
                   "assert sum([1, 'x'])==2",
                   "items=[" + ",".join("1" for _ in range(300)) + "]; assert len(items)==301"):
        assert evidence._literal_expectations(ast.parse(source)) is None
    source = "assert len([1, 2])==3" + " " * 8192
    output = 'Traceback (most recent call last):\n  File "<string>", line 1, in <module>\nAssertionError\n'
    assert evidence._probe_invocation_error(["python3", "-c", source], output, len(output.encode())) is None


def test_schema_advertises_retraction_but_not_derived_fields():
    from cortex_runtime.orchestration.contracts import RESULT
    fields = RESULT["properties"]["checks"]["items"]["properties"]
    assert "invocation_error" in fields
    assert fields["invocation_error"]["required"] == ["corrected_by", "reason"]
    assert "retracted_by" not in fields and "closed_by" not in fields
    assert "never passed" in fields["invocation_error"]["properties"]["reason"]["description"]
