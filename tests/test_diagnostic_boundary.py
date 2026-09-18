"""Fail-open coverage for passive diagnostics and fail-closed boundaries."""
from io import StringIO
import importlib.util
from importlib.machinery import SourceFileLoader
from pathlib import Path

import pytest

from cortex_runtime.contracts import StoreError
from cortex_runtime.diagnostic_boundary import (
    MAX_DIAGNOSTIC_BYTES,
    diagnostic_failure,
    emit_bounded_diagnostic,
    run_passive_diagnostic,
)
from cortex_runtime import server


ROOT = Path(__file__).resolve().parents[1]
AUDIT_SPEC = importlib.util.spec_from_loader(
    "cortex_live_smoke_for_diagnostic_tests",
    SourceFileLoader("cortex_live_smoke_for_diagnostic_tests", str(ROOT / "scripts/cortex-live-smoke")),
)
AUDIT = importlib.util.module_from_spec(AUDIT_SPEC)
AUDIT_SPEC.loader.exec_module(AUDIT)


def test_created_draft_observation_retains_exact_public_identity():
    fields = server.safe_observation_fields('create_draft', {'template': 'pipeline'},
                                           {'draft_id': 'd_0123456789ab', 'draft_path': '/private/draft'})
    assert fields['draft_id'] == 'd_0123456789ab'
    assert fields['template'] == 'pipeline'
    assert '/private' not in repr(fields)
    observer = AUDIT._observer_namespace('draft_identity_test')
    base = {'thread_id': 'coordinator', 'tool': 'mcp__cortex__create_draft'}
    assert observer['canonical_mcp_call_key']({**base, **fields}) == observer['canonical_mcp_call_key'](
        {**base, 'draft_id': 'd_0123456789ab', 'template': 'pipeline'})


def test_passive_failure_continues_with_bounded_value_free_diagnostic():
    emitted = []

    def fail():
        raise RuntimeError("private path and payload must never be retained")

    result = run_passive_diagnostic(fail, operation="telemetry", emit=emitted.append)

    assert result["execution_continued"] is True
    assert result["diagnostic"]["diagnostic_outcome"] == "failed"
    assert result["diagnostic"]["evidence_admissibility"] == "unverified"
    assert emitted == [result["diagnostic"]]
    assert "private path" not in repr(result)
    assert "payload" not in repr(result)
    assert set(result["diagnostic"]) == {
        "diagnostic_outcome", "diagnostic_operation", "diagnostic_code",
        "error_type", "execution_continued", "evidence_admissibility",
    }


def test_logging_failure_itself_is_nonblocking_and_emission_is_bounded():
    class BrokenLogger:
        def __call__(self, _finding):
            raise OSError("private logger details")

    result = run_passive_diagnostic(
        lambda: (_ for _ in ()).throw(TypeError("private serialization details")),
        operation="diagnostic_serialization",
        emit=BrokenLogger(),
    )
    assert result["execution_continued"] is True
    assert result["diagnostic"]["error_type"] == "TypeError"

    stream = StringIO()
    emit_bounded_diagnostic(stream, result["diagnostic"])
    line = stream.getvalue().rstrip("\n")
    assert len(line.encode("utf-8")) <= len("Cortex passive diagnostic: ".encode()) + MAX_DIAGNOSTIC_BYTES
    assert "private" not in line


@pytest.mark.parametrize("emitter_error", [StoreError("file_conflict"), PermissionError("private")])
def test_passive_emitter_store_and_permission_failures_are_swallowed(emitter_error):
    def failed_action():
        raise RuntimeError("passive action failed")

    def failed_emitter(_finding):
        raise emitter_error

    result = run_passive_diagnostic(
        failed_action, operation="telemetry", emit=failed_emitter)

    assert result["execution_continued"] is True
    assert result["diagnostic"]["evidence_admissibility"] == "unverified"


@pytest.mark.parametrize("error", [StoreError("file_conflict"), PermissionError("private")])
def test_passive_storage_and_permission_failures_do_not_stop_work(error):
    result = run_passive_diagnostic(
        lambda: (_ for _ in ()).throw(error), operation="observation",
        emit=lambda _finding: (_ for _ in ()).throw(error))
    assert result["execution_continued"] is True
    assert result["diagnostic"]["evidence_admissibility"] == "unverified"
    assert "private" not in repr(result)


def test_non_passive_operation_cannot_use_fail_open_boundary():
    with pytest.raises(ValueError, match="not a passive diagnostic"):
        run_passive_diagnostic(lambda: None, operation="publication")


def test_server_observation_failure_does_not_change_request_continuation(monkeypatch):
    monkeypatch.setattr(server, "_write_observation", lambda *_args: (_ for _ in ()).throw(RuntimeError("private")))
    stderr = StringIO()
    monkeypatch.setattr(server.sys, "stderr", stderr)

    result = server.observe("tools/call", "success")

    assert result["execution_continued"] is True
    assert result["diagnostic"]["evidence_admissibility"] == "unverified"
    assert "private" not in stderr.getvalue()


def test_diagnostic_failure_record_is_value_free_for_unusual_exception_type():
    class PrivateException(Exception):
        pass

    finding = diagnostic_failure("logging", PrivateException("secret value"))
    assert finding["error_type"] == "PrivateException"
    assert "secret value" not in repr(finding)


def test_class_b_integrity_finding_keeps_current_host_audit_exit_one():
    classification = {
        "evidence_valid": True,
        "score_eligible": True,
        "evidence_integrity_invalidators": [{"code": "replay"}],
    }
    observational = {"status": "observational_accept"}
    assert AUDIT.current_host_audit_exit_code(
        classification, observational, failures=[], host_failures=[],
        audit_policy=[], observational_diagnostics=[],
    ) == 1
