"""T024: recovery.summary_validation -- adversarial semantic-summary fixtures.

Fixtures below stand in for the task card's "Haiku produces
contradictory/oversized summary fixtures" -- literal good/bad JSON payloads
exercising each rejection path, independent of any live model call.
"""
from __future__ import annotations

import json

from cortex_runtime.recovery.summary_validation import (
    MAX_SUMMARY_BYTES,
    validate_summary,
)

AUTHORITATIVE = dict(
    task_id="t_1", assignment_ref="a_1", assignment_status="running", completed=False,
    checks=[{"name": "tests_pass", "status": "pending"}],
    constraint_ids=["sr_1", "sr_2"],
)

GOOD_SUMMARY = dict(
    provenance="derived_summary",
    text="Work in progress on the backend endpoint.",
    references=[{"ref": "r_report1"}],
    claims=dict(task_id="t_1", assignment_status="running"),
    checks=[{"name": "tests_pass", "status": "pending"}],
    constraints_acknowledged=["sr_1", "sr_2"],
)


def test_good_summary_is_valid():
    result = validate_summary(GOOD_SUMMARY, known_refs={"r_report1"}, authoritative=AUTHORITATIVE)
    assert result.valid and result.violations == ()


def test_not_json_is_rejected():
    result = validate_summary("not { json", known_refs=set())
    assert not result.valid and result.violations == ("not_valid_json",)


def test_top_level_list_is_rejected():
    result = validate_summary(json.dumps([1, 2, 3]))
    assert not result.valid and "top_level_must_be_object" in result.violations


def test_missing_required_fields_rejected():
    result = validate_summary({"provenance": "derived_summary"})
    assert not result.valid
    assert any("missing_required_fields" in v for v in result.violations)


def test_wrong_provenance_label_always_rejected():
    bad = dict(GOOD_SUMMARY, provenance="worker_report")
    result = validate_summary(bad, known_refs={"r_report1"}, authoritative=AUTHORITATIVE)
    assert not result.valid
    assert any("provenance_must_be_derived_summary" in v for v in result.violations)


def test_oversized_summary_rejected_never_truncated():
    bad = dict(GOOD_SUMMARY, text="x" * (MAX_SUMMARY_BYTES + 1))
    result = validate_summary(bad, known_refs={"r_report1"}, authoritative=AUTHORITATIVE)
    assert not result.valid
    assert any("exceeds_size_cap" in v for v in result.violations)


def test_unresolvable_reference_rejected():
    bad = dict(GOOD_SUMMARY, references=[{"ref": "r_fabricated"}])
    result = validate_summary(bad, known_refs={"r_report1"}, authoritative=AUTHORITATIVE)
    assert not result.valid
    assert any("unresolved_reference" in v for v in result.violations)


def test_fabricated_completion_claim_cannot_change_authoritative_status():
    bad = dict(GOOD_SUMMARY, claims=dict(task_id="t_1", assignment_status="running", completed=True))
    result = validate_summary(bad, known_refs={"r_report1"}, authoritative=AUTHORITATIVE)
    assert not result.valid
    assert any("fabricated_completion_claim" in v for v in result.violations)


def test_contradicted_assignment_status_rejected():
    bad = dict(GOOD_SUMMARY, claims=dict(task_id="t_1", assignment_status="completed"))
    result = validate_summary(bad, known_refs={"r_report1"}, authoritative=AUTHORITATIVE)
    assert not result.valid
    assert any("contradicts_assignment_status" in v for v in result.violations)


def test_contradicted_id_rejected():
    bad = dict(GOOD_SUMMARY, claims=dict(task_id="t_WRONG", assignment_status="running"))
    result = validate_summary(bad, known_refs={"r_report1"}, authoritative=AUTHORITATIVE)
    assert not result.valid
    assert any("contradicts_task_id" in v for v in result.violations)


def test_contradicted_check_status_rejected():
    bad = dict(GOOD_SUMMARY, checks=[{"name": "tests_pass", "status": "passed"}])
    result = validate_summary(bad, known_refs={"r_report1"}, authoritative=AUTHORITATIVE)
    assert not result.valid
    assert any("contradicts_check:tests_pass" in v for v in result.violations)


def test_omitted_constraint_is_always_fatal_even_when_not_strict():
    bad = dict(GOOD_SUMMARY, constraints_acknowledged=["sr_1"])  # drops sr_2
    result = validate_summary(bad, known_refs={"r_report1"}, authoritative=AUTHORITATIVE, strict=False)
    assert not result.valid
    assert any("omitted_constraints" in v for v in result.violations)


def test_non_strict_mode_accepts_non_constraint_contradiction_but_reports_it():
    bad = dict(GOOD_SUMMARY, checks=[{"name": "tests_pass", "status": "passed"}])
    strict_result = validate_summary(bad, known_refs={"r_report1"}, authoritative=AUTHORITATIVE, strict=True)
    lenient_result = validate_summary(bad, known_refs={"r_report1"}, authoritative=AUTHORITATIVE, strict=False)
    assert not strict_result.valid
    assert lenient_result.valid and lenient_result.violations
