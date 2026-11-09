"""The rendered journey must tell a replaced accessibility node from a process that really died."""
import importlib.util
import json
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]


def load(name):
    spec = importlib.util.spec_from_file_location(name, ROOT / f"packaging/container/{name}.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


rendered = load("check_rendered")
series = load("journey_series")


class AtspiError(RuntimeError):
    """The fields and text PyGObject gives an AT-SPI GError."""

    def __init__(self, code, message, domain="atspi_error"):
        super().__init__(f"{domain}: {message} ({code})")
        self.domain, self.code = domain, code


GONE = AtspiError(0, "The application no longer exists")
MISSING = AtspiError(1, "Object does not exist at path “/org/a11y/atspi/accessible/1”")


@pytest.mark.parametrize("error", [GONE, MISSING])
def test_a_replaced_node_is_tolerated_only_while_everything_is_alive(error):
    assert rendered.stale_node(error, None, [], None, True)


@pytest.mark.parametrize("app, web, compositor, page", [
    (-11, [], None, False),         # the app segfaulted
    (0, [], None, False),           # the app quit by itself
    (101, [], None, False),         # a Rust panic exit
    (None, ["crashed"], None, True),
    (None, ["exceeded_memory_limit"], None, False),
    (None, [], -9, True),           # the compositor died
    (None, [], None, False),        # alive but the page no longer answers
])
def test_the_same_error_after_a_real_death_stays_a_failure(app, web, compositor, page):
    assert not rendered.stale_node(GONE, app, web, compositor, page)
    assert not rendered.stale_node(MISSING, app, web, compositor, page)


@pytest.mark.parametrize("error", [
    RuntimeError("The application no longer exists"),
    AtspiError(2, "The application no longer exists"),
    AtspiError(0, "Something else"),
    AtspiError(0, "The application no longer exists", domain="g-dbus-error"),
])
def test_only_the_exact_atspi_refusal_counts_as_stale(error):
    assert not rendered.stale_node(error, None, [], None, True)


@pytest.mark.parametrize("args, expected", [
    ((None, [], -9, None, str(GONE)), "compositor_exited"),
    ((-11, [], -9, None, str(GONE)), "compositor_exited"),
    ((-11, [], None, None, str(GONE)), "app_exited"),
    ((0, ["crashed"], None, None, "probe_no_answer"), "app_exited"),
    ((None, ["crashed"], None, False, str(GONE)), "web_process_terminated"),
    ((None, [], None, True, str(GONE)), "accessibility_node_stale"),
    ((None, [], None, False, str(GONE)), "page_unresponsive"),
    ((None, [], None, True, "control_missing:Start"), "journey_step_failed"),
    ((None, [], None, True, ""), None),
])
def test_failure_class_names_the_strongest_evidence(args, expected):
    assert rendered.failure_class(*args) == expected


def test_exit_status_names_the_signal():
    assert rendered.exit_status(None) == {"state": "running"}
    assert rendered.exit_status(0) == {"state": "exited", "exit_code": 0}
    assert rendered.exit_status(101) == {"state": "exited", "exit_code": 101}
    assert rendered.exit_status(-11) == {"state": "signalled", "signal": "SIGSEGV"}
    assert rendered.exit_status(-200) == {"state": "signalled", "signal": "200"}


def test_stderr_summary_keeps_counts_and_never_the_text(tmp_path):
    log = tmp_path / "app.stderr"
    log.write_text("private-token-value panic\nEGL one\nEGL two\n")
    summary = rendered.stderr_summary(log)
    assert summary["lines"] == 3 and summary["markers"] == {"panic": 1, "EGL": 2}
    assert "private-token-value" not in json.dumps(summary)
    assert rendered.stderr_summary(tmp_path / "absent") == {"bytes": 0, "lines": 0, "markers": {}}


def test_web_terminations_are_read_in_order(tmp_path):
    assert rendered.web_terminations(tmp_path) == []
    (tmp_path / "web-process.log").write_text("0\n2\n")
    assert rendered.web_terminations(tmp_path) == ["crashed", "terminated_by_api"]


def test_the_probe_listens_for_web_process_death():
    assert 'g_signal_connect(web_view, "web-process-terminated"' in rendered.PROBE


def lines(*steps):
    return "not json\n" + "\n".join(json.dumps(step) for step in steps) + "\n"


COMPLETE = ({"step": "bundled_native_window", "status": "passed", "binary_sha256": "a" * 64}, {"step": "visual_audit", "status": "passed"},
            {"step": "accessibility_node_stale", "status": "diagnostic"},
            {"step": "run_evidence", "status": "diagnostic", "failure_class": None, "app": None,
             "app_processes": [], "page_answers": None, "web_process_terminations": [], "cores": [],
             "compositor": {"state": "running"}, "service": {"state": "running"}, "app_launches": 3},
            {"step": "visual_audit_summary", "status": "passed", "failures": 0}, {"step": "owned_process_cleanup", "status": "passed"})


def test_a_series_run_is_complete_only_with_summary_cleanup_and_exit_zero():
    result = series.summarize(lines(*COMPLETE), 0)
    assert result["complete"] and result["binary_sha256"] == "a" * 64
    assert result["stale_nodes_tolerated"] == 1 and result["screens_audited"] == 1
    assert not series.summarize(lines(*COMPLETE), 1)["complete"]
    assert not series.summarize(lines(*COMPLETE[:-1]), 0)["complete"]
    assert not series.summarize(lines(*COMPLETE[:-2], COMPLETE[-1]), 0)["complete"]
    assert not series.summarize("", 0)["complete"]


def test_a_series_run_that_died_keeps_its_exit_evidence():
    died = ({"step": "journey_incomplete", "status": "failed", "reason": "app_exited_during_request", "at": ["js:1"]},
            {"step": "native_process_evidence", "status": "diagnostic", "failure_class": "app_exited", "app": {"state": "signalled", "signal": "SIGSEGV"}},
            {"step": "visual_audit_summary", "status": "passed", "failures": 0}, {"step": "owned_process_cleanup", "status": "passed"})
    result = series.summarize(lines(*died), 0)
    assert not result["complete"]
    assert result["failure_class"] == "app_exited" and result["app"]["signal"] == "SIGSEGV" and result["failures"] == ["journey_incomplete"]


@pytest.mark.parametrize("status", ["failed", "pending", "unverified", "incomplete"])
def test_a_passing_summary_cannot_hide_an_unsuccessful_step(status):
    result = series.summarize(lines({"step": "web_process_terminated", "status": status}, *COMPLETE), 0)
    assert not result["complete"]
    if status == "failed":
        assert result["failures"] == ["web_process_terminated"]


@pytest.mark.parametrize("index", [0, 1, 3, 4, 5])
def test_a_series_run_requires_each_piece_of_evidence(index):
    assert not series.summarize(lines(*(step for i, step in enumerate(COMPLETE) if i != index)), 0)["complete"]


@pytest.mark.parametrize("field", sorted(COMPLETE[3].keys() - {"step"}))
def test_native_evidence_must_explicitly_observe_required_fields(field):
    evidence = dict(COMPLETE[3])
    del evidence[field]
    assert not series.summarize(lines(*COMPLETE[:3], evidence, *COMPLETE[4:]), 0)["complete"]


@pytest.mark.parametrize("field, value", [
    ("failure_class", "web_process_terminated"),
    ("web_process_terminations", ["crashed"]),
    ("web_process_terminations", None),
    ("cores", [{"name": "core", "bytes": 10}]),
    ("cores", None),
    ("app", {"state": "signalled", "signal": "SIGSEGV"}),
    ("app", {"state": "running"}),
    ("page_answers", False),
    ("app_processes", [{"name": "WebKitWebProcess"}]),
    ("compositor", {"state": "exited", "exit_code": 0}),
    ("compositor", {"state": "signalled", "signal": "SIGKILL"}),
    ("service", None),
    ("service", {"state": "exited", "exit_code": 1}),
    ("app_launches", 0),
    ("app_launches", True),
    ("status", "passed"),
])
def test_native_failures_cannot_be_overruled_by_a_passing_summary(field, value):
    evidence = {**COMPLETE[3], field: value}
    assert not series.summarize(lines(*COMPLETE[:3], evidence, *COMPLETE[4:]), 0)["complete"]


@pytest.mark.parametrize("index", [0, 3, 4, 5])
@pytest.mark.parametrize("contradictory", [False, True])
@pytest.mark.parametrize("first", [False, True])
def test_duplicate_or_contradictory_terminal_evidence_is_rejected(index, contradictory, first):
    duplicate = dict(COMPLETE[index])
    if contradictory:
        duplicate["status"] = "failed"
    steps = (duplicate, *COMPLETE) if first else (*COMPLETE, duplicate)
    assert not series.summarize(lines(*steps), 0)["complete"]


@pytest.mark.parametrize("first", [False, True])
def test_exception_native_evidence_cannot_be_hidden_by_run_evidence(first):
    failure = {"step": "native_process_evidence", "status": "diagnostic",
               "failure_class": "web_process_terminated", "web_process_terminations": ["crashed"]}
    steps = (failure, *COMPLETE) if first else (*COMPLETE, failure)
    result = series.summarize(lines(*steps), 0)
    assert not result["complete"]
    assert result["failure_class"] == "web_process_terminated"


@pytest.mark.parametrize("digest", [None, "", "abc", "g" * 64, 123])
def test_binary_identity_is_required_even_without_an_expected_digest(digest):
    identity = {**COMPLETE[0], "binary_sha256": digest}
    assert not series.summarize(lines(identity, *COMPLETE[1:]), 0)["complete"]


def test_x11_final_evidence_has_no_outer_compositor():
    evidence = {**COMPLETE[3], "compositor": None}
    assert series.summarize(lines(*COMPLETE[:3], evidence, *COMPLETE[4:]), 0)["complete"]


def test_terminal_records_must_be_in_producer_order():
    assert not series.summarize(lines(*COMPLETE[:3], COMPLETE[4], COMPLETE[3], COMPLETE[5]), 0)["complete"]


def test_truncated_json_evidence_is_not_ignored():
    assert not series.summarize(lines(*COMPLETE) + '{"step": "web_process_terminated"', 0)["complete"]


def test_series_cli_counts_a_contradictory_run_as_incomplete(tmp_path, monkeypatch, capsys):
    import subprocess
    import sys

    def run(command, **kwargs):
        kwargs["stdout"].write(lines({"step": "web_process_terminated", "status": "failed"}, *COMPLETE).encode())
        return subprocess.CompletedProcess(command, 0)

    monkeypatch.setattr(series.subprocess, "run", run)
    monkeypatch.setattr(sys, "argv", ["journey_series", "--artifacts", str(tmp_path),
                                    "--logs", str(tmp_path / "logs"), "--runs", "1"])
    assert series.main() == 1
    results = [json.loads(line) for line in capsys.readouterr().out.splitlines()]
    assert not results[0]["complete"]
    assert results[-1]["complete"] == 0 and results[-1]["incomplete"] == 1
