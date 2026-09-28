"""The runtime refuses a worker result whose observed changes match an unread
situational skill's trigger (orchestration.skill_gate).

The gate observes the project tree, not an engine's tool calls, so Codex
(native apply_patch), Claude (native Edit/Write) and Cortex project_write
edits are all seen the same way: as paths changed since the attempt's
retained ``worker_start`` manifest.
"""
import json

import pytest

from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.contracts import StoreError, error_help
from cortex_runtime.delegation import mcp_tools
from cortex_runtime.delegation import revision as delegation_revision
from cortex_runtime.orchestration import skill_gate
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.store import Store
from cortex_runtime.trusted_channel import resolve_channel
from runtime_scenarios import provision_fixture

MARKDOWN = '# Result\n\nConclusion: done.\n\n## Verification\n\nNone run.\n'


def setup_worker(tmp_path, *, role='backend-dev', start=True):
    (tmp_path / 'src').mkdir()
    (tmp_path / 'src' / 'app.py').write_text('VALUE = 1\n')
    (tmp_path / 'docs').mkdir()
    (tmp_path / 'docs' / 'guide.md').write_text('# Guide\n\nValue is 1.\n')
    (tmp_path / 'README.md').write_text('# Project\n')
    store = Store(tmp_path / '.cortex', project_root=tmp_path)
    task = bootstrap_task(store, SourceEvent(origin='inline', event_id='one'), 'Build', 'owner')
    coordinator = resolve_channel(store, task.channel_id, task.secret)
    worker, assignment = provision_fixture(store, coordinator, role=role)
    if start:
        # What delegation.provisioning retains for every shared-root attempt.
        delegation_revision.record_attempt_revision(store, worker.attempt_id, f'worker_start:{worker.attempt_id}',
                                                    tmp_path)
    return store, task, worker


def result_args(key='result'):
    return dict(report=dict(title='Result', markdown=MARKDOWN), summary='Done.', checks=[], request_key=key)


def rejections(store, task):
    with store.connection() as db:
        return [row[0] for row in db.execute(
            "SELECT outcome FROM observability_events WHERE task_id=? AND category='submission_rejection'",
            (task.task_id,))]


def recorded_requests(store, task, worker):
    with store.connection() as db:
        return db.execute("SELECT COUNT(*) FROM runtime_requests WHERE task_id=? AND actor=? AND operation='result_submit'",
                          (task.task_id, worker.channel_id)).fetchone()[0]


def native_edit(tmp_path, *paths):
    """A native engine edit (Codex apply_patch, Claude Edit): bytes on disk only."""
    for path in paths:
        target = tmp_path / path
        target.write_text(target.read_text() + 'changed\n' if target.exists() else 'new\n')


@pytest.mark.parametrize('paths', [
    ('docs/guide.md', 'src/app.py'),
    ('README.md', 'src/app.py'),
    ('SECURITY.md', 'src/new_module.py'),
])
def test_docs_changed_with_source_refuses_the_result_until_documentation_sync_is_read(tmp_path, paths):
    store, task, worker = setup_worker(tmp_path)
    native_edit(tmp_path, *paths)
    with pytest.raises(StoreError) as refused:
        mcp_tools.dispatch(store, worker, 'result_submit', result_args())
    error = refused.value
    assert str(error) == 'situational_skill_required'
    assert "skill='documentation-sync'" in error.expected
    assert "read_skill tool with skill='documentation-sync'" in error.correction
    assert 'same request_key may be reused' in error.correction
    assert paths[0] in error.received and paths[1] in error.received
    assert error_help('situational_skill_required').startswith('Refused before anything was recorded')
    # Refused before anything was recorded or published; counted as a rejection.
    assert recorded_requests(store, task, worker) == 0
    assert rejections(store, task) == ['situational_skill_required']

    body = mcp_tools.dispatch(store, worker, 'read_skill', dict(skill='documentation-sync'))
    assert body['skill'] == 'documentation-sync' and body['body'].strip()
    # The identical retry, same request_key, now goes through.
    receipt = mcp_tools.dispatch(store, worker, 'result_submit', result_args())
    assert receipt['recorded'] and receipt['report_ref'].startswith('r_')
    assert recorded_requests(store, task, worker) == 1


def test_cortex_project_write_edits_are_observed_the_same_way(tmp_path):
    store, task, worker = setup_worker(tmp_path)
    mcp_tools.dispatch(store, worker, 'project_write', dict(path='docs/new.md', content='# New\n',
                                                             expected_sha256='absent', request_key='doc'))
    mcp_tools.dispatch(store, worker, 'project_write', dict(path='src/extra.py', content='X = 2\n',
                                                             expected_sha256='absent', request_key='src'))
    with pytest.raises(StoreError, match='situational_skill_required'):
        mcp_tools.dispatch(store, worker, 'result_submit', result_args())


@pytest.mark.parametrize('paths', [
    ('src/app.py',),                        # code only
    ('docs/guide.md',),                     # docs only
    ('README.md', 'docs/guide.md'),         # docs only, several
    ('docs/guide.md', 'notes/plan.txt'),    # docs with other prose
    ('docs/guide.md', 'src/__pycache__/app.cpython-312.pyc'),  # docs with bytecode cache
])
def test_no_refusal_without_documentation_and_behavior_changes_together(tmp_path, paths):
    store, task, worker = setup_worker(tmp_path)
    (tmp_path / 'notes').mkdir()
    (tmp_path / 'src' / '__pycache__').mkdir()
    native_edit(tmp_path, *paths)
    receipt = mcp_tools.dispatch(store, worker, 'result_submit', result_args())
    assert receipt['recorded']
    assert rejections(store, task) == []


def test_changes_made_before_the_attempt_started_are_not_attributed_to_it(tmp_path):
    store, task, worker = setup_worker(tmp_path, start=False)
    native_edit(tmp_path, 'docs/guide.md', 'src/app.py')
    delegation_revision.record_attempt_revision(store, worker.attempt_id, f'worker_start:{worker.attempt_id}', tmp_path)
    assert mcp_tools.dispatch(store, worker, 'result_submit', result_args())['recorded']


def test_read_only_roles_are_not_gated(tmp_path):
    store, task, worker = setup_worker(tmp_path, role='code-reviewer')
    native_edit(tmp_path, 'docs/guide.md', 'src/app.py')
    assert mcp_tools.dispatch(store, worker, 'result_submit', result_args())['recorded']


def test_a_read_by_another_attempt_does_not_clear_this_attempts_gate(tmp_path):
    store, task, worker = setup_worker(tmp_path)
    coordinator = resolve_channel(store, task.channel_id, task.secret)
    other, _ = provision_fixture(store, coordinator, key='other')
    mcp_tools.dispatch(store, other, 'read_skill', dict(skill='documentation-sync'))
    native_edit(tmp_path, 'docs/guide.md', 'src/app.py')
    with pytest.raises(StoreError, match='situational_skill_required'):
        mcp_tools.dispatch(store, worker, 'result_submit', result_args())


def test_reading_an_unrelated_skill_does_not_clear_the_gate(tmp_path):
    store, task, worker = setup_worker(tmp_path)
    mcp_tools.dispatch(store, worker, 'read_skill', dict(skill='context-compaction'))
    native_edit(tmp_path, 'docs/guide.md', 'src/app.py')
    with pytest.raises(StoreError, match='situational_skill_required'):
        mcp_tools.dispatch(store, worker, 'result_submit', result_args())


def test_without_a_start_manifest_the_gate_records_it_cannot_observe_instead_of_passing_silently(tmp_path):
    store, task, worker = setup_worker(tmp_path, start=False)
    native_edit(tmp_path, 'docs/guide.md', 'src/app.py')
    assert mcp_tools.dispatch(store, worker, 'result_submit', result_args())['recorded']
    with store.connection() as db:
        row = db.execute("SELECT outcome, detail_json FROM observability_events WHERE task_id=? AND category='skill_gate'",
                         (task.task_id,)).fetchone()
    assert row['outcome'] == 'unobservable'
    assert json.loads(row['detail_json']) == {'reason': 'no_start_revision', 'skills': ['documentation-sync']}


def test_a_recorded_request_replays_without_being_gated_again(tmp_path):
    store, task, worker = setup_worker(tmp_path)
    receipt = mcp_tools.dispatch(store, worker, 'result_submit', result_args())
    native_edit(tmp_path, 'docs/guide.md', 'src/app.py')
    assert mcp_tools.dispatch(store, worker, 'result_submit', result_args()) == receipt


def test_path_classification():
    assert skill_gate.is_documentation_path('docs/acp/roles.md')
    assert skill_gate.is_documentation_path('integrations/paseo/docs/x.md')
    assert skill_gate.is_documentation_path('README.md') and skill_gate.is_documentation_path('pkg/README.md')
    assert skill_gate.is_documentation_path('SECURITY.md')
    assert not skill_gate.is_documentation_path('docs/diagram.png')
    assert not skill_gate.is_documentation_path('notes.md')
    assert skill_gate.is_behavior_path('src/app.py') and skill_gate.is_behavior_path('config/settings.json')
    assert not skill_gate.is_behavior_path('CHANGELOG.md')
    assert not skill_gate.is_behavior_path('src/__pycache__/a.cpython-312.pyc')
    assert not skill_gate.is_behavior_path('.claude/scheduled_tasks.lock')


def test_advertised_descriptions_state_the_gate():
    tools = {tool['name']: tool for tool in mcp_tools.tool_definitions('worker', role_id='worker-backend-dev', stage='work')}
    for name in ('result_submit', 'read_skill'):
        description = tools[name]['description']
        assert 'documentation-sync' in description and '{gates}' not in description
    assert 'situational_skill_required' in tools['result_submit']['description']
