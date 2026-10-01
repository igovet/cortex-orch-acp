"""Bounded, metadata-only inspection of an explicitly bound ACP task.

This is not an acceptance decision or a live-process probe. It never opens a
writable Store, initializes storage, reads report bodies or repairs state.
"""
from contextlib import contextmanager
import fcntl
import hashlib
import json
import os
from pathlib import Path
import re
import sqlite3
import stat
from urllib.parse import quote

from ..acp.sessions import get_session
from ..configuration.service import ConfigurationService
from ..contracts import StoreError
from ..project_storage import canonical_project
from ..store import APPLICATION_ID, regular

MAX_ROWS = 10000
MAX_SOURCE_BYTES = 32 * 1024 * 1024
MAX_METADATA_BYTES = 4 * 1024 * 1024


def _encode(value):
    return json.dumps(value, sort_keys=True, ensure_ascii=False, separators=(',', ':'))


def _digest(value):
    return hashlib.sha256(_encode(value).encode()).hexdigest()


class _SnapshotStore:
    """Read-only connection interface for existing public read APIs."""
    def __init__(self, db):
        self.db = db

    @contextmanager
    def connection(self):
        yield self.db


@contextmanager
def _snapshot(project):
    root = Path(canonical_project(project))
    directory = root / '.cortex'
    info = directory.lstat()
    if (not stat.S_ISDIR(info.st_mode) or info.st_uid != os.getuid()
            or stat.S_IMODE(info.st_mode) & 0o077):
        raise StoreError('unsafe_storage')
    path = directory / 'cortex.sqlite3'
    regular(path)
    for suffix in ('-wal', '-shm'):
        sidecar = Path(str(path) + suffix)
        if sidecar.exists() or sidecar.is_symlink():
            regular(sidecar)
    lock = directory / '.access.lock'
    regular(lock)
    fd = os.open(lock, os.O_RDONLY | os.O_NOFOLLOW)
    db = None
    try:
        # Freeze all cooperating Store connections, without creating anything.
        # Immutable reads do not create SHM/WAL, but must NEVER ignore a live
        # WAL. Require a checkpointed main database under the exclusive lock.
        fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        wal = Path(str(path) + '-wal')
        if wal.exists() and wal.stat().st_size:
            raise StoreError('inspection_uncheckpointed_wal')
        db = sqlite3.connect('file:' + quote(str(path), safe='/') + '?mode=ro&immutable=1', uri=True, timeout=5)
        db.row_factory = sqlite3.Row
        db.execute('PRAGMA query_only=ON')
        db.execute('BEGIN')
        version = db.execute('PRAGMA user_version').fetchone()[0]
        if db.execute('PRAGMA application_id').fetchone()[0] != APPLICATION_ID or version != 12:
            raise StoreError('legacy_storage_migration_required' if version == 11 else 'unsupported_storage')
        yield root, db, _SnapshotStore(db)
    finally:
        if db is not None:
            db.close()
        os.close(fd)


def _native(parameters):
    # Unknown native keys can contain private provider data; reveal only their
    # digest. The three route axes are bounded, exact, never clipped or guessed.
    result = {}
    for key in ('effort', 'thought_level', 'mode'):
        if key in parameters:
            value = parameters[key]
            if not isinstance(value, str) or len(value) > 256 or any(ord(c) < 32 for c in value):
                raise StoreError('inspection_unsafe_parameter')
            result[key] = value
    return result


def discover_binding(project, session_id):
    """Discover only exact session/task/current durable ownership metadata.

    All three identities come from one guarded immutable snapshot. This is
    not a latest-task lookup or evidence that a host/engine is still alive.
    """
    if (not isinstance(project, str) or not Path(project).is_absolute()
            or not isinstance(session_id, str)
            or not re.fullmatch(r'acps_[0-9a-f]{12}', session_id)):
        raise StoreError('inspection_invalid_binding')
    with _snapshot(project) as (root, db, _store):
        rows = db.execute('''SELECT s.project_root AS session_project, s.task_id,
                             t.project_root AS task_project, o.epoch
                             FROM acp_host_sessions s
                             JOIN tasks t ON t.id=s.task_id
                             JOIN task_owners o ON o.task_id=t.id
                             WHERE s.session_id=? LIMIT 2''', (session_id,)).fetchall()
        if len(rows) != 1:
            raise StoreError('inspection_binding_mismatch')
        row = rows[0]
        if (row['session_project'] != str(root) or row['task_project'] != str(root)
                or not re.fullmatch(r't_[0-9a-f]{12}', row['task_id'] or '')
                or type(row['epoch']) is not int or row['epoch'] < 1):
            raise StoreError('inspection_binding_mismatch')
        binding = dict(project_root=str(root), session_id=session_id,
                       task_id=row['task_id'], owner_epoch=row['epoch'])
        return dict(binding=binding, binding_sha256=_digest(binding),
                    provenance='guarded-immutable-session-task-current-owner')


def read_binding_page(project, session_id, task_id, owner_epoch, *, cursor=None, limit=100):
    """One consistent read snapshot; cursors fail closed on any projected drift.

    Caller must obtain the ACP session from the owned host's current runtimeInfo
    (the existing Paseo parsePaseoRuntimeSessionId/resolveAgentTask route). An
    arbitrary host agent label is deliberately not accepted as a binding.
    """
    if (not isinstance(project, str) or not Path(project).is_absolute()
            or not re.fullmatch(r'acps_[0-9a-f]{12}', session_id or '')
            or not re.fullmatch(r't_[0-9a-f]{12}', task_id or '')
            or type(owner_epoch) is not int or owner_epoch < 1
            or type(limit) is not int or not 1 <= limit <= 200):
        raise StoreError('inspection_invalid_binding')
    with _snapshot(project) as (root, db, store):
        session = get_session(store, session_id)
        task = db.execute('SELECT project_root FROM tasks WHERE id=?', (task_id,)).fetchone()
        owner = db.execute('SELECT epoch FROM task_owners WHERE task_id=?', (task_id,)).fetchone()
        if (session is None or session['task_id'] != task_id or session['project_root'] != str(root)
                or task is None or task[0] != str(root) or owner is None or owner[0] != owner_epoch):
            raise StoreError('inspection_binding_mismatch')
        rows = []
        # Bound even the existing history API before it materializes rows.
        # Oversized underlying evidence is a refusal, never a clipped digest.
        for table, column in (('configuration_revisions', 'parameters'), ('runtime_tasks', 'synthesis'),
                              ('task_events', 'payload_json'), ('runtime_events', 'payload_json'),
                              ('runtime_requests', 'result_json')):
            count, size = db.execute(f'SELECT COUNT(*),COALESCE(SUM(LENGTH(CAST({column} AS BLOB))),0) FROM {table} WHERE task_id=?', (task_id,)).fetchone()
            if count > MAX_ROWS or size > MAX_SOURCE_BYTES:
                raise StoreError('inspection_evidence_overflow')

        def select(kind, sql):
            values = db.execute(sql + ' LIMIT ?', (task_id, MAX_ROWS + 1)).fetchall()
            if len(values) > MAX_ROWS:
                raise StoreError('inspection_evidence_overflow')
            for row in values:
                rows.append(dict(section=kind, **dict(row)))

        # Existing public APIs preserve native configuration state labels and
        # confirmation references; never call current_or_latest an ACTIVE row.
        history = ConfigurationService(store).history(task_id)
        if len(history) > MAX_ROWS:
            raise StoreError('inspection_evidence_overflow')
        for revision in history:
            rows.append(dict(section='configuration', id=revision['id'], revision=revision['revision'],
                             state=revision['state'], route_id=revision['route_id'],
                             engine=revision['route_engine'], connection_id=revision['route_connection_id'],
                             parameters=_native(revision['parameters']),
                             parameters_sha256=_digest(revision['parameters']),
                             confirmation_evidence_ref=revision['confirmation_evidence_ref'], error_code=revision['error_code']))
        select('runtime', 'SELECT state,revision,review_cycle,updated_at FROM runtime_tasks WHERE task_id=?')
        runtime = db.execute('SELECT synthesis FROM runtime_tasks WHERE task_id=?', (task_id,)).fetchone()
        if runtime is not None and runtime['synthesis'] is not None:
            rows.append(dict(section='synthesis', text_sha256=_digest(runtime['synthesis'])))
        select('nodes', 'SELECT revision,node_id,role,stage,status,assignment_ref FROM runtime_nodes WHERE task_id=? ORDER BY revision,rowid')
        select('attempts', 'SELECT idempotency_key,attempt_id,assignment_id,role_id,status,step,owner_epoch,config_revision,engine_session_ref,error_code FROM delegation_provisioning WHERE task_id=? ORDER BY created_at,idempotency_key')
        select('dispatch', 'SELECT id,attempt_id,role,engine,connection_id,model,config_revision_id,decision,params_fingerprint FROM attempt_route_audit WHERE task_id=? ORDER BY rowid')
        select('process_observations', 'SELECT attempt_id,role,engine_session_ref,connection_id,recorded_at,terminated_at FROM attempt_process_registry WHERE task_id=? ORDER BY recorded_at,attempt_id')
        select('capacity', 'SELECT reservation_id,attempt_id,assignment_ref,kind,state,release_observation FROM capacity_reservations WHERE task_id=? ORDER BY reserved_at,reservation_id')
        select('reports', 'SELECT r.id,r.kind,e.sequence,e.digest,e.size_bytes FROM reports r JOIN editions e ON e.report_id=r.id WHERE r.task_id=? ORDER BY e.sequence')
        inputs = db.execute('SELECT origin,event_id,captured_at,ledger_state FROM source_events WHERE task_id=? ORDER BY captured_at,origin,event_id LIMIT ?', (task_id, MAX_ROWS + 1)).fetchall()
        if len(inputs) > MAX_ROWS:
            raise StoreError('inspection_evidence_overflow')
        for source in inputs:
            rows.append(dict(section='inputs', identity_sha256=_digest([source['origin'], source['event_id']]),
                             captured_at=source['captured_at'], ledger_state=source['ledger_state']))
        for table, type_key in (('task_events', 'event_type'), ('runtime_events', 'type')):
            values = db.execute(f'SELECT * FROM {table} WHERE task_id=? ORDER BY id LIMIT ?', (task_id, MAX_ROWS + 1)).fetchall()
            if len(values) > MAX_ROWS:
                raise StoreError('inspection_evidence_overflow')
            for row in values:
                payload = json.loads(row['payload_json'])
                item = dict(section=table, id=row['id'], type=row[type_key], payload_sha256=_digest(payload))
                if table == 'task_events':
                    item.update(assignment_ref=row['assignment_ref'], consumed_by=row['consumed_by'],
                                delivered_at=row['delivered_at'], superseded=bool(row['superseded']),
                                dedup_sha256=_digest(row['dedup_key']))
                # Only runtime-created identity fields, never body/title/text,
                # tool arguments, command output, private logs or credentials.
                if payload.get('kind') == 'worker_configuration_confirmed':
                    for key in ('kind', 'attempt_id', 'engine_session_ref', 'config_revision_id',
                                'engine', 'connection_id', 'model'):
                        item[key] = payload.get(key)
                    item['parameters'] = _native(payload['parameters'])
                    item['confirmation_evidence_ref'] = payload['confirmation_evidence_ref']
                elif payload.get('kind') == 'host_disconnected':
                    item['kind'] = 'host_disconnected'
                    item['history'] = payload.get('history')
                    item['unknown_actions_count'] = len(payload.get('unknown_outcome_actions', []))
                    item['interrupted_workers_count'] = len(payload.get('interrupted_workers', []))
                if row[type_key] in ('VERIFICATION_PASSED', 'VERIFICATION_FAILED'):
                    item['verification'] = row[type_key]
                    item['checks'] = [dict(exit_code=c.get('exit_code'), complete=c.get('complete'),
                                           argv_sha256=_digest(c.get('argv')), large_output_id=c.get('large_output_id'),
                                           revision_digest=c.get('revision_digest'), evidence_ref=c.get('tool_call_ref'))
                                      for c in payload.get('checks', [])]
                rows.append(item)
        receipts = db.execute('SELECT actor,request_key,operation,result_json FROM runtime_requests WHERE task_id=? ORDER BY rowid LIMIT ?', (task_id, MAX_ROWS + 1)).fetchall()
        if len(receipts) > MAX_ROWS:
            raise StoreError('inspection_evidence_overflow')
        for receipt in receipts:
            result = json.loads(receipt['result_json'])
            if receipt['operation'] in {'command_run', 'outside_command', 'outside_write', 'project_write',
                                        'project_mkdir', 'project_delete', 'project_skill_link', 'draft_write', 'external_call'}:
                rows.append(dict(section='action_receipts', identity_sha256=_digest([receipt['actor'], receipt['request_key']]),
                                 operation=receipt['operation'], tool_call_ref=result.get('tool_call_ref'),
                                 complete=result.get('complete'), exit_code=result.get('exit_code'),
                                 result_sha256=_digest(result)))
        if len(rows) > MAX_ROWS or len(_encode(rows).encode()) > MAX_METADATA_BYTES:
            raise StoreError('inspection_evidence_overflow')
        binding = dict(project_root=str(root), session_id=session_id, task_id=task_id, owner_epoch=owner_epoch)
        digest = _digest(dict(binding=binding, rows=rows))
        offset = 0
        if cursor is not None:
            match = re.fullmatch(r'([0-9a-f]{64}):([0-9]{1,5})', cursor)
            if match is None or match[1] != digest:
                raise StoreError('inspection_snapshot_changed')
            offset = int(match[2])
            if offset <= 0 or offset >= len(rows):
                raise StoreError('inspection_invalid_cursor')
        end = min(offset + limit, len(rows))
        return dict(binding=binding, snapshot_sha256=digest, total=len(rows), offset=offset,
                    rows=rows[offset:end], next_cursor=f'{digest}:{end}' if end < len(rows) else None,
                    complete=end == len(rows),
                    limits=['host identity must be independently observed; no host-agent assertion inferred',
                            'dispatch is not native confirmation; require ACTIVE or worker_configuration_confirmed',
                            'closed_at is not disconnect; require retained host_disconnected event',
                            'native resource/process liveness and SIGKILL are not probed',
                            'metadata hashes do not verify report body bytes or acceptance'])
