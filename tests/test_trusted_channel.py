"""Portable trusted-channel identity: no Codex metadata, no impersonation."""
import json
import uuid

import pytest

from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.contracts import StoreError
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.project_storage import ProjectResolver
from cortex_runtime.server import Server
from cortex_runtime.store import Store
from cortex_runtime.trusted_channel import issue_channel, resolve_channel, revoke_channel


def bootstrap(tmp_path, owner='owner-1', text='Build a page.'):
    store = Store(tmp_path/'.cortex', project_root=tmp_path)
    result = bootstrap_task(store, SourceEvent(origin='cli', event_id=str(uuid.uuid4())), text, owner)
    return store, result


def server_for(tmp_path, channel_id, secret):
    return Server(project_resolver=ProjectResolver(lambda *_: str(tmp_path)),
                  channel_credentials=(channel_id, secret, str(tmp_path)))


def call(server, name, arguments=None, meta=None):
    return server.dispatch('tools/call', dict(name=name, arguments=arguments or {}, _meta=meta))


def ok(reply):
    assert not reply['isError'], reply
    return reply['structuredContent']


def error(reply, code):
    assert reply['isError'], reply
    body = json.loads(reply['content'][0]['text'])
    assert body['error'] == code, body
    return body


def write_a_report(server):
    draft = ok(call(server, 'create_draft', dict(template='general', request_key='k1')))
    from pathlib import Path
    path = Path(draft['draft_path'])
    marker = path.read_text().split('\n\n', 1)[0]+'\n\n'
    path.write_text(marker+'Evidence body.')
    return ok(call(server, 'write_report', dict(
        title='Evidence', summary='Verified.', author='worker',
        draft_id=draft['draft_id'], request_key='pub1')))


def test_portable_engine_fixture_can_call_the_runtime_without_codex_metadata(tmp_path):
    store, result = bootstrap(tmp_path)
    server = server_for(tmp_path, result.channel_id, result.secret)
    published = write_a_report(server)
    assert published['report_id'].startswith('r_')
    listed = ok(call(server, 'list_reports'))
    assert listed['reports'][0]['report_id'] == published['report_id']


def test_forged_channel_id_is_rejected(tmp_path):
    store, result = bootstrap(tmp_path)
    server = server_for(tmp_path, 'ch_'+'0'*12, result.secret)
    error(call(server, 'list_reports'), 'channel_unknown')


def test_wrong_secret_is_rejected(tmp_path):
    store, result = bootstrap(tmp_path)
    server = server_for(tmp_path, result.channel_id, 'wrong-secret-value')
    error(call(server, 'list_reports'), 'channel_unknown')


def test_worker_channel_of_task_a_cannot_read_or_write_task_b(tmp_path):
    store, result_a = bootstrap(tmp_path, owner='owner-a', text='Task A.')
    _, result_b = bootstrap(tmp_path, owner='owner-b', text='Task B.')
    server_a = server_for(tmp_path, result_a.channel_id, result_a.secret)
    published_a = write_a_report(server_a)
    server_b = server_for(tmp_path, result_b.channel_id, result_b.secret)
    # task B's coordinator cannot read task A's report by id.
    error(call(server_b, 'read_report', dict(report_id=published_a['report_id'])), 'not_found')
    listing_b = ok(call(server_b, 'list_reports'))
    assert published_a['report_id'] not in json.dumps(listing_b)


def test_expired_channel_is_rejected(tmp_path):
    store, result = bootstrap(tmp_path)
    past = '2000-01-01T00:00:00.000000+00:00'
    with store.connection() as db:
        db.execute("UPDATE trusted_channels SET expires_at=? WHERE channel_id=?", (past, result.channel_id))
    server = server_for(tmp_path, result.channel_id, result.secret)
    error(call(server, 'list_reports'), 'channel_expired')


def test_revoked_channel_is_rejected(tmp_path):
    store, result = bootstrap(tmp_path)
    revoke_channel(store, result.channel_id)
    server = server_for(tmp_path, result.channel_id, result.secret)
    error(call(server, 'list_reports'), 'channel_revoked')


def test_both_identity_kinds_present_is_ambiguous(tmp_path):
    store, result = bootstrap(tmp_path)
    server = server_for(tmp_path, result.channel_id, result.secret)
    thread = str(uuid.uuid4())
    meta = {'threadId': thread, 'x-codex-turn-metadata': {}}
    error(call(server, 'list_reports', meta=meta), 'ambiguous_identity')


def test_channel_path_rejects_task_creation(tmp_path):
    store, result = bootstrap(tmp_path)
    server = server_for(tmp_path, result.channel_id, result.secret)
    error(call(server, 'create_task', dict(project_root=str(tmp_path), request_key='k')),
          'channel_task_creation')


def test_secret_is_never_present_in_responses(tmp_path):
    store, result = bootstrap(tmp_path)
    server = server_for(tmp_path, result.channel_id, result.secret)
    reply = call(server, 'list_reports')
    assert result.secret not in json.dumps(reply)
    forged = call(server_for(tmp_path, result.channel_id, 'wrong'), 'list_reports')
    assert result.secret not in json.dumps(forged)


def _principal(store, channel_id):
    with store.connection() as db:
        return db.execute(
            'SELECT principal_thread_id FROM trusted_channels WHERE channel_id=?', (channel_id,)
        ).fetchone()[0]


def test_leaked_principal_thread_id_cannot_be_used_on_the_legacy_path(tmp_path):
    """Security regression (independent review blocker B1).

    A channel's internal principal thread id must never be usable through
    the unfenced legacy Codex ``_meta`` adapter: not for reads, not for
    mutations, and this must hold both before and after an explicit
    ownership transfer (a delayed/duplicated legacy attempt with a
    once-current owner must not succeed just because the channel epoch
    check does not apply to it).
    """
    from cortex_runtime.ownership import acquire_task_owner

    store, result = bootstrap(tmp_path)
    principal = _principal(store, result.channel_id)
    legacy_meta = {'threadId': principal, 'x-codex-turn-metadata': {}}
    legacy = Server(project_resolver=ProjectResolver(lambda *_: str(tmp_path)))

    with store.connection() as db:
        before = db.execute('SELECT COUNT(*) FROM drafts').fetchone()[0]
    error(call(legacy, 'list_reports', meta=legacy_meta), 'channel_managed_task')
    error(call(legacy, 'create_draft', dict(template='general', request_key='k-before'), meta=legacy_meta),
          'channel_managed_task')
    with store.connection() as db:
        assert db.execute('SELECT COUNT(*) FROM drafts').fetchone()[0] == before

    acquire_task_owner(store, result.task_id, 'owner-2', 1)

    error(call(legacy, 'list_reports', meta=legacy_meta), 'channel_managed_task')
    error(call(legacy, 'create_draft', dict(template='general', request_key='k-after'), meta=legacy_meta),
          'channel_managed_task')
    with store.connection() as db:
        assert db.execute('SELECT COUNT(*) FROM drafts').fetchone()[0] == before

    # The legitimate channel path is unaffected.
    server = server_for(tmp_path, result.channel_id, result.secret)
    ok(call(server, 'list_reports'))


def test_legacy_child_thread_cannot_register_under_a_leaked_channel_principal(tmp_path):
    store, result = bootstrap(tmp_path)
    principal = _principal(store, result.channel_id)
    legacy = Server(project_resolver=ProjectResolver(lambda *_: str(tmp_path)))
    child = str(uuid.uuid4())
    meta = {'threadId': child, 'x-codex-turn-metadata': {'parent_thread_id': principal}}
    error(call(legacy, 'list_reports', meta=meta), 'channel_managed_task')
    with store.connection() as db:
        assert db.execute('SELECT 1 FROM thread_bindings WHERE thread_id=?', (child,)).fetchone() is None


def test_channel_responses_never_disclose_principal_thread_ids(tmp_path):
    store, result = bootstrap(tmp_path)
    coordinator_principal = _principal(store, result.channel_id)
    server = server_for(tmp_path, result.channel_id, result.secret)
    published = write_a_report(server)
    listed = ok(call(server, 'list_reports'))
    for payload in (published, listed):
        text = json.dumps(payload)
        assert coordinator_principal not in text
    # binding.thread_id/parent_thread_id/receipt are masked, not merely absent.
    assert published['binding']['thread_id'] != coordinator_principal
    assert published['binding']['parent_thread_id'] is None


def test_legacy_created_task_is_unaffected_by_the_channel_fence(tmp_path):
    """A task created through the ordinary legacy path (no task_owners row)
    keeps its existing unrestricted legacy behavior."""
    legacy = Server(project_resolver=ProjectResolver(lambda *_: str(tmp_path)))
    thread = str(uuid.uuid4())
    meta = {'threadId': thread, 'x-codex-turn-metadata': {}}

    def legacy_call(name, arguments):
        return legacy.dispatch('tools/call', dict(name=name, arguments=arguments, _meta=meta))

    original_source = legacy.request_source
    legacy.request_source = lambda *_: 'Legacy text.'
    try:
        created = ok(legacy_call('create_task', dict(project_root=str(tmp_path), request_key='legacy-k')))
    finally:
        legacy.request_source = original_source
    assert created['original_report_id']
    ok(legacy_call('list_reports', {}))
