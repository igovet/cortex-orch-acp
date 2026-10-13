"""The runtime refuses a worker result whose observed changes match an unread
situational skill's trigger (orchestration.skill_gate).

The gate observes the project tree, not an engine's tool calls, so Codex
(native apply_patch), Claude (native Edit/Write) and Cortex project_write
edits are all seen the same way: as paths changed since the attempt's
retained ``worker_start`` manifest.
"""
import json
import sqlite3
from contextlib import contextmanager

import pytest

from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.contracts import StoreError, error_help
from cortex_runtime.delegation import mcp_tools
from cortex_runtime.delegation import revision as delegation_revision
from cortex_runtime.orchestration import gateway, skill_gate
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
    with store.connection() as db:
        db.execute("UPDATE runtime_nodes SET scope_json=? WHERE assignment_ref=?", (json.dumps(['.']),assignment))
        db.commit()
    if start:
        # What delegation.provisioning retains for every shared-root attempt.
        delegation_revision.record_attempt_revision(store, worker.attempt_id, f'worker_start:{worker.attempt_id}',
                                                    tmp_path)
    return store, task, worker


def result_args(key='result'):
    return dict(report=MARKDOWN, report_title='Result', summary='Done.', checks=[], request_key=key)


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
    other, other_assignment = provision_fixture(store, coordinator, key='other')
    with store.connection() as db:
        db.execute("UPDATE runtime_nodes SET scope_json=? WHERE assignment_ref=?",
                   (json.dumps(['other/**']), other_assignment))
        db.commit()
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
    assert skill_gate.is_documentation_path('docs/features/roles-and-delegation/index.md')
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


def test_successful_gateway_writes_hint_before_result_and_clear_after_skill_read(tmp_path):
    store,task,worker=setup_worker(tmp_path)
    first=mcp_tools.dispatch(store,worker,'project_write',dict(path='src/extra.py',content='X = 2',expected_sha256='absent'))
    assert 'required_before_result_submit' not in first
    read=mcp_tools.dispatch(store,worker,'project_read',dict(path='README.md'))
    doc=mcp_tools.dispatch(store,worker,'project_write',dict(path='README.md',content='# Updated',expected_sha256=read['sha256']))
    assert doc['sha256'] and len(doc['required_before_result_submit'])==1
    assert doc['required_before_result_submit'][0]['skill']=='documentation-sync'
    assert 'README.md' in doc['required_before_result_submit'][0]['reason']
    assert recorded_requests(store,task,worker)==0
    mkdir=mcp_tools.dispatch(store,worker,'project_mkdir',dict(path='notes'))
    assert mkdir['required_before_result_submit']==doc['required_before_result_submit']
    mcp_tools.dispatch(store,worker,'read_skill',dict(skill='documentation-sync'))
    after=mcp_tools.dispatch(store,worker,'project_write',dict(path='docs/new.md',content='# New',expected_sha256='absent'))
    assert 'required_before_result_submit' not in after


def test_gateway_hint_uses_own_writes_and_result_gate_reports_unobservable_start(tmp_path):
    store,task,worker=setup_worker(tmp_path,start=False)
    first=mcp_tools.dispatch(store,worker,'project_write',dict(path='src/new.py',content='X=2',expected_sha256='absent'))
    assert 'required_before_result_submit' not in first
    # The hint observes no tree: this attempt's own gateway writes are exact
    # evidence even without a retained start revision.
    second=mcp_tools.dispatch(store,worker,'project_write',dict(path='docs/new.md',content='# New',expected_sha256='absent'))
    assert second['required_before_result_submit'][0]['skill']=='documentation-sync'
    with store.connection() as db:
        assert db.execute("SELECT COUNT(*) FROM observability_events WHERE category='skill_gate' AND outcome='unobservable'").fetchone()[0]==0
    # The result gate still observes the tree and never treats the missing
    # start revision as a pass.
    assert skill_gate.required_before_result_submit(store,worker,stage=None,role_ids=('backend-dev',))==[]
    with store.connection() as db:
        assert db.execute("SELECT COUNT(*) FROM observability_events WHERE category='skill_gate' AND outcome='unobservable'").fetchone()[0]>0


def test_advisory_observation_failure_does_not_reject_a_committed_write(tmp_path,monkeypatch):
    store,task,worker=setup_worker(tmp_path)
    def unavailable(*args,**kwargs):
        raise OSError('unavailable observation')
    monkeypatch.setattr(skill_gate,'required_before_result_submit',unavailable)
    args=dict(path='src/new.py',content='X=2',expected_sha256='absent')
    receipt=mcp_tools.dispatch(store,worker,'project_write',args)
    assert receipt['skill_hint_unavailable']
    assert (tmp_path/'src/new.py').read_text()=='X=2'
    assert mcp_tools.dispatch(store,worker,'project_write',args)==receipt


@pytest.mark.parametrize('operation', ['project_write', 'project_mkdir'])
@pytest.mark.parametrize('failure_point', ['connection', 'node_lookup'])
@pytest.mark.parametrize('error_type', [StoreError, sqlite3.OperationalError, RuntimeError])
def test_advisory_database_failure_preserves_committed_mutation_and_replay(
        tmp_path, monkeypatch, operation, failure_point, error_type):
    store, task, worker = setup_worker(tmp_path)
    target = tmp_path / 'src' / 'new'
    args = dict(path='src/new', request_key='advisory-db-failure')
    if operation == 'project_write':
        args.update(content='X=2', expected_sha256='absent')
    original_connection = store.connection
    failures = []

    def fail():
        failures.append(failure_point)
        raise error_type('storage_busy')

    class Connection:
        def __init__(self, db):
            self.db = db

        def __getattr__(self, name):
            return getattr(self.db, name)

        def execute(self, sql, *parameters):
            if not failures and sql.startswith('SELECT n.stage,n.role FROM runtime_nodes'):
                assert target.exists()
                assert not self.db.in_transaction
                fail()
            return self.db.execute(sql, *parameters)

    @contextmanager
    def connection():
        # Do not disturb authorization or the mutation transaction; inject only
        # after the filesystem change, at the advisory observation boundary.
        if not failures and target.exists() and failure_point == 'connection':
            fail()
        with original_connection() as db:
            yield Connection(db) if failure_point == 'node_lookup' else db

    monkeypatch.setattr(store, 'connection', connection)
    try:
        receipt = gateway.dispatch(store, worker, operation, args)
    finally:
        assert failures == [failure_point]
        if operation == 'project_write':
            assert target.read_text() == 'X=2'
        else:
            assert target.is_dir()
    assert receipt['skill_hint_unavailable'] == 'observation_failed; result_submit still enforces the gate'
    assert 'required_before_result_submit' not in receipt
    assert receipt['sha256'] if operation == 'project_write' else receipt['created']
    # A replay must return the retained success, not repeat the mutation or hint.
    assert gateway.dispatch(store, worker, operation, args) == receipt
    with original_connection() as db:
        row = db.execute(
            'SELECT result_json FROM runtime_requests WHERE task_id=? AND actor=? AND request_key=?',
            (task.task_id, worker.channel_id, args['request_key'])).fetchone()
        assert json.loads(row['result_json']) == dict(complete=True, receipt=receipt)
    # Losing the early reminder never clears the authoritative submission gate.
    native_edit(tmp_path, 'docs/guide.md', 'src/app.py')
    with pytest.raises(StoreError, match='situational_skill_required'):
        mcp_tools.dispatch(store, worker, 'result_submit', result_args())


def test_parallel_workers_do_not_inherit_each_others_documentation_gate(tmp_path):
    store,task,worker=setup_worker(tmp_path)
    coordinator=resolve_channel(store,task.channel_id,task.secret)
    other,assignment=provision_fixture(store,coordinator,key='parallel')
    with store.connection() as db:
        db.execute("UPDATE runtime_nodes SET scope_json=? WHERE assignment_ref="
                   "(SELECT idempotency_key FROM delegation_provisioning WHERE attempt_id=?)",
                   (json.dumps(['src/stats/**','tests/unit/**']),worker.attempt_id))
        db.execute("UPDATE runtime_nodes SET scope_json=? WHERE assignment_ref=?",
                   (json.dumps(['src/app.py','README.md','docs/**']),assignment))
        db.commit()
    delegation_revision.record_attempt_revision(store,other.attempt_id,f'worker_start:{other.attempt_id}',tmp_path)
    (tmp_path/'src/stats').mkdir()
    native_edit(tmp_path,'src/stats/words.py','src/app.py','README.md','docs/guide.md')
    changed,_=skill_gate.observed_changed_paths(store,task.task_id,worker.attempt_id)
    assert skill_gate.attributable_changed_paths(store,worker,changed)=={'src/stats/words.py'}
    # F2 fix: the early shared-tree hint is advisory, but it is also
    # attributed -- this worker's own changed paths are only code, so it
    # must never be hinted about the sibling's unrelated docs/README edits.
    assert not skill_gate.required_before_result_submit(store,worker,stage='execution',
                                                        role_ids=['backend-dev'],advisory=True)
    assert mcp_tools.dispatch(store,worker,'result_submit',result_args())['recorded']
    with pytest.raises(StoreError,match='situational_skill_required') as error:
        mcp_tools.dispatch(store,other,'result_submit',result_args('other-result'))
    assert 'README.md' in error.value.received and 'src/stats/words.py' not in error.value.received


def test_own_gateway_writes_outside_declared_scope_still_require_skill(tmp_path):
    store,task,worker=setup_worker(tmp_path)
    coordinator=resolve_channel(store,task.channel_id,task.secret)
    _,assignment=provision_fixture(store,coordinator,key='docs-owner')
    with store.connection() as db:
        db.execute("UPDATE runtime_nodes SET scope_json=? WHERE assignment_ref="
                   "(SELECT idempotency_key FROM delegation_provisioning WHERE attempt_id=?)",
                   (json.dumps(['src/**']),worker.attempt_id))
        db.execute("UPDATE runtime_nodes SET scope_json=? WHERE assignment_ref=?",
                   (json.dumps(['docs/**']),assignment))
        db.commit()
    # Gateway receipts provide attribution even for an overlapping reservation.
    for path,content in [('src/extra.py','X=2'),('docs/new.md','# New')]:
        mcp_tools.dispatch(store,worker,'project_write',dict(path=path,content=content,expected_sha256='absent'))
    with pytest.raises(StoreError,match='situational_skill_required') as error:
        mcp_tools.dispatch(store,worker,'result_submit',result_args())
    assert 'docs/new.md' in error.value.received and 'src/extra.py' in error.value.received


def test_native_edits_in_sibling_scope_only_produce_advisory_hint(tmp_path):
    store,task,worker=setup_worker(tmp_path)
    coordinator=resolve_channel(store,task.channel_id,task.secret)
    _,other=provision_fixture(store,coordinator,key='overlapping-owner')
    with store.connection() as db:
        db.execute("UPDATE runtime_nodes SET scope_json=? WHERE assignment_ref=?", (json.dumps(['.']),other))
        db.commit()  # both reserve '.': native authorship is unknown
    native_edit(tmp_path,'docs/guide.md','src/app.py')
    changed,_=skill_gate.observed_changed_paths(store,task.task_id,worker.attempt_id)
    assert skill_gate.attributable_changed_paths(store,worker,changed)==set()
    assert mcp_tools.dispatch(store,worker,'result_submit',result_args())['recorded']


@pytest.mark.parametrize('other_role,other_revision', [('architect',1),('backend-dev',0)])
def test_readonly_or_prior_revision_scopes_cannot_hide_own_changes(tmp_path,other_role,other_revision):
    store,task,worker=setup_worker(tmp_path)
    coordinator=resolve_channel(store,task.channel_id,task.secret)
    _,other=provision_fixture(store,coordinator,key='not-current-writer',role=other_role)
    with store.connection() as db:
        db.execute("UPDATE runtime_nodes SET scope_json=?,revision=? WHERE assignment_ref=?",
                   (json.dumps(['.']),other_revision,other))
        db.commit()
    native_edit(tmp_path,'docs/guide.md','src/app.py')
    with pytest.raises(StoreError,match='situational_skill_required'):
        mcp_tools.dispatch(store,worker,'result_submit',result_args())


def test_sibling_gateway_changes_outside_its_reservation_are_not_charged_to_own_scope(tmp_path):
    store,task,worker=setup_worker(tmp_path)
    coordinator=resolve_channel(store,task.channel_id,task.secret)
    other,_=provision_fixture(store,coordinator,key='unreserved-sibling-write')
    delegation_revision.record_attempt_revision(store,other.attempt_id,f'worker_start:{other.attempt_id}',tmp_path)
    (tmp_path/'src/stats').mkdir()
    native_edit(tmp_path,'src/stats/words.py')
    for path,content in [('docs/guide.md','# Changed by sibling'),('src/app.py','X=2')]:
        current=mcp_tools.dispatch(store,other,'project_read',dict(path=path))
        mcp_tools.dispatch(store,other,'project_write',
                           dict(path=path,content=content,expected_sha256=current['sha256']))
    changed,_=skill_gate.observed_changed_paths(store,task.task_id,worker.attempt_id)
    assert skill_gate.attributable_changed_paths(store,worker,changed)=={'src/stats/words.py'}
    assert mcp_tools.dispatch(store,worker,'result_submit',result_args())['recorded']
    with pytest.raises(StoreError,match='situational_skill_required'):
        mcp_tools.dispatch(store,other,'result_submit',result_args('sibling-result'))


def test_advisory_hint_is_attributed_like_the_hard_gate(tmp_path):
    """F2 regression: a code-only worker's gateway write receipt must not
    carry required_before_result_submit just because a sibling wrote
    docs/** -- both this worker's own gateway writes were code, so the
    sibling's docs edit is never attributable to it even in advisory mode."""
    store,task,worker=setup_worker(tmp_path)
    coordinator=resolve_channel(store,task.channel_id,task.secret)
    other,assignment=provision_fixture(store,coordinator,key='docs-sibling')
    with store.connection() as db:
        db.execute("UPDATE runtime_nodes SET scope_json=? WHERE assignment_ref="
                   "(SELECT idempotency_key FROM delegation_provisioning WHERE attempt_id=?)",
                   (json.dumps(['src/**']),worker.attempt_id))
        db.execute("UPDATE runtime_nodes SET scope_json=? WHERE assignment_ref=?",
                   (json.dumps(['docs/**']),assignment))
        db.commit()
    delegation_revision.record_attempt_revision(store,other.attempt_id,f'worker_start:{other.attempt_id}',tmp_path)
    receipt=mcp_tools.dispatch(store,worker,'project_write',
                               dict(path='src/extra.py',content='X=2',expected_sha256='absent'))
    assert 'required_before_result_submit' not in receipt
    current=mcp_tools.dispatch(store,other,'project_read',dict(path='docs/guide.md'))
    mcp_tools.dispatch(store,other,'project_write',
                       dict(path='docs/guide.md',content='# Changed by sibling',expected_sha256=current['sha256']))
    # Written after the sibling's docs edit: the shared-tree delta now
    # includes docs/guide.md, but it is still not this worker's own change.
    receipt=mcp_tools.dispatch(store,worker,'project_write',
                               dict(path='src/more.py',content='Y=3',expected_sha256='absent'))
    assert 'required_before_result_submit' not in receipt
    assert mcp_tools.dispatch(store,worker,'result_submit',result_args())['recorded']

def test_project_edit_attributed_outside_declared_scope_and_gated_at_result(tmp_path):
    store, task, worker = setup_worker(tmp_path)
    with store.connection() as db:
        db.execute("UPDATE runtime_nodes SET scope_json=? WHERE assignment_ref=(SELECT idempotency_key FROM delegation_provisioning WHERE attempt_id=?)",
                   (json.dumps(['src']), worker.attempt_id))
        db.commit()
    native_edit(tmp_path, 'src/app.py')
    mcp_tools.dispatch(store, worker, 'project_read', dict(path='docs/guide.md'))
    receipt = mcp_tools.dispatch(store, worker, 'project_edit', dict(path='docs/guide.md',
        edits=[dict(old_string='Value is 1.', new_string='Value is 2.')]))
    # The write receipt's hint sees only gateway writes, not the native edit.
    assert 'required_before_result_submit' not in receipt
    changed = skill_gate.attributable_changed_paths(store, worker, {'src/app.py', 'docs/guide.md'})
    assert changed == {'src/app.py', 'docs/guide.md'}
    with pytest.raises(StoreError, match='situational_skill_required'):
        mcp_tools.dispatch(store, worker, 'result_submit', result_args())
