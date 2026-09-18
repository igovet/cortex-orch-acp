"""One active mutating coordinator owner per task, with fencing/epoch transfer."""
from concurrent.futures import ThreadPoolExecutor
import multiprocessing
import uuid

import pytest

from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.contracts import StoreError
from cortex_runtime.ownership import acquire_task_owner, current_owner_epoch
from cortex_runtime.portable.identity import OwnershipEpoch, SourceEvent
from cortex_runtime.store import Store
from cortex_runtime.trusted_channel import issue_channel, resolve_channel


def bootstrap(tmp_path, owner='owner-1'):
    store = Store(tmp_path/'.cortex', project_root=tmp_path)
    result = bootstrap_task(store, SourceEvent(origin='cli', event_id=str(uuid.uuid4())), 'Text.', owner)
    return store, result


def test_first_acquisition_starts_at_epoch_one_and_transfer_bumps_it(tmp_path):
    store, result = bootstrap(tmp_path)
    with store.connection() as db:
        assert current_owner_epoch(db, result.task_id) == 1
    transferred = acquire_task_owner(store, result.task_id, 'owner-2', 1)
    assert isinstance(transferred, OwnershipEpoch) and transferred.epoch == 2
    with store.connection() as db:
        assert current_owner_epoch(db, result.task_id) == 2


def test_stale_expected_epoch_is_rejected(tmp_path):
    store, result = bootstrap(tmp_path)
    with pytest.raises(StoreError):
        acquire_task_owner(store, result.task_id, 'owner-2', 99)


def test_concurrent_thread_acquisition_has_exactly_one_epoch_one_winner(tmp_path):
    store = Store(tmp_path/'.cortex', project_root=tmp_path)
    ev = SourceEvent(origin='cli', event_id=str(uuid.uuid4()))
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        db.execute("INSERT INTO tasks VALUES (?,?,?)", ('t_race0000001', str(tmp_path), store._now()))
        db.commit()
    task_id = 't_race0000001'

    def attempt(owner):
        try:
            return acquire_task_owner(store, task_id, owner, None)
        except StoreError:
            return None

    with ThreadPoolExecutor(max_workers=8) as pool:
        results = list(pool.map(attempt, [f'owner-{i}' for i in range(8)]))
    winners = [r for r in results if r is not None]
    assert len(winners) == 1
    assert winners[0].epoch == 1


def _process_attempt(db_path, project_root, task_id, owner, queue):
    from cortex_runtime.store import Store as _Store
    from cortex_runtime.ownership import acquire_task_owner as _acquire
    from cortex_runtime.contracts import StoreError as _Error
    store = _Store(db_path, project_root=project_root)
    try:
        result = _acquire(store, task_id, owner, None)
        queue.put(result.epoch)
    except _Error:
        queue.put(None)


def test_concurrent_process_acquisition_has_exactly_one_winner(tmp_path):
    store = Store(tmp_path/'.cortex', project_root=tmp_path)
    task_id = 't_race0000002'
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        db.execute("INSERT INTO tasks VALUES (?,?,?)", (task_id, str(tmp_path), store._now()))
        db.commit()
    ctx = multiprocessing.get_context('spawn')
    queue = ctx.Queue()
    procs = [ctx.Process(target=_process_attempt,
                          args=(tmp_path/'.cortex', str(tmp_path), task_id, f'p-owner-{i}', queue))
             for i in range(4)]
    for p in procs: p.start()
    for p in procs: p.join(timeout=30)
    outcomes = [queue.get(timeout=5) for _ in procs]
    winners = [o for o in outcomes if o is not None]
    assert len(winners) == 1 and winners[0] == 1


def test_delayed_mutation_from_stale_owner_channel_is_rejected_and_writes_nothing(tmp_path):
    from cortex_runtime.server import Server

    store, result = bootstrap(tmp_path)
    stale_channel_id, stale_secret = result.channel_id, result.secret
    acquire_task_owner(store, result.task_id, 'owner-2', 1)
    with store.connection() as db:
        assert current_owner_epoch(db, result.task_id) == 2
        before = db.execute('SELECT COUNT(*) FROM drafts').fetchone()[0]
    server = Server(channel_credentials=(stale_channel_id, stale_secret, str(tmp_path)))
    reply = server.dispatch('tools/call', dict(
        name='create_draft', arguments=dict(template='general', request_key='stale-attempt')))
    assert reply['isError']
    import json
    body = json.loads(reply['content'][0]['text'])
    assert body['error'] == 'stale_ownership_epoch'
    assert body['write_state'] == 'rejected'
    with store.connection() as db:
        after = db.execute('SELECT COUNT(*) FROM drafts').fetchone()[0]
    assert before == after


def test_observer_reads_succeed_without_owning(tmp_path):
    from cortex_runtime.server import Server

    store, result = bootstrap(tmp_path)
    acquire_task_owner(store, result.task_id, 'owner-2', 1)
    server = Server(channel_credentials=(result.channel_id, result.secret, str(tmp_path)))
    reply = server.dispatch('tools/call', dict(name='list_reports', arguments={}))
    assert not reply['isError'], reply


def test_stale_owner_channel_is_rejected_on_a_delegation_operation(tmp_path):
    """SEC-13 (T030 fix): a coordinator channel whose owner epoch has been
    superseded (recovery bumped ownership before revoking the old channel --
    see server.py's DELEGATION operations comment) must be refused on a
    delegation mutating operation (``delegate``), not only on the
    MUTATING_TOOLS path exercised by
    test_delayed_mutation_from_stale_owner_channel_is_rejected_and_writes_nothing."""
    from cortex_runtime.server import Server

    store, result = bootstrap(tmp_path)
    stale_channel_id, stale_secret = result.channel_id, result.secret
    acquire_task_owner(store, result.task_id, 'owner-2', 1)
    with store.connection() as db:
        before = db.execute('SELECT COUNT(*) FROM task_events').fetchone()[0]
    server = Server(channel_credentials=(stale_channel_id, stale_secret, str(tmp_path)))
    reply = server.dispatch('tools/call', dict(
        name='delegate', arguments=dict(
            title='T', brief='B', role='backend-dev', acceptance='A', request_key='stale-delegate')))
    import json
    assert reply['isError'], reply
    body = json.loads(reply['content'][0]['text'])
    assert body['error'] == 'stale_ownership_epoch'
    assert body['write_state'] == 'rejected'
    with store.connection() as db:
        after = db.execute('SELECT COUNT(*) FROM task_events').fetchone()[0]
    assert before == after


def test_model_supplied_binding_arguments_cannot_forge_identity(tmp_path):
    """SEC-13: a delegation-operation caller cannot override its own
    channel-derived identity (task_id/channel_id/workspace_binding_id/etc.)
    by supplying those as tool arguments -- the runtime bridge derives
    identity only from the resolved trusted channel, never from caller
    arguments, and the schema (``additionalProperties: False``) refuses any
    such forged binding argument outright."""
    from cortex_runtime.server import Server

    store, result = bootstrap(tmp_path)
    server = Server(channel_credentials=(result.channel_id, result.secret, str(tmp_path)))
    forged_args = dict(
        title='T', brief='B', role='backend-dev', acceptance='A', request_key='forge1',
        task_id='t_' + '0' * 12, channel_id='ch_' + '0' * 12, workspace_binding_id='wb_forged',
    )
    reply = server.dispatch('tools/call', dict(name='delegate', arguments=forged_args))
    import json
    assert reply['isError'], reply
    body = json.loads(reply['content'][0]['text'])
    assert body['error'] == 'invalid_arguments', body
