"""Durable action identity prevents duplicate or uncertain side effects."""
import json
import pytest
from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.store import Store
from cortex_runtime.contracts import StoreError
from cortex_runtime.trusted_channel import resolve_channel
from cortex_runtime.delegation import mcp_tools
from cortex_runtime.orchestration.gateway import dispatch
from runtime_scenarios import provision_fixture


@pytest.fixture
def worker(tmp_path):
    store=Store(tmp_path/'.cortex',project_root=tmp_path)
    task=bootstrap_task(store,SourceEvent(origin='receipts',event_id='one'),'Write','owner')
    coordinator=resolve_channel(store,task.channel_id,task.secret)
    channel,ref=provision_fixture(store,coordinator)
    directory=tmp_path/'fixture'/ref;directory.mkdir(parents=True)
    return store,channel,directory


def test_mutation_replay_does_not_repeat_or_replace_content(worker):
    store,ch,directory=worker
    path=str((directory/'sentinel').relative_to(store.project_root))
    args=dict(path=path,content='one',expected_sha256='absent',request_key='write')
    receipt=dispatch(store,ch,'project_write',args)
    (directory/'sentinel').write_text('external')
    assert dispatch(store,ch,'project_write',args)==receipt
    assert (directory/'sentinel').read_text()=='external'
    with pytest.raises(StoreError,match='request_key_conflict'):dispatch(store,ch,'project_write',dict(args,content='changed'))


def test_delete_checks_exact_content_and_replays_receipt(worker,monkeypatch):
    import hashlib
    from cortex_runtime.orchestration import consent
    monkeypatch.setattr(consent,'request',lambda *a,**k: True)  # the user allowed it
    store,ch,directory=worker;target=directory/'sentinel';target.write_text('keep')
    args=dict(path=str(target.relative_to(store.project_root)),expected_sha256=hashlib.sha256(b'keep').hexdigest(),request_key='delete')
    assert dispatch(store,ch,'project_delete',args)==dict(deleted=True)
    target.write_text('new')
    assert dispatch(store,ch,'project_delete',args)==dict(deleted=True)
    assert target.read_text()=='new'


def test_delete_accepts_binary_file_with_matching_raw_hash(worker,monkeypatch):
    import hashlib
    from cortex_runtime.orchestration import consent
    monkeypatch.setattr(consent,'request',lambda *a,**k: True)
    store,ch,directory=worker
    target=directory/'binary.dat';data=b'\x00\xff\x80binary'
    target.write_bytes(data)
    args=dict(path=str(target.relative_to(store.project_root)),
              expected_sha256=hashlib.sha256(data).hexdigest(),request_key='delete-binary')
    assert dispatch(store,ch,'project_delete',args)==dict(deleted=True)
    assert not target.exists()


def test_delete_refuses_binary_file_with_wrong_raw_hash(worker,monkeypatch):
    from cortex_runtime.orchestration import consent
    monkeypatch.setattr(consent,'request',lambda *a,**k: True)
    store,ch,directory=worker
    target=directory/'binary.dat';data=b'\x00\xff\x80binary'
    target.write_bytes(data)
    args=dict(path=str(target.relative_to(store.project_root)),
              expected_sha256='0'*64,request_key='delete-binary-wrong-hash')
    with pytest.raises(StoreError,match='gateway_write_conflict'):
        dispatch(store,ch,'project_delete',args)
    assert target.read_bytes()==data


def test_project_read_keeps_utf8_text_hash_and_character_paging(worker):
    import hashlib
    store,ch,directory=worker
    target=directory/'text.txt';data='héllo'.encode('utf-8')
    target.write_bytes(data)
    result=dispatch(store,ch,'project_read',dict(path=str(target.relative_to(store.project_root)),offset=1,limit=2))
    assert result==dict(content='él',sha256=hashlib.sha256(data).hexdigest(),
                       total_characters=5,next_offset=3)


def test_project_read_returns_binary_hash_size_and_marker(worker):
    import hashlib
    store,ch,directory=worker
    target=directory/'binary.dat';data=b'\x00\xff\x80binary'
    target.write_bytes(data)
    result=dispatch(store,ch,'project_read',dict(path=str(target.relative_to(store.project_root))))
    assert result==dict(sha256=hashlib.sha256(data).hexdigest(),size_bytes=len(data),binary=True)


@pytest.mark.parametrize('link_kind',['symlink','hardlink'])
def test_delete_refuses_symlink_and_hardlink_files(worker,monkeypatch,link_kind):
    import hashlib
    from cortex_runtime.orchestration import consent
    monkeypatch.setattr(consent,'request',lambda *a,**k: True)
    store,ch,directory=worker
    source=directory/'source.dat';data=b'linked bytes';source.write_bytes(data)
    target=directory/'linked.dat'
    if link_kind=='symlink':
        target.symlink_to(source)
    else:
        target.hardlink_to(source)
    args=dict(path=str(target.relative_to(store.project_root)),
              expected_sha256=hashlib.sha256(data).hexdigest(),request_key='delete-'+link_kind)
    with pytest.raises(StoreError):
        dispatch(store,ch,'project_delete',args)
    assert source.read_bytes()==data
    if link_kind=='symlink':
        assert target.is_symlink()
    else:
        assert target.read_bytes()==data


def test_evidence_failure_after_command_preserves_unknown_outcome(worker,monkeypatch):
    store,ch,directory=worker
    from cortex_runtime.recovery import large_output_store
    monkeypatch.setattr(large_output_store,'persist_output',lambda *a,**k:(_ for _ in ()).throw(StoreError('test_persistence_failure')))
    target=str((directory/'count').relative_to(store.project_root))
    args=dict(argv=['python3','-B','-c',f'from pathlib import Path; Path({target!r}).write_text("ran")'],request_key='command')
    with pytest.raises(StoreError,match='test_persistence_failure'):dispatch(store,ch,'command_run',args)
    assert (directory/'count').read_text()=='ran'
    (directory/'count').write_text('do not repeat')
    with pytest.raises(StoreError,match='gateway_action_outcome_unknown'):dispatch(store,ch,'command_run',args)
    assert (directory/'count').read_text()=='do not repeat'


def test_rejected_precondition_receipt_is_stable(worker):
    store,ch,directory=worker;target=directory/'sentinel';target.write_text('one')
    args=dict(path=str(target.relative_to(store.project_root)),content='two',expected_sha256='absent',request_key='write')
    with pytest.raises(StoreError,match='gateway_write_conflict'):dispatch(store,ch,'project_write',args)
    target.unlink()
    with pytest.raises(StoreError,match='gateway_write_conflict'):dispatch(store,ch,'project_write',args)
    assert not target.exists()


def test_a_deletion_the_user_did_not_allow_keeps_the_file(worker,monkeypatch):
    import hashlib
    from cortex_runtime.orchestration import consent
    monkeypatch.setattr(consent,'request',lambda *a,**k: False)
    store,ch,directory=worker;target=directory/'sentinel';target.write_text('keep')
    args=dict(path=str(target.relative_to(store.project_root)),expected_sha256=hashlib.sha256(b'keep').hexdigest(),request_key='delete')
    with pytest.raises(StoreError,match='deletion_not_confirmed'):
        dispatch(store,ch,'project_delete',args)
    assert target.read_text()=='keep'


def test_outside_actions_run_only_after_the_user_allows_them_on_a_card(worker,tmp_path):
    """Outside the project the sandbox is read-only: a write or a command
    there waits for the user's card, and runs as the user when allowed."""
    import threading, time
    from cortex_runtime.orchestration import consent
    store,ch,directory=worker
    outside=tmp_path.parent/(tmp_path.name+'-outside.txt')
    cards=[]
    class Runtime:
        _store=store
        def permission_requester_for_task(self,task_id):
            def ask(engine_session_id,params,timeout):
                cards.append(params['toolCall']['title'])
                assert params['toolCall']['title'].startswith('Confirm: ')
                return {'outcome':{'outcome':'selected','optionId':'allow' if params['toolCall']['rawInput']['kind'].startswith('outside') else 'deny'}}
            return ask
    stop=threading.Event()
    def controller():
        while not stop.is_set():
            consent.relay(Runtime(),ch.task_id,'en'); time.sleep(.05)
    threading.Thread(target=controller,daemon=True).start()
    try:
        with pytest.raises(StoreError,match='outside_path_inside_project'):
            dispatch(store,ch,'outside_write',dict(path=str(directory/'x'),content='x',request_key='in'))
        assert dispatch(store,ch,'outside_write',dict(path=str(outside),content='hi',request_key='w'))['byte_count']==2
        assert outside.read_text()=='hi' and any(str(outside) in card for card in cards)
        done=dispatch(store,ch,'outside_command',dict(argv=['echo','hello'],request_key='c'))
        assert done['exit_code']==0 and 'hello' in done['output']
    finally:
        stop.set(); outside.unlink(missing_ok=True)
