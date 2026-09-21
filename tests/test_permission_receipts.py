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


def test_delete_checks_exact_content_and_replays_receipt(worker):
    import hashlib
    store,ch,directory=worker;target=directory/'sentinel';target.write_text('keep')
    args=dict(path=str(target.relative_to(store.project_root)),expected_sha256=hashlib.sha256(b'keep').hexdigest(),request_key='delete')
    assert dispatch(store,ch,'project_delete',args)==dict(deleted=True)
    target.write_text('new')
    assert dispatch(store,ch,'project_delete',args)==dict(deleted=True)
    assert target.read_text()=='new'


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
