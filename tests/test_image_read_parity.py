"""project_read/read_source behave like Claude's native Read for images.

The owner's explicit decision was: no new MCP tool; the existing read tools
(``project_read``, ``read_source``) must themselves return images the way
Claude's native ``Read`` does -- a real MCP ``image`` content block -- instead
of either a useless ``binary: true`` marker (``project_read`` on a PNG) or a
giant base64 text block that overflows an engine's tool-result token limit
(``read_source`` on a stored image attachment; see report r_5eb34035eceb).
"""
import base64
import hashlib
import json
import uuid

from cortex_runtime import server
from cortex_runtime.acp import replay_store
from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.orchestration import gateway
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.portable.roles import resolve_role
from cortex_runtime.store import Store
from cortex_runtime.trusted_channel import issue_channel_tx, resolve_channel
from runtime_scenarios import reserve_assignment

PNG_BYTES = b"\x89PNG\r\n\x1a\n\x00opaque-png-bytes"


def _worker(tmp_path):
    """A running worker assignment and its channel credentials."""
    project = tmp_path / 'project'
    project.mkdir()
    store = Store(project / '.cortex', project_root=project)
    boot = bootstrap_task(store, SourceEvent(origin='image-read', event_id='one'), 'Check', 'owner')
    coordinator = resolve_channel(store, boot.channel_id, boot.secret)
    reserved = reserve_assignment(store, coordinator, dict(title='w', brief='w', role='backend-dev',
                                                           acceptance='Exact report', request_key='w'))
    assignment, attempt = 'a_' + uuid.uuid4().hex[:12], 'xa_' + uuid.uuid4().hex[:12]
    with store.connection() as db:
        db.execute('BEGIN IMMEDIATE')
        channel, secret = issue_channel_tx(db, coordinator.task_id, 'worker', coordinator.owner_epoch,
                                           parent_channel_id=coordinator.channel_id,
                                           assignment_id=assignment, attempt_id=attempt)
        stamp = store._now()
        db.execute('INSERT INTO delegation_provisioning VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)',
                   (reserved['assignment_ref'], coordinator.task_id, assignment, attempt,
                    resolve_role('backend-dev'), 'ready', 'ready', 'shared_project_root', channel, None, None, None,
                    'qualified', None, coordinator.owner_epoch, stamp, stamp))
        db.commit()
    return project, store, channel, secret


def test_project_read_of_supported_image_returns_mcp_image_block(tmp_path):
    project, store, channel, secret = _worker(tmp_path)
    (project / 'shot.png').write_bytes(PNG_BYTES)
    reply = server.Server(channel_credentials=(channel, secret, str(project))).dispatch(
        'tools/call', dict(name='project_read', arguments=dict(path='shot.png')))
    assert not reply['isError']
    image_block, text_block = reply['content']
    assert image_block['type'] == 'image' and image_block['mimeType'] == 'image/png'
    assert base64.b64decode(image_block['data']) == PNG_BYTES
    metadata = json.loads(text_block['text'])
    assert metadata == dict(mime='image/png', sha256=hashlib.sha256(PNG_BYTES).hexdigest(),
                            size_bytes=len(PNG_BYTES), path='shot.png')
    assert reply['structuredContent'] == metadata


def test_project_read_detects_image_by_content_not_extension(tmp_path):
    project, store, channel, secret = _worker(tmp_path)
    (project / 'shot.bin').write_bytes(PNG_BYTES)  # extensionless/renamed screenshot
    reply = server.Server(channel_credentials=(channel, secret, str(project))).dispatch(
        'tools/call', dict(name='project_read', arguments=dict(path='shot.bin')))
    assert not reply['isError']
    image_block = reply['content'][0]
    assert image_block['type'] == 'image' and image_block['mimeType'] == 'image/png'


def test_project_read_oversized_image_is_refused_not_silently_truncated(tmp_path, monkeypatch):
    project, store, channel, secret = _worker(tmp_path)
    monkeypatch.setattr(gateway, 'MAX_PROJECT_READ_IMAGE_BYTES', 32)
    (project / 'huge.png').write_bytes(PNG_BYTES + b'x' * 64)
    reply = server.Server(channel_credentials=(channel, secret, str(project))).dispatch(
        'tools/call', dict(name='project_read', arguments=dict(path='huge.png')))
    assert reply['isError']
    error = json.loads(reply['content'][0]['text'])
    assert error['error'] == 'gateway_image_too_large'


def test_project_read_of_text_file_stays_a_single_text_block(tmp_path):
    project, store, channel, secret = _worker(tmp_path)
    (project / 'note.txt').write_text('hello', encoding='utf-8')
    reply = server.Server(channel_credentials=(channel, secret, str(project))).dispatch(
        'tools/call', dict(name='project_read', arguments=dict(path='note.txt',line_numbers=False)))
    assert not reply['isError']
    assert len(reply['content']) == 1 and reply['content'][0]['type'] == 'text'
    assert json.loads(reply['content'][0]['text'])['content'] == 'hello'


def test_project_read_of_opaque_binary_keeps_the_old_marker(tmp_path):
    project, store, channel, secret = _worker(tmp_path)
    (project / 'data.bin').write_bytes(b'\x00\xff\x80not-an-image')
    reply = server.Server(channel_credentials=(channel, secret, str(project))).dispatch(
        'tools/call', dict(name='project_read', arguments=dict(path='data.bin')))
    assert not reply['isError']
    assert len(reply['content']) == 1 and reply['content'][0]['type'] == 'text'
    assert json.loads(reply['content'][0]['text'])['binary'] is True


def _image_attachment_task(tmp_path, image_bytes=PNG_BYTES, mime='image/png'):
    store = Store(tmp_path / '.cortex', project_root=tmp_path)
    blocks = [dict(type='image', data=base64.b64encode(image_bytes).decode('ascii'), mimeType=mime)]
    resolved = replay_store.resolve_prompt_blocks(blocks, str(tmp_path))
    staged = replay_store.stage_blocks(str(tmp_path), resolved)
    boot = bootstrap_task(store, SourceEvent(origin='image-source', event_id=str(uuid.uuid4())),
                          'hello', 'owner-1', blocks=staged)
    return store, boot


def test_read_source_of_stored_image_attachment_returns_mcp_image_block(tmp_path):
    store, boot = _image_attachment_task(tmp_path)
    reply = server.Server(channel_credentials=(boot.channel_id, boot.secret, str(tmp_path))).dispatch(
        'tools/call', dict(name='read_source', arguments=dict(source_ref=f'{boot.report_id}:0')))
    assert not reply['isError']
    image_block, text_block = reply['content']
    assert image_block['type'] == 'image' and image_block['mimeType'] == 'image/png'
    assert base64.b64decode(image_block['data']) == PNG_BYTES
    metadata = json.loads(text_block['text'])
    assert metadata['kind'] == 'image' and metadata['mime'] == 'image/png'
    assert metadata['sha256'] == hashlib.sha256(PNG_BYTES).hexdigest()
    assert metadata['total_size'] == len(PNG_BYTES)
    assert metadata['availability'] == 'stored'
    assert 'chunk' not in metadata and 'image_base64' not in metadata
    assert reply['structuredContent'] == metadata


def test_read_source_whole_image_ignores_a_small_limit(tmp_path):
    """A partial image is useless to render: the whole body comes back even
    when the caller passes a tiny limit (unlike a text/char-paged read)."""
    store, boot = _image_attachment_task(tmp_path)
    reply = server.Server(channel_credentials=(boot.channel_id, boot.secret, str(tmp_path))).dispatch(
        'tools/call', dict(name='read_source', arguments=dict(source_ref=f'{boot.report_id}:0', limit=4)))
    assert not reply['isError']
    image_block = reply['content'][0]
    assert base64.b64decode(image_block['data']) == PNG_BYTES


def test_read_source_of_text_attachment_stays_a_single_text_block(tmp_path):
    (tmp_path / 'a.txt').write_text('body text', encoding='utf-8')
    store = Store(tmp_path / '.cortex', project_root=tmp_path)
    blocks = [dict(type='resource_link', name='a.txt', uri=f'file://{tmp_path}/a.txt')]
    resolved = replay_store.resolve_prompt_blocks(blocks, str(tmp_path))
    staged = replay_store.stage_blocks(str(tmp_path), resolved)
    boot = bootstrap_task(store, SourceEvent(origin='text-source', event_id='one'), 'hello', 'owner-1', blocks=staged)
    reply = server.Server(channel_credentials=(boot.channel_id, boot.secret, str(tmp_path))).dispatch(
        'tools/call', dict(name='read_source', arguments=dict(source_ref=f'{boot.report_id}:0')))
    assert not reply['isError']
    assert len(reply['content']) == 1 and reply['content'][0]['type'] == 'text'
    assert json.loads(reply['content'][0]['text'])['chunk'] == 'body text'
