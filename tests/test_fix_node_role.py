"""Review remedies use writing specialists without widening reviewed scope."""
import json
from types import SimpleNamespace

import pytest

from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.delegation import mcp_tools
from cortex_runtime.orchestration import controller, scheduler
from cortex_runtime.orchestration.contracts import READ_ONLY
from cortex_runtime.orchestration.state import (
    TaskMachine, attempt_routing, fix_role, review_provider_preference,
)
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.portable.roles import resolve_role
from cortex_runtime.routing import auto_route, store as routing_store
from cortex_runtime.store import Store
from cortex_runtime.trusted_channel import issue_channel_tx, resolve_channel


def _review(tmp_path, role, scope, findings=True, **node_fields):
    # Read-only plan admission requires its context paths to exist.
    for name in ('src/handler.py', 'tests/test_handler.py', 'docs/guide.md'):
        path = tmp_path / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text('original\n')
    store = Store(tmp_path / '.cortex', project_root=tmp_path)
    task = bootstrap_task(store, SourceEvent(origin='test', event_id='fix-role'), 'Implement feature', 'owner')
    state = TaskMachine(store)
    state.create(task.task_id, 'Implement feature')
    channel = resolve_channel(store, task.channel_id, task.secret)
    state.submit_plan(channel, dict(
        tasks=[dict(id='source', role=role, objective='Investigate the implementation',
                    dependencies=[], scope=scope, model='auto', **node_fields)],
        verification=[['python3', '-B', '-m', 'pytest', '-q']],
    ), 'plan')
    state.move(task.task_id, 'EXECUTING')
    (tmp_path / 'src/handler.py').write_text('changed\n')
    result = dict(summary='Apply the remedy.', report_ref='r_' + 'a' * 12, checks=[])
    with store.connection() as db:
        db.execute("UPDATE runtime_nodes SET status='completed',result_json=? WHERE task_id=?",
                   (json.dumps(result), task.task_id))
        db.commit()
    state.add_review(task.task_id)
    review = dict(node_id='_review_1', result=dict(
        summary='Implement the missing guard.', report_ref='r_' + 'b' * 12,
        checks=[], verdict='REQUEST_CHANGES',
    ))
    if findings is True:
        path = {'docs': 'docs/guide.md', 'tests': 'tests/test_handler.py'}.get(
            scope[0].split('/')[0], 'src/handler.py')
        review['result']['findings'] = [
            dict(node='source', files=[path], fix='Implement the missing guard.', cross_cutting=False),
        ]
    elif findings is not None:
        review['result']['findings'] = findings
    with store.connection() as db:
        db.execute("UPDATE runtime_nodes SET status='completed',result_json=? WHERE task_id=? AND stage='review'",
                   (json.dumps(review['result']), task.task_id))
        db.commit()
    return state, channel, review


def _fix(state, channel, review):
    state.add_fix(channel.task_id, review)
    return next(n for n in state.context(channel.task_id)['nodes'] if n['node_id'] == '_fix_1_source')


def _provision(state, channel, node, attempt):
    assignment = node['assignment_ref']
    with state.store.connection() as db:
        worker_id, secret = issue_channel_tx(
            db, channel.task_id, 'worker', channel.owner_epoch,
            parent_channel_id=channel.channel_id, assignment_id=assignment, attempt_id=attempt,
        )
        db.execute(
            "INSERT INTO delegation_provisioning "
            "(idempotency_key,task_id,assignment_id,attempt_id,role_id,step,status,channel_id,"
            "instruction_delivery,owner_epoch,created_at,updated_at) "
            "VALUES(?,?,?,?,?,'ready','ready',?,'qualified',?,'now','now')",
            (assignment, channel.task_id, assignment, attempt, node['role'], worker_id, channel.owner_epoch),
        )
        db.commit()
    return resolve_channel(state.store, worker_id, secret)


@pytest.mark.parametrize('role', sorted(READ_ONLY - {'senior-consultant'}))
def test_read_only_source_gets_writing_fix_with_reviewed_scope(tmp_path, role):
    # Reviewer/consultant/build-verification are runtime-reserved, not plan roles.
    state, channel, review = _review(tmp_path, 'architect', ['src/**', 'tests/**'])
    with state.store.connection() as db:
        db.execute("UPDATE runtime_nodes SET role=? WHERE task_id=? AND node_id='source'",
                   (role, channel.task_id))
        db.commit()
    fix = _fix(state, channel, review)
    assert fix['role'] == 'worker-backend-dev'
    assert fix['role'] not in READ_ONLY
    assert fix['scope'] == ['src/**', 'tests/**']
    assert fix['dependencies'] == ['_review_1']
    assert fix['stage'] == 'execution'
    assert 'src/handler.py' in fix['objective']
    assert review['result']['report_ref'] in fix['objective']


@pytest.mark.parametrize('scope, expected', [
    (['docs/**'], 'worker-technical-writer'),
    (['docs', 'README.md', 'SECURITY.md'], 'worker-technical-writer'),
    (['tests/**'], 'worker-qa-engineer'),
    (['tests/test_handler.py'], 'worker-qa-engineer'),
    (['docs/**', 'tests/**'], 'worker-backend-dev'),
    (['.'], 'worker-backend-dev'),
    (['**'], 'worker-backend-dev'),
])
def test_read_only_fix_role_is_selected_deterministically_by_scope(tmp_path, scope, expected):
    # Exact README/SECURITY context files must exist for read-only admission.
    for name in ('README.md', 'SECURITY.md'):
        (tmp_path / name).write_text('original\n')
    state, channel, review = _review(tmp_path, 'explorer', scope, findings=None)
    fix = _fix(state, channel, review)
    assert fix['role'] == expected
    assert fix['scope'] == scope
    assert fix['objective'] == 'Address review findings within the assigned scope: Implement the missing guard.'


@pytest.mark.parametrize('scope, file, expected', [
    (['docs/**'], 'docs/guide.md', 'worker-technical-writer'),
    (['tests/**'], 'tests/test_handler.py', 'worker-qa-engineer'),
])
def test_targeted_findings_keep_the_required_files_in_the_writing_scope(tmp_path, scope, file, expected):
    state, channel, review = _review(tmp_path, 'explorer', scope)
    fix = _fix(state, channel, review)
    assert fix['role'] == expected
    assert fix['scope'] == scope
    assert file in fix['objective']
    assert review['result']['report_ref'] in fix['objective']


@pytest.mark.parametrize('role', ['backend-dev', 'frontend-dev', 'qa-engineer', 'technical-writer', 'general'])
def test_writing_source_role_and_scope_are_unchanged(tmp_path, role):
    state, channel, review = _review(tmp_path, role, ['src/**'])
    fix = _fix(state, channel, review)
    assert fix['role'] == resolve_role(role)
    assert fix['scope'] == ['src/**']


@pytest.mark.parametrize('path', ['other.py', '../outside.py', '.cortex/private.md', '.git/config'])
def test_unconfinable_findings_never_widen_the_read_only_fix_scope(tmp_path, path):
    findings = [dict(node='source', files=[path], fix='Remedy.', cross_cutting=False)]
    state, channel, review = _review(tmp_path, 'security-auditor', ['src/**'], findings)
    fix = _fix(state, channel, review)
    assert fix['role'] == 'worker-backend-dev'
    assert fix['scope'] == ['src/**']
    assert path not in fix['scope']


def test_fix_dispatch_uses_new_role_brief_and_write_authority(tmp_path, monkeypatch):
    state, channel, review = _review(tmp_path, 'architect', ['src/**'])
    fix = _fix(state, channel, review)
    sent = []
    real_dispatch = scheduler.dispatch_assignment

    def capture(store, channel, args):
        sent.append(args)
        return real_dispatch(store, channel, args)

    monkeypatch.setattr(scheduler, 'dispatch_assignment', capture)
    controller._dispatch_ready(SimpleNamespace(_store=state.store), state, channel.task_id, channel,
                               state.context(channel.task_id), None)
    (request,) = sent
    assert request['role'] == fix['role'] == 'worker-backend-dev'
    brief = json.loads(request['brief'])
    assert brief['objective'] == fix['objective']
    assert brief['scope'] == ['src/**']
    assert brief['dependency_reports'] == [dict(node='_review_1', report_ref=review['result']['report_ref'])]
    assert 'review verdict' not in request['acceptance']
    with state.store.connection() as db:
        reserved = dict(db.execute("SELECT * FROM runtime_nodes WHERE task_id=? AND node_id='_fix_1_source'",
                                   (channel.task_id,)).fetchone())
    worker = _provision(state, channel, reserved, 'xa_fix')
    routing = attempt_routing(state.store, channel.task_id, 'xa_fix')
    assert routing.escalate
    assert auto_route.tier_for(reserved['role'], escalate=routing.escalate,
                               complexity=routing.complexity) == 'deep'
    # Exercise real channel-verified file operations, not just role metadata.
    read = mcp_tools.dispatch(state.store, worker, 'project_read', dict(path='src/handler.py'))
    mcp_tools.dispatch(state.store, worker, 'project_write', dict(
        path='src/handler.py', content='fixed\n', expected_sha256=read['full_content_sha256'],
    ))
    assert (tmp_path / 'src/handler.py').read_text() == 'fixed\n'


def test_new_fix_is_reviewed_again_with_independent_provider_preference(tmp_path):
    state, channel, review = _review(tmp_path, 'architect', ['src/**'])
    _fix(state, channel, review)
    (reserved,) = state.claim_ready(channel.task_id, channel.channel_id)
    _provision(state, channel, reserved, 'xa_fix')
    routing_store.record_attempt_audit(
        state.store, attempt_id='xa_fix', task_id=channel.task_id, role=reserved['role'],
        engine='claude-agent-acp', connection_id='cn_claude', model='sonnet',
        params_fingerprint='fp', config_revision_id=None, decision='authorized', detail={},
    )
    with state.store.connection() as db:
        db.execute("UPDATE runtime_nodes SET status='completed' WHERE task_id=? AND node_id='_fix_1_source'",
                   (channel.task_id,))
        db.commit()
    state.add_review(channel.task_id)
    (reviewer,) = state.claim_ready(channel.task_id, channel.channel_id)
    assert reviewer['stage'] == 'review'
    assert reviewer['role'] == 'worker-code-reviewer'
    assert '_fix_1_source' in json.loads(reviewer['dependencies_json'])
    _provision(state, channel, reviewer, 'xa_review')
    assert review_provider_preference(state.store, channel.task_id, 'xa_review') == (
        'codex-acp', 'prefer_codex_after_claude_execution',
    )


def test_fix_role_handles_nonwriting_alias_and_empty_scope_conservatively():
    assert fix_role('senior-consultant', ['src/**']) == 'worker-backend-dev'
    assert fix_role('explorer', []) == 'worker-backend-dev'
