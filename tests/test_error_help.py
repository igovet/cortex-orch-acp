"""Every refusal a model can receive tells it what to change."""
from __future__ import annotations

import json
import pathlib
import re

import pytest

from cortex_runtime.contracts import ERROR_HELP, StoreError, error_help
from cortex_runtime.orchestration.state import validate_plan
from cortex_runtime.server import Server

STORAGE_TEXT = ERROR_HELP["storage_error"]
SOURCE = pathlib.Path(__file__).resolve().parents[1] / "src" / "cortex_runtime"


def _raised_codes():
    codes = set()
    for path in SOURCE.rglob("*.py"):
        codes.update(re.findall(r"StoreError\(\s*['\"]([a-z][a-z0-9_]*)['\"]", path.read_text()))
    return codes


def _plan(**task):
    base = dict(id="investigate", role="worker-debugger", objective="Find the cause.", dependencies=[], scope=["src/**"])
    return dict(tasks=[dict(base, **task)], verification=[["python3", "-m", "pytest", "-q"]])


def test_a_forbidden_scope_says_what_to_change_not_that_storage_failed():
    """A coordinator's plan with a forbidden scope was told storage failed,
    retried the identical plan, and failed again (live, 2026-09-22)."""
    with pytest.raises(StoreError) as excinfo:
        validate_plan(_plan(scope=[".cortex/**"]))
    reply = json.loads(Server.error(excinfo.value)["content"][0]["text"])
    assert reply["error"] == "forbidden_assignment_scope"
    assert ".cortex" in reply["message"] and "Storage" not in reply["message"]
    assert reply["write_state"] == "rejected"


@pytest.mark.parametrize("code", sorted(_raised_codes() - {"storage_error", "file_conflict", "unsafe_storage"}))
def test_no_refusal_falls_back_to_the_storage_failure_text(code):
    assert error_help(code) != STORAGE_TEXT


def test_an_uncertain_outcome_is_never_reported_as_rejected():
    reply = json.loads(Server.error(StoreError("gateway_action_outcome_unknown"))["content"][0]["text"])
    assert reply["write_state"] == "uncertain"
    assert "Do not repeat it" in reply["message"]
    plain = json.loads(Server.error(StoreError("some_future_refusal"))["content"][0]["text"])
    assert plain["write_state"] == "rejected"
    # A keyless precondition refusal may be retried identically after correction;
    # an explicit request_key retains the refusal for that same corrected action.
    assert "retried identically after correction" in plain["message"]
    assert "request_key retains the refusal" in plain["message"]
