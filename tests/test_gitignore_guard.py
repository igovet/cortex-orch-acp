"""Effective Git semantics and real runtime gateway/submission boundaries."""
import hashlib
import subprocess
import sys

import pytest

from cortex_runtime.contracts import StoreError
from cortex_runtime.delegation import mcp_tools
from cortex_runtime.orchestration import gateway, ignore_guard
from test_situational_skill_gate import setup_worker, result_args


def ignored(root):
    subprocess.run(['git', 'init', '-q', str(root)], check=True)
    # --no-index proves ignore semantics without pretending tracked data is removed.
    result = subprocess.run(['git', '-C', str(root), 'check-ignore', '--no-index', '--stdin'],
                            input='.cortex/cortex.sqlite3\n.cortex/reports/a.md\n',
                            text=True, capture_output=True, check=True)
    assert result.stdout.splitlines() == ['.cortex/cortex.sqlite3', '.cortex/reports/a.md']


@pytest.mark.parametrize('content', ['', '*.py\n', '.cortex/\n',
    '.cortex/\n!.cortex/\n!.cortex/**\n', '/.cortex/\n!/*/\n',
    '# keep\r\n!important.txt\r\n', 'no-newline', '*\x85/.cortex/'])
def test_write_preserves_rules_overrides_negations_and_is_idempotent(tmp_path, content):
    store, _, worker = setup_worker(tmp_path)
    args = dict(path='.gitignore', content=content, expected_sha256='absent', request_key='ignore')
    receipt = mcp_tools.dispatch(store, worker, 'project_write', args)
    expected = ignore_guard.protected_content(content)
    assert (tmp_path / '.gitignore').read_bytes() == expected.encode()
    assert receipt['sha256'] == hashlib.sha256(expected.encode()).hexdigest()
    assert ignore_guard.protected_content(expected) == expected
    ignored(tmp_path)
    assert mcp_tools.dispatch(store, worker, 'project_write', args) == receipt
    with pytest.raises(StoreError, match='gateway_write_conflict'):
        mcp_tools.dispatch(store, worker, 'project_write', dict(args, expected_sha256='absent', request_key='stale'))


@pytest.mark.parametrize('edit', ['overwrite', 'remove', 'absent'])
def test_native_edit_is_reconciled_before_result_publication(tmp_path, edit):
    store, _, worker = setup_worker(tmp_path, start=edit != 'remove')
    path = tmp_path / '.gitignore'
    if edit == 'overwrite':
        path.write_text('!.cortex/\n')
    elif edit == 'remove':
        from cortex_runtime.delegation.revision import record_attempt_revision
        path.write_text('/.cortex/\n')
        record_attempt_revision(store, worker.attempt_id, f'worker_start:{worker.attempt_id}', tmp_path)
        path.unlink()
    else:
        assert mcp_tools.dispatch(store, worker, 'result_submit', result_args())['recorded']
        assert not path.exists()
        return
    receipt = mcp_tools.dispatch(store, worker, 'result_submit', result_args())
    assert receipt['recorded']
    ignored(tmp_path)
    before = path.read_bytes()
    assert mcp_tools.dispatch(store, worker, 'result_submit', result_args()) == receipt
    assert path.read_bytes() == before


@pytest.mark.parametrize('code', ["open('.gitignore','w').write('!.cortex/\\n')",
                                  "import os; os.unlink('.gitignore')",
                                  "open('.gitignore','w').write('!.cortex/\\n'); raise SystemExit(2)"])
def test_command_completion_repairs_even_after_nonzero_exit(tmp_path, code):
    store, _, worker = setup_worker(tmp_path)
    (tmp_path / '.gitignore').write_text('/.cortex/\n')
    receipt = mcp_tools.dispatch(store, worker, 'command_run',
        dict(argv=[sys.executable, '-B', '-c', code], request_key='command', timeout_seconds=30))
    assert receipt['complete'], receipt
    assert receipt['exit_code'] == (2 if 'SystemExit' in code else 0)
    ignored(tmp_path)


def test_delete_retains_only_required_rule_after_confirmation(tmp_path, monkeypatch):
    from cortex_runtime.orchestration import consent
    store, _, worker = setup_worker(tmp_path)
    content = b'old\n'
    (tmp_path / '.gitignore').write_bytes(content)
    monkeypatch.setattr(consent, 'request', lambda *a, **kw: True)
    receipt = mcp_tools.dispatch(store, worker, 'project_delete', dict(path='.gitignore',
        expected_sha256=hashlib.sha256(content).hexdigest(), request_key='delete'))
    assert receipt['deleted'] is False and receipt['gitignore_guard']
    assert (tmp_path / '.gitignore').read_text() == '/.cortex/\n'


def test_no_follow_and_conflict_protections(tmp_path):
    target = tmp_path / 'other'
    target.write_text('keep\n')
    (tmp_path / '.gitignore').symlink_to(target)
    with pytest.raises(OSError):
        gateway._write(tmp_path, dict(path='.gitignore', content='', expected_sha256='absent'))
    assert target.read_text() == 'keep\n'


def test_read_only_result_never_repairs(tmp_path):
    store, _, worker = setup_worker(tmp_path, role='code-reviewer')
    # Fixture uses a work-stage read-only role, not an independent review stage.
    (tmp_path / '.gitignore').write_text('!.cortex/\n')
    assert mcp_tools.dispatch(store, worker, 'result_submit', result_args())['recorded']
    assert (tmp_path / '.gitignore').read_text() == '!.cortex/\n'


def test_tracked_content_is_not_untracked(tmp_path):
    store, _, worker = setup_worker(tmp_path)
    subprocess.run(['git', 'init', '-q', str(tmp_path)], check=True)
    subprocess.run(['git', '-C', str(tmp_path), 'add', '-f', '.cortex/cortex.sqlite3'], check=True)
    before = subprocess.check_output(['git', '-C', str(tmp_path), 'ls-files', '--stage'])
    ignore_guard.enforce(store, worker)
    assert subprocess.check_output(['git', '-C', str(tmp_path), 'ls-files', '--stage']) == before
    ignored(tmp_path)


def test_denied_ignore_refuses_result_without_mutation(tmp_path, monkeypatch):
    store, _, worker = setup_worker(tmp_path)
    (tmp_path / '.gitignore').write_text('!.cortex/\n')
    monkeypatch.setattr(gateway, 'write_denials', lambda *a: ['.gitignore'])
    with pytest.raises(StoreError, match='gateway_scope_denied'):
        mcp_tools.dispatch(store, worker, 'result_submit', result_args())
    assert (tmp_path / '.gitignore').read_text() == '!.cortex/\n'


def test_repair_keeps_digest_concurrency_fence(tmp_path, monkeypatch):
    store, _, worker = setup_worker(tmp_path)
    (tmp_path / '.gitignore').write_text('one\n')
    original = gateway._write
    def concurrent(root, args):
        (tmp_path / '.gitignore').write_text('concurrent\n')
        return original(root, args)
    monkeypatch.setattr(gateway, '_write', concurrent)
    with pytest.raises(StoreError, match='gateway_write_conflict'):
        ignore_guard.enforce(store, worker)
    assert (tmp_path / '.gitignore').read_text() == 'concurrent\n'


def test_revoked_authority_never_repairs(tmp_path):
    store, _, worker = setup_worker(tmp_path)
    with store.connection() as db:
        db.execute("UPDATE trusted_channels SET revoked_at='2026-10-03T00:00:00+00:00' WHERE channel_id=?",
                   (worker.channel_id,))
        db.commit()
    with pytest.raises(StoreError, match='gateway_channel_inactive'):
        ignore_guard.enforce(store, worker)
    assert not (tmp_path / '.gitignore').exists()


def test_consented_outside_command_cannot_bypass_project_guard(tmp_path, monkeypatch):
    from cortex_runtime.orchestration import consent
    store, _, worker = setup_worker(tmp_path)
    monkeypatch.setattr(consent, 'request', lambda *a, **kw: True)
    receipt = mcp_tools.dispatch(store, worker, 'outside_command', dict(
        argv=[sys.executable, '-B', '-c', "open('.gitignore','w').write('!.cortex/\\n')"],
        cwd=str(tmp_path), request_key='outside', timeout_seconds=30))
    assert receipt['exit_code'] == 0 and receipt['complete']
    ignored(tmp_path)


def test_command_unsafe_replacement_is_not_success(tmp_path):
    store, _, worker = setup_worker(tmp_path)
    receipt = mcp_tools.dispatch(store, worker, 'command_run', dict(
        argv=[sys.executable, '-B', '-c', "import os; os.symlink('README.md','.gitignore')"],
        request_key='unsafe', timeout_seconds=30))
    assert receipt['exit_code'] == 0
    assert not receipt['complete'] and receipt['failure'] == 'gitignore_guard_failed'
    assert (tmp_path / 'README.md').read_text() == '# Project\n'


def test_self_ignored_ignore_file_removal_is_observed(tmp_path):
    from cortex_runtime.delegation.revision import record_attempt_revision
    store, _, worker = setup_worker(tmp_path, start=False)
    subprocess.run(['git', 'init', '-q', str(tmp_path)], check=True)
    (tmp_path / '.gitignore').write_text('*\n/.cortex/\n')
    record_attempt_revision(store, worker.attempt_id, f'worker_start:{worker.attempt_id}', tmp_path)
    (tmp_path / '.gitignore').unlink()
    # Removing '*' also exposes fixture docs/code; retain the independent gate.
    mcp_tools.dispatch(store, worker, 'read_skill', dict(skill='documentation-sync'))
    assert mcp_tools.dispatch(store, worker, 'result_submit', result_args())['recorded']
    ignored(tmp_path)


def test_missing_start_manifest_conservatively_ensures_rule(tmp_path):
    store, _, worker = setup_worker(tmp_path, start=False)
    assert mcp_tools.dispatch(store, worker, 'result_submit', result_args())['recorded']
    ignored(tmp_path)
