"""Field-level contract regressions from retained result-submit rejections."""

import pytest
from jsonschema import Draft202012Validator

from cortex_runtime.contracts import StoreError
from cortex_runtime.delegation.mcp_tools import tool_definitions
from cortex_runtime.orchestration.contracts import validate


def _schema(stage="review"):
    return next(tool["inputSchema"] for tool in tool_definitions(
        "worker", role_id="worker-code-reviewer", stage=stage)
        if tool["name"] == "result_submit")


def _args(**extra):
    payload = dict(report=dict(title="Review", markdown="# Review\n\nObserved."),
                   summary="Observed", checks=[], request_key="result-one")
    payload.update(extra)
    return payload


def test_review_schema_requires_verdict_and_names_the_missing_field():
    schema = _schema()
    assert "verdict" in schema["required"]
    assert list(Draft202012Validator(schema).iter_errors(_args()))
    with pytest.raises(StoreError) as caught:
        validate(schema, _args())
    assert caught.value.field == "arguments.verdict"
    assert "APPROVE" in caught.value.expected


def test_review_verdict_spelling_is_explicit_and_lowercase_alias_is_safe():
    schema = _schema()
    assert not list(Draft202012Validator(schema).iter_errors(_args(verdict="approve")))
    validate(schema, _args(verdict="approve"))
    with pytest.raises(StoreError) as caught:
        validate(schema, _args(verdict="approved"))
    assert caught.value.field == "arguments.verdict"
    assert "APPROVE" in caught.value.correction


def test_completed_check_may_supply_argv_but_not_runtime_closed_by():
    schema = _schema()
    check = dict(name="tests", outcome="passed", command=["python3", "-m", "pytest"],
                 tool_call_ref="gateway:" + "a" * 32)
    payload = _args(verdict="APPROVE", checks=[check])
    assert not list(Draft202012Validator(schema).iter_errors(payload))
    validate(schema, payload)
    with pytest.raises(StoreError) as caught:
        validate(schema, _args(verdict="APPROVE", checks=[dict(check, closed_by="gateway:" + "b" * 32)]))
    assert caught.value.field == "arguments.checks[0].closed_by"
    assert "Omit closed_by" in caught.value.correction


def test_multiple_result_errors_are_named_without_replaying_one_at_a_time():
    with pytest.raises(StoreError) as caught:
        validate(_schema(), dict(summary="Observed", checks=[dict(name="GUI", outcome="not_run")],
                                 request_key="result-one"))
    fields = {item["field"] for item in caught.value.violations}
    assert {"arguments.verdict", "arguments.checks[0].blocker"} <= fields
    assert any("report" in field for field in fields)


def test_observed_review_omission_is_named_by_the_plain_required_list():
    """Session 973dbe54 (2026-10-06, amino-ai-app): a code reviewer sent exactly
    the advertised required fields -- summary, checks, request_key, verdict --
    and was refused, because the report rule lived only in a root
    if/then/else that models do not follow. The report is now itself in the
    required list, so the omission is visible without evaluating any
    conditional."""
    schema = _schema()
    observed = dict(summary='APPROVE. The fix resolves the finding.', verdict='APPROVE',
                    request_key='review2_final_verdict',
                    checks=[dict(name='typecheck', outcome='passed', tool_call_ref='gateway:' + 'a' * 32),
                            dict(name='device check', outcome='not_run', blocker='No USB device is attached.')])
    assert set(schema['required']) == {'report', 'summary', 'checks', 'request_key', 'verdict'}
    assert not {'if', 'then', 'else', 'oneOf', 'anyOf', 'allOf'} & set(schema)
    assert set(schema['required']) - set(observed) == {'report'}
    with pytest.raises(StoreError) as caught:
        validate(schema, observed)
    assert caught.value.field == 'arguments.report' and caught.value.received == 'missing'
    assert 'summary, verdict and findings never replace it' in caught.value.expected
    for report in (dict(title='Review', markdown='# Review\n\nObserved.'), dict(report_ref='r_' + 'a' * 12)):
        payload = dict(observed, report=report)
        assert not list(Draft202012Validator(schema).iter_errors(payload))
        validate(schema, payload)


@pytest.mark.parametrize('stage', ['review', 'execution'])
def test_no_result_field_is_required_only_through_a_root_conditional(stage):
    schema = _schema(stage)
    assert schema['required'][0] == 'report'
    assert not {'if', 'then', 'else'} & set(schema)


@pytest.mark.parametrize('stage',['execution','review'])
def test_assignment_outcome_is_optional_and_only_accepts_intentional_failure(stage):
    args=_args(assignment_outcome='failed',**({'verdict':'APPROVE'} if stage=='review' else {}))
    validate(_schema(stage),args)
    with pytest.raises(StoreError) as caught:
        validate(_schema(stage),dict(args,assignment_outcome='completed'))
    assert caught.value.field=='arguments.assignment_outcome'
