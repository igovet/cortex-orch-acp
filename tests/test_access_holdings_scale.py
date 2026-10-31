"""Holdings equivalence, large histories and read-to-write snapshot invalidation."""
from contextlib import contextmanager
import hashlib
import json
from pathlib import Path
import sqlite3

import pytest

from cortex_runtime.delegation.capacity_store import CAPACITY_SCHEMA
from cortex_runtime.delegation.store import DELEGATION_SCHEMA
from cortex_runtime.orchestration import access
from cortex_runtime.orchestration.state import SCHEMA, event
from cortex_runtime.store import Store
from test_runtime_task_machine import machine, plan
from test_worker_wait_continuation import bound


OLD_HOLDINGS = """SELECT n.* FROM runtime_nodes n JOIN runtime_tasks t ON t.task_id=n.task_id
    WHERE (n.assignment_ref IS NOT NULL OR n.revision=t.revision) AND NOT EXISTS (
        SELECT 1 FROM runtime_events e WHERE e.task_id=n.task_id AND e.type='WORKER_RELEASED'
        AND json_extract(e.payload_json,'$.assignment_ref')=n.assignment_ref)
    AND NOT EXISTS (SELECT 1 FROM delegation_provisioning p JOIN capacity_reservations r
        ON r.attempt_id=p.attempt_id AND r.kind='worker' AND r.state='released'
        WHERE p.task_id=n.task_id AND p.idempotency_key=n.assignment_ref
        AND NOT EXISTS (SELECT 1 FROM capacity_reservations a WHERE a.attempt_id=p.attempt_id
                        AND a.state IN ('reserved','ambiguous')))
    AND ((n.status='running' AND t.state IN ('EXECUTING','REVIEWING'))
        OR EXISTS (SELECT 1 FROM delegation_provisioning p WHERE p.task_id=n.task_id
                   AND p.idempotency_key=n.assignment_ref))"""


def test_large_history_holdings_equivalence_and_indexed_decorrelated_plan():
    with sqlite3.connect(':memory:') as db:
        db.row_factory = sqlite3.Row
        db.executescript(SCHEMA + DELEGATION_SCHEMA + CAPACITY_SCHEMA)
        db.executemany(
            "INSERT INTO runtime_tasks(task_id,state,revision,goal,created_at,updated_at) VALUES(?,?,1,'g','s','s')",
            [(f't{i}', 'EXECUTING' if i < 5 else 'COMPLETED') for i in range(60)])
        nodes, provisions, reservations, releases = [], [], [], []
        for i in range(2500):
            task = 't0' if i < 1000 else f't{1 + i % 59}'
            ref = None if i % 30 == 0 else f'ref{i}'
            nodes.append((task, 0 if i % 11 == 0 else 1, f'n{i}', 'worker-backend-dev',
                          'work', '[]', '["src"]', 'execution',
                          'completed' if i % 3 == 0 and ref is not None else 'running', ref))
            if ref is not None:
                provisions.append((ref, task, f'a{i}'))
                reservations.append((f'r{i}', task, ref, f'a{i}',
                                     'reserved' if i % 7 == 0 else 'released'))
                # A released reservation must not defeat another live or
                # ambiguous reservation of the same attempt.
                if i % 13 == 0:
                    reservations.append((f'live{i}', task, ref, f'a{i}', 'ambiguous'))
                if i % 5 != 0:
                    releases.append((task, 'WORKER_RELEASED', json.dumps(dict(assignment_ref=ref))))
        db.executemany("""INSERT INTO runtime_nodes(task_id,revision,node_id,role,objective,
            dependencies_json,scope_json,stage,status,assignment_ref) VALUES(?,?,?,?,?,?,?,?,?,?)""", nodes)
        db.executemany("""INSERT INTO delegation_provisioning(idempotency_key,task_id,attempt_id,
            role_id,step,status,owner_epoch,created_at,updated_at) VALUES(?,?,?,'writer','ready','ready',1,'s','s')""",
                       provisions)
        db.executemany("""INSERT INTO capacity_reservations(reservation_id,task_id,assignment_ref,
            attempt_id,state,connection_id,kind,reserved_at,updated_at) VALUES(?,?,?,?,?,'c','worker','s','s')""",
                       reservations)
        # Missing/null release refs, duplicates and a ref in the wrong task
        # must never poison NOT IN or release a different task's holder.
        releases.extend([('t0', 'WORKER_RELEASED', '{}'),
                         ('t0', 'WORKER_RELEASED', '{"assignment_ref":null}'),
                         ('t59', 'WORKER_RELEASED', '{"assignment_ref":"ref35"}'),
                         releases[0]])
        db.executemany("""INSERT INTO runtime_events(task_id,type,payload_json,actor,created_at)
            VALUES(?,?,?,'runtime','s')""", releases)
        filler = '{"padding":"' + 'x' * 100 + '"}'
        db.executemany("""INSERT INTO runtime_events(task_id,type,payload_json,actor,created_at)
            VALUES(?,'TOOL_FINISHED',?,'runtime','s')""",
                       (('t0' if i < 48000 else f't{i % 60}', filler)
                        for i in range(250000 - len(releases))))
        assert db.execute('SELECT COUNT(*) FROM runtime_nodes').fetchone()[0] == 2500
        assert db.execute('SELECT COUNT(*) FROM runtime_events').fetchone()[0] == 250000
        statements = []
        db.set_trace_callback(statements.append)
        actual = access.holdings(db)
        db.set_trace_callback(None)
        expected = db.execute(OLD_HOLDINGS).fetchall()
        assert {tuple(row) for row in actual} == {tuple(row) for row in expected}
        assert actual and any(row['assignment_ref'] is None for row in actual)
        assert any(row['assignment_ref'] == 'ref35' for row in actual)
        query_plan = [row['detail'] for row in db.execute('EXPLAIN QUERY PLAN ' + statements[-1])]
        assert any('LIST SUBQUERY' in detail for detail in query_plan), query_plan
        assert any('runtime_events_type' in detail for detail in query_plan), query_plan
        assert any('capacity_by_attempt' in detail for detail in query_plan), query_plan
        assert not any(detail.startswith(('SCAN runtime_events', 'SCAN e', 'SCAN r', 'SCAN a'))
                       for detail in query_plan), query_plan


def test_query_indexes_are_additive_idempotent_and_initializing_only(tmp_path):
    store = Store(tmp_path / '.cortex', project_root=tmp_path)
    names = ('runtime_events_type', 'capacity_by_attempt')
    with store.connection() as db:
        for name in names:
            db.execute(f'DROP INDEX {name}')
        assert db.execute('PRAGMA user_version').fetchone()[0] == 12
    before = hashlib.sha256(store.path.read_bytes()).hexdigest()
    Store(store.directory, initialize=False, project_root=tmp_path)
    assert hashlib.sha256(store.path.read_bytes()).hexdigest() == before
    Store(store.directory, initialize=False, runtime_access=True, project_root=tmp_path)
    with store.connection() as db:
        assert not db.execute("SELECT name FROM sqlite_master WHERE name IN (?,?)", names).fetchall()
    for _ in range(2):
        Store(store.directory, project_root=tmp_path)
        with store.connection() as db:
            assert db.execute('PRAGMA user_version').fetchone()[0] == 12
            assert [r['name'] for r in db.execute('PRAGMA index_info(runtime_events_type)')] == [
                'type', 'task_id', 'id']
            assert [r['name'] for r in db.execute('PRAGMA index_info(capacity_by_attempt)')] == ['attempt_id']


def _observe_lock_boundary(monkeypatch, store, *, before_lock=None, after_scopes=None):
    """Inject real foreign commits at deterministic read/write boundaries."""
    connection = store.connection
    original_scopes = access.scope_holdings
    observations = []
    fired = False

    class ObservedConnection:
        def __init__(self, db):
            self.db = db
            self.write_locked = False

        def execute(self, sql, *args):
            nonlocal fired
            if sql == 'BEGIN IMMEDIATE':
                # The read snapshot must have ended, not be upgraded in WAL.
                assert not self.db.in_transaction
                if before_lock is not None and not fired:
                    fired = True
                    before_lock(connection)
            result = self.db.execute(sql, *args)
            if sql == 'BEGIN IMMEDIATE':
                self.write_locked = True
            return result

        def commit(self):
            self.db.commit()
            self.write_locked = False

        def __getattr__(self, name):
            return getattr(self.db, name)

    @contextmanager
    def observed_connection(*args, **kwargs):
        with connection(*args, **kwargs) as db:
            yield ObservedConnection(db)

    def observed_scopes(db):
        nonlocal fired
        observations.append(db.write_locked)
        result = original_scopes(db)
        if after_scopes is not None and not fired:
            fired = True
            after_scopes(connection)
        return result

    monkeypatch.setattr(store, 'connection', observed_connection)
    monkeypatch.setattr(access, 'scope_holdings', observed_scopes)
    return observations


def _request(store, worker, path):
    return access.request(store, worker, dict(paths=[path], reason='required', request_key='snapshot'))


def test_access_reuses_scopes_outside_write_lock(bound, monkeypatch):
    store, _, _, workers = bound
    observations = _observe_lock_boundary(monkeypatch, store)
    receipt = _request(store, workers[0], 'part1.txt')
    assert receipt['reason'] == 'active_writer_conflict'
    assert observations == [False]


@pytest.mark.parametrize('boundary', ['before_lock', 'after_scopes'])
@pytest.mark.parametrize('change', ['release', 'grant'])
def test_access_foreign_commit_invalidates_cached_scopes(bound, monkeypatch, boundary, change):
    store, _, coordinator, workers = bound
    path = 'part1.txt' if change == 'release' else 'new.py'

    def mutate(connection):
        with connection() as db:
            ref = db.execute('SELECT idempotency_key FROM delegation_provisioning WHERE attempt_id=?',
                             (workers[1].attempt_id,)).fetchone()[0]
            payload = dict(assignment_ref=ref)
            if change == 'grant':
                payload['added_paths'] = [path]
            event(db, coordinator.task_id, 'WORKER_RELEASED' if change == 'release' else 'ACCESS_GRANTED', payload)
            db.commit()

    observations = _observe_lock_boundary(monkeypatch, store, **{boundary: mutate})
    receipt = _request(store, workers[0], path)
    if change == 'release':
        assert receipt['status'] == 'granted'
    else:
        assert receipt['reason'] == 'active_writer_conflict'
    assert observations == [False, True]


def test_access_rechecks_filesystem_even_when_database_is_unchanged(bound, monkeypatch):
    store, _, _, workers = bound

    def mutate(_):
        (Path(store.project_root) / 'link').symlink_to(store.project_root, target_is_directory=True)

    _observe_lock_boundary(monkeypatch, store, before_lock=mutate)
    assert _request(store, workers[0], 'link/file.py')['reason'] == 'unsafe_path'


@pytest.mark.parametrize('boundary', ['before_lock', 'after_scopes'])
@pytest.mark.parametrize('foreign_commit', [False, True])
def test_scheduler_reselects_with_validated_scope_snapshot(machine, monkeypatch, foreign_commit, boundary):
    state, channel = machine
    proposed = plan(2)
    for node in proposed['tasks']:
        node['scope'] = ['src/**']
    state.submit_plan(channel, proposed, 'p')
    state.move(channel.task_id, 'EXECUTING')

    def mutate(connection):
        with connection() as db:
            db.execute("UPDATE runtime_nodes SET status='running' WHERE task_id=? AND node_id='N1'",
                       (channel.task_id,))
            db.commit()

    observations = _observe_lock_boundary(
        monkeypatch, state.store, **{boundary: mutate} if foreign_commit else {})
    selected = state.claim_ready(channel.task_id, channel.channel_id)
    assert [n['node_id'] for n in selected] == ([] if foreign_commit else ['N0'])
    assert observations == ([False, True] if foreign_commit else [False])


def test_scheduler_rechecks_time_dependent_readiness_without_a_database_commit(machine, monkeypatch):
    state, channel = machine
    state.submit_plan(channel, plan(), 'p')
    state.move(channel.task_id, 'EXECUTING')
    backoff = {}

    def mutate(_):
        backoff['N0'] = dict(until='later')

    monkeypatch.setattr(type(state), '_retry_backoff', staticmethod(lambda *args: backoff.copy()))
    observations = _observe_lock_boundary(monkeypatch, state.store, before_lock=mutate)
    assert state.claim_ready(channel.task_id, channel.channel_id) == []
    assert observations == [False]
