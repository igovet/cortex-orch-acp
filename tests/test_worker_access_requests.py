"""No-paid access reservations, decision continuation and actual launch authority."""
import concurrent.futures
import json
import os
from pathlib import Path
import shutil
import subprocess
import threading
from types import SimpleNamespace

import pytest

from cortex_runtime.acp import questions_store
from cortex_runtime.acp.orchestrating_runtime import OrchestratingRuntime, PromptOutcome
from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.contracts import StoreError
from cortex_runtime.delegation import capacity, mcp_tools
from cortex_runtime.delegation.live_adapters import _jail_for
from cortex_runtime.orchestration import access, policy, question_batches, worker_waits
from cortex_runtime.orchestration.sandbox import engine_jail
from cortex_runtime.orchestration.state import TaskMachine, event
from cortex_runtime.portable.engine_contract import StopReason
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.recovery.capsule import build_capsule
from cortex_runtime.store import Store
from cortex_runtime import task_workspace
from orchestration_fakes import FakeConfigSnapshotSource, FakeSessionRuntime, FakeWorkspace
from test_worker_wait_continuation import bound, _plan


@pytest.fixture(autouse=True)
def fast_access_backoff(monkeypatch):
    # Integration cases retain the bounded number of checks without wall-clock
    # sleeps; the deadline regression below exercises the production delays.
    monkeypatch.setattr(access, 'RETRY_DELAYS_SECONDS', (0, 0, 0))


def _exhaust(store, channel):
    for _ in access.RETRY_DELAYS_SECONDS:
        access.reconcile(store, channel.task_id)
    return _wait(store, channel)


def _request(store, channel, paths, key='access'):
    return mcp_tools.dispatch(store, channel, 'access_request',
                              dict(paths=paths, reason='Required for this assignment', request_key=key))


def _wait(store, channel):
    return worker_waits.list_waits(store, channel.task_id)[-1]


def _claim(store, channel):
    wait = _wait(store, channel)
    return worker_waits.claim(store, channel.task_id, wait['assignment_ref'], wait['engine_session_ref'])


def _release(store, channel):
    with store.connection() as db:
        row = db.execute('SELECT idempotency_key FROM delegation_provisioning WHERE attempt_id=?',
                          (channel.attempt_id,)).fetchone()
        event(db, channel.task_id, 'WORKER_RELEASED', dict(assignment_ref=row[0]))
        db.commit()


def test_safe_extension_is_atomic_replayable_and_visible_not_a_fake_remount(bound):
    store, machine, coordinator, workers = bound
    before = machine.context(coordinator.task_id)['plan']
    receipt = _request(store, workers[0], ['new/deep/file.py', 'new/deep/file.py'])
    assert receipt['status'] == 'granted' and receipt['complete']
    assert receipt['enforcement'] == 'already_project_writable'
    assert receipt['added_paths'] == ['new/deep/file.py']
    assert receipt == _request(store, workers[0], ['new/deep/file.py', 'new/deep/file.py'])
    with pytest.raises(StoreError, match='request_key_conflict'):
        _request(store, workers[0], ['different'], key='access')
    context = machine.context(coordinator.task_id)
    assert context['plan'] == before
    assert context['nodes'][0]['scope'] == ['part0.txt']
    assert context['nodes'][0]['access']['effective_declared_scope'] == ['part0.txt', 'new/deep/file.py']
    assert context['nodes'][0]['access']['declared_scope_revision'] == 1
    assert _request(store, workers[0], ['new/deep/file.py'], key='covered')['status'] == 'already_covered'
    recipient = dict(role='worker', assignment_ref=context['nodes'][0]['assignment_ref'], attempt_id=workers[0].attempt_id)
    capsule = build_capsule(store, coordinator.task_id, recipient)
    assert capsule['sections']['pipeline_and_assignments']['runtime']['nodes'][0]['access']['declared_scope_revision'] == 1


@pytest.mark.parametrize('path', ['../escape', '/absolute', '.cortex/private', '.git/config',
                                  'nested/.cortex/x', '.', 'dir//file', 'dir/./file',
                                  'glob/*', 'back\\slash', 'bad\x00path'])
def test_unsafe_request_is_runtime_denial_without_node_failure(bound, path):
    store, machine, coordinator, workers = bound
    receipt = _request(store, workers[0], [path])
    assert receipt['status'] == 'pending' and receipt['reason'] == 'unsafe_path'
    wait = _wait(store, coordinator)
    assert wait['status'] == 'ready' and wait['question_id'] is None
    assert not questions_store.list_open(store, coordinator.task_id)
    decision = json.loads(_claim(store, coordinator)[1])
    assert decision['status'] == 'denied' and decision['reason'] == 'unsafe_path'
    assert machine.context(coordinator.task_id)['state'] == 'EXECUTING'
    assert all(n['status'] == 'running' for n in machine.context(coordinator.task_id)['nodes'])
    mcp_tools.dispatch(store, workers[0], 'project_write',
                       dict(path='part0.txt', content='alternative', expected_sha256='absent', request_key='alternative'))


@pytest.mark.parametrize('kind', ['symlink', 'parent_symlink', 'hardlink', 'fifo'])
def test_existing_topology_cannot_be_granted(bound, kind):
    store, _, coordinator, workers = bound
    root = Path(store.project_root)
    (root / 'target').write_text('original')
    if kind == 'symlink':
        (root / 'bad').symlink_to('target')
        path = 'bad'
    elif kind == 'parent_symlink':
        (root / 'bad').symlink_to(root, target_is_directory=True)
        path = 'bad/new'
    elif kind == 'hardlink':
        os.link(root / 'target', root / 'bad')
        path = 'bad'
    else:
        os.mkfifo(root / 'bad')
        path = 'bad'
    assert _request(store, workers[0], [path])['reason'] == 'unsafe_path'
    assert (root / 'target').read_text() == 'original'


@pytest.mark.parametrize('applied', [False, True])
def test_owner_denials_including_queued_policy_are_not_overridden(bound, applied):
    store, _, coordinator, workers = bound
    policy.steer(store, coordinator.task_id, 'deny-write: blocked', 'owner-denial')
    if applied:
        with store.connection() as db:
            event(db, coordinator.task_id, 'STEERING_APPLIED', {}, 'owner-denial')
            db.commit()
    receipt = _request(store, workers[0], ['blocked/file'])
    assert receipt['reason'] == 'deny_write_policy'
    wait = _wait(store, coordinator)
    assert not questions_store.list_open(store, coordinator.task_id)
    assert json.loads(_claim(store, coordinator)[1])['reason'] == 'deny_write_policy'
    with store.connection() as db:
        assert not access.grants(db, coordinator.task_id, wait['assignment_ref'])


@pytest.mark.parametrize('read_only', ['role', 'review'])
def test_read_only_access_cannot_elevate_a_running_worker(bound, read_only):
    store, _, coordinator, workers = bound
    with store.connection() as db:
        db.execute('UPDATE runtime_nodes SET ' + ("role='worker-explorer'" if read_only == 'role' else "stage='review'")
                   + ' WHERE task_id=? AND node_id=?', (coordinator.task_id, 'N0'))
        db.commit()
    assert _request(store, workers[0], ['new'])['reason'] == 'read_only_role'
    wait = _wait(store, coordinator)
    assert not questions_store.list_open(store, coordinator.task_id)
    assert json.loads(_claim(store, coordinator)[1])['status'] == 'denied'
    with pytest.raises(StoreError, match='gateway_scope_denied'):
        mcp_tools.dispatch(store, workers[0], 'project_write',
                           dict(path='new', content='bad', expected_sha256='absent', request_key='bad'))


def test_conflict_holds_until_observed_release_then_same_attempt_claims(bound):
    store, machine, coordinator, workers = bound
    receipt = _request(store, workers[0], ['part1.txt'])
    assert receipt['reason'] == 'active_writer_conflict'
    wait = _wait(store, coordinator)
    with pytest.raises(StoreError, match='worker_waiting_for_answer'):
        mcp_tools.dispatch(store, workers[0], 'project_write',
                           dict(path='part1.txt', content='bad', expected_sha256='absent', request_key='bad'))
    # A result without observed process release does not release its holdings.
    with store.connection() as db:
        db.execute("UPDATE runtime_nodes SET status='completed' WHERE node_id='N1'")
        db.commit()
    access.reconcile(store, coordinator.task_id)
    assert _wait(store, coordinator)['status'] == 'open'
    _release(store, workers[1])
    access.reconcile(store, coordinator.task_id)
    claimed, decision = _claim(store, coordinator)
    assert json.loads(decision)['status'] == 'granted'
    assert claimed['attempt_id'] == workers[0].attempt_id
    assert claimed['channel_id'] == workers[0].channel_id
    assert questions_store.get(store, wait['question_id']) is None  # no user question was created
    assert not questions_store.answer(store, wait['question_id'], 'late', 'text')
    assert _claim(store, coordinator) is None
    mcp_tools.dispatch(store, workers[0], 'project_write',
                       dict(path='part1.txt', content='new logical action', expected_sha256='absent', request_key='new-write'))
    assert machine.context(coordinator.task_id)['state'] == 'EXECUTING'


def test_dead_worker_of_abandoned_task_releases_scope_only_on_confirmed_exit(bound):
    store, machine, coordinator, workers = bound
    with store.connection() as db:
        ref1 = db.execute('SELECT idempotency_key FROM delegation_provisioning WHERE attempt_id=?',
                          (workers[1].attempt_id,)).fetchone()[0]
    machine.move(coordinator.task_id, 'BLOCKED', blocker='abandoned')
    with store.connection() as db:
        db.execute("UPDATE runtime_nodes SET status='failed' WHERE node_id='N1'")
        db.commit()

    def held():
        with store.connection() as db:
            return access.conflicting(db, coordinator.task_id, 'other', ['part1.txt'])

    # No WORKER_RELEASED and no confirmed exit: the scope stays held.
    assert held()
    cap = capacity.reserve(store, 'cn_test', coordinator.task_id, workers[1].attempt_id, 'worker', assignment_ref=ref1)
    assert held()
    capacity.mark_ambiguous(store, cap)
    assert held()
    with store.connection() as db:
        db.execute("UPDATE capacity_reservations SET state='released',release_observation='process_exited' "
                   "WHERE reservation_id=?", (cap,))
        db.commit()
    assert not held()
    # A later live reservation for the same attempt holds the scope again.
    capacity.reserve(store, 'cn_test', coordinator.task_id, workers[1].attempt_id, 'worker', assignment_ref=ref1)
    assert held()


def test_concurrent_grants_serialize_and_fifo_prevents_overtaking(bound):
    store, _, coordinator, workers = bound
    with concurrent.futures.ThreadPoolExecutor(max_workers=2) as pool:
        receipts = list(pool.map(lambda w: _request(store, w, ['shared']), workers))
    assert sorted(r['status'] for r in receipts) == ['granted', 'pending']
    winner = workers[receipts.index(next(r for r in receipts if r['status'] == 'granted'))]
    loser = workers[1] if winner == workers[0] else workers[0]
    _release(store, winner)
    access.reconcile(store, coordinator.task_id)
    wait = _wait(store, coordinator)
    assert wait['attempt_id'] == loser.attempt_id and wait['status'] == 'ready'


def test_conflict_cycle_is_denied_by_runtime_and_can_replan(bound):
    store, machine, coordinator, workers = bound
    _request(store, workers[0], ['part1.txt'])
    _request(store, workers[1], ['part0.txt'])
    _exhaust(store, coordinator)
    waits = worker_waits.list_waits(store, coordinator.task_id)
    assert all(w['status'] == 'ready' for w in waits)
    assert not questions_store.list_open(store, coordinator.task_id)
    claimed = worker_waits.claim(store, coordinator.task_id, waits[0]['assignment_ref'], 'es_test0')
    assert json.loads(claimed[1])['status'] == 'denied'
    mcp_tools.dispatch(store, workers[0], 'scope_request',
                       dict(scope=['part1.txt'], reason='Resolve cycle with replacement plan', request_key='fallback'))
    assert machine.context(coordinator.task_id)['state'] == 'RECOVERING'
    assert all(w['status'] not in worker_waits.ACTIVE for w in worker_waits.list_waits(store, coordinator.task_id))


def test_stop_and_new_policy_win_before_continuation(bound):
    store, machine, coordinator, workers = bound
    _request(store, workers[0], ['part1.txt'])
    policy.steer(store, coordinator.task_id, 'deny-write: part1.txt', 'queued')
    _release(store, workers[1])
    access.reconcile(store, coordinator.task_id)
    wait = _wait(store, coordinator)
    assert wait['retry_at'] is None and wait['reason'] == 'deny_write_policy'
    assert not questions_store.list_open(store, coordinator.task_id)
    assert _wait(store, coordinator)['access_decision']['status'] == 'denied'
    machine.cancel(coordinator.task_id)
    assert _claim(store, coordinator) is None


def test_revalidate_ready_grant_before_claim_and_retain_true_decision(bound):
    store, machine, coordinator, workers = bound
    _request(store, workers[0], ['part1.txt'])
    _release(store, workers[1])
    access.reconcile(store, coordinator.task_id)
    assert _wait(store, coordinator)['access_decision']['status'] == 'granted'
    policy.steer(store, coordinator.task_id, 'deny-write: part1.txt', 'late-policy')
    assert json.loads(_claim(store, coordinator)[1])['reason'] == 'deny_write_policy'
    node = machine.context(coordinator.task_id)['nodes'][0]
    assert node['status'] == 'running'
    assert node['access']['last_request']['decision']['reason'] == 'deny_write_policy'


def test_scope_grant_does_not_bypass_fresh_write_topology_checks(bound):
    store, _, _, workers = bound
    assert _request(store, workers[0], ['ordinary'])['status'] == 'granted'
    root = Path(store.project_root)
    (root / 'target').write_text('unchanged')
    (root / 'ordinary').symlink_to('target')
    with pytest.raises(StoreError):
        mcp_tools.dispatch(store, workers[0], 'project_write',
                           dict(path='ordinary', content='bad', expected_sha256='absent', request_key='write'))
    assert (root / 'target').read_text() == 'unchanged'


def test_worker_only_advertisement_and_bounded_closed_schema(bound):
    store, _, coordinator, workers = bound
    assert 'access_request' not in {t['name'] for t in mcp_tools.tool_definitions('coordinator')}
    assert 'access_request' in {t['name'] for t in mcp_tools.tool_definitions('worker')}
    with pytest.raises(StoreError):
        _request(store, coordinator, ['new'])
    for change in [dict(paths=[]), dict(paths=['x'] * 17), dict(paths=['x' * 513]),
                   dict(reason=' '), dict(unadvertised=True)]:
        args = dict(paths=['x'], reason='Required', request_key='schema')
        args.update(change)
        with pytest.raises(StoreError, match='invalid_arguments'):
            mcp_tools.dispatch(store, workers[0], 'access_request', args)


def test_added_paths_are_retained_by_targeted_review_fixes(bound):
    store, machine, coordinator, workers = bound
    _request(store, workers[0], ['extra'])
    with store.connection() as db:
        db.execute("UPDATE runtime_tasks SET state='REVIEWING',review_cycle=1 WHERE task_id=?", (coordinator.task_id,))
        db.commit()
    machine.add_fix(coordinator.task_id, dict(node_id='_review_1', result=dict(
        summary='Fix extra', report_ref='r_123456789abc',
        findings=[dict(node='N0', files=['extra'], fix='Correct extra', cross_cutting=False)])))
    fixes = [n for n in machine.context(coordinator.task_id)['nodes'] if n['node_id'].startswith('_fix_')]
    assert len(fixes) == 1 and fixes[0]['scope'] == ['part0.txt', 'extra']
    assert machine.context(coordinator.task_id)['plan']['tasks'][0]['scope'] == ['part0.txt']


def test_pending_fifo_blocks_new_scheduler_overtakers(bound):
    store, machine, coordinator, workers = bound
    _request(store, workers[0], ['part1.txt'])
    boot = bootstrap_task(store, SourceEvent(origin='access-test', event_id='overtaker'), 'Other', 'owner')
    machine.create(boot.task_id, 'Other')
    other = SimpleNamespace(task_id=boot.task_id, channel_id='coordinator', role='coordinator')
    plan = _plan()
    plan['tasks'][0]['scope'] = ['part1.txt']
    machine.submit_plan(other, plan, 'plan')
    machine.move(boot.task_id, 'EXECUTING')
    _release(store, workers[1])
    # The old holding is gone but its older queued access requester takes priority.
    assert machine.claim_ready(boot.task_id, 'coordinator') == []
    access.reconcile(store, coordinator.task_id)
    assert _wait(store, coordinator)['status'] == 'ready'
    assert machine.claim_ready(boot.task_id, 'coordinator') == []


def test_extension_reservations_block_later_scheduler_claims_across_tasks(bound):
    store, machine, coordinator, workers = bound
    _request(store, workers[0], ['later'])
    boot = bootstrap_task(store, SourceEvent(origin='access-test', event_id='other'), 'Other', 'owner')
    machine.create(boot.task_id, 'Other')
    other = SimpleNamespace(task_id=boot.task_id, channel_id='coordinator', role='coordinator')
    plan = _plan()
    plan['tasks'][0]['scope'] = ['later/file']
    machine.submit_plan(other, plan, 'plan')
    machine.move(boot.task_id, 'EXECUTING')
    assert machine.claim_ready(boot.task_id, 'coordinator') == []
    # An independent linked workspace does not share source reservations.
    root = Path(store.project_root) / 'separate'
    root.mkdir()
    with store.connection() as db:
        task_workspace.record_task(db, boot.task_id, str(root))
        db.commit()
    assert len(machine.claim_ready(boot.task_id, 'coordinator')) == 1


@pytest.mark.requires_bwrap
def test_real_production_jail_already_writes_sixth_file_and_keeps_denial(bound, tmp_path):
    if shutil.which('bwrap') is None:
        pytest.skip('bubblewrap is required')
    store, _, coordinator, workers = bound
    root = Path(store.project_root)
    (root / 'blocked').write_text('original')
    policy.steer(store, coordinator.task_id, 'deny-write: blocked', 'owner-denial')
    with store.connection() as db:
        db.execute("UPDATE runtime_nodes SET scope_json=? WHERE task_id=? AND node_id='N0'",
                   (json.dumps([f'file{i}' for i in range(5)]), coordinator.task_id))
        event(db, coordinator.task_id, 'STEERING_APPLIED', {}, 'owner-denial')
        db.commit()
    # Issue real test credentials for the existing assignment, then use the
    # unmocked production launch-authority path.
    from cortex_runtime.trusted_channel import issue_channel_tx, resolve_channel
    with store.connection() as db:
        channel_id, secret = issue_channel_tx(db, coordinator.task_id, 'worker', coordinator.owner_epoch,
                                             parent_channel_id=coordinator.channel_id,
                                             assignment_id=workers[0].assignment_id, attempt_id=workers[0].attempt_id)
        db.execute('UPDATE delegation_provisioning SET channel_id=? WHERE attempt_id=?',
                   (channel_id, workers[0].attempt_id))
        db.execute('UPDATE trusted_channels SET revoked_at=? WHERE channel_id=?',
                   (store._now(), workers[0].channel_id))
        db.commit()
    workers[0] = resolve_channel(store, channel_id, secret)
    jail = _jail_for(store, (channel_id, secret, str(root)))
    assert jail['scopes'] == ['.'] and jail['readonly'] == ['blocked']
    scratch = root / 'scratch'
    scratch.mkdir()
    home = root / 'home'
    home.mkdir()
    with engine_jail(str(root), str(scratch), ['/bin/sh', '-c',
                     'echo sixth > sixth; echo bad > blocked 2>/dev/null || echo denied'],
                     jail['scopes'], readonly=jail['readonly'], env={'HOME': str(home), 'PATH': '/usr/bin:/bin'}) as (argv, fds):
        result = subprocess.run(argv, pass_fds=fds, capture_output=True, text=True, timeout=30)
    assert result.returncode == 0 and 'denied' in result.stdout
    assert (root / 'sixth').read_text() == 'sixth\n'
    assert (root / 'blocked').read_text() == 'original'
    assert _request(store, workers[0], ['sixth'])['enforcement'] == 'already_project_writable'


@pytest.mark.parametrize('mode', ['grant', 'denial', 'conflict', 'external_conflict'])
def test_access_decision_continues_same_session_through_result_review_verification(tmp_path, mode):
    store = Store(tmp_path / '.cortex', project_root=tmp_path)
    boot = bootstrap_task(store, SourceEvent(origin='flow-access', event_id=mode), 'Implement', 'owner')
    identities, prompts = [], []
    pending = threading.Event()
    if mode in {'external_conflict'}:
        machine = TaskMachine(store)
        other_boot = bootstrap_task(store, SourceEvent(origin='flow-access', event_id='external'), 'Other', 'owner')
        machine.create(other_boot.task_id, 'Other')
        other = SimpleNamespace(task_id=other_boot.task_id, channel_id='coordinator', role='coordinator')
        plan = _plan()
        plan['tasks'][0]['scope'] = ['external']
        machine.submit_plan(other, plan, 'plan')
        machine.move(other_boot.task_id, 'EXECUTING')
        external = machine.claim_ready(other_boot.task_id, 'coordinator')[0]
    def router(store, channel, ref, launch, instructions):
        def behavior(store, channel, ref, message, call_index):
            if channel.role == 'coordinator':
                if TaskMachine(store).context(boot.task_id)['state'] == 'PLANNING':
                    mcp_tools.dispatch(store, channel, 'plan_submit', dict(plan=_plan(2), request_key='plan'))
                else:
                    mcp_tools.dispatch(store, channel, 'synthesis_submit', dict(text='Verified.'))
                return PromptOutcome(StopReason.END_TURN, '')
            with store.connection() as db:
                node = db.execute('SELECT n.* FROM runtime_nodes n JOIN delegation_provisioning p '
                                  'ON n.assignment_ref=p.idempotency_key WHERE p.attempt_id=?', (channel.attempt_id,)).fetchone()
            if node['node_id'] == 'N0':
                identities.append((channel.attempt_id, channel.channel_id, ref))
                prompts.append(message)
                if call_index == 0:
                    path = ('extra' if mode == 'grant' else '.cortex/private' if mode == 'denial'
                            else 'external' if mode in {'external_conflict'} else 'part1.txt')
                    receipt = _request(store, channel, [path])
                    pending.set()
                    if mode != 'grant':
                        assert receipt['status'] == 'pending'
                        return PromptOutcome(StopReason.END_TURN, '')
                else:
                    assert 'Runtime access decision continuation' in message
                    if mode in {'denial', 'external_conflict'}:
                        assert 'denied' in message
                    else:
                        # A zero-delay test cycle may exhaust before the
                        # sibling's confirmed release; either is a runtime
                        # decision, never a user access question.
                        assert 'granted' in message or 'denied' in message
                mcp_tools.dispatch(store, channel, 'project_write',
                                   dict(path='extra', content='done', expected_sha256='absent', request_key='write'))
            if node['node_id'] == 'N1':
                assert pending.wait(10)
            args = dict(report='# Observed\n\nFinished.', report_title='Observed',
                        summary='Finished.', checks=[], request_key='result')
            if node['stage'] == 'review':
                args['verdict'] = 'APPROVE'
            mcp_tools.dispatch(store, channel, 'result_submit', args)
            return PromptOutcome(StopReason.END_TURN, '')
        return behavior
    engine = FakeSessionRuntime(store, router)
    runtime = OrchestratingRuntime(store, engine, FakeWorkspace(), FakeConfigSnapshotSource(), allow_unqualified_delivery=True)
    class Elicitor:
        def elicit(self, *args):
            pytest.fail('runtime access decisions must never reach the user')
    try:
        runtime.start_prompt(boot.task_id, 'incoming', 'Implement', lambda text: True, threading.Event(),
                             host_elicitor=Elicitor())
        assert TaskMachine(store).context(boot.task_id)['state'] == 'COMPLETED'
        assert (tmp_path / 'extra').read_text() == 'done'
        assert len(set(identities)) == 1 and len(prompts) == (1 if mode == 'grant' else 2)
        with store.connection() as db:
            assert not db.execute("SELECT 1 FROM task_events WHERE event_type='assignment_failed'").fetchone()
            assert not db.execute("SELECT 1 FROM runtime_events WHERE type='ASSIGNMENT_RETRIED'").fetchone()
    finally:
        runtime.shutdown()


def test_unbound_running_node_reserves_scope_and_excludes_stale_revisions(bound):
    store, machine, coordinator, _ = bound
    with store.connection() as db:
        db.execute("UPDATE runtime_nodes SET assignment_ref=NULL WHERE task_id=?",
                   (coordinator.task_id,))
        db.execute("UPDATE runtime_nodes SET status='pending',scope_json=? "
                   "WHERE task_id=? AND node_id='N1'",
                   (json.dumps(['part0.txt']), coordinator.task_id))
        db.execute("INSERT INTO runtime_nodes SELECT task_id,revision-1,node_id,role,objective,"
                   "dependencies_json,scope_json,stage,status,assignment_ref,result_json "
                   "FROM runtime_nodes WHERE task_id=? AND node_id='N0'", (coordinator.task_id,))
        db.commit()
        held = access.holdings(db)
        assert [(n['node_id'], n['revision']) for n in held] == [
            ('N0', machine.context(coordinator.task_id)['revision'])]
    # A current running reservation remains conservative even before binding;
    # stale unbound revisions must not reserve the same path forever.
    assert machine.claim_ready(coordinator.task_id, coordinator.channel_id) == []
    assert machine.context(coordinator.task_id)['nodes'][1]['status'] == 'pending'


def _card_runtime(store):
    return OrchestratingRuntime(store, FakeSessionRuntime(store, lambda *args: None), FakeWorkspace(),
                                FakeConfigSnapshotSource(), allow_unqualified_delivery=True)


def test_retry_deadlines_later_success_and_no_card(bound, monkeypatch):
    from datetime import datetime, timedelta, timezone
    store, _, coordinator, workers = bound
    monkeypatch.setattr(access, 'RETRY_DELAYS_SECONDS', (1, 2, 4))
    clock = [datetime.now(timezone.utc)]
    monkeypatch.setattr(access, 'now', lambda: clock[0].isoformat(timespec='microseconds'))
    original = access.predicate
    checks = []
    def counted(*args):
        checks.append(clock[0])
        return original(*args)
    monkeypatch.setattr(access, 'predicate', counted)
    _request(store, workers[0], ['part1.txt'])
    runtime = _card_runtime(store)
    cards = []
    class Elicitor:
        def elicit(self, *args):
            cards.append(args)
    try:
        first = _wait(store, coordinator)
        assert first['retry_attempt'] == 0 and first['retry_at']
        for _ in range(3):
            runtime._service_worker_questions(coordinator.task_id, 'incoming', lambda t: True, 'en', Elicitor())
        assert len(checks) == 1 and not cards
        assert not questions_store.list_open(store, coordinator.task_id)
        clock[0] += timedelta(seconds=1)
        runtime._service_worker_questions(coordinator.task_id, 'incoming', lambda t: True, 'en', Elicitor())
        second = _wait(store, coordinator)
        assert second['retry_attempt'] == 1
        assert datetime.fromisoformat(second['retry_at']) == clock[0] + timedelta(seconds=2)
        _release(store, workers[1])
        clock[0] += timedelta(seconds=1)
        access.reconcile(store, coordinator.task_id)
        assert _wait(store, coordinator)['status'] == 'open'
        clock[0] += timedelta(seconds=1)
        runtime._service_worker_questions(coordinator.task_id, 'incoming', lambda t: True, 'en', Elicitor())
        claimed, answer = _claim(store, coordinator)
        assert json.loads(answer)['status'] == 'granted'
        assert claimed['engine_session_ref'] == 'es_test0' and not cards
        assert not questions_store.list_open(store, coordinator.task_id)
        with store.connection() as db:
            assert not db.execute("SELECT 1 FROM pending_questions WHERE answer_text IS NOT NULL").fetchone()
    finally:
        runtime.shutdown()


def test_exhaustion_is_bounded_persisted_runtime_denial(bound):
    store, _, coordinator, workers = bound
    _request(store, workers[0], ['part1.txt'])
    wait = _exhaust(store, coordinator)
    assert wait['status'] == 'ready' and wait['retry_at'] is None
    assert wait['retry_attempt'] == 2 and wait['question_id'] is None
    assert wait['access_decision']['reason'] == 'active_writer_conflict'
    assert not questions_store.list_open(store, coordinator.task_id)
    _release(store, workers[1])
    for _ in range(5):
        access.reconcile(store, coordinator.task_id)
    assert _wait(store, coordinator) == wait
    assert json.loads(_claim(store, coordinator)[1])['status'] == 'denied'


def test_new_request_after_runtime_denial_uses_fresh_checks(bound):
    store, _, coordinator, workers = bound
    _request(store, workers[0], ['part1.txt'])
    _exhaust(store, coordinator)
    assert json.loads(_claim(store, coordinator)[1])['status'] == 'denied'
    _release(store, workers[1])
    assert _request(store, workers[0], ['part1.txt'], key='fresh')['status'] == 'granted'


def test_access_waits_never_reach_question_delivery(bound):
    store, _, coordinator, workers = bound
    _request(store, workers[0], ['part1.txt'])
    runtime = _card_runtime(store)
    shown = []
    class Elicitor:
        def elicit(self, *args):
            shown.append(args)
    try:
        _exhaust(store, coordinator)
        assert not runtime._service_worker_questions(coordinator.task_id, 'incoming', shown.append, 'ru', Elicitor())
        assert not shown and not runtime._worker_question_deliveries
        assert json.loads(_claim(store, coordinator)[1])['status'] == 'denied'
    finally:
        runtime.shutdown()


@pytest.mark.parametrize('path', ['.cortex/private', 'a' * 512])
def test_nonretryable_denial_is_immediate_without_question(bound, path):
    store, _, coordinator, workers = bound
    if not path.startswith('.cortex'):
        policy.steer(store, coordinator.task_id, 'deny-write: ' + path, 'deny')
    mcp_tools.dispatch(store, workers[0], 'access_request', dict(
        paths=[path], reason='x' + '\n' * 3999, request_key='bounded'))
    wait = _wait(store, coordinator)
    assert wait['status'] == 'ready' and wait['retry_at'] is None and 'retry_attempt' not in wait
    assert wait['question_id'] is None and not questions_store.list_open(store, coordinator.task_id)


@pytest.mark.parametrize('exhausted', [False, True])
def test_authority_loss_fences_retry_or_ready_decision(bound, exhausted):
    store, _, coordinator, workers = bound
    _request(store, workers[0], ['part1.txt'])
    if exhausted:
        _exhaust(store, coordinator)
    with store.connection() as db:
        db.execute('UPDATE trusted_channels SET revoked_at=? WHERE channel_id=?',
                   (store._now(), workers[0].channel_id))
        db.commit()
    access.reconcile(store, coordinator.task_id)
    assert _claim(store, coordinator) is None
    assert _wait(store, coordinator)['status'] in {'cancelled', 'orphaned'}
    assert not questions_store.list_open(store, coordinator.task_id)


def test_optionless_worker_wait_is_rejected_before_card(bound):
    store, _, coordinator, workers = bound
    receipt = mcp_tools.dispatch(store, workers[0], 'ask_question', dict(
        question='Which target?', options=['small', 'large'], request_key='question'))
    with store.connection() as db:
        wait = worker_waits.current(db, coordinator.task_id, _wait(store, coordinator)['assignment_ref'])
        del wait['options']
        worker_waits._change(db, wait, 'open')
        db.commit()
    runtime = _card_runtime(store)
    cards = []
    try:
        with pytest.raises(StoreError, match='question_options_required'):
            runtime._service_worker_questions(coordinator.task_id, 'incoming', cards.append, 'en', None)
        assert not cards and not runtime._worker_question_deliveries
    finally:
        runtime.shutdown()


def test_legacy_access_question_is_retired_and_late_answer_cannot_grant(bound):
    store, _, coordinator, workers = bound
    _request(store, workers[0], ['.cortex/private'])
    old = _wait(store, coordinator)
    question_id = 'pq_legacy_access'
    with store.connection() as db:
        db.execute("INSERT INTO pending_questions(id,task_id,assignment_ref,dedup_key,text,options_json,status,created_at) "
                   "VALUES(?,?,?,?,?,?,'open',?)",
                   (question_id, coordinator.task_id, old['assignment_ref'], 'legacy', 'Access?',
                    json.dumps(['Retry access check', 'Continue without this access']), store._now()))
        legacy = dict(old, question_id=question_id, question='Access?',
                      options=['Retry access check', 'Continue without this access'])
        legacy.pop('retry_at', None)
        worker_waits._change(db, legacy, 'open')
        db.commit()
    # Same prose is a genuine user decision unless its exact identity was
    # recorded as an access wait. No keyword classifier suppresses it.
    genuine = questions_store.create_if_absent(
        store, coordinator.task_id, 'genuine', 'Access?',
        options=['Retry access check', 'Continue without this access'])
    assert [q.id for q in questions_store.list_open(store, coordinator.task_id)] == [genuine.id]
    assert not questions_store.answer(store, question_id, 'Retry access check', 'text')
    access.reconcile(store, coordinator.task_id)
    assert questions_store.get(store, question_id).status == 'cancelled'
    replacement = _wait(store, coordinator)
    assert replacement['question_id'] is None and replacement['status'] == 'ready'
    assert json.loads(_claim(store, coordinator)[1])['reason'] == 'unsafe_path'
    assert not questions_store.answer(store, question_id, 'Retry access check', 'elicitation')


def test_legacy_exhausted_conflict_rechecks_without_user_card(bound):
    store, _, coordinator, workers = bound
    _request(store, workers[0], ['part1.txt'])
    old = _exhaust(store, coordinator)
    with store.connection() as db:
        worker_waits._change(db, old, 'open')
        db.commit()
    _release(store, workers[1])
    access.reconcile(store, coordinator.task_id)
    assert json.loads(_claim(store, coordinator)[1])['status'] == 'granted'
    assert not questions_store.list_open(store, coordinator.task_id)


def test_concurrent_reconciliation_does_not_duplicate_grant(bound):
    store, _, coordinator, workers = bound
    _request(store, workers[0], ['part1.txt'])
    _release(store, workers[1])
    with concurrent.futures.ThreadPoolExecutor(max_workers=2) as pool:
        list(pool.map(lambda _: access.reconcile(store, coordinator.task_id), range(2)))
    wait = _wait(store, coordinator)
    with store.connection() as db:
        assert len(access.grants(db, coordinator.task_id, wait['assignment_ref'])) == 1
        assert db.execute("SELECT count(*) FROM runtime_events WHERE type='ACCESS_DECIDED'").fetchone()[0] == 1
    assert json.loads(_claim(store, coordinator)[1])['status'] == 'granted'


def test_backoff_checks_once_per_deadline_without_catchup_loop(bound, monkeypatch):
    from datetime import datetime, timedelta, timezone
    store, _, coordinator, workers = bound
    monkeypatch.setattr(access, 'RETRY_DELAYS_SECONDS', (1, 2, 4))
    clock = [datetime.now(timezone.utc)]
    monkeypatch.setattr(access, 'now', lambda: clock[0].isoformat(timespec='microseconds'))
    original = access.predicate
    checks = []
    def counted(*args):
        checks.append(clock[0])
        return original(*args)
    monkeypatch.setattr(access, 'predicate', counted)
    _request(store, workers[0], ['part1.txt'])
    for attempt, delay in enumerate((1, 2, 4)):
        wait = _wait(store, coordinator)
        assert wait['retry_attempt'] == attempt
        assert datetime.fromisoformat(wait['retry_at']) == clock[0] + timedelta(seconds=delay)
        # A stalled scheduler runs a single due check, then spaces the next
        # one relative to the observed check time rather than catching up.
        clock[0] += timedelta(seconds=delay + 10)
        access.reconcile(store, coordinator.task_id)
        assert len(checks) == attempt + 2
        access.reconcile(store, coordinator.task_id)
        assert len(checks) == attempt + 2
    wait = _wait(store, coordinator)
    assert wait['retry_at'] is None and wait['access_decision']['status'] == 'denied'
