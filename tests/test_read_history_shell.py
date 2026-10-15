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
