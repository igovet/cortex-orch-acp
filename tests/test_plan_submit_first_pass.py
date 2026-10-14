"""First-pass plan_submit: observed first-attempt rejections of legitimate plans.

Each case reproduces a rejection pattern retained in real coordinator sessions
(2026-10-05..14): a read-only reviewer named the directory its predecessor
writer creates; a read-only task named the target a dependent writer creates
(the remedy the refusal itself prescribes); optional node fields arrived as
JSON null; and verification was placed inside a task or beside plan.
"""
import json
from pathlib import Path
import uuid

import pytest

from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.contracts import StoreError
from cortex_runtime.delegation import mcp_tools
from cortex_runtime.orchestration.contracts import COORDINATOR, NODE, validate
from cortex_runtime.orchestration.state import TaskMachine
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.store import Store
from cortex_runtime.trusted_channel import resolve_channel


@pytest.fixture
def coordinator(tmp_path):
    project = tmp_path / 'project'
    (project / 'apps/landing/public').mkdir(parents=True)
    (project / 'apps/landing/public/index.html').write_text('<html></html>\n')
    (project / 'apps/landing/tests').mkdir()
    store = Store(project / '.cortex', project_root=project)
    task = bootstrap_task(store, SourceEvent(origin='cli', event_id=str(uuid.uuid4())), 'Build the landing page.', 'owner-1')
    machine = TaskMachine(store)
    machine.create(task.task_id, 'Build the landing page.')
    return store, machine, resolve_channel(store, task.channel_id, task.secret)


def _task(id, role, scope, dependencies=(), **fields):
    return dict(id=id, role=role, objective=f'{id} objective', dependencies=list(dependencies),
                scope=list(scope), model='auto', **fields)


def _submit(store, channel, tasks, key='plan'):
    plan = dict(tasks=tasks, verification=[['node', '--test', 'apps/landing/tests']])
    return mcp_tools.dispatch(store, channel, 'plan_submit', dict(plan=plan, request_key=key))


def test_reviewer_may_name_the_path_its_predecessor_writer_creates(coordinator):
    store, machine, channel = coordinator
    accepted = _submit(store, channel, [
        _task('build', 'frontend-dev', ['apps/landing/public/v2/**', 'apps/landing/tests/v2-*.mjs']),
        _task('visual_review', 'ux-designer', ['apps/landing/public/v2'], ['build']),
        _task('a11y_review', 'accessibility-auditor', ['apps/landing/public/v2', 'apps/landing/tests/artifacts'], ['build']),
        _task('refine', 'frontend-dev', ['apps/landing/public/v2/**', 'apps/landing/tests/**'],
              ['visual_review', 'a11y_review']),
    ])
    assert accepted['state'] == 'PLAN_READY'
    assert machine.context(channel.task_id)['state'] == 'PLAN_READY'


def test_read_only_task_may_name_the_target_its_dependent_writer_creates(coordinator):
    # The refusal's own remedy: a dependent writing task creates the file.
    store, machine, channel = coordinator
    accepted = _submit(store, channel, [
        _task('art_direction', 'ux-designer', ['apps/landing/public/index.html', 'apps/landing/public/home']),
        _task('implement', 'frontend-dev', ['apps/landing/public/home/**'], ['art_direction']),
        _task('review', 'accessibility-auditor', ['apps/landing/public/home'], ['implement']),
    ])
    assert accepted['state'] == 'PLAN_READY'


@pytest.mark.parametrize('writer_dependencies', [[], None])
def test_output_path_without_an_ordered_writer_is_still_refused_with_every_entry(coordinator, writer_dependencies):
    store, machine, channel = coordinator
    tasks = [_task('design', 'ux-designer', ['apps/landing/public/index.html', 'docs/design.md', 'docs/tokens.css'])]
    if writer_dependencies is not None:
        # A parallel writer is unordered: the reviewer could read before it writes.
        tasks.append(_task('writer', 'frontend-dev', ['docs/**'], writer_dependencies))
    with pytest.raises(StoreError) as refused:
        _submit(store, channel, tasks)
    error = refused.value
    assert str(error) == 'read_only_scope_not_writable'
    assert error.field == 'tasks[design].scope'
    assert [item['received'] for item in error.violations] == ['docs/design.md', 'docs/tokens.css']
    assert 'depends on' in error.correction and 'writing task' in error.correction
    assert machine.context(channel.task_id)['state'] == 'PLANNING'


def test_null_optional_node_fields_are_treated_as_omitted(coordinator):
    store, machine, channel = coordinator
    node = _task('build', 'frontend-dev', ['apps/landing/public/**'], model_reason=None, complexity=None, work_kind=None)
    accepted = _submit(store, channel, [node])
    assert accepted['state'] == 'PLAN_READY'
    with store.connection() as db:
        saved = json.loads(db.execute('SELECT plan_json FROM runtime_tasks WHERE task_id=?',
                                      (channel.task_id,)).fetchone()[0])
    assert set(saved['tasks'][0]) == {'id', 'role', 'objective', 'dependencies', 'scope', 'model'}


def test_null_model_reason_on_a_non_auto_hint_still_requires_a_reason(coordinator):
    store, machine, channel = coordinator
    node = _task('build', 'frontend-dev', ['apps/landing/public/**'], model_reason=None)
    node['model'] = 'claude-agent-acp'
    with pytest.raises(StoreError) as refused:
        _submit(store, channel, [node])
    assert str(refused.value) == 'model_reason_required'


def test_null_required_node_field_is_still_refused(coordinator):
    store, machine, channel = coordinator
    node = _task('build', 'frontend-dev', ['apps/landing/public/**'])
    node['model'] = None
    with pytest.raises(StoreError) as refused:
        _submit(store, channel, [node])
    assert str(refused.value) == 'invalid_arguments'
    assert refused.value.field == 'arguments.plan.tasks[0].model'


@pytest.mark.parametrize('arguments,field', [
    (dict(plan=dict(tasks=[dict(_task('a', 'backend-dev', ['src/**']), verification=[['true']])],
                    verification=[['true']]), request_key='k'),
     'arguments.plan.tasks[0].verification'),
    (dict(plan=dict(tasks=[_task('a', 'backend-dev', ['src/**'])]), verification=[['true']], request_key='k'),
     'arguments.verification'),
])
def test_misplaced_field_correction_names_its_advertised_location(arguments, field):
    with pytest.raises(StoreError) as refused:
        validate(COORDINATOR['plan_submit'], arguments)
    misplaced = next(item for item in refused.value.violations if item['field'] == field)
    assert 'arguments.plan.verification' in misplaced['correction']
    assert misplaced['received'] == 'unexpected field'


def test_unknown_field_correction_is_unchanged():
    arguments = dict(plan=dict(tasks=[dict(_task('a', 'backend-dev', ['src/**']), role_note='x')],
                               verification=[['true']]), request_key='k')
    with pytest.raises(StoreError) as refused:
        validate(COORDINATOR['plan_submit'], arguments)
    assert refused.value.correction == "Omit role_note; use only fields listed in this tool's live schema."


def test_advertised_schema_states_the_read_only_and_null_rules():
    scope = NODE['properties']['scope']['description']
    assert 'read_only_scope_not_writable' in scope
    assert 'must already exist' in scope and 'depends on' in scope
    for name in ('model_reason', 'complexity', 'work_kind'):
        assert 'null' in NODE['properties'][name]['description'], name
