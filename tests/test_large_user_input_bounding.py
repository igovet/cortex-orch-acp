"""A megabyte user message (a pasted Paseo chat-history summary) must not be
inlined into every coordinator turn: the goal, the retained user_message
event and the forwarded prompt block each once overflowed the context budget
before the engine was prompted, and every later turn repeated it."""
import threading
import uuid

from cortex_runtime.acp import replay_store
from cortex_runtime.acp.orchestrating_runtime import OrchestratingRuntime, PromptOutcome
from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.delegation.workspace_isolation import IsolatedSnapshotWorkspaceProvisioner
from cortex_runtime.orchestration import user_text
from cortex_runtime.orchestration.mcp import _task_get_context
from cortex_runtime.portable.engine_contract import StopReason
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.store import Store
from cortex_runtime.trusted_channel import resolve_channel

from orchestration_fakes import FakeConfigSnapshotSource, FakeSessionRuntime

BIG = '<chat-history-summary>\n' + ''.join(f'[User] сообщение {i}: проверить спотовую торговлю\n' for i in range(20000))


def _boot(tmp_path, text=BIG):
    store = Store(tmp_path / '.cortex', project_root=tmp_path)
    return store, bootstrap_task(store, SourceEvent(origin='acp:big', event_id=str(uuid.uuid4())), text, 'owner')


def test_small_text_is_unchanged(tmp_path):
    store, boot = _boot(tmp_path, 'короткая задача')
    assert user_text.bound_text(store, boot.task_id, 'короткая задача') == 'короткая задача'


def test_large_text_keeps_head_tail_and_exact_source_reference(tmp_path):
    store, boot = _boot(tmp_path)
    bounded = user_text.bound_text(store, boot.task_id, BIG)
    data = BIG.encode('utf-8')
    assert len(bounded.encode('utf-8')) < user_text.INLINE_LIMIT_BYTES + 1024 < len(data)
    ref = replay_store.list_sources(store, boot.task_id)['entries'][0]['source_ref']
    head = data[:user_text.HEAD_BYTES].decode('utf-8', 'ignore')
    offset = len(head.encode('utf-8'))
    assert bounded.startswith(head)
    assert bounded.endswith(data[-user_text.TAIL_BYTES:].decode('utf-8', 'ignore'))
    assert f'read_source source_ref={ref} offset={offset}' in bounded
    page = replay_store.read_source(store, boot.task_id, ref, offset, 4096)
    assert page['chunk'].encode('utf-8').startswith(data[offset:offset + 4000])


def test_coordinator_turns_and_context_read_stay_bounded(tmp_path):
    class RecordingSessionRuntime(FakeSessionRuntime):
        def __init__(self, *args, **kwargs):
            super().__init__(*args, **kwargs)
            self.blocks = []

        def prompt(self, session_ref, message, cancel_event, *, prompt_blocks=None,
                   on_delta=None, on_tool_call=None, on_display_update=None):
            self.blocks.append(prompt_blocks)
            return super().prompt(session_ref, message, cancel_event, on_delta=on_delta,
                                  on_tool_call=on_tool_call, on_display_update=on_display_update)

    store, boot = _boot(tmp_path)
    seen = []

    def router(_store, _channel, _session_ref, _launch, _instructions):
        def behavior(_store, _channel, _ref, message, _call_index):
            seen.append(message)
            return PromptOutcome(StopReason.END_TURN, 'done')
        return behavior

    engine = RecordingSessionRuntime(store, router)
    runtime = OrchestratingRuntime(
        store, engine, IsolatedSnapshotWorkspaceProvisioner(store, str(store.project_root)),
        FakeConfigSnapshotSource(), allow_unqualified_delivery=True,
    )
    try:
        runtime.start_prompt(boot.task_id, 's1', BIG, lambda _text, **_kw: True, threading.Event(),
                             prompt_blocks=[dict(type='text', text=BIG)])
        runtime.start_prompt(boot.task_id, 's1', 'Продолжай', lambda _text, **_kw: True, threading.Event(),
                             prompt_blocks=[dict(type='text', text='Продолжай')])
    finally:
        runtime.shutdown()

    assert seen
    limit = 4 * user_text.INLINE_LIMIT_BYTES
    for message in seen:
        assert len(message.encode('utf-8')) < limit
    for blocks in filter(None, engine.blocks):
        assert sum(len(b.get('text', '').encode('utf-8')) for b in blocks) < limit
    assert any('Продолжай' in message for message in seen)
    context = _task_get_context(store, resolve_channel(store, boot.channel_id, boot.secret))
    assert len(context['goal'].encode('utf-8')) < user_text.INLINE_LIMIT_BYTES + 1024
    assert 'read_source source_ref=' in context['goal']
