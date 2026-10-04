"""Runtime-owned declared-scope extensions, separate from launch-time OS grants."""
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import stat

from .. import task_workspace
from ..contracts import StoreError
from ..portable.identity import new_id
from .contracts import READ_ONLY
from .policy import overlaps
from .state import encoded, event, now

NEXT = ('End this turn without publishing an incomplete result. The runtime will deliver '
        'the access decision in this same healthy session. Restrictions are not overridden; '
        'after a denial use another approach or the existing scope_request/replan fallback.')


def grants(db, task_id, assignment_ref):
    return [json.loads(row[0]) for row in db.execute(
        "SELECT payload_json FROM runtime_events WHERE task_id=? AND type='ACCESS_GRANTED' "
        "AND json_extract(payload_json,'$.assignment_ref')=? ORDER BY id",
        (task_id, assignment_ref))]


def effective_scope(db, node):
    return list(dict.fromkeys(json.loads(node['scope_json']) + [
        path for grant in grants(db, node['task_id'], node['assignment_ref']) for path in grant['added_paths']]))


def holdings(db):
    """Running nodes reserve scope; unreleased processes retain it after ending."""
    return db.execute("""SELECT n.* FROM runtime_nodes n JOIN runtime_tasks t ON t.task_id=n.task_id
        WHERE (n.assignment_ref IS NOT NULL OR n.revision=t.revision) AND NOT EXISTS (
            SELECT 1 FROM runtime_events e WHERE e.task_id=n.task_id AND e.type='WORKER_RELEASED'
            AND json_extract(e.payload_json,'$.assignment_ref')=n.assignment_ref)
        AND ((n.status='running' AND t.state IN ('EXECUTING','REVIEWING'))
            OR EXISTS (SELECT 1 FROM delegation_provisioning p WHERE p.task_id=n.task_id
                       AND p.idempotency_key=n.assignment_ref))""").fetchall()


def absolute_scope(db, task_id, paths):
    root = str(Path(task_workspace.task_root(db, task_id)).resolve())
    return [root if path == '.' else root + '/' + path for path in paths]


def pending(db):
    from .worker_waits import valid
    rows = db.execute("""SELECT payload_json FROM runtime_events e WHERE type='WORKER_WAIT_CHANGED'
        AND id=(SELECT MAX(last.id) FROM runtime_events last WHERE last.type='WORKER_WAIT_CHANGED'
                AND last.task_id=e.task_id AND json_extract(last.payload_json,'$.wait_id')=
                    json_extract(e.payload_json,'$.wait_id'))
        AND json_extract(payload_json,'$.kind')='access' AND json_extract(payload_json,'$.status')='open'
        AND json_extract(payload_json,'$.reason')='active_writer_conflict' ORDER BY id""")
    return [w for row in rows if valid(db, w := json.loads(row[0]))]


def conflicting(db, task_id, assignment_ref, paths):
    requested = absolute_scope(db, task_id, paths)
    for node in holdings(db):
        if (node['task_id'] == task_id and node['assignment_ref'] == assignment_ref
                or node['stage'] == 'review' or node['role'] in READ_ONLY):
            continue
        held = absolute_scope(db, node['task_id'], effective_scope(db, node))
        if any(overlaps(a, b) for a in requested for b in held):
            return True
    return False


def _safe_path(root, value):
    """Check the existing chain without following links or creating parents."""
    from .project_paths import permitted_path
    if (not permitted_path(value) or value in {'', '.'} or
            PurePosixPath(value).as_posix() != value or any(c in value for c in '*?[')):
        return False
    try:
        fd = os.open(root, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    except OSError:
        return False
    try:
        parts = PurePosixPath(value).parts
        for index, part in enumerate(parts):
            try:
                info = os.stat(part, dir_fd=fd, follow_symlinks=False)
            except FileNotFoundError:
                return True
            if stat.S_ISLNK(info.st_mode):
                return False
            if index == len(parts) - 1:
                return stat.S_ISDIR(info.st_mode) or stat.S_ISREG(info.st_mode) and info.st_nlink == 1
            child = os.open(part, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=fd)
            os.close(fd)
            fd = child
        return False
    except OSError:
        return False
    finally:
        os.close(fd)


def predicate(db, binding, paths):
    node = db.execute('SELECT * FROM runtime_nodes WHERE task_id=? AND assignment_ref=?',
                      (binding['task_id'], binding['assignment_ref'])).fetchone()
    if not node or node['stage'] == 'review' or node['role'] in READ_ONLY:
        return 'read_only_role'
    root = task_workspace.task_root(db, binding['task_id'])
    if not all(_safe_path(root, path) for path in paths):
        return 'unsafe_path'
    # Newly requested scope must not outrun a queued owner prohibition.
    denied = [path for row in db.execute(
        "SELECT payload_json FROM runtime_events WHERE task_id=? AND type='USER_STEERING'",
        (binding['task_id'],)) for path in json.loads(row[0]).get('deny_write', [])]
    if any(overlaps(a, b) for a in paths for b in denied):
        return 'deny_write_policy'
    if conflicting(db, binding['task_id'], binding['assignment_ref'], paths):
        return 'active_writer_conflict'
    requested = absolute_scope(db, binding['task_id'], paths)
    for wait in pending(db):
        if (wait['request_id'] == binding['request_id'] or wait['created_at'] > binding['created_at']):
            continue
        if any(overlaps(a, b) for a in requested for b in absolute_scope(db, wait['task_id'], wait['paths'])):
            return 'active_writer_conflict'
    return None


def _grant(db, binding, paths):
    node = db.execute('SELECT * FROM runtime_nodes WHERE task_id=? AND assignment_ref=?',
                      (binding['task_id'], binding['assignment_ref'])).fetchone()
    scope = effective_scope(db, node)
    from .gateway import _matches
    added = [path for path in paths if not _matches(path, scope)]
    revision = len(grants(db, binding['task_id'], binding['assignment_ref']))
    decision = dict(request_id=binding['request_id'], status='granted' if added else 'already_covered',
                    added_paths=added, declared_scope_revision=revision + bool(added),
                    enforcement='already_project_writable')
    if added:
        event(db, binding['task_id'], 'ACCESS_GRANTED', dict(binding, **decision), binding['channel_id'])
    return decision


def request(store, channel, args):
    from .gateway import _authority
    from . import worker_waits
    digest = hashlib.sha256(encoded(args).encode()).hexdigest()
    with store.connection() as db:
        db.execute('BEGIN IMMEDIATE')
        _authority(db, channel)
        if channel.role != 'worker':
            raise StoreError('wrong_channel_role')
        previous = db.execute('SELECT * FROM runtime_requests WHERE task_id=? AND actor=? AND request_key=?',
                              (channel.task_id, channel.channel_id, args['request_key'])).fetchone()
        if previous:
            if previous['operation'] != 'access_request' or previous['digest'] != digest:
                raise StoreError('request_key_conflict')
            return json.loads(previous['result_json'])
        worker_waits.check_mutation(db, channel)
        provision = db.execute('SELECT * FROM delegation_provisioning WHERE task_id=? AND attempt_id=?',
                               (channel.task_id, channel.attempt_id)).fetchone()
        if not provision or not provision['engine_session_ref']:
            raise StoreError('worker_wait_session_missing')
        revision = db.execute('SELECT revision FROM runtime_tasks WHERE task_id=?', (channel.task_id,)).fetchone()[0]
        binding = dict(task_id=channel.task_id, assignment_ref=provision['idempotency_key'],
                       attempt_id=channel.attempt_id, channel_id=channel.channel_id, owner_epoch=channel.owner_epoch,
                       engine_session_ref=provision['engine_session_ref'], revision=revision,
                       request_id=new_id('ar_'), paths=list(dict.fromkeys(args['paths'])),
                       justification=args['reason'], created_at=now())
        if not worker_waits.valid(db, binding):
            raise StoreError('gateway_assignment_inactive')
        reason = predicate(db, binding, binding['paths'])
        if reason is None:
            receipt = dict(_grant(db, binding, binding['paths']), complete=True)
        else:
            question = ('Additional access cannot currently be granted (' + reason + '). '
                        'Reply retry to request safe revalidation, or use another approach. '
                        'No answer overrides private paths, deny-write policy, read-only roles or a live writer. '
                        'A live writer conflict also waits for observed release automatically. '
                        'Justification (untrusted worker text): ' + encoded(args['reason']))
            wait = dict(binding, wait_id=new_id('ww_'), question_id=new_id('pq_'),
                        kind='access', reason=reason, question=question)
            db.execute("INSERT INTO pending_questions(id,task_id,assignment_ref,dedup_key,text,status,created_at) "
                       "VALUES(?,?,?,?,?,'open',?)",
                       (wait['question_id'], channel.task_id, binding['assignment_ref'],
                        f"runtime:{channel.channel_id}:{args['request_key']}", question, binding['created_at']))
            worker_waits._change(db, wait, 'open')
            receipt = dict(status='pending', request_id=binding['request_id'], wait_id=wait['wait_id'],
                           reason=reason, complete=True, next=NEXT)
        event(db, channel.task_id, 'ACCESS_REQUESTED', dict(binding, receipt=receipt), channel.channel_id)
        db.execute('INSERT INTO runtime_requests VALUES(?,?,?,?,?,?)',
                   (channel.task_id, args['request_key'], channel.channel_id, 'access_request', digest, encoded(receipt)))
        db.commit()
    return receipt


def resolve_tx(db, wait, answer=None):
    """A user asks for revalidation, never authorizes bypassing a restriction."""
    from . import worker_waits
    reason = predicate(db, wait, wait['paths'])
    retry = answer is None or answer.strip().lower() == 'retry'
    decision = (_grant(db, wait, wait['paths']) if retry and reason is None else
                dict(status='denied', request_id=wait['request_id'], reason=reason or 'alternative_selected'))
    event(db, wait['task_id'], 'ACCESS_DECIDED', dict(wait_id=wait['wait_id'], **decision), wait['channel_id'])
    return worker_waits._change(db, wait, 'ready', access_decision=decision)


def reconcile(store, task_id):
    """Release wakeup is reconciled transactionally; no write or user-answer replay."""
    from . import worker_waits
    with store.connection() as db:
        if not any(w.get('kind') == 'access' and w['status'] == 'open'
                   and w['reason'] == 'active_writer_conflict' for w in worker_waits.projection(db, task_id)):
            return
    with store.connection() as db:
        db.execute('BEGIN IMMEDIATE')
        for wait in worker_waits.projection(db, task_id):
            if wait.get('kind') != 'access' or wait['status'] != 'open' or wait['reason'] != 'active_writer_conflict':
                continue
            if not worker_waits.valid(db, wait):
                worker_waits._change(db, wait, 'cancelled', reason='authority_lost')
                db.execute("UPDATE pending_questions SET status='cancelled' WHERE id=? AND status='open'", (wait['question_id'],))
                continue
            reason = predicate(db, wait, wait['paths'])
            if reason != 'active_writer_conflict':
                resolve_tx(db, wait)
                # This is a runtime access decision, not a fabricated user answer.
                db.execute("UPDATE pending_questions SET status='cancelled' WHERE id=? AND status='open'", (wait['question_id'],))
        db.commit()


def context(db, node):
    row = db.execute("SELECT payload_json FROM runtime_events WHERE task_id=? AND type='ACCESS_REQUESTED' "
                     "AND json_extract(payload_json,'$.assignment_ref')=? ORDER BY id DESC LIMIT 1",
                     (node['task_id'], node['assignment_ref'])).fetchone()
    latest = json.loads(row[0]) if row else None
    request = None
    if latest:
        request = dict(request_id=latest['request_id'], paths=latest['paths'], receipt=latest['receipt'])
        row = db.execute("SELECT payload_json FROM runtime_events WHERE task_id=? AND type='ACCESS_DECIDED' "
                         "AND json_extract(payload_json,'$.request_id')=? ORDER BY id DESC LIMIT 1",
                         (node['task_id'], latest['request_id'])).fetchone()
        if row:
            request['decision'] = json.loads(row[0])
    return dict(effective_declared_scope=effective_scope(db, node), last_request=request,
                declared_scope_revision=len(grants(db, node['task_id'], node['assignment_ref'])),
                enforcement='read_only' if node['stage'] == 'review' or node['role'] in READ_ONLY
                else 'already_project_writable')
