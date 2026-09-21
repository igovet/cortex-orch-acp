"""Draft publication uses channel-owned IDs regardless of engine cwd."""
from pathlib import Path
import pytest
from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.store import Store
from cortex_runtime.trusted_channel import resolve_channel
from cortex_runtime.orchestration.state import TaskMachine
from cortex_runtime.orchestration.gateway import dispatch
from cortex_runtime.contracts import StoreError


@pytest.mark.parametrize('cwd_kind',['project','isolated','unrelated_store'])
def test_draft_gateway_is_bound_to_project_not_engine_cwd(tmp_path,monkeypatch,cwd_kind):
    root=tmp_path/'project';root.mkdir()
    store=Store(root/'.cortex',project_root=root)
    task=bootstrap_task(store,SourceEvent(origin='test',event_id='draft'),'Inspect','owner')
    TaskMachine(store).create(task.task_id,'Inspect')
    channel=resolve_channel(store,task.channel_id,task.secret)
    cwd=root if cwd_kind=='project' else tmp_path/cwd_kind
    cwd.mkdir(exist_ok=True)
    if cwd_kind=='unrelated_store': (cwd/'.cortex').mkdir()
    monkeypatch.chdir(cwd)
    draft=store.call('create_draft',dict(template='general',request_key='draft'),channel.principal_thread_id,trusted=True)
    content=draft['required_first_line']+'\n\n# Report\n\nObserved result.\n'
    dispatch(store,channel,'draft_write',dict(draft_id=draft['draft_id'],content=content,request_key='fill'))
    result=store.call('write_report',dict(draft_id=draft['draft_id'],title='Report',summary='Observed result',author='runtime-test',request_key='publish'),channel.principal_thread_id,trusted=True)
    assert result['report_id']
    with pytest.raises(StoreError):
        dispatch(store,channel,'draft_write',dict(draft_id=draft['draft_id'],content=content,request_key='overwrite'))
    if cwd!=root:
        assert not list(cwd.rglob('*.md'))
