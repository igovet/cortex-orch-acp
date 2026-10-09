"""T032: offline format upgrade/rollback safety -- plan/migrate/rollback,
concurrency exclusion, crash recovery and preservation of everything the
migration tooling does not own.

Compatibility conclusion this file exercises directly (see
docs/project/format-migration.md): the only versioned schema change this
runtime has ever needed is format 10 -> 11 (T005); every addition since is
one independent, self-healing, all-or-nothing optional table group (see
split.py's group inventory) created by ``CREATE TABLE IF NOT EXISTS`` on
the next ``Store()`` open. ``plan()`` therefore reports a no-op for any
format-11 vintage, and only a genuine format-10 store needs ``migrate()``.
"""
from __future__ import annotations

import fcntl
import hashlib
import json
import os
from pathlib import Path
import sqlite3
import subprocess
import sys
import textwrap

import pytest

from cortex_runtime.contracts import StoreError
from cortex_runtime.migrate import APPLICATION_ID, migrate, plan, rollback
from cortex_runtime.store import Store

from storage_fixture import call_store
from test_markdown_store import create, write

RUNTIME_ROOT = str(Path(__file__).resolve().parents[1] / "src")


def _private_backup(tmp_path, name="backup.sqlite3"):
    directory = tmp_path / "backup-dir"
    directory.mkdir(mode=0o700, exist_ok=True)
    directory.chmod(0o700)
    return directory / name

# Same enumerated drop list the pre-existing offline-migration test in
# test_storage_v11.py uses to build an authentic pre-T005 format-10
# fixture (every table format 11 ever added, across every task since,
# minus whatever this exact runtime build still owes to a later optional
# group -- see that test's own comment for why the list is exhaustive
# for the tables format 10 could never have had).
_V11_ONLY_TABLES = (
    "binding_receipts", "source_revisions", "report_provenance", "draft_revisions",
    "task_changes", "source_turns", "source_resets", "trusted_channels", "source_events",
    "task_owners", "task_route_selections", "configuration_revisions",
    "project_configuration_defaults", "format_history", "attempt_revisions",
)


def _downgrade_to_v10(store):
    with store.connection() as db:
        for table in _V11_ONLY_TABLES:
            db.execute("DROP TABLE " + table)
        db.execute("PRAGMA user_version=10")


# --- plan(): read-only, no mutation --------------------------------------

def test_plan_reports_no_op_for_current_format_without_mutating(tmp_path):
    store = Store(tmp_path / "private")
    task = create(store, tmp_path)
    before = hashlib.sha256((tmp_path / "private" / "cortex.sqlite3").read_bytes()).hexdigest()
    result = plan(tmp_path / "private")
    assert result["action"] == "none" and result["current_version"] == 12
    after = hashlib.sha256((tmp_path / "private" / "cortex.sqlite3").read_bytes()).hexdigest()
    assert before == after


def test_plan_reports_exact_migration_for_format_10_without_mutating(tmp_path):
    store = Store(tmp_path / "private")
    create(store, tmp_path)
    _downgrade_to_v10(store)
    before = hashlib.sha256((tmp_path / "private" / "cortex.sqlite3").read_bytes()).hexdigest()
    result = plan(tmp_path / "private")
    assert result == dict(action="migrate", from_version=10, to_version=12,
                           tables_present=result["tables_present"],
                           note=result["note"])
    after = hashlib.sha256((tmp_path / "private" / "cortex.sqlite3").read_bytes()).hexdigest()
    assert before == after


def test_plan_refuses_unknown_format(tmp_path):
    store = Store(tmp_path / "private")
    create(store, tmp_path)
    with store.connection() as db:
        db.execute("PRAGMA user_version=999")
    with pytest.raises(StoreError, match="unsupported_storage"):
        plan(tmp_path / "private")


def test_migrate_format11_removes_legacy_hook_tables_preserves_acp_data(tmp_path):
    store = Store(tmp_path / "private")
    task = create(store, tmp_path)
    with store.connection() as db:
        db.executescript("""
        CREATE TABLE hook_events (
            task_id TEXT NOT NULL, thread_id TEXT NOT NULL, event_key TEXT PRIMARY KEY,
            event_name TEXT NOT NULL, metadata TEXT NOT NULL, created_at TEXT NOT NULL
        );
        CREATE TABLE hook_pending_sources (
            task_id TEXT NOT NULL, thread_id TEXT NOT NULL, turn_id TEXT NOT NULL,
            created_at TEXT NOT NULL, PRIMARY KEY(thread_id,turn_id)
        );
        CREATE TABLE hook_hints (
            task_id TEXT NOT NULL, thread_id TEXT NOT NULL, event_name TEXT NOT NULL,
            state_key TEXT NOT NULL, PRIMARY KEY(thread_id,event_name)
        );
        CREATE TABLE hook_agent_bindings (
            agent_id TEXT PRIMARY KEY, parent_thread_id TEXT NOT NULL, task_id TEXT NOT NULL,
            receipt TEXT NOT NULL UNIQUE, created_at TEXT NOT NULL
        );
        CREATE TABLE hook_command_sessions (
            task_id TEXT NOT NULL, worker_thread_id TEXT NOT NULL, parent_thread_id TEXT NOT NULL,
            command_session_id TEXT NOT NULL, cwd TEXT NOT NULL, created_at TEXT NOT NULL,
            PRIMARY KEY(task_id,command_session_id)
        );
        """)
        db.execute("PRAGMA user_version=11")
        db.commit()
    backup = tmp_path / "legacy-backup.sqlite3"
    result = migrate(store.directory, backup, access_stopped=True)
    assert result["from_version"] == 11 and result["to_version"] == 12
    reopened = Store(store.directory, initialize=True)
    with reopened.connection() as db:
        names = {row[0] for row in db.execute("SELECT name FROM sqlite_master WHERE type='table'")}
        assert not names.intersection({"hook_events", "hook_pending_sources", "hook_hints", "hook_agent_bindings", "hook_command_sessions"})
        assert db.execute("SELECT 1 FROM tasks WHERE id=?", (task,)).fetchone()
        assert db.execute("PRAGMA integrity_check").fetchone()[0] == "ok"
    assert backup.exists()


def test_store_open_automatically_migrates_format11_and_keeps_verified_backup(tmp_path):
    store = Store(tmp_path / "private")
    with store.connection() as db:
        db.execute("CREATE TABLE hook_events (event_key TEXT PRIMARY KEY)")
        db.execute("PRAGMA user_version=11")
        db.commit()

    reopened = Store(store.directory, initialize=True)
    with reopened.connection() as db:
        assert db.execute("PRAGMA user_version").fetchone()[0] == 12
        assert db.execute("SELECT name FROM sqlite_master WHERE name='hook_events'").fetchone() is None
        assert db.execute("PRAGMA integrity_check").fetchone()[0] == "ok"
    backups = list((store.directory / ".migration-backups").glob("cortex.sqlite3.format11-*.bak"))
    assert len(backups) == 1
    # A second open is a format-12 no-op and does not create another backup.
    Store(store.directory, initialize=True)
    assert list((store.directory / ".migration-backups").glob("cortex.sqlite3.format11-*.bak")) == backups


def test_concurrent_auto_migration_accepts_post_probe_v12(monkeypatch, tmp_path):
    """A peer may commit v12 after this opener's v11 probe but before migrate()."""
    store = Store(tmp_path / "private")
    with store.connection() as db:
        db.execute("PRAGMA user_version=11")
        db.commit()

    import cortex_runtime.migrate as migration
    original = migration.migrate

    def peer_finished(directory, backup, *, access_stopped=False):
        with sqlite3.connect(Path(directory) / "cortex.sqlite3") as db:
            db.execute("PRAGMA user_version=12")
            db.commit()
        raise StoreError("unsupported_storage")

    monkeypatch.setattr(migration, "migrate", peer_finished)
    assert migration.auto_migrate_format11(store.directory) is None
    monkeypatch.setattr(migration, "migrate", original)


def test_live_format_probe_retries_transient_sqlite_lock(monkeypatch, tmp_path):
    """A short SQLite lock during concurrent Store startup is recoverable."""
    store = Store(tmp_path / "private")
    import cortex_runtime.migrate as migration
    real_connect = migration.sqlite3.connect
    calls = 0

    def flaky_connect(*args, **kwargs):
        nonlocal calls
        calls += 1
        if calls <= 2:
            raise sqlite3.OperationalError("database is locked")
        return real_connect(*args, **kwargs)

    monkeypatch.setattr(migration.sqlite3, "connect", flaky_connect)
    assert migration._read_live_header(store.directory / "cortex.sqlite3") == (APPLICATION_ID, 12)
    assert calls == 3


def test_live_format_probe_fails_closed_on_persistent_sqlite_lock(monkeypatch, tmp_path):
    store = Store(tmp_path / "private")
    import cortex_runtime.migrate as migration

    def always_locked(*args, **kwargs):
        raise sqlite3.OperationalError("database is locked")

    monkeypatch.setattr(migration.sqlite3, "connect", always_locked)
    with pytest.raises(StoreError, match="storage_busy"):
        migration._read_live_header(store.directory / "cortex.sqlite3")


def test_migration_backup_includes_committed_wal_rows(monkeypatch, tmp_path):
    """The logical backup must include committed rows still held in a WAL."""
    store = Store(tmp_path / "private")
    with store.connection() as db:
        db.execute("CREATE TABLE hook_events (event_key TEXT PRIMARY KEY, payload TEXT)")
        db.execute("PRAGMA user_version=11")
        db.commit()
    source = store.directory / "cortex.sqlite3"
    keeper = sqlite3.connect(source, isolation_level=None)
    keeper.execute("PRAGMA journal_mode=WAL")
    keeper.execute("INSERT INTO hook_events VALUES ('wal-row', 'committed')")
    backup = tmp_path / "wal-backup.sqlite3"
    from cortex_runtime import _fault_injection
    original_checkpoint = _fault_injection.checkpoint

    def after_backup(name):
        if name == "migrate_after_backup_before_schema":
            with sqlite3.connect(backup) as check:
                assert check.execute("SELECT payload FROM hook_events WHERE event_key='wal-row'").fetchone() == ("committed",)
            keeper.close()
            raise RuntimeError("stop-after-backup")
        return original_checkpoint(name)

    monkeypatch.setattr(_fault_injection, "checkpoint", after_backup)
    with pytest.raises(RuntimeError, match="stop-after-backup"):
        migrate(store.directory, backup, access_stopped=True)


def test_automatic_migration_failure_leaves_format11_for_retry(tmp_path, monkeypatch):
    store = Store(tmp_path / "private")
    with store.connection() as db:
        db.execute("CREATE TABLE hook_events (event_key TEXT PRIMARY KEY)")
        db.execute("PRAGMA user_version=11")
        db.commit()

    from cortex_runtime import _fault_injection
    original = _fault_injection.checkpoint
    monkeypatch.setattr(_fault_injection, "checkpoint",
                        lambda name: (_ for _ in ()).throw(RuntimeError("injected"))
                        if name == "migrate_after_backup_before_schema" else original(name))
    with pytest.raises(RuntimeError, match="injected"):
        Store(store.directory, initialize=True)
    check = sqlite3.connect(store.directory / "cortex.sqlite3")
    try:
        assert check.execute("PRAGMA user_version").fetchone()[0] == 11
    finally:
        check.close()
    assert list((store.directory / ".migration-backups").glob("cortex.sqlite3.format11-*.bak"))
    monkeypatch.setattr(_fault_injection, "checkpoint", original)
    Store(store.directory, initialize=True)
    check = sqlite3.connect(store.directory / "cortex.sqlite3")
    try:
        assert check.execute("PRAGMA user_version").fetchone()[0] == 12
    finally:
        check.close()


def test_non_initializing_open_refuses_format11_without_mutating(tmp_path):
    store = Store(tmp_path / "private")
    with store.connection() as db:
        db.execute("PRAGMA user_version=11")
        db.commit()
    with pytest.raises(StoreError, match="legacy_storage_migration_required"):
        Store(store.directory, initialize=False)
    assert not (store.directory / ".migration-backups").exists()


def test_plan_no_store_yet_is_a_no_op(tmp_path):
    result = plan(tmp_path / "private")
    assert result["action"] == "none"


def _snapshot(directory):
    db = directory / "cortex.sqlite3"
    names = sorted(p.name for p in directory.iterdir())
    digests = {}
    for suffix in ("", "-wal", "-shm"):
        path = Path(str(db) + suffix)
        if path.exists():
            digests[path.name] = hashlib.sha256(path.read_bytes()).hexdigest()
    return names, digests


def test_plan_never_creates_wal_or_shm_sidecars_when_absent(tmp_path):
    """T032 fresh-review fix: mode=ro alone lets SQLite create -wal/-shm on
    open when neither exists; plan() must be truly inert (immutable=1)."""
    store = Store(tmp_path / "private")
    create(store, tmp_path)
    with store.connection() as db:
        db.execute("PRAGMA wal_checkpoint(TRUNCATE)")
    directory = tmp_path / "private"
    wal = Path(str(directory / "cortex.sqlite3") + "-wal")
    shm = Path(str(directory / "cortex.sqlite3") + "-shm")
    assert not wal.exists() and not shm.exists()
    before = _snapshot(directory)
    plan(directory)
    after = _snapshot(directory)
    assert before == after
    assert not wal.exists() and not shm.exists()


def test_plan_leaves_pre_existing_wal_and_shm_sidecars_unchanged(tmp_path):
    store = Store(tmp_path / "private")
    create(store, tmp_path)
    directory = tmp_path / "private"
    with store.connection() as db:
        db.execute("CREATE TABLE IF NOT EXISTS zzz_plan_probe(id INTEGER)")
        db.execute("INSERT INTO zzz_plan_probe VALUES (1)")
        wal = Path(str(directory / "cortex.sqlite3") + "-wal")
        shm = Path(str(directory / "cortex.sqlite3") + "-shm")
        assert wal.exists() and shm.exists()
        before = _snapshot(directory)
        result = plan(directory)
        after = _snapshot(directory)
    assert result["action"] == "none"
    assert before == after


def test_plan_succeeds_against_a_read_only_parent_directory(tmp_path):
    store = Store(tmp_path / "private")
    create(store, tmp_path)
    directory = tmp_path / "private"
    with store.connection() as db:
        db.execute("PRAGMA wal_checkpoint(TRUNCATE)")
    mode = directory.stat().st_mode
    os.chmod(directory, 0o500)
    try:
        result = plan(directory)
    finally:
        os.chmod(directory, mode)
    assert result["action"] == "none"


# --- migrate(): backup, atomic apply, format_history -----------------------

def test_migrate_records_format_history_and_backup_digest(tmp_path):
    store = Store(tmp_path / "private")
    create(store, tmp_path)
    _downgrade_to_v10(store)
    backup = _private_backup(tmp_path)
    result = migrate(tmp_path / "private", backup, access_stopped=True)
    expected_digest = hashlib.sha256(backup.read_bytes()).hexdigest()
    assert result["backup_digest"] == expected_digest
    with sqlite3.connect(tmp_path / "private" / "cortex.sqlite3") as raw:
        row = raw.execute(
            "SELECT action,from_version,to_version,backup_path,backup_digest FROM format_history"
        ).fetchone()
    assert row == ("migrate", 10, 12, str(backup), expected_digest)


# --- rollback(): verified restore only, never a second root ----------------

def test_rollback_restores_verified_backup_byte_for_byte(tmp_path):
    store = Store(tmp_path / "private")
    create(store, tmp_path)
    _downgrade_to_v10(store)
    backup_dir = tmp_path / "backup-dir"
    backup_dir.mkdir(mode=0o700)
    backup = backup_dir / "backup.sqlite3"
    migrate(tmp_path / "private", backup, access_stopped=True)
    backup_bytes = backup.read_bytes()
    result = rollback(tmp_path / "private", backup, access_stopped=True, confirm=True)
    assert result["restored_version"] == 10
    assert (tmp_path / "private" / "cortex.sqlite3").read_bytes() == backup_bytes
    # never recreates a second storage root, never deletes .cortex
    assert (tmp_path / "private").is_dir()
    log = json.loads((tmp_path / "private" / "rollback.log").read_text().splitlines()[-1])
    assert log["restored_from"] == str(backup)


def test_rollback_requires_access_stopped_and_explicit_confirm(tmp_path):
    store = Store(tmp_path / "private")
    create(store, tmp_path)
    _downgrade_to_v10(store)
    backup_dir = tmp_path / "backup-dir"
    backup_dir.mkdir(mode=0o700)
    backup = backup_dir / "backup.sqlite3"
    migrate(tmp_path / "private", backup, access_stopped=True)
    with pytest.raises(StoreError, match="migration_requires_stopped_access"):
        rollback(tmp_path / "private", backup, confirm=True)
    with pytest.raises(StoreError, match="rollback_requires_confirmation"):
        rollback(tmp_path / "private", backup, access_stopped=True)
    # a dry (unconfirmed) attempt never mutates the live store
    with sqlite3.connect(tmp_path / "private" / "cortex.sqlite3") as raw:
        assert raw.execute("PRAGMA user_version").fetchone()[0] == 12


def test_rollback_refuses_corrupt_backup(tmp_path):
    store = Store(tmp_path / "private")
    create(store, tmp_path)
    backup_dir = tmp_path / "backup-dir"
    backup_dir.mkdir(mode=0o700)
    garbage = backup_dir / "garbage.sqlite3"
    garbage.write_bytes(b"not a sqlite database")
    garbage.chmod(0o600)
    with pytest.raises((StoreError, sqlite3.DatabaseError)):
        rollback(tmp_path / "private", garbage, access_stopped=True, confirm=True)
    with sqlite3.connect(tmp_path / "private" / "cortex.sqlite3") as raw:
        assert raw.execute("PRAGMA user_version").fetchone()[0] == 12


def test_rollback_requires_private_backup_parent(tmp_path):
    store = Store(tmp_path / "private")
    create(store, tmp_path)
    _downgrade_to_v10(store)
    backup = tmp_path / "shared-backup" / "backup.sqlite3"
    backup.parent.mkdir(mode=0o755)
    # ``mkdir(mode=...)`` is itself masked by the ambient umask (0o755 under
    # a 0o077 umask silently becomes 0o700, the very thing this test needs
    # to *not* be true) -- chmod sets the exact bits regardless of umask.
    backup.parent.chmod(0o755)
    migrate(tmp_path / "private", backup, access_stopped=True)
    with pytest.raises(StoreError, match="unsafe_storage"):
        rollback(tmp_path / "private", backup, access_stopped=True, confirm=True)


def test_rollback_refuses_backup_from_a_foreign_application(tmp_path):
    store = Store(tmp_path / "private")
    create(store, tmp_path)
    backup_dir = tmp_path / "backup-dir"
    backup_dir.mkdir(mode=0o700)
    foreign = backup_dir / "foreign.sqlite3"
    with sqlite3.connect(foreign) as raw:
        raw.execute("CREATE TABLE unrelated (id INTEGER)")
        raw.execute("PRAGMA application_id=42")
        raw.execute("PRAGMA user_version=11")
    os.chmod(foreign, 0o600)
    with pytest.raises(StoreError, match="rollback_backup_incompatible"):
        rollback(tmp_path / "private", foreign, access_stopped=True, confirm=True)


def test_rollback_refuses_an_unknown_newer_format(tmp_path):
    store = Store(tmp_path / "private")
    create(store, tmp_path)
    backup_dir = tmp_path / "backup-dir"
    backup_dir.mkdir(mode=0o700)
    newer = backup_dir / "newer.sqlite3"
    with sqlite3.connect(store.directory / "cortex.sqlite3") as source:
        target = sqlite3.connect(newer)
        source.backup(target)
        target.execute("PRAGMA user_version=13")
        target.commit(); target.close()
    os.chmod(newer, 0o600)
    with pytest.raises(StoreError, match="rollback_backup_incompatible"):
        rollback(tmp_path / "private", newer, access_stopped=True, confirm=True)


# --- concurrency: no overlapping mutating access ----------------------------

def test_migrate_refused_while_a_store_connection_is_open(tmp_path):
    store = Store(tmp_path / "private")
    create(store, tmp_path)
    _downgrade_to_v10(store)
    with store.connection():
        with pytest.raises(StoreError, match="storage_busy"):
            migrate(tmp_path / "private", tmp_path / "backup.sqlite3", access_stopped=True)


def test_rollback_refused_while_a_store_connection_is_open(tmp_path):
    store = Store(tmp_path / "private")
    create(store, tmp_path)
    _downgrade_to_v10(store)
    backup = _private_backup(tmp_path)
    migrate(tmp_path / "private", backup, access_stopped=True)
    with store.connection():
        with pytest.raises(StoreError, match="storage_busy"):
            rollback(tmp_path / "private", backup, access_stopped=True, confirm=True)


def test_second_mutating_migration_process_is_refused_while_first_holds_the_lock(tmp_path):
    """A second migrate/rollback (old or new runtime) can never run
    concurrently against the same project -- proven with a real held OS
    file lock, the same primitive Store.connection() itself takes."""
    store = Store(tmp_path / "private")
    create(store, tmp_path)
    _downgrade_to_v10(store)
    lock_path = tmp_path / "private" / ".access.lock"
    holder = os.open(lock_path, os.O_CREAT | os.O_RDWR, 0o600)
    fcntl.flock(holder, fcntl.LOCK_EX)
    try:
        with pytest.raises(StoreError, match="storage_busy"):
            migrate(tmp_path / "private", tmp_path / "backup.sqlite3", access_stopped=True)
    finally:
        fcntl.flock(holder, fcntl.LOCK_UN)
        os.close(holder)


# --- preservation: everything migrate/rollback do not own ------------------

def test_migrate_preserves_reports_drafts_and_every_row_it_does_not_touch(tmp_path):
    """A workspace_snapshots row (T025, a v11-only optional group migrate()
    never imports or drops) must survive migrate() untouched, exactly like
    the report/draft rows the pre-existing offline-migration test already
    covers -- migrate() only ever adds rows to the tables it explicitly
    schemas in, never touches anything else."""
    store = Store(tmp_path / "private")
    task = create(store, tmp_path)
    write(store, task, "Preserved report body.")
    with store.connection() as db:
        db.execute(
            "INSERT INTO workspace_snapshots(workspace_binding_id,task_id,attempt_id,project_root,"
            "mode,manifest_json,base_kind,created_at) VALUES (?,?,?,?,?,?,?,strftime('%Y-%m-%dT%H:%M:%fZ','now'))",
            ("wb-planted", task, "attempt-planted", str(tmp_path), "isolated", "{}", "clean"),
        )
    task_dir = tmp_path / "private" / task
    markdown_before = {p.name: p.read_bytes() for p in task_dir.glob("*.md")}
    _downgrade_to_v10(store)
    backup = _private_backup(tmp_path)
    migrate(tmp_path / "private", backup, access_stopped=True)
    markdown_after = {p.name: p.read_bytes() for p in task_dir.glob("*.md")}
    assert markdown_before == markdown_after
    raw = sqlite3.connect(tmp_path / "private" / "cortex.sqlite3")
    assert raw.execute(
        "SELECT project_root FROM workspace_snapshots WHERE workspace_binding_id='wb-planted'"
    ).fetchone() == (str(tmp_path),)
    raw.close()
    upgraded = Store(tmp_path / "private")
    assert call_store(upgraded, "list_reports", dict(task_id=task))["reports"]


def test_rollback_never_touches_markdown_engine_state_or_credentials(tmp_path):
    """Rollback only ever replaces cortex.sqlite3 (+sidecars) inside the
    one storage directory it is given -- proven by planting sentinels
    representing the project's own draft/report Markdown plus the
    completely separate connection/credential/engine-state roots
    (see connections/registry.py: $XDG_CONFIG_HOME/cortex,
    $XDG_STATE_HOME/cortex) and asserting every one is byte-identical
    after a real rollback."""
    store = Store(tmp_path / "private")
    task = create(store, tmp_path)
    write(store, task, "Report that must survive rollback on disk.")
    task_dir = tmp_path / "private" / task
    markdown_before = {p.name: p.read_bytes() for p in task_dir.glob("*.md")}

    config_root = tmp_path / "config-root" / "cortex"
    config_root.mkdir(parents=True)
    credential = config_root / "secret.json"
    credential.write_text('{"api_key":"fake-not-real"}')
    state_root = tmp_path / "state-root" / "cortex" / "conn-1" / "codex-acp"
    state_root.mkdir(parents=True)
    engine_install = state_root / "auth.json"
    engine_install.write_text('{"engine":"planted"}')
    credential_digest_before = hashlib.sha256(credential.read_bytes()).hexdigest()
    engine_digest_before = hashlib.sha256(engine_install.read_bytes()).hexdigest()

    _downgrade_to_v10(store)
    backup = _private_backup(tmp_path)
    migrate(tmp_path / "private", backup, access_stopped=True)
    rollback(tmp_path / "private", backup, access_stopped=True, confirm=True)

    assert {p.name: p.read_bytes() for p in task_dir.glob("*.md")} == markdown_before
    assert hashlib.sha256(credential.read_bytes()).hexdigest() == credential_digest_before
    assert hashlib.sha256(engine_install.read_bytes()).hexdigest() == engine_digest_before
    assert (tmp_path / "private").is_dir()  # .cortex itself is never deleted


# --- crash matrix: os._exit(137) at each documented checkpoint -------------

_MIGRATE_CHILD = textwrap.dedent(
    """
    import sys
    sys.path.insert(0, {runtime_root!r})
    from cortex_runtime.migrate import migrate
    migrate({directory!r}, {backup!r}, access_stopped=True)
    print("completed-without-crash")
    """
)

_ROLLBACK_CHILD = textwrap.dedent(
    """
    import sys
    sys.path.insert(0, {runtime_root!r})
    from cortex_runtime.migrate import rollback
    rollback({directory!r}, {backup!r}, access_stopped=True, confirm=True)
    print("completed-without-crash")
    """
)


def _run_crashing(template, directory, backup, checkpoint):
    script = template.format(runtime_root=RUNTIME_ROOT, directory=str(directory), backup=str(backup))
    env = dict(os.environ); env["CORTEX_FAULT_INJECTION_POINT"] = checkpoint
    return subprocess.run([sys.executable, "-c", script], env=env, capture_output=True, text=True, timeout=30)


@pytest.mark.parametrize("checkpoint", [
    "migrate_before_backup",
    "migrate_after_backup_before_schema",
    "migrate_after_schema_before_commit",
])
def test_migrate_crash_at_each_checkpoint_leaves_a_reconcilable_v10_store(tmp_path, checkpoint):
    store = Store(tmp_path / "private")
    task = create(store, tmp_path)
    write(store, task, "Pre-migration report.")
    _downgrade_to_v10(store)
    backup = _private_backup(tmp_path)

    result = _run_crashing(_MIGRATE_CHILD, tmp_path / "private", backup, checkpoint)
    assert result.returncode != 0 and "completed-without-crash" not in result.stdout

    lock_path = tmp_path / "private" / ".access.lock"
    probe = os.open(lock_path, os.O_RDWR)
    try:
        fcntl.flock(probe, fcntl.LOCK_EX | fcntl.LOCK_NB)  # never left held by the dead child
    finally:
        fcntl.flock(probe, fcntl.LOCK_UN); os.close(probe)

    raw = sqlite3.connect(tmp_path / "private" / "cortex.sqlite3")
    version = raw.execute("PRAGMA user_version").fetchone()[0]
    assert raw.execute("PRAGMA integrity_check").fetchone()[0] == "ok"
    raw.close()
    # Never a half-applied format: either still cleanly 10 (crash before the
    # final commit -- the whole schema+backfill transaction auto-rolled
    # back) or already cleanly 12 with a recorded format_history row.
    assert version in (10, 12)
    if version == 10:
        # A fresh attempt with a new backup path fully recovers.
        fresh_backup = tmp_path / "backup-retry.sqlite3"
        retry = migrate(tmp_path / "private", fresh_backup, access_stopped=True)
        assert retry["to_version"] == 12
    else:
        raw = sqlite3.connect(tmp_path / "private" / "cortex.sqlite3")
        assert raw.execute("SELECT COUNT(*) FROM format_history").fetchone()[0] == 1
        raw.close()
    upgraded = Store(tmp_path / "private", initialize=False)
    assert call_store(upgraded, "list_reports", dict(task_id=task))["reports"]


@pytest.mark.parametrize("checkpoint", [
    "rollback_before_restore",
    "rollback_after_temp_write",
    "rollback_after_replace_before_confirm",
])
def test_rollback_crash_at_each_checkpoint_leaves_a_reconcilable_store(tmp_path, checkpoint):
    store = Store(tmp_path / "private")
    task = create(store, tmp_path)
    write(store, task, "Pre-rollback report.")
    _downgrade_to_v10(store)
    backup = _private_backup(tmp_path)
    migrate(tmp_path / "private", backup, access_stopped=True)
    backup_bytes = backup.read_bytes()

    result = _run_crashing(_ROLLBACK_CHILD, tmp_path / "private", backup, checkpoint)
    assert result.returncode != 0 and "completed-without-crash" not in result.stdout

    lock_path = tmp_path / "private" / ".access.lock"
    probe = os.open(lock_path, os.O_RDWR)
    try:
        fcntl.flock(probe, fcntl.LOCK_EX | fcntl.LOCK_NB)
    finally:
        fcntl.flock(probe, fcntl.LOCK_UN); os.close(probe)

    db_path = tmp_path / "private" / "cortex.sqlite3"
    # Either still the pre-rollback (post-migrate, v12) content, or already
    # the fully-restored v10 backup -- never a partially-written file: a
    # stale ".rollback-*.tmp" (crash before the atomic replace) never
    # reaches the live path at all.
    raw = sqlite3.connect(db_path)
    assert raw.execute("PRAGMA integrity_check").fetchone()[0] == "ok"
    version = raw.execute("PRAGMA user_version").fetchone()[0]
    raw.close()
    assert version in (10, 12)
    if version == 12:
        assert db_path.read_bytes() != backup_bytes
        retry = rollback(tmp_path / "private", backup, access_stopped=True, confirm=True)
        assert retry["restored_version"] == 10
        assert db_path.read_bytes() == backup_bytes
    else:
        assert db_path.read_bytes() == backup_bytes
    # A stray temp file from a crash between the write and the replace is
    # cleaned up by the next attempt, never mistaken for live data.
    assert not list((tmp_path / "private").glob(".rollback-*.tmp"))
