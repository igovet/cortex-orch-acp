"""Project-local storage regressions for the channeled ACP path (T033).

Covers the still-live guards that lost coverage when the Codex-plugin-harness
era fixtures (native thread index, hooks) were deleted along with the old,
much larger ``test_project_storage.py``: the one-way legacy storage
transfer, the ``project_storage_mismatch`` guard, and (moved in from the
deleted ``test_no_alternate_storage_root.py``) the single-store-root
invariant and the rejected-database byte/journal-mode guard.
"""
from concurrent.futures import ThreadPoolExecutor
from datetime import datetime, timedelta
from pathlib import Path
import subprocess
import sqlite3
import sys
import uuid

import pytest

from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.contracts import StoreError
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.project_storage import bind_workspace, migrate_legacy_project_storage
from cortex_runtime.server import Server
from cortex_runtime.store import Store, APPLICATION_ID

import cortex_clear


def bootstrap(project, owner='owner-1', text='Build a page.'):
    store = Store(project/'.cortex', project_root=project)
    result = bootstrap_task(store, SourceEvent(origin='cli', event_id=str(uuid.uuid4())), text, owner)
    return store, result


def server_for(project, channel_id, secret):
    return Server(channel_credentials=(channel_id, secret, str(project)))


def write_a_report(server):
    draft = server.dispatch('tools/call', dict(
        name='create_draft', arguments=dict(template='general', request_key='k1')))
    assert not draft['isError'], draft
    path = Path(draft['structuredContent']['draft_path'])
    marker = path.read_text().split('\n\n', 1)[0] + '\n\n'
    path.write_text(marker + 'Evidence body.')
    published = server.dispatch('tools/call', dict(
        name='write_report', arguments=dict(title='Evidence', summary='Verified.', author='worker',
                                             draft_id=draft['structuredContent']['draft_id'], request_key='pub1')))
    assert not published['isError'], published
    return published['structuredContent']


# --- migrate_legacy_project_storage -------------------------------------

@pytest.mark.parametrize('contents', ['database', 'reports', 'complete'])
def test_one_way_legacy_storage_transfer_moves_only_absent_components(tmp_path, contents):
    project = tmp_path/'project'; project.mkdir()
    legacy = project/'.codex'/'cortex'; legacy.mkdir(parents=True, mode=0o700)
    task = 't_0123456789ab'
    if contents in {'database', 'complete'}:
        database = legacy/'cortex.sqlite3'
        database.write_bytes(b'legacy database'); database.chmod(0o600)
    if contents in {'reports', 'complete'}:
        reports = legacy/task; reports.mkdir(mode=0o700)
        report = reports/'r_0123456789ab.md'
        report.write_text('legacy report'); report.chmod(0o600)

    assert migrate_legacy_project_storage(project) is True
    destination = project/'.cortex'
    if contents in {'database', 'complete'}:
        assert (destination/'cortex.sqlite3').read_bytes() == b'legacy database'
        assert not (legacy/'cortex.sqlite3').exists()
    if contents in {'reports', 'complete'}:
        assert (destination/task/'r_0123456789ab.md').read_text() == 'legacy report'
        assert not (legacy/task).exists()


def test_one_way_legacy_storage_transfer_is_noop_when_source_is_absent(tmp_path):
    project = tmp_path/'project'; project.mkdir()
    destination = project/'.cortex'; destination.mkdir(mode=0o700)
    database = destination/'cortex.sqlite3'
    database.write_bytes(b'current database'); database.chmod(0o600)
    assert migrate_legacy_project_storage(project) is False
    assert database.read_bytes() == b'current database'
    assert not (project/'.codex'/'cortex').exists()


@pytest.mark.parametrize('conflict', ['database', 'reports'])
def test_one_way_legacy_storage_transfer_rejects_conflicts_without_moving(tmp_path, conflict):
    project = tmp_path/'project'; project.mkdir()
    legacy = project/'.codex'/'cortex'; legacy.mkdir(parents=True, mode=0o700)
    destination = project/'.cortex'; destination.mkdir(mode=0o700)
    task = 't_0123456789ab'
    if conflict == 'database':
        source = legacy/'cortex.sqlite3'; target = destination/'cortex.sqlite3'
        source.write_bytes(b'legacy'); target.write_bytes(b'current')
        source.chmod(0o600); target.chmod(0o600)
    else:
        source = legacy/task; target = destination/task
        source.mkdir(mode=0o700); target.mkdir(mode=0o700)
        for root, body in ((source, 'legacy'), (target, 'current')):
            report = root/'pipeline.md'; report.write_text(body); report.chmod(0o600)
    with pytest.raises(StoreError, match='legacy_storage_conflict'):
        migrate_legacy_project_storage(project)
    assert source.exists() and target.exists()
    if conflict == 'database':
        assert source.read_bytes() == b'legacy' and target.read_bytes() == b'current'
    else:
        assert (source/'pipeline.md').read_text() == 'legacy'
        assert (target/'pipeline.md').read_text() == 'current'


# --- project_storage_mismatch --------------------------------------------

def test_foreign_project_database_copy_is_rejected(tmp_path):
    """A store whose persisted task rows name a different project than the
    one it is opened against must fail closed, and must not be able to
    silently adopt the copied project's data (T030's ``test_copied_foreign_
    project_database_is_rejected``, restored for the channeled path)."""
    project_a, project_b = tmp_path/'project-a', tmp_path/'project-b'
    project_a.mkdir(); project_b.mkdir()
    bootstrap(project_a)

    import shutil
    shutil.copytree(project_a/'.cortex', project_b/'.cortex')

    with pytest.raises(StoreError, match='project_storage_mismatch'):
        Store(project_b/'.cortex', initialize=False, project_root=project_b)
    copied = project_b/'.cortex'/'cortex.sqlite3'
    with sqlite3.connect(copied) as db:
        assert db.execute('PRAGMA journal_mode=DELETE').fetchone()[0] == 'delete'
    before = copied.read_bytes()
    with pytest.raises(StoreError, match='project_storage_mismatch'):
        Store(project_b/'.cortex', project_root=project_b)
    assert copied.read_bytes() == before


# --- rejected foreign/invalid database is left untouched ------------------

@pytest.mark.parametrize('kind', ['foreign', 'format10'])
def test_rejected_database_keeps_bytes_and_journal_mode(tmp_path, kind):
    directory = tmp_path/'.cortex'
    directory.mkdir(parents=True, mode=0o700)
    database = directory/'cortex.sqlite3'
    db = sqlite3.connect(database)
    try:
        db.execute('CREATE TABLE foreign_data(value TEXT)')
        db.execute("INSERT INTO foreign_data VALUES ('preserved')")
        if kind == 'format10':
            db.execute(f'PRAGMA application_id={APPLICATION_ID}')
            db.execute('PRAGMA user_version=10')
        db.commit()
    finally:
        db.close()
    database.chmod(0o600)
    before = database.read_bytes()
    with pytest.raises(StoreError, match='unsupported_storage'):
        Store(directory, project_root=str(tmp_path))
    assert database.read_bytes() == before
    assert all(not Path(str(database)+suffix).exists() for suffix in ('-wal', '-shm', '-journal'))


# --- single-store-root invariant ------------------------------------------

def _git(cwd, home, *args):
    import os
    env = dict(os.environ, GIT_CONFIG_GLOBAL='/dev/null', GIT_CONFIG_NOSYSTEM='1', HOME=str(home))
    subprocess.run(['git', '-c', 'user.name=t', '-c', 'user.email=t@t', *args],
                    cwd=cwd, env=env, check=True, capture_output=True)


@pytest.fixture(autouse=True)
def _restore_process_umask():
    """Undo the process-wide ``os.umask`` this file's real calls leave set.

    ``Server.__init__`` (server.py) and ``cortex_clear.main()`` both call
    ``os.umask(0o077)`` and never restore it; that is pre-existing,
    out-of-scope behavior in files this task does not own. Left unguarded, a
    test file that happens to sort before ``tests/test_package.py`` in
    pytest's default collection order would leak that umask forward and
    break unrelated tests there. This file exercises both call paths, so it
    restores the umask this process had on entry after every test.
    """
    import os
    original = os.umask(0o022)
    os.umask(original)
    yield
    os.umask(original)


def _walk_sqlite_and_cortex(root):
    import os
    sqlite_files, cortex_dirs, legacy_dirs = [], [], []
    for base, directories, files in os.walk(root):
        for name in files:
            if name == 'cortex.sqlite3':
                sqlite_files.append(Path(base)/name)
        for name in directories:
            if name == '.cortex':
                cortex_dirs.append(Path(base)/name)
            if Path(base).name == '.codex' and name == 'cortex':
                legacy_dirs.append(Path(base)/name)
    return sqlite_files, cortex_dirs, legacy_dirs


def test_representative_operations_create_exactly_one_project_store(tmp_path):
    home = tmp_path/'git-home'; home.mkdir()
    project = tmp_path/'project'; project.mkdir()
    _git(project, home, 'init', '-q')
    (project/'f.txt').write_text('hi')
    _git(project, home, 'add', 'f.txt')
    _git(project, home, 'commit', '-q', '-m', 'init')
    worktree = tmp_path/'worktree'
    _git(project, home, 'worktree', 'add', '-q', str(worktree), '-b', 'feature')

    store, result = bootstrap(project)
    from cortex_runtime.orchestration.state import TaskMachine
    TaskMachine(store).create(result.task_id,"Storage fixture")
    server = server_for(project, result.channel_id, result.secret)
    write_a_report(server)

    with store.connection() as db:
        task_id = db.execute('SELECT id FROM tasks').fetchone()[0]
    binding = bind_workspace(project, task_id, worktree)
    assert binding.workspace_root == str(worktree.resolve())

    argv = ['cortex_clear.py', '--project-root', str(project), '--days', '0']
    old_argv = sys.argv
    sys.argv = argv
    try:
        cortex_clear.main()
    finally:
        sys.argv = old_argv

    sqlite_files, cortex_dirs, legacy_dirs = _walk_sqlite_and_cortex(tmp_path)
    assert sqlite_files == [project/'.cortex'/'cortex.sqlite3']
    assert cortex_dirs == [project/'.cortex']
    assert legacy_dirs == []
    assert not (worktree/'.cortex').exists()
    assert not (project/'.codex').exists()


def _run_cortex_clear(argv):
    old_argv = sys.argv
    sys.argv = ['cortex_clear.py', *argv]
    try:
        cortex_clear.main()
    finally:
        sys.argv = old_argv


@pytest.mark.parametrize('days', [0, 1, 36500])
@pytest.mark.parametrize('flags', [[], ['--dry-run'], ['--force'], ['--force', '--dry-run']])
def test_clear_without_database_reports_cutoff_without_creating_store(tmp_path, capsys, days, flags):
    import json
    from datetime import timezone

    before = datetime.now(timezone.utc) - timedelta(days=days)
    _run_cortex_clear(['--project-root', str(tmp_path), '--days', str(days), *flags])
    after = datetime.now(timezone.utc) - timedelta(days=days)
    result = json.loads(capsys.readouterr().out)
    cutoff = datetime.fromisoformat(result.pop('cutoff'))
    assert cutoff.utcoffset() == timedelta(0)
    assert before <= cutoff <= after
    expected = dict(deleted_tasks=0, retention_days=days, skipped_protected=0,
                    force='--force' in flags, deleted_orphan_sessions=0,
                    vacuum_status='not_needed', reclaimed_bytes=0)
    if '--dry-run' in flags:
        expected.update(would_delete_tasks=0, would_delete_orphan_sessions=0)
    assert result == expected
    assert not (tmp_path / '.cortex').exists()


def test_keep_thread_flag_protects_a_bound_task_but_codex_env_no_longer_does(tmp_path, monkeypatch):
    """B5/S3,N1: the only accepted protection route is the real, repeatable
    ``--keep-thread <id>`` flag matched against ``thread_bindings``. The
    Codex-plugin-harness-era ``CODEX_THREAD_ID``/``CODEX_SESSION_ID``
    environment fallback is gone -- setting either must no longer protect
    a task, even when its value is the task's own real thread id."""
    project = tmp_path / 'project'
    project.mkdir()
    store, result = bootstrap(project)
    with store.connection() as db:
        task_id = db.execute('SELECT id FROM tasks').fetchone()[0]
        thread_id = db.execute(
            'SELECT thread_id FROM thread_bindings WHERE task_id=?', (task_id,)).fetchone()[0]

    monkeypatch.setenv('CODEX_THREAD_ID', thread_id)
    monkeypatch.setenv('CODEX_SESSION_ID', thread_id)
    import json as _json
    import io
    import contextlib
    out = io.StringIO()
    with contextlib.redirect_stdout(out):
        _run_cortex_clear(['--project-root', str(project), '--days', '0'])
    unprotected = _json.loads(out.getvalue())
    assert {key: unprotected[key] for key in
            ("deleted_tasks", "retention_days", "skipped_protected")} == dict(
                deleted_tasks=1, retention_days=0, skipped_protected=0)
    assert set(unprotected) == {
        "deleted_tasks", "retention_days", "skipped_protected", "cutoff", "force",
        "deleted_orphan_sessions", "vacuum_status", "reclaimed_bytes",
        "db_bytes_before", "db_bytes_after",
    }
    cutoff = datetime.fromisoformat(unprotected["cutoff"].replace("Z", "+00:00"))
    assert cutoff.tzinfo is not None and cutoff.utcoffset() == timedelta(0)
    with store.connection() as db:
        assert db.execute('SELECT id FROM tasks').fetchall() == []

    monkeypatch.delenv('CODEX_THREAD_ID', raising=False)
    monkeypatch.delenv('CODEX_SESSION_ID', raising=False)
    store2, result2 = bootstrap(project, owner='owner-2')
    with store2.connection() as db:
        task_id2 = db.execute('SELECT id FROM tasks').fetchone()[0]
        thread_id2 = db.execute(
            'SELECT thread_id FROM thread_bindings WHERE task_id=?', (task_id2,)).fetchone()[0]

    out2 = io.StringIO()
    with contextlib.redirect_stdout(out2):
        _run_cortex_clear(['--project-root', str(project), '--days', '0',
                            '--keep-thread', thread_id2])
    protected = _json.loads(out2.getvalue())
    assert {key: protected[key] for key in
            ("deleted_tasks", "retention_days", "skipped_protected")} == dict(
                deleted_tasks=0, retention_days=0, skipped_protected=1)
    assert set(protected) == {
        "deleted_tasks", "retention_days", "skipped_protected", "cutoff", "force",
        "deleted_orphan_sessions", "vacuum_status", "reclaimed_bytes",
    }
    cutoff = datetime.fromisoformat(protected["cutoff"].replace("Z", "+00:00"))
    assert cutoff.tzinfo is not None and cutoff.utcoffset() == timedelta(0)
    with store2.connection() as db:
        assert [row[0] for row in db.execute('SELECT id FROM tasks')] == [task_id2]


def test_the_real_launcher_creates_exactly_one_project_store(tmp_path):
    """SEC-11/T034: bootstrap a task the way a real host actually does --
    through the shipped launcher entry point (``cortex_acp.py``, spawned as
    a real subprocess exactly the way Zed would launch it -- see
    ``acp_fakes.spawn_production_agent``) via ``session/new`` +
    ``session/prompt`` -- and confirm the launcher/spawn path itself never
    creates ``project/.codex/cortex``, a second ``.cortex`` anywhere, or
    any other alternate storage root. ``session/prompt`` is expected to
    fail with an explicit unresolved-route error (no connection is
    configured in this isolated test HOME) rather than reach a model --
    the durable task/session storage is created by ``session/new`` before
    that point regardless."""
    import json as _json

    from acp_fakes import spawn_production_agent

    project = tmp_path / 'project'
    project.mkdir()

    proc = spawn_production_agent()
    try:
        def send(message):
            proc.stdin.write(_json.dumps(message) + '\n')
            proc.stdin.flush()

        def recv():
            return _json.loads(proc.stdout.readline())

        send(dict(jsonrpc='2.0', id=1, method='initialize',
                   params=dict(protocolVersion=1, clientCapabilities={},
                               clientInfo=dict(name='t', version='1'))))
        recv()
        send(dict(jsonrpc='2.0', id=2, method='session/new',
                   params=dict(cwd=str(project), mcpServers=[])))
        session_id = recv()['result']['sessionId']
        recv()  # available_commands_update notification

        send(dict(jsonrpc='2.0', id=3, method='session/prompt',
                   params=dict(sessionId=session_id, prompt=[dict(type='text', text='Build a page.')])))
        reply = recv()
        assert reply['id'] == 3
        # No connection/route is configured in this isolated HOME, so the
        # real production runtime must fail explicitly (never a hidden
        # provider/native fallback, never a real model call) -- the durable
        # store this test is checking was already created by session/new.
        assert 'error' in reply
    finally:
        proc.stdin.close()
        proc.wait(timeout=15)
    assert proc.returncode == 0

    sqlite_files, cortex_dirs, legacy_dirs = _walk_sqlite_and_cortex(tmp_path)
    assert sqlite_files == [project/'.cortex'/'cortex.sqlite3']
    assert cortex_dirs == [project/'.cortex']
    assert legacy_dirs == []
    assert not (project/'.codex').exists()


@pytest.mark.parametrize('runtime_access', [False, True])
def test_inspection_open_does_not_recreate_additive_schema_or_rows(tmp_path, runtime_access):
    store, task = bootstrap(tmp_path)
    with store.connection() as db:
        db.execute("DROP TABLE task_language")
        db.execute("DROP TABLE attempt_revisions")
    before = store.path.read_bytes()
    files_before = {p.name: p.read_bytes() for p in store.directory.iterdir() if p.is_file()}
    inspected = Store(store.directory, initialize=False, project_root=tmp_path, runtime_access=runtime_access)
    assert inspected.path.read_bytes() == before
    assert {p.name: p.read_bytes() for p in store.directory.iterdir() if p.is_file()} == files_before
    with inspected.connection(read_only=True) as db:
        assert db.execute("SELECT 1 FROM tasks WHERE id=?", (task.task_id,)).fetchone()
        assert db.execute("SELECT 1 FROM sqlite_master WHERE name='task_language'").fetchone() is None
        assert db.execute("SELECT 1 FROM sqlite_master WHERE name='attempt_revisions'").fetchone() is None
    assert store.path.read_bytes() == before
    # Only a normal writable runtime open repairs the optional schema.
    Store(store.directory, project_root=tmp_path)
    with store.connection() as db:
        assert db.execute("SELECT 1 FROM sqlite_master WHERE name='task_language'").fetchone()
        assert db.execute("SELECT 1 FROM sqlite_master WHERE name='attempt_revisions'").fetchone()


def test_inspection_open_does_not_create_missing_storage_directory(tmp_path):
    missing = tmp_path / "missing" / ".cortex"
    with pytest.raises(FileNotFoundError):
        Store(missing, initialize=False)
    assert not missing.parent.exists()


def test_read_only_constructor_connection_cannot_write(tmp_path):
    store, _task = bootstrap(tmp_path)
    with store.connection(read_only=True) as db:
        with pytest.raises(sqlite3.OperationalError, match="readonly"):
            db.execute("CREATE TABLE accidental_write(id)")


def test_inspection_rejects_uncheckpointed_wal_without_creating_sidecars(tmp_path):
    store, _task = bootstrap(tmp_path)
    # A raw external connection deliberately leaves committed WAL data live.
    db = sqlite3.connect(store.path)
    try:
        db.execute("CREATE TABLE uncheckpointed(id)")
        db.commit()
        before = {p.name: p.read_bytes() for p in store.directory.iterdir() if p.is_file()}
        with pytest.raises(StoreError, match="storage_inspection_requires_checkpoint"):
            Store(store.directory, initialize=False, project_root=tmp_path)
        assert {p.name: p.read_bytes() for p in store.directory.iterdir() if p.is_file()} == before
    finally:
        db.close()
    Store(store.directory, initialize=False, project_root=tmp_path)


@pytest.mark.parametrize('wal_write', [False, True])
def test_runtime_open_shares_held_connection_and_reads_current_wal(tmp_path, wal_write):
    store, _task = bootstrap(tmp_path)
    with store.connection() as held:
        if wal_write:
            held.execute("CREATE TABLE live_marker(value)")
            held.execute("INSERT INTO live_marker VALUES ('committed')")
            assert Path(str(store.path) + "-wal").stat().st_size > 0
        # A writer may hold BEGIN IMMEDIATE longer than the access-lock timeout.
        # The constructor only validates, so it must not wait for this writer.
        held.execute("BEGIN IMMEDIATE")
        with ThreadPoolExecutor(max_workers=1) as pool:
            opened = pool.submit(
                Store, store.directory, initialize=False, project_root=tmp_path,
                runtime_access=True,
            ).result(timeout=5)
        with opened.connection() as db:
            assert db.execute("PRAGMA user_version").fetchone()[0] == 12
            if wal_write:
                assert db.execute("SELECT value FROM live_marker").fetchone()[0] == 'committed'
        held.rollback()


def test_runtime_open_still_respects_exclusive_maintenance(tmp_path, monkeypatch):
    import cortex_runtime.store as store_module
    store, _task = bootstrap(tmp_path)
    monkeypatch.setattr(store_module, 'SHARED_ACCESS_WAIT_SECONDS', 0.01)
    with store.connection(exclusive=True):
        with pytest.raises(StoreError, match='storage_busy'):
            Store(store.directory, initialize=False, project_root=tmp_path, runtime_access=True)


@pytest.mark.parametrize('method', ['tools/list', 'tools/call'])
def test_gateway_opens_while_runtime_connection_holds_live_wal(tmp_path, method):
    from cortex_runtime.orchestration.state import TaskMachine
    store, task = bootstrap(tmp_path)
    TaskMachine(store).create(task.task_id, "Concurrent gateway open")
    server = server_for(tmp_path, task.channel_id, task.secret)
    with store.connection() as held:
        held.execute("CREATE TABLE live_marker(value)")
        # Keep the shared access lock and committed WAL alive through dispatch.
        reply = server.dispatch(method, {} if method == 'tools/list' else dict(
            name='list_reports', arguments={}))
    assert 'error' not in reply, reply
    assert not reply.get('isError'), reply


def test_production_runtime_starts_with_a_held_runtime_connection(tmp_path):
    from cortex_runtime.acp.production_runtime import ProductionOrchestratingRuntime
    store, _task = bootstrap(tmp_path)
    production = ProductionOrchestratingRuntime(environ={})
    with store.connection() as held:
        held.execute("CREATE TABLE live_marker(value)")
        runtime = production._runtime_for_project(str(tmp_path))
    assert runtime is production._runtime_for_project(str(tmp_path))


@pytest.mark.parametrize('runtime_access', [False, True])
@pytest.mark.parametrize('version', [10, 11, 99])
def test_non_initializing_modes_refuse_legacy_and_unknown_formats(tmp_path, runtime_access, version):
    store, _task = bootstrap(tmp_path)
    with store.connection() as db:
        db.execute(f"PRAGMA user_version={version}")
    before = {p.name: p.read_bytes() for p in store.directory.iterdir() if p.is_file()}
    error = 'legacy_storage_migration_required' if version == 11 else 'unsupported_storage'
    with pytest.raises(StoreError, match=error):
        Store(store.directory, initialize=False, project_root=tmp_path, runtime_access=runtime_access)
    assert {p.name: p.read_bytes() for p in store.directory.iterdir() if p.is_file()} == before
    assert not (store.directory / ".migration-backups").exists()
