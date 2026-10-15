"""A worker's check submission never loops on an unclosable failure.

The observed sequence: a host rejected an oversized command_run result, the
worker wrapped its checks in shell scripts that redirected the output and
ended with ``code=$?; ...; exit $code``, a check failed, the exact same argv
was rerun with exit 0, and result_submit was refused with open_failed_check
nine times because every such passing rerun was classified trivial. The
refusal only asked for an exact rerun. The worker finally omitted the
failed checks and the result was accepted.

Now the plain status-capture idiom is proved and verifies; any other variable
exit stays unverified with ``status_variable_unproven`` and the refusal says
that no exact rerun can close it; refusals name the actual gap; a failure a
submission presented can never be dropped later; and command_run inlines at
most 64 KiB while check evidence uses the complete retained output.
"""
import json

import pytest

from cortex_runtime.contracts import StoreError
from cortex_runtime.delegation import mcp_tools
from cortex_runtime.delegation.evidence import (
    current_outcomes, omitted_failures, open_failure, presented_failures, trivial_command, trivial_reason)
from cortex_runtime.trusted_channel import resolve_channel
from runtime_scenarios import provision_fixture
from test_check_history import TOGGLE, _fixture, _node, _run, _submit, _tamper

pytestmark = pytest.mark.usefixtures("unjailed_gateway_command")

# The project check itself: exits 0 once fixed.flag exists, 1 before.
CHECK = "python3 -B -c \"import pathlib,sys; sys.exit(0 if pathlib.Path('fixed.flag').exists() else 1)\""
# The worker's idiom: redirect the output, capture the status, print a tail.
CAPTURE = ["bash", "-lc", CHECK + " > check.log 2>&1; code=$?; tail -n 5 check.log; exit $code"]
# The same capture with a command between that the proof does not model
# (only printers and readers may run there): its status passes through, but
# that is not proved, so even an exit-0 run of this argv cannot verify.
UNPROVEN = ["bash", "-lc", CHECK + " > check.log 2>&1; code=$?; cp check.log last.log; exit $code"]


@pytest.mark.parametrize("script", [
    "cd web && npm run build > /tmp/build.log 2>&1; code=$?; tail -n 40 /tmp/build.log; exit $code",
    "npm test > t.log 2>&1\ncode=$?\ncat t.log | tail -5\necho \"exit=$code\"\nexit $code",
    "pytest -q; rc=\"$?\"; exit \"$rc\"",
    "set -o pipefail; pytest | tee log; code=$?; exit ${code}",
    "set -e; pytest > log; code=$?; wc -l log && exit $code",
    "node -e \"process.exit(require('fs').existsSync('x') ? 0 : 1)\"; code=$?; echo 'done:' \"$code\"; exit $code",
])
def test_the_plain_status_capture_idiom_is_not_trivial(script):
    for shell in ("bash", "sh", "dash"):
        assert trivial_command([shell, "-c", script]) is False
        assert trivial_reason(["timeout", "600", shell, "-lc", script]) is None


@pytest.mark.parametrize("script,reason", [
    ("pytest | tee log; code=$?; exit $code", "status_variable_unproven"),  # tee's status, no pipefail
    ("pytest || true; code=$?; exit $code", "status_variable_unproven"),
    ("pytest; code=$?; code=0; exit $code", "status_variable_unproven"),  # reassigned
    ("pytest; code=$?; true; code=$?; exit $code", "status_variable_unproven"),
    ("pytest && code=$?; exit $code", "status_variable_unproven"),  # assigned only on success
    ("pytest; code=$?; tail log || exit 0; exit $code", "status_variable_unproven"),
    ("pytest; code=$?; tail log || exit $code", "status_variable_unproven"),
    ("trap 'exit 0' EXIT; pytest; code=$?; exit $code", "status_variable_unproven"),
    ("readonly code=0; pytest; code=$?; exit $code", "status_variable_unproven"),
    ("pytest; code=$?; eval \"$x\"; exit $code", "status_variable_unproven"),
    ("pytest; code=$?; printf -v code 0; exit $code", "status_variable_unproven"),
    ("pytest; code=$?; rm -f log; exit $code", "status_variable_unproven"),  # not a print/read command
    ("pytest; set -e; code=$?; exit $code", "status_variable_unproven"),
    ("! pytest; code=$?; exit $code", "status_variable_unproven"),
    ("pytest &\ncode=$?; exit $code", "status_variable_unproven"),
    ("pytest; code=$?; exit $other", "status_variable_unproven"),
    ("pytest; code='$?'; exit $code", "status_variable_unproven"),  # literal value, not expansion
    ("echo code=$?; pytest; 'code=$?'; exit $code", "status_variable_unproven"),  # decoy argument
    ("echo 'code=$?'; pytest; 'code=$?'; exit $code", "status_variable_unproven"),
    ("pytest; CODE=$?; exit $CODE", "status_variable_unproven"),
    ("pytest; _=$?; exit $_", "status_variable_unproven"),
    ("echo tests passed; code=$?; exit $code", "status_variable_unproven"),  # trivial underlying command
    ("pytest; echo done; code=$?; exit $code", "status_variable_unproven"),
    ("pytest; code=$?; echo \"$((code=0))\"; exit $code", "status_variable_unproven"),
    ("pytest; code=$?; echo x #; code=0\nexit $code", "trivial_command"),
    ("pytest; code=$?; (exit 0); exit $code", "trivial_command"),
    # Existing fail-closed forms keep their classification.
    ("pytest || true", "trivial_command"), ("pytest; exit 0", "trivial_command"),
    ("pytest -q 2>&1 | tail -5", "trivial_command"), ("cat log; grep -q ok log", "trivial_command"),
])
def test_unsafe_status_wrappers_stay_unverified(script, reason):
    assert trivial_reason(["bash", "-c", script]) == reason


@pytest.mark.parametrize("shell,prefix", [
    (shell, prefix)
    for shell in ("bash", "sh", "dash")
    for prefix in (
        "x=1 trap 'exit 0' EXIT",
        "x=1 y=two trap 'exit 0' EXIT",
        'x="one two" tr"ap" "exit 0" EXIT',
        "> trap.log x=1 trap 'exit 0' EXIT",
        "2> trap.log x=1 trap 'exit 0' EXIT",
    )
] + [("bash", "x+=1 trap 'exit 0' EXIT")])
def test_assignment_prefixed_trap_cannot_verify_a_masked_failure(tmp_path, prefix, shell):
    import subprocess

    check = 'python3 -B -c "import sys; sys.exit(7)"'
    assert subprocess.run([shell, "-c", check], cwd=tmp_path).returncode == 7
    store, task, worker, _ = _fixture(tmp_path, "prefixed-trap")
    argv = [shell, "-c", prefix + "; " + check + "; code=$?; exit $code"]
    run = _run(store, worker, "masked", argv)
    # The EXIT trap overrides the captured failure, despite the final exit.
    assert run["exit_code"] == 0
    receipt = mcp_tools.dispatch(store, worker, "result_submit", _args([
        dict(name="unit tests", outcome="passed", tool_call_ref=run["tool_call_ref"])]))
    assert receipt["checks"][0]["outcome"] == "unverified"
    assert receipt["checks"][0]["evidence_gap"] == "status_variable_unproven"
    assert trivial_reason(argv) == "status_variable_unproven"


@pytest.mark.parametrize("command", [
    "trap 'exit 0' EXIT", "readonly code=0", "eval true", "exec true",
    "unset code", "set +e", "command true", "builtin true", "alias x=true",
    "declare code=0", "read code", "export code=0", "pytest",
])
@pytest.mark.parametrize("prefix", ["x=1", "x=1 y=2", 'x="one two"', "'x=1'", "x+=1", "2> log x=1"])
def test_assignment_prefixed_commands_are_outside_the_capture_grammar(command, prefix):
    # Reject rather than strip assignments and mistake an argument for a
    # proved command head. Quoted assignment-looking words are not broadened.
    script = prefix + " " + command + "; pytest; code=$?; exit $code"
    for shell in ("bash", "sh", "dash"):
        assert trivial_reason([shell, "-c", script]) == "status_variable_unproven"


def test_status_capture_is_proved_only_for_plain_posix_shells():
    script = "pytest; code=$?; exit $code"
    assert trivial_reason(["zsh", "-c", script]) == "status_variable_unproven"  # zsh `status` aliases $?
    assert trivial_reason(["ksh", "-c", script]) == "status_variable_unproven"
    assert trivial_reason(["bash", "+e", "-c", script]) == "status_variable_unproven"
    assert trivial_reason(["bash", "-c", "pytest; exit $?"]) is None
    assert trivial_reason(["pwd"]) == "trivial_command"


# A check that fails with 7, and the two masking scripts: a parameter
# expansion in a printer argument evaluates an arithmetic assignment to the
# captured variable (array subscript, substring offset) in the parent shell.
FAILS_7 = "python3 -B -c 'import sys; sys.exit(7)'"
EXPANSION_MASKS = [
    FAILS_7 + "; code=$?; echo \"${x[code=0]}\"; exit $code",
    "x=hello; " + FAILS_7 + "; code=$?; echo \"${x:code=0}\"; exit $code",
]


PRINTF_MASKS = [
    FAILS_7 + '; code=$?; printf "%n" code; exit $code',
    'opt=-v; ' + FAILS_7 + '; code=$?; printf "$opt" code 0; exit $code',
    'opt=-v; ' + FAILS_7 + '; code=$?; printf $opt code 0; exit $code',
    'fmt=%n; ' + FAILS_7 + '; code=$?; printf "$fmt" code; exit $code',
]


@pytest.mark.parametrize("script", PRINTF_MASKS)
def test_printf_cannot_verify_a_masked_failure(tmp_path, script):
    import subprocess

    # Establish the check's failure independently of the wrapper.
    assert subprocess.run(["bash", "-c", FAILS_7], cwd=tmp_path).returncode == 7
    store, task, worker, _ = _fixture(tmp_path, "printf-mask")
    argv = ["bash", "-c", script]
    run = _run(store, worker, "masked", argv)
    # Bash printf assigns zero via %n or -v, even with plain variable arguments.
    assert run["exit_code"] == 0
    receipt = mcp_tools.dispatch(store, worker, "result_submit", _args([
        dict(name="unit tests", outcome="passed", tool_call_ref=run["tool_call_ref"])]))
    assert receipt["checks"][0]["outcome"] == "unverified"
    assert receipt["checks"][0]["evidence_gap"] == "status_variable_unproven"
    assert trivial_reason(argv) == "status_variable_unproven"


@pytest.mark.parametrize("shell", ["bash", "sh", "dash"])
def test_even_literal_read_only_printf_is_outside_the_capture_proof(shell):
    # No format/options evaluator: reject all intervening printf, not just
    # known mutating spellings. echo/tail wrappers remain covered separately.
    script = FAILS_7 + '; code=$?; printf "%s" "$code"; exit $code'
    assert trivial_reason([shell, "-c", script]) == "status_variable_unproven"


@pytest.mark.parametrize("script", EXPANSION_MASKS)
def test_parameter_expansion_arithmetic_cannot_verify_a_masked_failure(tmp_path, script):
    store, task, worker, _ = _fixture(tmp_path, "expansion-mask")
    argv = ["bash", "-c", script]
    run = _run(store, worker, "masked", argv)
    assert run["exit_code"] == 0  # bash assigned code=0 while printing
    receipt = mcp_tools.dispatch(store, worker, "result_submit", _args([
        dict(name="unit tests", outcome="passed", tool_call_ref=run["tool_call_ref"])]))
    assert receipt["checks"][0]["outcome"] == "unverified"
    assert receipt["checks"][0]["evidence_gap"] == "status_variable_unproven"
    for shell in ("bash", "sh", "dash"):
        assert trivial_reason([shell, "-c", script]) == "status_variable_unproven"


@pytest.mark.parametrize("script", [
    # Printer arguments, unquoted or quoted, and nested or other expansion forms.
    "pytest; code=$?; echo ${x[code=0]}; exit $code",
    "pytest; code=$?; printf '%s' \"${x:code=0}\"; exit $code",
    "pytest; code=$?; tail -n \"${x[code=0]}\" log; exit $code",
    "pytest; code=$?; echo \"a${x[code=0]}b\"; exit $code",
    "pytest; code=$?; echo \"${x[${y[code=0]}]}\"; exit $code",
    "pytest; code=$?; echo \"${code:=0}\"; exit $code",
    "pytest; code=$?; echo \"${!x}\" \"${#x}\"; exit $code",
    "pytest; code=$?; echo $'x' $\"y\"; exit $code",
    "pytest; code=$?; echo \"$1\" \"$@\" $$; exit $code",
    "pytest; code=$?; echo \"$(date)\"; exit $code",  # command substitution is not modelled
    "pytest; code=$?; echo \"$[code=0]\"; exit $code",
    # Redirection targets, after and before the capture.
    "pytest; code=$?; echo ok > \"${x[code=0]}\"; exit $code",
    "pytest; code=$?; cat < ${x[code=0]}; exit $code",
    "pytest > \"${x[code=0]}\" 2>&1; code=$?; exit $code",
    "pytest > $log 2>&1; code=$?; exit $code",
    # Expansion before the capture, in the check's own arguments.
    "echo \"${x[code=0]}\"; pytest; code=$?; exit $code",
    "pytest \"${x[code=0]}\"; code=$?; exit $code",
    # A dynamic command head, brace expansion and an array assignment, which
    # could run exit or assign instead of the shown command; unquoted glob
    # characters are outside the literal grammar too.
    "pytest; code=$?; $cmd; exit $code",
    "pytest; {exit,0}; code=$?; exit $code",
    "pytest; x[a=0]=1; code=$?; exit $code",
    "pytest; code=$?; echo *.log; exit $code",
    "pytest; code=$?; echo ok > ~/log; exit $code",
])
def test_expansions_outside_the_capture_grammar_stay_unproven(script):
    for shell in ("bash", "sh", "dash"):
        assert trivial_reason([shell, "-c", script]) == "status_variable_unproven"


def test_long_rejected_words_are_classified_without_heavy_backtracking():
    import time

    started = time.monotonic()
    for word in ("a" * 4000 + "*", "$a" * 2000 + "*", '"' + "$a" * 2000 + '$1"'):
        script = "pytest; code=$?; echo " + word + "; exit $code"
        assert trivial_reason(["bash", "-c", script]) == "status_variable_unproven"
    assert time.monotonic() - started < 5


@pytest.mark.parametrize("script", [
    "pytest; code=$?; echo \"${x[code=0]}; exit $code",
    "pytest; code=$?; echo '${x}\"${x[code=0]}\"'\"; exit $code",
])
def test_unbalanced_quoting_is_unparsable_and_trivial(script):
    # Not even tokenized: no status variable is recognized, so the general
    # fail-closed gap applies; never a proved capture.
    for shell in ("bash", "sh", "dash"):
        assert trivial_reason([shell, "-c", script]) == "trivial_command"


@pytest.mark.parametrize("script", [
    FAILS_7 + "; code=$?; echo '${x[code=0]}' '$(date)'; exit $code",  # single quotes: literal text
    FAILS_7 + " > log 2>&1; code=$?; echo \"exit=$code\" \"${code}\" $code; tail -n 5 log; exit \"${code}\"",
    "x=hello; " + FAILS_7 + "; rc=\"$?\"; echo \"x is $x\"; exit $rc",
])
def test_literal_and_simple_variable_printer_arguments_stay_proved(tmp_path, script):
    import subprocess

    for shell in ("bash", "sh", "dash"):
        assert trivial_reason([shell, "-c", script]) is None
        # The proved form really exits with the check's own status.
        assert subprocess.run([shell, "-c", script], cwd=tmp_path, capture_output=True).returncode == 7


def _args(checks, **extra):
    return dict(report="# Result\n\nObserved.", summary="Observed", checks=checks, request_key="result", **extra)


def _counts(store):
    with store.connection() as db:
        return tuple(db.execute(sql).fetchone()[0] for sql in (
            "SELECT COUNT(*) FROM runtime_requests WHERE operation='result_submit'",
            "SELECT COUNT(*) FROM task_events WHERE event_type='worker_result'",
            "SELECT COUNT(*) FROM reports"))


@pytest.mark.parametrize("name,capture", [("code", "code=$?"), ("rc", 'rc="$?"')])
def test_the_observed_sequence_closes_on_an_exact_capture_rerun(tmp_path, name, capture):
    store, task, worker, assignment = _fixture(tmp_path, "capture-closes")
    script = CAPTURE[-1].replace("code=$?", capture).replace("exit $code", f"exit ${name}")
    argv = [*CAPTURE[:-1], script]
    failed = _run(store, worker, "first", argv)
    (tmp_path / "fixed.flag").write_text("fixed")
    passed = _run(store, worker, "rerun", argv)
    assert (failed["exit_code"], passed["exit_code"]) == (1, 0)
    receipt = mcp_tools.dispatch(store, worker, "result_submit", _args([
        dict(name="unit tests", outcome="failed", tool_call_ref=failed["tool_call_ref"]),
        dict(name="unit tests", outcome="passed", tool_call_ref=passed["tool_call_ref"])]))
    assert receipt["checks"] == [dict(name="unit tests", outcome="failed", closed_by=passed["tool_call_ref"]),
                                 dict(name="unit tests", outcome="passed")]
    assert receipt["node_status"] == _node(store, task, assignment)["status"] == "completed"


@pytest.mark.parametrize("capture", [
    "'code=$?'", '"code=$?"', "'code'=$?", '"code"=$?',
    'co"de"=$?', "co'de'=$?", 'code"=" $?', 'code"="$?',
])
@pytest.mark.parametrize("shell", ["bash", "sh", "dash"])
def test_quoted_assignment_syntax_cannot_verify_a_masked_failure(tmp_path, capture, shell):
    store, task, worker, _ = _fixture(tmp_path, "quoted-capture")
    # The check fails, but quoting the name/equals sign makes a command,
    # not an assignment. Its failure leaves the initialized code at zero.
    argv = [shell, "-c", "code=0; " + CHECK + "; " + capture + "; exit $code"]
    run = _run(store, worker, "masked", argv)
    assert run["exit_code"] == 0
    assert "not found" in run["output"]
    receipt = mcp_tools.dispatch(store, worker, "result_submit", _args([
        dict(name="unit tests", outcome="passed", tool_call_ref=run["tool_call_ref"])]))
    assert receipt["checks"][0]["outcome"] == "unverified"
    assert receipt["checks"][0]["evidence_gap"] == "status_variable_unproven"
    assert trivial_reason(argv) == "status_variable_unproven"


def test_an_unprovable_status_wrapper_is_refused_with_its_gap_and_the_failed_assignment_path(tmp_path):
    store, task, worker, assignment = _fixture(tmp_path, "unproven")
    first = _run(store, worker, "first", UNPROVEN)
    (tmp_path / "fixed.flag").write_text("fixed")
    second = _run(store, worker, "rerun", UNPROVEN)
    assert (first["exit_code"], second["exit_code"]) == (1, 0)
    failed, rerun = first["tool_call_ref"], second["tool_call_ref"]
    args = _args([dict(name="unit tests", outcome="failed", tool_call_ref=failed),
                  dict(name="unit tests", outcome="passed", tool_call_ref=rerun)])
    before = _counts(store)
    with pytest.raises(StoreError, match="open_failed_check") as caught:
        mcp_tools.dispatch(store, worker, "result_submit", args)
    error = caught.value
    assert "status_variable_unproven" in error.correction
    assert "no exact rerun can close this failure" in error.correction
    assert 'Send assignment_outcome="failed"' in error.correction
    # The refusal never asks for another exact rerun, and never echoes the script.
    assert "Rerun exactly the same argv" not in error.correction
    assert CHECK not in error.correction and "cp check.log" not in error.correction
    assert "argv_sha256" in error.correction and failed in error.correction
    assert _counts(store) == before
    # An identical retry gets the identical refusal and records nothing.
    with pytest.raises(StoreError, match="open_failed_check") as again:
        mcp_tools.dispatch(store, worker, "result_submit", args)
    assert again.value.correction == error.correction and _counts(store) == before
    # The passed claim itself is kept unverified with the precise gap.
    receipt = mcp_tools.dispatch(store, worker, "result_submit", dict(args, assignment_outcome="failed"))
    assert receipt["checks"][0] == dict(name="unit tests", outcome="failed")
    assert (receipt["checks"][1]["outcome"], receipt["checks"][1]["evidence_gap"]) == (
        "unverified", "status_variable_unproven")
    assert receipt["node_status"] == _node(store, task, assignment)["status"] == "failed"


@pytest.mark.parametrize("case,expected", [
    ("absent", "No later run of exactly this command exists"),
    ("different", "runs a different command; a different argv never closes"),
    ("later_failure", "failed too"),
    ("unresolved", "has no established result (call_incomplete"),
    ("uncited", "passed, but no passed check of this result cites it"),
    ("unverified_failure", "has no verified failing receipt of this attempt (tool_call_ref_missing"),
    ("pass_later_failed", "exited nonzero, and the latest run decides"),
])
def test_refusals_name_the_failed_identity_and_the_actual_gap(tmp_path, case, expected):
    store, task, worker, _ = _fixture(tmp_path, "explain-" + case)
    failed = _run(store, worker, "first")["tool_call_ref"]
    checks = [dict(name="unit tests", outcome="failed", tool_call_ref=failed)]
    if case == "different":
        (tmp_path / "fixed.flag").write_text("fixed")
        other = _run(store, worker, "other", TOGGLE + ["extra"])["tool_call_ref"]
        checks.append(dict(name="unit tests", outcome="passed", tool_call_ref=other))
    elif case == "later_failure":
        _run(store, worker, "again")
    elif case == "unresolved":
        (tmp_path / "fixed.flag").write_text("fixed")
        again = _run(store, worker, "again")["tool_call_ref"]
        _tamper(store, again, lambda saved: saved.update(complete=False))
    elif case == "uncited":
        (tmp_path / "fixed.flag").write_text("fixed")
        _run(store, worker, "again")
    elif case == "unverified_failure":
        checks = [dict(name="unit tests", outcome="failed")]
    elif case == "pass_later_failed":
        (tmp_path / "fixed.flag").write_text("fixed")
        passed = _run(store, worker, "pass")["tool_call_ref"]
        (tmp_path / "fixed.flag").unlink()
        failed = _run(store, worker, "later")["tool_call_ref"]
        checks = [dict(name="unit tests", outcome="passed", tool_call_ref=passed)]
    with pytest.raises(StoreError, match="open_failed_check") as caught:
        mcp_tools.dispatch(store, worker, "result_submit", _args(checks))
    correction = caught.value.correction
    assert expected in correction
    assert 'assignment_outcome="failed"' in correction and TOGGLE[-1] not in correction
    if case != "unverified_failure":
        assert failed in correction and "argv_sha256" in correction and "python3" in correction


def test_a_presented_failure_cannot_be_omitted_relabelled_or_dropped_by_an_unrelated_pass(tmp_path):
    store, task, worker, assignment = _fixture(tmp_path, "omission")
    failed = _run(store, worker, "failed")["tool_call_ref"]
    unrelated = _run(store, worker, "unrelated", ["python3", "-B", "-c", "print('ok')"])["tool_call_ref"]
    presented = [dict(name="unit tests", outcome="failed", tool_call_ref=failed)]
    with pytest.raises(StoreError, match="open_failed_check"):
        mcp_tools.dispatch(store, worker, "result_submit", _args(presented))
    with store.connection() as db:
        assert presented_failures(db, task.task_id, worker.attempt_id) == [failed]
    for checks, extra in [
        ([], {}),
        ([dict(name="unit tests", outcome="passed", tool_call_ref=unrelated)], {}),
        ([dict(name="unit tests", outcome="not_run", blocker="The suite needs a database that is unavailable.")], {}),
        ([dict(name="renamed", outcome="unverified", tool_call_ref=failed, blocker="Not established.")], {}),
        ([], dict(assignment_outcome="failed")),
    ]:
        before = _counts(store)
        with pytest.raises(StoreError, match="omitted_failed_check") as caught:
            mcp_tools.dispatch(store, worker, "result_submit", _args(checks, **extra))
        assert failed in caught.value.received and failed in caught.value.correction
        assert _counts(store) == before
        assert _node(store, task, assignment)["status"] == "running"
    # Explicit failed-assignment reporting with the failure listed is accepted.
    receipt = mcp_tools.dispatch(store, worker, "result_submit", _args(
        [dict(name="renamed check", outcome="failed", tool_call_ref=failed)], assignment_outcome="failed"))
    assert receipt["node_status"] == _node(store, task, assignment)["status"] == "failed"


def test_a_presented_failure_closes_on_an_exact_latest_rerun_after_a_refusal(tmp_path):
    store, task, worker, assignment = _fixture(tmp_path, "omission-closed")
    failed = _run(store, worker, "failed")["tool_call_ref"]
    with pytest.raises(StoreError, match="open_failed_check"):
        mcp_tools.dispatch(store, worker, "result_submit", _args([dict(name="unit tests", outcome="failed",
                                                                         tool_call_ref=failed)]))
    (tmp_path / "fixed.flag").write_text("fixed")
    rerun = _run(store, worker, "rerun")["tool_call_ref"]
    # The rerun alone does not stand in for the presented failure.
    with pytest.raises(StoreError, match="omitted_failed_check"):
        mcp_tools.dispatch(store, worker, "result_submit", _args([dict(name="unit tests", outcome="passed",
                                                                         tool_call_ref=rerun)]))
    receipt = mcp_tools.dispatch(store, worker, "result_submit", _args([
        dict(name="unit tests", outcome="failed", tool_call_ref=failed),
        dict(name="unit tests", outcome="passed", tool_call_ref=rerun)]))
    assert receipt["checks"][0]["closed_by"] == rerun
    assert receipt["node_status"] == _node(store, task, assignment)["status"] == "completed"
    # An identical retry of the accepted result returns the same receipt.
    assert mcp_tools.dispatch(store, worker, "result_submit", _args([
        dict(name="unit tests", outcome="failed", tool_call_ref=failed),
        dict(name="unit tests", outcome="passed", tool_call_ref=rerun)])) == receipt


def test_only_own_genuine_presented_failures_are_retained(tmp_path):
    store, task, worker, assignment = _fixture(tmp_path, "retained-scope")
    coordinator = resolve_channel(store, task.channel_id, task.secret)
    other, _ = provision_fixture(store, coordinator, key="other")
    foreign = _run(store, other, "foreign")["tool_call_ref"]
    # An incidental search that exits 1 and is never cited is not a check.
    search = _run(store, worker, "search", ["grep", "-q", "absent-text", "README.md"])
    assert search["exit_code"] != 0
    zero = _run(store, worker, "zero", ["python3", "-B", "-c", "print('ok')"])["tool_call_ref"]
    pending = _run(store, worker, "pending")["tool_call_ref"]
    _tamper(store, pending, lambda saved: saved.update(complete=False))
    checks = [dict(name="foreign", outcome="failed", tool_call_ref=foreign),
              dict(name="claimed", outcome="failed", tool_call_ref=zero),
              dict(name="claimed again", outcome="failed", tool_call_ref=zero),
              dict(name="pending", outcome="failed", tool_call_ref=pending)]
    with pytest.raises(StoreError, match="open_failed_check"):
        mcp_tools.dispatch(store, worker, "result_submit", _args(checks))
    with store.connection() as db:
        # Pending evidence is retained so it is checked again once it has a result.
        assert presented_failures(db, task.task_id, worker.attempt_id) == [pending]
        assert omitted_failures(db, task.task_id, worker.attempt_id, []) == []
    _tamper(store, pending, lambda saved: saved.update(complete=True))
    with store.connection() as db:
        assert omitted_failures(db, task.task_id, worker.attempt_id, []) == [pending]
    with pytest.raises(StoreError, match="omitted_failed_check"):
        mcp_tools.dispatch(store, worker, "result_submit", _args([]))
    # A healthy result that lists it and fails explicitly is accepted.
    receipt = mcp_tools.dispatch(store, worker, "result_submit", _args(
        [dict(name="pending", outcome="failed", tool_call_ref=pending)], assignment_outcome="failed"))
    assert receipt["node_status"] == "failed"


def test_open_failure_rederives_a_dropped_presented_failure(tmp_path):
    store, task, worker, _ = _fixture(tmp_path, "rederive")
    failed = _run(store, worker, "failed")["tool_call_ref"]
    (tmp_path / "fixed.flag").write_text("fixed")
    rerun = _run(store, worker, "rerun")["tool_call_ref"]
    with pytest.raises(StoreError, match="open_failed_check"):
        mcp_tools.dispatch(store, worker, "result_submit", _args([dict(name="unit tests", outcome="failed",
                                                                         tool_call_ref=failed)]))
    passed_only = [dict(name="unit tests", outcome="passed", tool_call_ref=rerun)]
    with store.connection() as db:
        assert open_failure(db, task.task_id, worker.attempt_id, passed_only) is True
        assert current_outcomes(db, task.task_id, worker.attempt_id, passed_only)[0]["outcome"] == "passed"


def test_command_output_is_inlined_up_to_64_kib_and_still_verifies_from_the_retained_output(tmp_path):
    store, task, worker, assignment = _fixture(tmp_path, "inline-bound")
    big = ["python3", "-B", "-c", "import sys; sys.stdout.write('x' * 70000 + 'END')"]
    run = _run(store, worker, "big", big)
    assert run["exit_code"] == 0 and run["complete"]
    assert len(run["output"]) == 65536 and run["output_truncated"] is True and run["byte_count"] == 70003
    # An exact retry replays the same bounded receipt.
    assert _run(store, worker, "big", big) == run
    pages, offset = [], 0
    while offset is not None:
        page = mcp_tools.dispatch(store, worker, "read_large_output",
                                  dict(large_output_id=run["large_output_id"], offset=offset, limit=65536))
        pages.append(page["chunk"])
        offset = page["next_offset"]
    assert "".join(pages).endswith("END") and len("".join(pages)) == 70003
    small = _run(store, worker, "small", ["python3", "-B", "-c", "print('ok')"])
    assert (small["output"], small["output_truncated"], small["byte_count"]) == ("ok\n", False, 3)
    receipt = mcp_tools.dispatch(store, worker, "result_submit", _args(
        [dict(name="large output check", outcome="passed", tool_call_ref=run["tool_call_ref"])]))
    assert receipt["checks"] == [dict(name="large output check", outcome="passed")]
    assert receipt["node_status"] == _node(store, task, assignment)["status"] == "completed"
    with store.connection() as db:
        saved = [json.loads(row[0]) for row in db.execute(
            "SELECT result_json FROM runtime_requests WHERE operation='command_run'")]
    assert all(len(entry["receipt"]["output"]) <= 65536 for entry in saved)


def test_advertised_contracts_describe_the_closure_omission_and_inline_bound():
    tools = {t["name"]: t for t in mcp_tools.tool_definitions("worker", role_id="worker-backend-dev",
                                                              stage="execution")}
    result = tools["result_submit"]
    assert "omitted_failed_check" in result["description"]
    assert "no exact rerun can close it" in result["description"]
    properties = result["inputSchema"]["properties"]
    assert "omitted_failed_check" in properties["assignment_outcome"]["description"]
    check = properties["checks"]["items"]["properties"]
    assert "omitted_failed_check" in check["outcome"]["description"]
    assert "status_variable_unproven" in check["tool_call_ref"]["description"]
    assert "64 KiB" in tools["command_run"]["description"] and "byte_count" in tools["command_run"]["description"]
    from cortex_runtime.contracts import ERROR_HELP
    assert "omitted_failed_check" in ERROR_HELP and "no exact rerun can close it" in ERROR_HELP["open_failed_check"]
