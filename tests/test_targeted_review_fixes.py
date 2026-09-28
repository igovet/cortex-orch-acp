"""REQUEST_CHANGES with structured findings fixes only the named plan tasks.

A local finding must not send fix assignments to tasks with nothing to fix;
anything the runtime cannot confine (no findings, an unknown task, a
cross-cutting problem, a file outside the task's scope) keeps the
conservative fix of every original task.
"""
import json
from types import SimpleNamespace

import pytest

from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.contracts import StoreError
from cortex_runtime.delegation import contracts as delegation_contracts
from cortex_runtime.delegation import mcp_tools
from cortex_runtime.orchestration import state as state_module
from cortex_runtime.orchestration.state import TaskMachine
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.store import Store


@pytest.fixture
def machine(tmp_path):
    store = Store(tmp_path / '.cortex', project_root=tmp_path)
    task = bootstrap_task(store, SourceEvent(origin='test', event_id='one'), 'Implement feature', 'owner')
    machine = TaskMachine(store)
    machine.create(task.task_id, 'Implement feature')
    channel = SimpleNamespace(task_id=task.task_id, channel_id='coordinator', role='coordinator')
    return machine, channel, tmp_path


def chain_plan():
    """api <- service <- ui, plus an independent docs task."""
    def node(key, dependencies, scope):
        return dict(id=key, role='backend-dev', objective=f'Implement {key}', dependencies=dependencies,
                    scope=scope, model='auto')
    return dict(tasks=[node('api', [], ['src/api/**']),
                       node('service', ['api'], ['src/service']),
                       node('ui', ['service'], ['src/ui/*.py']),
                       node('docs', [], ['docs/guide.md'])],
                verification=[['python3', '-B', '-m', 'pytest', '-q']])


def _complete_pending(state, task_id, **result):
    with state.store.connection() as db:
        db.execute("UPDATE runtime_nodes SET status='completed',result_json=? WHERE task_id=? AND status IN ('pending','running')",
                   (json.dumps(dict(dict(report_ref='r_' + 'a' * 12, summary='done', checks=[]), **result)), task_id))
        db.commit()


def _review(state, channel, tmp_path, findings=None):
    state.submit_plan(channel, chain_plan(), 'plan'); state.move(channel.task_id, 'EXECUTING')
    (tmp_path / 'changed.py').write_text('changed = True\n')
    _complete_pending(state, channel.task_id)
    state.add_review(channel.task_id)
    result = dict(report_ref='r_' + 'b' * 12, summary='The shared summary of every problem.', checks=[],
                  verdict='REQUEST_CHANGES')
    if findings is not None:
        result['findings'] = findings
    _complete_pending(state, channel.task_id, **{k: v for k, v in result.items() if k != 'report_ref'})
    return dict(node_id='_review_1', result=result)


def _fixes(state, task_id):
    return {n['node_id']: n for n in state.context(task_id)['nodes'] if n['node_id'].startswith('_fix_')}


def _requested(state, task_id):
    with state.store.connection() as db:
        rows = db.execute("SELECT payload_json FROM runtime_events WHERE task_id=? AND type='REVIEW_CHANGES_REQUESTED' ORDER BY id",
                          (task_id,)).fetchall()
    return [json.loads(row[0]) for row in rows]


def finding(node, files, fix='Handle the empty input.', cross_cutting=False):
    return dict(node=node, files=files, fix=fix, cross_cutting=cross_cutting)


def test_a_local_finding_creates_a_single_fix_with_only_its_findings(machine):
    state, channel, tmp_path = machine
    review = _review(state, channel, tmp_path, [finding('service', ['src/service/handler.py'], 'Reject a blank name.'),
                                                finding('service', ['./src/service/model.py'], 'Keep the id stable.')])
    state.add_fix(channel.task_id, review)
    fixes = _fixes(state, channel.task_id)
    assert list(fixes) == ['_fix_1_service']
    fix = fixes['_fix_1_service']
    assert fix['dependencies'] == ['_review_1']
    assert fix['scope'] == ['src/service'] and fix['status'] == 'pending'
    assert 'r_' + 'b' * 12 in fix['objective']
    assert 'Reject a blank name.' in fix['objective'] and 'Keep the id stable.' in fix['objective']
    assert 'shared summary' not in fix['objective']
    assert _requested(state, channel.task_id) == [dict(review='_review_1', fixes=['_fix_1_service'],
                                                       targeted=True, fallback=None)]
    assert state.context(channel.task_id)['state'] == 'EXECUTING'


def test_targeted_fixes_keep_the_plan_order_between_them(machine):
    """ui depends on api only through service; with service unselected the
    ui fix still waits for the api fix. docs stays untouched."""
    state, channel, tmp_path = machine
    review = _review(state, channel, tmp_path, [finding('ui', ['src/ui/view.py']),
                                                finding('api', ['src/api/v1/routes.py'])])
    state.add_fix(channel.task_id, review)
    fixes = _fixes(state, channel.task_id)
    assert set(fixes) == {'_fix_1_api', '_fix_1_ui'}
    assert fixes['_fix_1_api']['dependencies'] == ['_review_1']
    assert fixes['_fix_1_ui']['dependencies'] == ['_review_1', '_fix_1_api']
    # Only the api fix is ready; ui waits for it exactly as in the plan.
    ready = [n['node_id'] for n in state.claim_ready(channel.task_id, 'coordinator')]
    assert ready == ['_fix_1_api']


@pytest.mark.parametrize('findings, reason', [
    (None, 'no_findings'),
    ([], 'no_findings'),
    ([finding('missing', ['src/api/x.py'])], 'unknown_node'),
    ([finding('_review_1', ['src/api/x.py'])], 'unknown_node'),
    ([finding('api', ['src/api/x.py']), finding('service', ['src/service/x.py'], cross_cutting=True)], 'cross_cutting'),
    ([finding('api', ['src/api/x.py'], cross_cutting=None)], 'cross_cutting'),
    ([finding('api', ['src/service/x.py'])], 'files_outside_scope'),
    ([finding('ui', ['src/ui/view.txt'])], 'files_outside_scope'),
    ([finding('api', ['src/api/../service/x.py'])], 'files_outside_scope'),
    ([finding('api', ['/abs/src/api/x.py'])], 'files_outside_scope'),
    ([finding('api', [])], 'files_outside_scope'),
    ([finding('api', ['src/api/x.py'], fix='x' * 2000)] * 4, 'findings_too_long'),
])
def test_unconfinable_findings_fall_back_to_fixing_every_original_task(machine, findings, reason):
    state, channel, tmp_path = machine
    review = _review(state, channel, tmp_path, findings)
    state.add_fix(channel.task_id, review)
    fixes = _fixes(state, channel.task_id)
    assert list(fixes) == ['_fix_1_api', '_fix_1_service', '_fix_1_ui', '_fix_1_docs']
    for fix in fixes.values():
        assert fix['dependencies'] == ['_review_1']
        assert fix['objective'] == 'Address review findings within the assigned scope: The shared summary of every problem.'
    (requested,) = _requested(state, channel.task_id)
    assert requested['targeted'] is False and requested['fallback'] == reason


def test_the_review_cycle_ceiling_is_unchanged_for_targeted_findings(machine):
    assert state_module.MAX_REVIEW_CYCLES == 3
    state, channel, tmp_path = machine
    review = _review(state, channel, tmp_path, [finding('api', ['src/api/x.py'])])
    with state.store.connection() as db:
        db.execute('UPDATE runtime_tasks SET review_cycle=? WHERE task_id=?', (state_module.MAX_REVIEW_CYCLES, channel.task_id))
        db.commit()
    state.add_fix(channel.task_id, review)
    # The stop asks the user (the state leaves REVIEWING for the stop
    # question); no fix of any kind is created.
    assert state.context(channel.task_id)['state'] not in {'REVIEWING', 'EXECUTING'}
    assert _fixes(state, channel.task_id) == {} and _requested(state, channel.task_id) == []
    with state.store.connection() as db:
        payloads = [row[0] for row in db.execute('SELECT payload_json FROM runtime_events WHERE task_id=?', (channel.task_id,))]
    assert any('review_cycle_limit' in payload for payload in payloads)


def _result_schema(stage):
    (tool,) = [t for t in mcp_tools.tool_definitions('worker', role_id='worker-code-reviewer', stage=stage)
               if t['name'] == 'result_submit']
    return tool['inputSchema']


def test_findings_are_offered_to_reviewers_only_and_described_in_the_schema():
    review, execution = _result_schema('review'), _result_schema('execution')
    assert 'findings' in review['properties'] and 'findings' not in review['required']
    assert 'findings' not in execution['properties']
    item = review['properties']['findings']['items']
    assert set(item['required']) == {'node', 'files', 'fix', 'cross_cutting'}
    assert item['properties']['cross_cutting']['type'] == 'boolean'
    description = review['properties']['findings']['description']
    assert 'REQUEST_CHANGES' in description and 'scope' in description and 'every original plan task' in description


def test_findings_are_validated_by_the_runtime_contract():
    args = dict(report_ref='r_' + 'a' * 12, summary='Changes needed.', checks=[], request_key='k',
                verdict='REQUEST_CHANGES', findings=[finding('api', ['src/api/x.py'])])
    delegation_contracts.validate('worker', 'result_submit', args)
    for bad in (dict(finding('api', ['src/api/x.py']), cross_cutting='false'),
                dict(finding('api', ['src/api/x.py']), node='_fix_1_api'),
                {k: v for k, v in finding('api', ['src/api/x.py']).items() if k != 'files'}):
        with pytest.raises(StoreError, match='invalid_arguments'):
            delegation_contracts.validate('worker', 'result_submit', dict(args, findings=[bad]))


def test_an_execution_worker_cannot_submit_findings(tmp_path):
    from cortex_runtime.trusted_channel import resolve_channel
    from runtime_scenarios import provision_fixture, publish_fixture_report
    store = Store(tmp_path / '.cortex', project_root=tmp_path)
    task = bootstrap_task(store, SourceEvent(origin='findings', event_id='one'), 'Build', 'owner')
    coordinator = resolve_channel(store, task.channel_id, task.secret)
    worker, _assignment = provision_fixture(store, coordinator)
    report = publish_fixture_report(store, worker)
    args = dict(report_ref=report, summary='Done', checks=[], request_key='result',
                findings=[finding('api', ['src/api/x.py'])])
    with pytest.raises(StoreError, match='reviewer_only_findings'):
        mcp_tools.dispatch(store, worker, 'result_submit', args)
