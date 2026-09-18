"""Portable trusted-channel identity: no Codex metadata, no impersonation."""
import json
import uuid

import pytest

from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.contracts import StoreError
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.server import Server
from cortex_runtime.store import Store
from cortex_runtime.trusted_channel import issue_channel, resolve_channel, revoke_channel


def bootstrap(tmp_path, owner='owner-1', text='Build a page.'):
    store = Store(tmp_path/'.cortex', project_root=tmp_path)
    result = bootstrap_task(store, SourceEvent(origin='cli', event_id=str(uuid.uuid4())), text, owner)
    return store, result


def server_for(tmp_path, channel_id, secret):
    return Server(channel_credentials=(channel_id, secret, str(tmp_path)))


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


def test_unchanneled_server_rejects_every_call(tmp_path):
    """T033: the Codex-plugin-harness-era unchanneled ``_meta`` identity path
    was removed. A server started without channel credentials must reject
    every tool call, regardless of any ``_meta`` supplied."""
    store, result = bootstrap(tmp_path)
    unchanneled = Server()
    error(call(unchanneled, 'list_reports'), 'channel_required')
    thread = str(uuid.uuid4())
    meta = {'threadId': thread, 'x-codex-turn-metadata': {}}
    error(call(unchanneled, 'list_reports', meta=meta), 'channel_required')
    error(call(unchanneled, 'create_task', dict(project_root=str(tmp_path), request_key='k'), meta=meta),
          'channel_required')


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
