import pytest

from cortex_runtime.acp.read_history import ReadHistoryFilter, metadata_only_read


def test_shell_call_titled_with_its_command_keeps_output():
    # Claude's Bash is titled with its own command line.
    assert not metadata_only_read('cat notes.md | grep todo', 'execute', {'command': 'cat notes.md | grep todo'})
    assert not metadata_only_read('cat notes.md', None, {'command': 'cat notes.md'})
    history = ReadHistoryFilter()
    history.project(dict(toolCallId='bash-1', title='cat notes.md', kind='execute',
                         rawInput={'command': 'cat notes.md'}))
    done = history.project(dict(toolCallId='bash-1', status='completed', rawOutput='todo: one'))
    assert done['rawOutput'] == 'todo: one'


def test_native_reads_stay_metadata_only():
    assert metadata_only_read('Read notes.md', 'read', {'file_path': 'notes.md'})
    # The engine's own read kind still makes a shell-shaped call a read.
    assert metadata_only_read('Read notes.md', 'read', {'command': 'cat notes.md'})
    assert metadata_only_read('cat notes.md', None, {})


@pytest.mark.parametrize('kind', ['execute', 'terminal'])
@pytest.mark.parametrize('provisional', [False, True])
@pytest.mark.parametrize('id_key,input_key,output_key', [
    ('toolCallId', 'rawInput', 'rawOutput'),
    ('tool_call_id', 'raw_input', 'raw_output'),
])
def test_split_shell_updates_keep_output(kind, provisional, id_key, input_key, output_key):
    history = ReadHistoryFilter()
    initial = {id_key: 'shell', 'title': 'cat a | grep b'}
    if not provisional:
        initial['kind'] = kind
    history.project(initial)
    # Explicit shell evidence can arrive before the command input does.
    history.project({id_key: 'shell', 'kind': kind})
    history.project({id_key: 'shell', input_key: {'command': 'cat a | grep b'}})
    output = {id_key: 'shell', output_key: 'shell result', 'content': ['shell result']}
    assert history.project(output) == output
    terminal = {id_key: 'shell', 'status': 'completed'}
    assert history.project(terminal) == terminal
    assert history.project({**initial, output_key: 'late output'})[output_key] == 'late output'


def test_provisional_read_can_be_replaced_by_its_command():
    history = ReadHistoryFilter()
    history.project(dict(toolCallId='shell', title='cat a | grep b'))
    done = history.project(dict(toolCallId='shell', title='cat a | grep b',
                                rawInput={'command': 'cat a | grep b'}, rawOutput='result'))
    assert done['rawOutput'] == 'result'
    assert history.project(dict(toolCallId='shell', rawOutput='later'))['rawOutput'] == 'later'


@pytest.mark.parametrize('initial', [
    {'title': 'cat a | grep b', 'kind': 'read'},
    {'title': 'mcp.cortex.project_read', 'kind': 'execute'},
    {'title': 'mcp__cortex__read_skill', 'kind': 'terminal'},
    {'title': 'project_read', 'kind': 'execute'},
    {'title': 'opaque', 'rawInput': {'server': 'cortex', 'tool': 'project_read'}},
])
def test_explicit_read_never_restores_bodies_after_shell_shaped_updates(initial):
    history = ReadHistoryFilter()
    history.project(dict(toolCallId='read', **initial))
    for update in [
        {'kind': 'execute', 'rawInput': {'command': 'cat a | grep b'}},
        {'rawOutput': 'native read body', 'content': ['native read body'], 'output': 'native read body'},
        {'status': 'completed', 'rawOutput': 'native read body'},
        {'status': 'failed', 'error': 'native read body', 'rawOutput': 'native read body'},
    ]:
        projected = history.project(dict(toolCallId='read', **update))
        assert 'native read body' not in str(projected)
    assert metadata_only_read(initial.get('title'), initial.get('kind'), initial.get('rawInput'))


@pytest.mark.parametrize('kind', ['execute', 'terminal'])
@pytest.mark.parametrize('provisional', [False, True])
def test_split_shell_survives_wire_persistence_and_replay(tmp_path, tmp_path_factory, kind, provisional):
    import threading

    from cortex_runtime.acp import replay_store
    from cortex_runtime.acp.agent import Agent
    from test_acp_input_history_and_replay import _CountingFakeRuntime, _RecordingTransport

    home = tmp_path_factory.mktemp('shell-display-home')
    agent = Agent(_CountingFakeRuntime(), environ=dict(
        HOME=str(home), XDG_CONFIG_HOME=str(home / 'config'), XDG_STATE_HOME=str(home / 'state')))
    transport = _RecordingTransport()
    agent.bind_transport(transport)
    agent.initialized = True
    sid = agent._session_new(dict(cwd=str(tmp_path), mcpServers=[]))['sessionId']
    emit = agent._make_emit_display_update(sid, threading.Event())
    initial = dict(sessionUpdate='tool_call', toolCallId='shell', title='cat a | grep b', status='in_progress')
    if not provisional:
        initial['kind'] = kind
    assert emit(initial)
    assert emit(dict(sessionUpdate='tool_call_update', toolCallId='shell', kind=kind))
    assert emit(dict(sessionUpdate='tool_call_update', toolCallId='shell', rawInput={'command': 'cat a | grep b'}))
    assert emit(dict(sessionUpdate='tool_call_update', toolCallId='shell', rawOutput='SHELL_OUTPUT_MARKER'))
    assert emit(dict(sessionUpdate='tool_call_update', toolCallId='shell', status='completed'))
    # Interleaved native reads remain compact even after contradictory shell evidence.
    assert emit(dict(sessionUpdate='tool_call', toolCallId='read', title='cat a', kind='read'))
    assert emit(dict(sessionUpdate='tool_call_update', toolCallId='read', kind='execute',
                     rawOutput='READ_BODY_MARKER', status='completed'))
    store = agent._store_for(str(tmp_path))
    with store.connection() as db:
        rows = [row[0] for row in db.execute('select update_json from host_updates where session_id=?', (sid,))]
    replay = replay_store.replay_session(store, sid)
    for display in (transport.notifications, rows, replay):
        assert 'SHELL_OUTPUT_MARKER' in str(display)
        assert 'READ_BODY_MARKER' not in str(display)
        assert 'completed' in str(display)
