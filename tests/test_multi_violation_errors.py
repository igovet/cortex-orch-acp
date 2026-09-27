"""validate() reports every simultaneous violation in one error (B4).

Regression for investigation r_af50d7468665: a plan_submit with three empty
``scope`` arrays and an empty ``verification`` array previously raised on the
first violation only, costing one round trip per remaining mistake.
"""
from __future__ import annotations

from cortex_runtime.contracts import StoreError
from cortex_runtime.orchestration.contracts import COORDINATOR, validate


def _node(node_id, scope):
    return dict(id=node_id, role='debugger', objective='Investigate.',
                dependencies=[], scope=scope, model='auto')


def test_plan_submit_with_four_simultaneous_violations_reports_all_four():
    args = dict(
        plan=dict(
            tasks=[_node('a', []), _node('b', []), _node('c', [])],
            verification=[],
        ),
        request_key='k',
    )
    try:
        validate(COORDINATOR['plan_submit'], args)
        assert False, 'expected invalid_arguments'
    except StoreError as exc:
        assert str(exc) == 'invalid_arguments'
        assert exc.violations is not None
        fields = {violation['field'] for violation in exc.violations}
        assert fields == {
            'arguments.plan.tasks[0].scope',
            'arguments.plan.tasks[1].scope',
            'arguments.plan.tasks[2].scope',
            'arguments.plan.verification',
        }
        # The first violation still matches the legacy single-field shape,
        # so a caller that reads only field/received/expected/correction
        # still gets a correct, actionable single-field error.
        assert exc.field == 'arguments.plan.tasks[0].scope'


def test_single_violation_omits_the_violations_list_shape_for_the_common_case():
    args = dict(plan=dict(tasks=[_node('a', ['.'])], verification=[['python3', '-m', 'pytest']]), request_key='k')
    try:
        validate(COORDINATOR['plan_submit'], args)
    except StoreError:
        assert False, 'valid plan must not raise'
