"""Regression coverage for channel-bound safe replacement and atomic exact edits."""
import hashlib
import json
import os
import stat

import pytest

from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.contracts import StoreError
from cortex_runtime.delegation import mcp_tools
from cortex_runtime.orchestration import gateway
from cortex_runtime.orchestration.project_files import MAX_INLINE_BYTES, inline_size
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.store import Store
from cortex_runtime.trusted_channel import resolve_channel
from runtime_scenarios import provision_fixture


@pytest.fixture
def worker(tmp_path):
    store = Store(tmp_path/'.cortex', project_root=tmp_path)
    task = bootstrap_task(store, SourceEvent(origin='file-tools', event_id='one'), 'Build', 'owner')
    coordinator = resolve_channel(store, task.channel_id, task.secret)
    channel, _ = provision_fixture(store, coordinator)
    return store, channel, coordinator, tmp_path


def call(worker, operation, **args):
    return mcp_tools.dispatch(worker[0], worker[1], operation, args)


def read(worker, **args):
    return call(worker, 'project_read', path='file.txt', line_numbers=False, **args)


def edit(worker, edits, **args):
    return call(worker, 'project_edit', path='file.txt', edits=edits, **args)


def test_partial_page_sha_never_authorizes_overwrite_and_complete_pages_accumulate(worker):
    root = worker[3]
    original = 'first\nmiddle\nlast\n'
    (root/'file.txt').write_text(original)
    page = read(worker, offset=3, limit=1)
    assert page['content'] == 'last\n'
    assert page['total_lines'] == 3 and page['start_line'] == page['end_line'] == 3
    assert page['partial'] and not page['complete']
    assert page['full_content_sha256'] is None
    with pytest.raises(StoreError, match='gateway_complete_read_required'):
        call(worker, 'project_write', path='file.txt', expected_sha256=page['sha256'], content='last plus addition\n')
    assert (root/'file.txt').read_text() == original
    page = read(worker, limit=2)
    assert page['next_offset'] == 3
    assert page['partial'] and page['read_coverage_complete']
    assert page['full_content_sha256'] == hashlib.sha256(original.encode()).hexdigest()
    result = call(worker, 'project_write', path='file.txt', expected_sha256=page['sha256'], content='short\n')
    assert result['previous_size_bytes'] == len(original)
    assert result['previous_lines'] == 3 and result['new_lines'] == 1 and result['drastic_shrink']


def test_existing_write_requires_read_even_with_a_correct_explicit_digest(worker):
    (worker[3]/'file.txt').write_text('original')
    with pytest.raises(StoreError, match='gateway_complete_read_required'):
        call(worker, 'project_write', path='file.txt', expected_sha256=hashlib.sha256(b'original').hexdigest(), content='new')


def test_new_file_creation_and_replay_do_not_require_read(worker):
    args = dict(path='missing/parents/file.txt', content='hello\n', expected_sha256='absent')
    result = call(worker, 'project_write', **args)
    assert result['previous_size_bytes'] == result['previous_lines'] == 0
    assert result['size_bytes'] == 6 and result['new_lines'] == 1
    assert call(worker, 'project_write', **args) == result


@pytest.mark.parametrize('operation', ['project_write', 'project_edit'])
def test_stale_read_refuses_without_changing_external_work(worker, operation):
    target = worker[3]/'file.txt'
    target.write_text('before\n')
    observed = read(worker)
    target.write_text('external work\n')
    args = dict(path='file.txt')
    if operation == 'project_write':
        args.update(content='replacement', expected_sha256=hashlib.sha256(target.read_bytes()).hexdigest())
    else:
        args.update(edits=[dict(old_string='external', new_string='edited')])
    with pytest.raises(StoreError, match='gateway_stale_read'):
        call(worker, operation, **args)
    assert target.read_text() == 'external work\n'
    assert observed['sha256'] != hashlib.sha256(target.read_bytes()).hexdigest()


def test_partial_read_allows_exact_edit_and_preserves_unseen_content(worker):
    target = worker[3]/'file.txt'
    target.write_text('unseen\ntarget\nunseen end\n')
    read(worker, offset=2, limit=1)
    receipt = edit(worker, [dict(old_string='target', new_string='changed')])
    assert target.read_text() == 'unseen\nchanged\nunseen end\n'
    assert receipt['edits_applied'] == 1 and '-target' in receipt['diff']
    assert receipt['sha256'] == hashlib.sha256(target.read_bytes()).hexdigest()
    assert receipt['oldText'] == 'unseen\ntarget\nunseen end\n'


def test_exact_unique_replace_all_and_multi_edit_atomicity(worker):
    target = worker[3]/'file.txt'
    target.write_text('one\none\ntail\n')
    read(worker)
    with pytest.raises(StoreError, match='gateway_edit_not_unique'):
        edit(worker, [dict(old_string='one', new_string='two')])
    with pytest.raises(StoreError, match='gateway_edit_not_found'):
        edit(worker, [dict(old_string='one', new_string='two', replace_all=True),
                      dict(old_string='missing', new_string='oops')])
    assert target.read_text() == 'one\none\ntail\n'
    assert not list(worker[3].glob('.cortex-write-*'))
    receipt = edit(worker, [dict(old_string='one', new_string='two', replace_all=True),
                           dict(old_string='tail\n', new_string='tail\nappended\n')])
    assert target.read_text() == 'two\ntwo\ntail\nappended\n'
    assert receipt['edits_applied'] == 2


def test_edits_require_read_exact_whitespace_and_different_content(worker):
    (worker[3]/'file.txt').write_text('  a\n')
    with pytest.raises(StoreError, match='gateway_read_required'):
        edit(worker, [dict(old_string='a', new_string='b')])
    read(worker)
    with pytest.raises(StoreError, match='gateway_edit_not_found'):
        edit(worker, [dict(old_string=' a \n', new_string='b')])
    with pytest.raises(StoreError, match='gateway_edit_no_change'):
        edit(worker, [dict(old_string='a', new_string='a')])
    receipt = edit(worker, [dict(old_string='  a\n', new_string='')], request_key='delete-text')
    assert (worker[3]/'file.txt').read_text() == ''
    assert receipt['drastic_shrink'] and receipt['new_lines'] == 0
    assert edit(worker, [dict(old_string='  a\n', new_string='')], request_key='delete-text') == receipt


def test_read_state_is_channel_local_and_survives_store_reopen(worker):
    store, channel, coordinator, root = worker
    (root/'file.txt').write_text('a\nb\n')
    observed = read(worker)
    other, _ = provision_fixture(store, coordinator, key='other')
    with pytest.raises(StoreError, match='gateway_complete_read_required'):
        mcp_tools.dispatch(store, other, 'project_write',
                           dict(path='file.txt', content='oops', expected_sha256=observed['sha256']))
    reopened = Store(root/'.cortex', project_root=root)
    receipt = mcp_tools.dispatch(reopened, channel, 'project_edit',
                                dict(path='file.txt', edits=[dict(old_string='a', new_string='c')]))
    assert receipt['sha256'] and (root/'file.txt').read_text() == 'c\nb\n'


@pytest.mark.parametrize('body', ['line\n'*10000, ('😀\\\"\t\x01'*500+'\n')*100, 'x'*10000+'\nend\n'], ids=['many-lines', 'escaped-unicode', 'long-line'])
def test_entire_mcp_result_is_bounded_including_json_escaping(worker, body):
    (worker[3]/'file.txt').write_text(body)
    result = call(worker, 'project_read', path='file.txt')
    assert inline_size(result) <= MAX_INLINE_BYTES
    assert result['partial']
    assert result['next_offset'] is not None or result['truncated_lines']
    assert result['content'].startswith('     1\t')
    assert result['full_content_sha256'] is None
    if len(body.splitlines()[0]) > 2000:
        assert 1 in result['truncated_lines']
        with pytest.raises(StoreError, match='gateway_complete_read_required'):
            call(worker, 'project_write', path='file.txt', content='small', expected_sha256=result['sha256'])


def test_empty_and_past_end_reads_and_complete_read_reset(worker):
    target = worker[3]/'file.txt'
    target.write_text('')
    empty = read(worker)
    assert empty['complete'] and empty['total_lines'] == 0 and empty['next_offset'] is None
    target.write_text('first\nlast\n')
    past = read(worker, offset=50)
    assert past['partial'] and not past['read_coverage_complete'] and past['notice']
    page = read(worker, limit=1)
    assert not page['read_coverage_complete']
    target.write_text('other\nlast\n')
    page = read(worker, offset=2, limit=1)
    assert not page['read_coverage_complete']  # cannot combine pages of different revisions


@pytest.mark.parametrize('path', ['.cortex/private', '.git/config', '../escape'])
def test_edit_private_paths_refused(worker, path):
    with pytest.raises(StoreError, match='gateway_path_denied'):
        call(worker, 'project_edit', path=path, edits=[dict(old_string='a', new_string='b')])


def test_edit_uses_same_role_scope_denial_and_symlink_refusal(worker):
    store, channel, coordinator, root = worker
    (root/'file.txt').write_text('a')
    read(worker)
    args = dict(path='file.txt', edits=[dict(old_string='a', new_string='b')])
    with pytest.raises(StoreError, match='gateway_scope_denied'):
        mcp_tools.dispatch(store, coordinator, 'project_edit', args)
    reviewer, assignment = provision_fixture(store, coordinator, key='review', role='code-reviewer')
    with pytest.raises(StoreError, match='gateway_scope_denied'):
        mcp_tools.dispatch(store, reviewer, 'project_edit', args)
    (root/'link').symlink_to(root/'file.txt')
    with pytest.raises(StoreError, match='gateway_io_failure'):
        call(worker, 'project_edit', path='link', edits=args['edits'])
    assert (root/'file.txt').read_text() == 'a'


def test_write_and_edit_fsync_file_before_replace_and_directory_after(worker, monkeypatch):
    target = worker[3]/'file.txt'
    target.write_text('a')
    read(worker)
    events = []
    fsync, replace = os.fsync, os.replace
    def sync(fd):
        events.append('directory' if stat.S_ISDIR(os.fstat(fd).st_mode) else 'file')
        return fsync(fd)
    def rename(*args, **kwargs):
        events.append('replace')
        return replace(*args, **kwargs)
    monkeypatch.setattr(os, 'fsync', sync)
    monkeypatch.setattr(os, 'replace', rename)
    edit(worker, [dict(old_string='a', new_string='b')])
    assert events == ['file', 'replace', 'directory']


def test_project_edit_unknown_receipt_is_a_recovery_blocker(worker):
    from cortex_runtime.recovery.worker_reconciliation import unknown_outcome_actions
    store, channel, _, _ = worker
    with store.connection() as db:
        db.execute('INSERT INTO runtime_requests VALUES(?,?,?,?,?,?)',
                   (channel.task_id, 'lost-edit', channel.channel_id, 'project_edit', 'digest',
                    json.dumps(dict(complete=False, tool_call_ref='gateway:'+'a'*32))))
        db.commit()
        actions = unknown_outcome_actions(db, channel.task_id)
    assert any(action['operation'] == 'project_edit' for action in actions)

def test_overlapping_matches_are_not_unique_and_expansion_is_bounded(worker):
    target = worker[3]/'file.txt'
    target.write_text('aaa')
    read(worker)
    with pytest.raises(StoreError, match='gateway_edit_not_unique'):
        edit(worker, [dict(old_string='aa', new_string='b')])
    target.write_text('a'*20)
    read(worker)
    with pytest.raises(StoreError, match='gateway_file_too_large'):
        edit(worker, [dict(old_string='a', new_string='b'*1000000, replace_all=True)])
    assert target.read_text() == 'a'*20


def test_edit_response_budget_counts_control_character_escaping(worker):
    target = worker[3]/'file.txt'
    target.write_text('x'*1900+'\n')
    read(worker)
    receipt = edit(worker, [dict(old_string='x'*1900, new_string='\x01'*1900)])
    assert inline_size(receipt) <= MAX_INLINE_BYTES
    assert target.read_text() == '\x01'*1900+'\n'


def test_edit_respects_user_denials_and_channel_revocation(worker):
    from cortex_runtime.orchestration.state import event
    store, channel, _, root = worker
    (root/'file.txt').write_text('a')
    read(worker)
    with store.connection() as db:
        event(db, channel.task_id, 'USER_STEERING', dict(deny_write=['file.txt']), 'user-denial')
        event(db, channel.task_id, 'STEERING_APPLIED', {}, 'user-denial')
        db.commit()
    with pytest.raises(StoreError, match='gateway_scope_denied'):
        edit(worker, [dict(old_string='a', new_string='b')])
    assert (root/'file.txt').read_text() == 'a'
    with store.connection() as db:
        db.execute("UPDATE trusted_channels SET revoked_at='2026-10-08' WHERE channel_id=?", (channel.channel_id,))
        db.commit()
    with pytest.raises(StoreError, match='gateway_channel_inactive'):
        edit(worker, [dict(old_string='a', new_string='b')])


def test_directory_pages_are_one_based_and_bounded(worker):
    root = worker[3]/'directory'
    root.mkdir()
    for index in range(150):
        (root/(f'{index:03}-'+'x'*200)).touch()
    result = call(worker, 'project_read', path='directory', offset=2, limit=2000)
    assert inline_size(result) <= MAX_INLINE_BYTES and result['entries'][0].startswith('001-')
    assert result['next_offset'] == 2+len(result['entries'])


def test_advisory_hint_cannot_push_edit_receipt_over_inline_limit(worker, monkeypatch):
    from cortex_runtime.orchestration import skill_gate
    (worker[3]/'file.txt').write_text('a')
    read(worker)
    monkeypatch.setattr(skill_gate, 'required_before_result_submit',
                        lambda *a, **k: [dict(skill='documentation-sync', reason='hint'*10000)])
    result = edit(worker, [dict(old_string='a', new_string='b')])
    assert inline_size(result) <= MAX_INLINE_BYTES
    assert result['skill_hint_unavailable'].startswith('inline_limit')
    assert (worker[3]/'file.txt').read_text() == 'b'


def test_single_heavily_escaped_line_always_makes_paging_progress(worker):
    (worker[3]/'file.txt').write_text('\x00'*2000+'\nend\n')
    first = read(worker, limit=1)
    assert inline_size(first) <= MAX_INLINE_BYTES
    assert first['end_line'] == 1 and first['next_offset'] == 2
    assert first['truncated_lines'] == [1] and first['content']
    last = read(worker, offset=first['next_offset'])
    assert last['content'] == 'end\n'
    assert not last['read_coverage_complete']


@pytest.mark.parametrize('line_numbers', [True, False])
def test_inline_budget_truncation_preserves_real_whitespace(worker, line_numbers):
    # NULs need JSON escaping even after the initial 2000-character line cap.
    body = '\x00'*3000+'\n'
    (worker[3]/'file.txt').write_text(body)
    result = call(worker, 'project_read', path='file.txt', line_numbers=line_numbers)
    content = result['content']
    assert content.endswith('… [line truncated by inline budget]\n')
    assert content.startswith('     1\t' if line_numbers else '\x00')
    assert '\\n' not in content and '\\t' not in content
    assert inline_size(result) <= MAX_INLINE_BYTES
    assert result['start_line'] == result['end_line'] == result['total_lines'] == 1
    assert result['next_offset'] is None and result['truncated_lines'] == [1]
    assert result['partial'] and not result['complete']
    assert not result['read_coverage_complete'] and result['full_content_sha256'] is None
    assert (worker[3]/'file.txt').read_text() == body


@pytest.mark.parametrize('before,old,new,after', [
    ('/.cortex/\n', '/.cortex/\n', '', '/.cortex/\n'),
    ('*.log\n/.cortex/\n', '/.cortex/\n', '!important.log\n',
     '*.log\n!important.log\n/.cortex/\n'),
    ('*.log\r\n/.cortex/\r\n', '*.log', '*.tmp',
     '*.tmp\r\n/.cortex/\r\n'),
])
def test_gitignore_edit_receipt_describes_exact_committed_bytes(worker, before, old, new, after):
    from cortex_runtime.orchestration.project_files import compact_diff
    target = worker[3]/'.gitignore'
    target.write_bytes(before.encode())
    call(worker, 'project_read', path='.gitignore')
    args = dict(path='.gitignore', edits=[dict(old_string=old, new_string=new)])
    receipt = call(worker, 'project_edit', **args)
    assert target.read_bytes() == after.encode()
    assert receipt['sha256'] == hashlib.sha256(after.encode()).hexdigest()
    assert receipt['size_bytes'] == receipt['byte_count'] == len(after.encode())
    assert receipt['new_lines'] == len(after.splitlines())
    assert receipt['oldText'] == before and receipt['newText'] == after
    assert receipt['diff'] == compact_diff(before, after, '.gitignore')['diff']
    assert not receipt['diff_truncated']
    if before == after:
        assert receipt['diff'] == ''
    assert call(worker, 'project_edit', **args) == receipt


@pytest.mark.parametrize('body', [
    ''.join(f'line-{index % 100:03}\n' if index else 'first000\n' for index in range(160000)),
    'x'*40000+'\n',
    'x\n'*1000,
], ids=['repetitive-1.44MB', 'character-budget', 'line-pair-budget'])
def test_diff_input_budget_never_invokes_unbounded_matcher(worker, monkeypatch, body):
    from cortex_runtime.orchestration import project_files
    target = worker[3]/'file.txt'
    target.write_text(body)
    read(worker, limit=1)
    def forbidden(*args, **kwargs):
        pytest.fail('Over-budget inputs must not enter difflib')
    monkeypatch.setattr(project_files.difflib, 'unified_diff', forbidden)
    receipt = edit(worker, [dict(old_string=body.splitlines()[0], new_string='changed',
                                 replace_all=True)])
    committed = target.read_bytes()
    assert committed == body.replace(body.splitlines()[0], 'changed').encode()
    assert receipt['sha256'] == hashlib.sha256(committed).hexdigest()
    assert receipt['size_bytes'] == len(committed)
    assert receipt['diff'] == '' and receipt['diff_truncated']
    assert receipt['diff_omitted'] == 'input_budget'
    assert inline_size(receipt) <= MAX_INLINE_BYTES
    assert 'oldText' not in receipt and 'newText' not in receipt


def test_gitignore_edit_size_cap_includes_restored_protection(worker):
    target = worker[3]/'.gitignore'
    before = b'x'*15999998+b'\n'
    target.write_bytes(before)
    call(worker, 'project_read', path='.gitignore', limit=1)
    with pytest.raises(StoreError, match='gateway_file_too_large'):
        call(worker, 'project_edit', path='.gitignore',
             edits=[dict(old_string='x\n', new_string='y\n')])
    assert target.read_bytes() == before
    assert not list(worker[3].glob('.cortex-write-*'))
