"""No-paid timing, immutable timeline and scheduler regressions."""
import importlib.util
import json
from pathlib import Path
import sqlite3

import pytest

from cortex_runtime.orchestration.measurement import attempt_event, attempt_wait
from cortex_runtime.orchestration.state import event
from test_runtime_task_machine import machine, plan
from test_engine_session_manager import _make_manager, REQUESTED_CAPABILITIES
from engine_fakes import default_scenario

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('timeline', ROOT / 'scripts/cortex_timeline.py')
timeline = importlib.util.module_from_spec(spec)
spec.loader.exec_module(timeline)


def rows(state, kind):
    with state.store.connection() as db:
        return [json.loads(r[0]) for r in db.execute(
            'SELECT payload_json FROM runtime_events WHERE type=? ORDER BY id', (kind,))]


def bind(state, ch, assignment, attempt='xa_measure'):
    from cortex_runtime.trusted_channel import issue_channel_tx
    with state.store.connection() as db:
        epoch = db.execute('SELECT epoch FROM task_owners WHERE task_id=?', (ch.task_id,)).fetchone()[0]
        parent, _ = issue_channel_tx(db, ch.task_id, 'coordinator', epoch)
        worker, _ = issue_channel_tx(db, ch.task_id, 'worker', epoch, parent_channel_id=parent,
                                    assignment_id='a_measure', attempt_id=attempt)
        stamp = state.store._now()
        db.execute('INSERT INTO delegation_provisioning VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)',
                   (assignment, ch.task_id, 'a_measure', attempt, 'worker-backend-dev', 'ready', 'ready',
                    'shared_project_root', worker, None, None, None, 'qualified', None, epoch, stamp, stamp))
        db.commit()


def test_real_engine_updates_emit_one_metadata_milestone_per_attempt(machine, tmp_path_factory):
    state, ch = machine
    state.submit_plan(ch, plan(), 'plan')
    state.move(ch.task_id, 'EXECUTING')
    assignment = state.claim_ready(ch.task_id, ch.channel_id)[0]['assignment_ref']
    bind(state, ch, assignment)
    manager = _make_manager(tmp_path_factory.mktemp('engine-timing'), default_scenario())
    manager._lifecycle_observer = lambda kind, session: attempt_event(
        state.store, ch.task_id, 'xa_measure', kind, session)
    try:
        manager.negotiate(REQUESTED_CAPABILITIES)
        session = manager.create_session(ch.task_id, 'route')
        assert len(rows(state, 'ENGINE_READY')) == 1
        assert rows(state, 'FIRST_MODEL_OUTPUT') == []
        list(manager.prompt_turn(session, 'private content'))
        list(manager.prompt_turn(session, 'private again'))
        # A replacement session in the same attempt must not duplicate milestones.
        manager.create_session(ch.task_id, 'route')
    finally:
        manager.shutdown(grace=2)
    for kind in ('ENGINE_READY', 'FIRST_MODEL_OUTPUT'):
        observed = rows(state, kind)
        assert len(observed) == 1
        assert observed[0] == dict(assignment_ref=assignment, attempt_id='xa_measure', engine_session_id=session)


def test_unknown_attempt_does_not_invent_milestones(machine):
    state, ch = machine
    attempt_event(state.store, ch.task_id, 'missing', 'ENGINE_READY', 'es_missing')
    assert not rows(state, 'ENGINE_READY')


def test_wait_reasons_change_only_and_clear(machine):
    state, ch = machine
    proposed = plan(6)
    proposed['tasks'][1]['dependencies'] = ['N0']
    state.submit_plan(ch, proposed, 'plan')
    state.move(ch.task_id, 'EXECUTING')
    first = state.claim_ready(ch.task_id, ch.channel_id)
    assert len(first) == 4
    reasons = rows(state, 'SCHEDULER_WAIT_REASON')
    assert {r['reason'] for r in reasons} == {'dependency', 'capacity_limit'}
    state.claim_ready(ch.task_id, ch.channel_id)
    assert rows(state, 'SCHEDULER_WAIT_REASON') == reasons
    with state.store.connection() as db:
        db.execute("UPDATE runtime_nodes SET status='completed' WHERE task_id=? AND node_id='N0'", (ch.task_id,))
        event(db, ch.task_id, 'WORKER_RELEASED', dict(assignment_ref=first[0]['assignment_ref']))
        db.commit()
    state.claim_ready(ch.task_id, ch.channel_id)
    assert any(r['node'] == 'N1' and r['reason'] is None for r in rows(state, 'SCHEDULER_WAIT_REASON'))


def test_scope_wait_and_route_capacity_are_not_flooded(machine):
    state, ch = machine
    proposed = plan(2)
    proposed['tasks'][1]['scope'] = proposed['tasks'][0]['scope']
    state.submit_plan(ch, proposed, 'plan')
    state.move(ch.task_id, 'EXECUTING')
    assert len(state.claim_ready(ch.task_id, ch.channel_id)) == 1
    assert rows(state, 'SCHEDULER_WAIT_REASON')[0]['reason'] == 'scope_overlap'
    state.claim_ready(ch.task_id, ch.channel_id)
    assert len(rows(state, 'SCHEDULER_WAIT_REASON')) == 1
    for _ in range(3):
        attempt_wait(state.store, ch.task_id, 'xa_wait', 'route_capacity', 'cn_wait')
    attempt_wait(state.store, ch.task_id, 'xa_wait', None, 'cn_wait')
    assert [r['reason'] for r in rows(state, 'SCHEDULER_WAIT_REASON')][1:] == ['route_capacity', None]


def snapshot_files(project):
    return {p.name: p.read_bytes() for p in (project / '.cortex').iterdir() if p.is_file()}


def test_timeline_fixture_missing_events_and_readonly(machine, tmp_path, capsys):
    state, ch = machine
    state.submit_plan(ch, plan(), 'plan')
    state.move(ch.task_id, 'EXECUTING')
    ref = state.claim_ready(ch.task_id, ch.channel_id)[0]['assignment_ref']
    before = snapshot_files(tmp_path)
    result = timeline.read_timeline(tmp_path, ch.task_id)
    assert snapshot_files(tmp_path) == before
    phases = result['phases']
    assert next(p for p in phases if p['name'] == 'coordinator_to_plan')['status'] == 'missing'
    assert next(p for p in phases if p['name'] == 'plan_to_baseline')['status'] == 'observed'
    assert next(p for p in phases if p['name'] == 'assignment_to_ready')['missing'] == ['end']
    assert next(p for p in phases if p['name'] == 'ready_to_output')['seconds'] is None
    assert timeline.main(['--project', str(tmp_path), '--task', ch.task_id, '--json']) == 0
    assert json.loads(capsys.readouterr().out)['complete'] is True
    assert timeline.main(['--project', str(tmp_path), '--task', ch.task_id]) == 0
    assert 'missing' in capsys.readouterr().out
    assert snapshot_files(tmp_path) == before


def test_timeline_does_not_create_storage_or_migrate(machine, tmp_path):
    missing = tmp_path / 'absent'
    with pytest.raises(Exception, match='invalid_project'):
        timeline.read_timeline(missing, 'task')
    assert not missing.exists()
    state, ch = machine
    path = tmp_path / '.cortex/cortex.sqlite3'
    with sqlite3.connect(path) as db:
        db.execute('PRAGMA user_version=11')
        db.execute('PRAGMA wal_checkpoint(TRUNCATE)')
    before = snapshot_files(tmp_path)
    with pytest.raises(Exception, match='legacy_storage_migration_required'):
        timeline.read_timeline(tmp_path, ch.task_id)
    assert snapshot_files(tmp_path) == before


def test_timeline_rejects_live_wal(machine, tmp_path):
    state, ch = machine
    wal = tmp_path / '.cortex/cortex.sqlite3-wal'
    wal.write_bytes(b'nonempty')
    wal.chmod(0o600)
    with pytest.raises(Exception, match='inspection_uncheckpointed_wal'):
        timeline.read_timeline(tmp_path, ch.task_id)


def sample(kind, number, **kwargs):
    return dict(id=number, type=kind, created_at=f'2026-10-05T09:00:{number:02d}+00:00',
                assignment_ref=None, attempt_id=None, node=None, revision=1, state=None, **kwargs)


def test_complete_breakdown_and_cycles():
    events = []
    def add(kind, **kw):
        r = sample(kind, len(events) + 1)
        r.update(kw)
        events.append(r)
    add('COORDINATOR_TURN_STARTED', state='PLANNING')
    add('PLAN_ACCEPTED')
    add('EXECUTION_BASELINE')
    add('ASSIGNMENT_CREATED', assignment_ref='assignment', node='review')
    add('REVIEW_STARTED', assignment_ref='assignment')
    add('ENGINE_READY', assignment_ref='assignment', attempt_id='attempt')
    add('FIRST_MODEL_OUTPUT', assignment_ref='assignment', attempt_id='attempt')
    add('TOOL_AUTHORIZED', assignment_ref='assignment', attempt_id='attempt')
    add('WORKER_COMPLETED', assignment_ref='assignment', attempt_id='attempt')
    add('WORKER_RELEASED', assignment_ref='assignment', attempt_id='attempt')
    add('TASK_STATE_CHANGED', state='VERIFYING')
    add('VERIFICATION_PASSED')
    add('TASK_STATE_CHANGED', state='RECONCILING')
    add('SYNTHESIS_SUBMITTED')
    phases = timeline.breakdown(events, {'assignment': 'review'})
    assert len(phases) == 10
    assert all(p['status'] == 'observed' for p in phases)
    assert next(p for p in phases if p['name'] == 'review')['seconds'] == 4
    events[6]['created_at'] = 'bad'
    assert any(p['status'] == 'invalid_timestamp_or_order' for p in timeline.breakdown(events))


def test_duplicate_or_cross_attempt_milestones_not_assumed():
    events = [sample('ASSIGNMENT_CREATED', 1), sample('ENGINE_READY', 2),
              sample('ENGINE_READY', 3), sample('FIRST_MODEL_OUTPUT', 4)]
    for row in events:
        row['assignment_ref'] = 'assignment'
        if row['type'] != 'ASSIGNMENT_CREATED':
            row['attempt_id'] = 'attempt'
    phases = timeline.breakdown(events)
    assert phases[0]['status'] == 'missing'


def test_real_capacity_loop_records_selected_route_wait(machine, tmp_path, monkeypatch):
    from cortex_runtime.delegation import capacity
    from cortex_runtime.delegation.live_adapters import EngineSessionManagerAdapter
    state, ch = machine
    holder = capacity.reserve(state.store, 'cn_wait', ch.task_id, 'xa_holder', 'worker', limit=1)
    original = capacity.reserve
    refused = []
    def reserve(*args, **kwargs):
        result = original(*args, **kwargs)
        if args[3] == 'xa_wait' and result is None and not refused:
            refused.append(True)
            capacity.release_confirmed(state.store, holder, 'process_exited')
        return result
    monkeypatch.setattr(capacity, 'reserve', reserve)
    adapter = EngineSessionManagerAdapter(lambda _: None, lambda _: str(tmp_path), store=state.store,
                                          capacity_queue_timeout_seconds=1)
    result = adapter._reserve_capacity_blocking('cn_wait', ch.task_id, 'xa_wait',
                                                {'assignment_id': 'a_wait'}, limit=1)
    assert result
    assert [r['reason'] for r in rows(state, 'SCHEDULER_WAIT_REASON')] == ['route_capacity', None]


def test_failed_session_new_never_emits_ready(tmp_path):
    manager = _make_manager(tmp_path, default_scenario())
    observed = []
    manager._lifecycle_observer = lambda *args: observed.append(args)
    try:
        manager.negotiate(REQUESTED_CAPABILITIES)
        original = manager._client.call
        def refuse(method, *args, **kwargs):
            if method == 'session/new':
                return {'sessionId': None}
            return original(method, *args, **kwargs)
        manager._client.call = refuse
        with pytest.raises(Exception, match='engine_invalid_session_response'):
            manager.create_session('task', 'route')
    finally:
        manager.shutdown(grace=2)
    assert observed == []
