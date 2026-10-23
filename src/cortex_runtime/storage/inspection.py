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


_CODE = re.compile(r'[A-Za-z_][A-Za-z0-9_.:-]{0,127}')
_RECEIPT_REF = re.compile(r'gateway:[0-9a-f]{32}')
_CHECK_OUTCOMES = {'passed', 'failed', 'not_run', 'unverified', 'retracted'}


def _code(value):
    """A runtime code or enum value; any other string is reported, not shown."""
    if value is None or isinstance(value, bool):
        return value
    return value if isinstance(value, str) and _CODE.fullmatch(value) else 'unsafe_value'


def _ref(value):
    # Model-authored check fields can hold arbitrary text: only an exact
    # gateway receipt shape is shown, anything else is marked unrecognized.
    if value is None:
        return None
    return value if isinstance(value, str) and _RECEIPT_REF.fullmatch(value) else 'unrecognized'


def _int(value):
    return value if type(value) is int else None


def _enum(value, allowed=_CHECK_OUTCOMES):
    if value is None:
        return None
    return value if isinstance(value, str) and value in allowed else 'unsafe_value'


def _route_decision(detail):
    """Allowlisted route-decision metadata; never candidates, usage or reasons."""
    if not isinstance(detail, dict):
        return {}
    decision = detail.get('route_decision') if isinstance(detail.get('route_decision'), dict) else {}
    request = decision.get('user_model_request') if isinstance(decision.get('user_model_request'), dict) else {}
    projected = dict(route_basis=_code(detail.get('route_basis')), effort=_code(detail.get('effort')),
                     effort_source=_code(detail.get('effort_source')))
    for key in ('basis', 'decided_by', 'role_tier', 'tier', 'preference', 'preference_status',
                'model_hint_status', 'cross_provider_review'):
        projected[key] = _code(decision.get(key))
    projected['chosen'] = decision.get('chosen') if isinstance(decision.get('chosen'), str) and re.fullmatch(
        r'[a-z0-9-]{1,64}/[A-Za-z0-9_.\[\]-]{1,128}', decision.get('chosen')) else _code(decision.get('chosen'))
    projected['user_model_request_status'] = _code(request.get('status'))
    projected['pin_effort_clamped'] = isinstance(decision.get('pin_effort_clamped'), dict)
    return projected


def _event_detail(kind, payload, channels, actor):
    """Runtime-created identity/outcome fields of one runtime event."""
    item = {}
    if kind in ('TOOL_AUTHORIZED', 'TOOL_COMPLETED', 'TOOL_REJECTED', 'CHECK_FAILURE_PRESENTED'):
        attempt, epoch = channels.get(actor, (None, None))
        item.update(operation=_code(payload.get('operation')), tool_call_ref=_ref(payload.get('tool_call_ref')),
                    channel_attempt_id=attempt, channel_owner_epoch=epoch)
        if kind == 'TOOL_REJECTED':
            item['error'] = _code(payload.get('error'))
        if kind == 'CHECK_FAILURE_PRESENTED':
            item['attempt_id'] = _code(payload.get('attempt_id'))
    elif kind in ('WORKER_COMPLETED', 'WORKER_FAILED'):
        result = payload.get('result') if isinstance(payload.get('result'), dict) else {}
        checks = result.get('checks') if isinstance(result.get('checks'), list) else []
        item.update(attempt_id=_code(payload.get('attempt_id')), node=_code(payload.get('node')),
                    verdict=_enum(result.get('verdict'), {'APPROVE', 'REQUEST_CHANGES', 'BLOCK'}),
                    project_digest=result.get('project_digest') if isinstance(result.get('project_digest'), str) and re.fullmatch(r'[0-9a-f]{64}', result['project_digest']) else None,
                    assignment_outcome=_enum(result.get('assignment_outcome'), {'failed'}),
                    checks=[dict(name_sha256=_digest(c.get('name')),
                                 outcome=_enum(c.get('outcome')), claimed_outcome=_enum(c.get('claimed_outcome')),
                                 evidence_gap=_code(c.get('evidence_gap')), tool_call_ref=_ref(c.get('tool_call_ref')),
                                 closed_by=_ref(c.get('closed_by')), retracted_by=_ref(c.get('retracted_by')))
                            for c in checks if isinstance(c, dict)],
                    checks_count=len(checks))
    elif kind in ('REVIEW_NOT_REQUIRED', 'EXECUTION_BASELINE'):
        item.update(revision=_int(payload.get('revision')), reason=_code(payload.get('reason')),
                    reference=_code(payload.get('reference')),
                    reference_revision=_int(payload.get('reference_revision')),
                    kind=_code(payload.get('kind')))
        for key in ('revision_digest', 'head'):
            value = payload.get(key)
            item[key] = value if isinstance(value, str) and re.fullmatch(r'[0-9a-f]{40,64}', value) else None
    elif kind == 'USER_QUESTION_REQUESTED':
        # Which kind of question and where it resumes; never its text/options.
        options = payload.get('options')
        questions = payload.get('questions')
        item.update(question_id=_code(payload.get('question_id')), batch_id=_code(payload.get('batch_id')),
                    runtime_stop=payload.get('runtime_stop') is True, after_verification=payload.get('after_verification') is True,
                    resume_state=_code(payload.get('resume_state')), revision=_int(payload.get('revision')),
                    blocker=_code(payload.get('blocker')),
                    options_count=len(options) if isinstance(options, list) else None,
                    questions_count=len(questions) if isinstance(questions, list) else None)
    elif kind in ('USER_STEERING', 'STEERING_APPLIED'):
        # The runtime pairs these by actor; expose only the pairing identity.
        item['actor_sha256'] = _digest(actor)
    return item


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
                              ('runtime_requests', 'result_json'), ('attempt_route_audit', 'decision_detail_json'),
                              ('observability_events', 'detail_json')):
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
        dispatch = db.execute('SELECT id,attempt_id,role,engine,connection_id,model,config_revision_id,decision,params_fingerprint,'
                              'decision_detail_json FROM attempt_route_audit WHERE task_id=? ORDER BY rowid LIMIT ?',
                              (task_id, MAX_ROWS + 1)).fetchall()
        if len(dispatch) > MAX_ROWS:
            raise StoreError('inspection_evidence_overflow')
        for audit in dispatch:
            values = dict(audit)
            detail = json.loads(values.pop('decision_detail_json') or '{}')
            rows.append(dict(section='dispatch', **values, route=_route_decision(detail)))
        select('process_observations', 'SELECT attempt_id,role,engine_session_ref,connection_id,recorded_at,terminated_at FROM attempt_process_registry WHERE task_id=? ORDER BY recorded_at,attempt_id')
        select('capacity', 'SELECT reservation_id,attempt_id,assignment_ref,kind,state,release_observation FROM capacity_reservations WHERE task_id=? ORDER BY reserved_at,reservation_id')
        select('reports', 'SELECT r.id,r.kind,e.sequence,e.digest,e.size_bytes FROM reports r JOIN editions e ON e.report_id=r.id WHERE r.task_id=? ORDER BY e.sequence')
        inputs = db.execute('SELECT origin,event_id,captured_at,ledger_state FROM source_events WHERE task_id=? ORDER BY captured_at,origin,event_id LIMIT ?', (task_id, MAX_ROWS + 1)).fetchall()
        if len(inputs) > MAX_ROWS:
            raise StoreError('inspection_evidence_overflow')
        for source in inputs:
            rows.append(dict(section='inputs', identity_sha256=_digest([source['origin'], source['event_id']]),
                             captured_at=source['captured_at'], ledger_state=source['ledger_state']))
        channel_rows = db.execute('SELECT channel_id,attempt_id,owner_epoch FROM trusted_channels WHERE task_id=? LIMIT ?',
                                  (task_id, MAX_ROWS + 1)).fetchall()
        if len(channel_rows) > MAX_ROWS:
            raise StoreError('inspection_evidence_overflow')
        channels = {c['channel_id']: (c['attempt_id'], c['owner_epoch']) for c in channel_rows}
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
                if table == 'runtime_events':
                    item.update(_event_detail(row[type_key], payload, channels, row['actor']))
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
        outputs = {o['id']: o for o in db.execute(
            'SELECT id,source_ref,byte_count,truncated,exit_code,signal,failure FROM large_outputs WHERE task_id=? LIMIT ?',
            (task_id, MAX_ROWS + 1)).fetchall()}
        if len(outputs) > MAX_ROWS:
            raise StoreError('inspection_evidence_overflow')
        for receipt in receipts:
            result = json.loads(receipt['result_json'])
            if receipt['operation'] in {'command_run', 'outside_command', 'outside_write', 'project_write', 'project_edit',
                                        'project_mkdir', 'project_delete', 'project_skill_link', 'draft_write', 'external_call'}:
                # A completed gateway receipt is stored as {complete, receipt};
                # a pending one carries its fields at the top level.
                inner = result.get('receipt') if isinstance(result.get('receipt'), dict) else result
                attempt, epoch = channels.get(receipt['actor'], (None, None))
                item = dict(section='action_receipts', identity_sha256=_digest([receipt['actor'], receipt['request_key']]),
                            operation=receipt['operation'], channel_attempt_id=attempt, channel_owner_epoch=epoch,
                            tool_call_ref=_ref(inner.get('tool_call_ref')),
                            complete=result.get('complete'), exit_code=_int(inner.get('exit_code')),
                            run_complete=inner.get('complete') if 'receipt' in result else None,
                            failure=_code(inner.get('failure')), error=_code(result.get('error')),
                            profile=_code(inner.get('profile')), large_output_id=_code(inner.get('large_output_id')),
                            result_sha256=_digest(result))
                retained = outputs.get(inner.get('large_output_id'))
                if retained is not None:
                    # Metadata of the retained output; its body is not read here.
                    item['retained_output'] = dict(
                        bound=retained['source_ref'] == inner.get('tool_call_ref'), byte_count=_int(retained['byte_count']),
                        truncated=bool(retained['truncated']), exit_code=_int(retained['exit_code']),
                        signal=_code(retained['signal']), failure=_code(retained['failure']))
                rows.append(item)
        observed = db.execute("SELECT id,category,outcome,attempt_id,created_at,detail_json FROM observability_events "
                              "WHERE task_id=? AND category IN ('validation','submission_rejection','gateway_refusal','tool_read_api') "
                              "ORDER BY created_at,id LIMIT ?", (task_id, MAX_ROWS + 1)).fetchall()
        if len(observed) > MAX_ROWS:
            raise StoreError('inspection_evidence_overflow')
        for row in observed:
            detail = json.loads(row['detail_json'] or '{}')
            fields = detail.get('fields') if isinstance(detail.get('fields'), list) else []
            rows.append(dict(section='observability', id=row['id'], category=row['category'], outcome=_code(row['outcome']),
                             attempt_id=row['attempt_id'], created_at=row['created_at'],
                             operation=_code(detail.get('operation')), field=_code(detail.get('field')),
                             fields=[_code(f) for f in fields[:20]]))
            if row['category'] == 'tool_read_api':
                from ..tool_read_api import BATCH_READS
                # Explicit metadata projection, never forwarding arbitrary detail keys.
                projected = {}
                for key in ('catalog_revision','item_hash','result_sha256','channel_hash','outer_hash','request_hash'):
                    value = detail.get(key)
                    if isinstance(value, str) and re.fullmatch(r'(?:batch:)?[0-9a-f]{32,64}', value):
                        projected[key] = value
                for key in ('item_index','elapsed_ms','input_bytes','output_bytes','executed_count',
                            'deferred_count','not_run_count','byte_count'):
                    value = _int(detail.get(key))
                    if value is not None and value >= 0:
                        projected[key] = value
                projected['operation'] = detail.get('operation') if detail.get('operation') in BATCH_READS | {'tools_discover','read_batch'} else None
                projected['error_code'] = _code(detail.get('error_code'))
                projected['large_output_id'] = _code(detail.get('large_output_id'))
                rows[-1].update(operation=projected['operation'], field=None, fields=[], read_api=projected)
        # Prepared read replies are separate from delivery observations and checks.
        from ..read_api_evidence import project as project_read_evidence
        from ..tool_read_api import encoded
        call_rows = db.execute(
            "SELECT id,attempt_id,detail_json FROM observability_events "
            "WHERE task_id=? AND category='read_api_call' ORDER BY created_at,id LIMIT ?",
            (task_id, MAX_ROWS + 1)).fetchall()
        if len(call_rows) > MAX_ROWS:
            raise StoreError('inspection_evidence_overflow')
        for call in call_rows:
            detail = project_read_evidence(json.loads(call['detail_json'] or '{}'))
            matches = [c for c in channel_rows if hashlib.sha256(encoded([
                c['channel_id'], c['owner_epoch'], c['attempt_id']])).hexdigest()[:32] == detail.get('channel_hash')
                and c['attempt_id'] == call['attempt_id'] and c['owner_epoch'] == owner_epoch]
            detail['bound'] = len(matches) == 1 and detail.get('owner_epoch') == owner_epoch
            rows.append(dict(section='read_api_calls', id=call['id'], attempt_id=call['attempt_id'], **detail))
        report_reads = db.execute(
            "SELECT attempt_id,report_id,request_id,outcome,created_at FROM report_read_observations "
            "WHERE task_id=? ORDER BY created_at,attempt_id,request_id LIMIT ?",
            (task_id, MAX_ROWS + 1)).fetchall()
        if len(report_reads) > MAX_ROWS:
            raise StoreError('inspection_evidence_overflow')
        for row in report_reads:
            # Native request ids may be arbitrary caller text: hash those.
            request = row['request_id']
            rows.append(dict(section='report_reads', attempt_id=row['attempt_id'], report_id=row['report_id'],
                             request_sha256=_digest(request), outcome=_code(row['outcome']),
                             batch_request=request if re.fullmatch(r'batch:[0-9a-f]{64}', request or '') else None,
                             created_at=row['created_at']))
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
                            'metadata hashes do not verify report body bytes or acceptance',
                            'retained-output rows are metadata; output body bytes are not read or verified',
                            'client-side schema rejections before a dispatched tools/call never reach the store',
                            'host-rendered worker-card status is not stored; it is unavailable here'])
