"""No binding secret ever reaches the model, an observation log, or disk bytes."""
import json
import uuid

from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.contracts import TOOLS
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.server import Server
from cortex_runtime.store import Store


def test_generated_secret_never_appears_in_the_advertised_tool_catalogue():
    # The catalogue never advertises a binding secret/credential as a tool argument.
    for tool in TOOLS:
        assert not {'secret', 'channel_secret', 'credential'} & set(tool['inputSchema']['properties'])


def test_generated_secret_never_appears_in_responses_observation_or_db_bytes(tmp_path, monkeypatch):
    observation_dir = tmp_path/'observations'
    monkeypatch.setenv('CORTEX_OBSERVATION_DIR', str(observation_dir))
    store = Store(tmp_path/'.cortex', project_root=tmp_path)
    result = bootstrap_task(store, SourceEvent(origin='cli', event_id=str(uuid.uuid4())), 'Build a page.', 'owner-1')
    from cortex_runtime.orchestration.state import TaskMachine
    TaskMachine(store).create(result.task_id,"Build a page.")
    secret = result.secret
    assert secret

    server = Server(channel_credentials=(result.channel_id, secret, str(tmp_path)))
    draft = server.dispatch('tools/call', dict(
        name='create_draft', arguments=dict(template='general', request_key='k1')))
    from pathlib import Path
    path = Path(draft['structuredContent']['draft_path'])
    marker = path.read_text().split('\n\n', 1)[0]+'\n\n'
    path.write_text(marker+'Evidence body.')
    published = server.dispatch('tools/call', dict(
        name='write_report', arguments=dict(title='Evidence', summary='Verified.', author='worker',
                                             draft_id=draft['structuredContent']['draft_id'], request_key='pub1')))
    listed = server.dispatch('tools/call', dict(name='list_reports', arguments={}))
    forged = server.dispatch('tools/call', dict(
        name='list_reports', arguments={}, _meta=None))

    for reply in (draft, published, listed, forged):
        assert secret not in json.dumps(reply)

    # Wrong-secret rejection must also never echo the real secret.
    bad_server = Server(channel_credentials=(result.channel_id, 'wrong-secret', str(tmp_path)))
    rejected = bad_server.dispatch('tools/call', dict(name='list_reports', arguments={}))
    assert rejected['isError']
    assert secret not in json.dumps(rejected)

    if observation_dir.exists():
        for entry in observation_dir.iterdir():
            assert secret not in entry.read_text()

    db_bytes = (tmp_path/'.cortex'/'cortex.sqlite3').read_bytes()
    assert secret.encode() not in db_bytes
    # Only the sha256 hash of the secret may be present.
    import hashlib
    digest = hashlib.sha256(secret.encode()).hexdigest()
    assert digest.encode() in db_bytes
