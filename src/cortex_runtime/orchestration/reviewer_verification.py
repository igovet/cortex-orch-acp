"""Reuse exact runtime verification observations for the current reviewer only.

Events and retained output are authority. Shutdown cancellation is durable so
it reaches the gateway in the production MCP server's separate process.
"""
import json
import time
import uuid

from ..contracts import StoreError
from ..delegation import revision

WAIT_SECONDS = 5.0
POLL_SECONDS = .05


def cancel_waits(store, task_ids):
    """Signal only this runtime's owned tasks, without changing lifecycle."""
    from .state import event
    with store.connection() as db:
        db.execute('BEGIN IMMEDIATE')
        for task_id in task_ids:
            attempts = [r[0] for r in db.execute(
                "SELECT p.attempt_id FROM runtime_nodes n JOIN runtime_tasks t ON t.task_id=n.task_id "
                "JOIN delegation_provisioning p ON p.task_id=n.task_id AND p.idempotency_key=n.assignment_ref "
                "JOIN trusted_channels c ON c.channel_id=p.channel_id "
                "WHERE n.task_id=? AND n.revision=t.revision AND t.state='REVIEWING' "
                "AND n.stage='review' AND n.status='running' AND c.revoked_at IS NULL ORDER BY p.attempt_id",
                (task_id,))]
            latest = db.execute(
                "SELECT id FROM runtime_events WHERE task_id=? AND type='CHECK_EXECUTION_STARTED' "
                "AND actor='runtime' ORDER BY id DESC LIMIT 1", (task_id,)).fetchone()
            cancelled = db.execute(
                "SELECT id,payload_json FROM runtime_events WHERE task_id=? "
                "AND type='REVIEWER_VERIFICATION_WAIT_CANCELLED' AND actor='runtime' "
                "ORDER BY id DESC LIMIT 1", (task_id,)).fetchone()
            if attempts and latest and not (cancelled and cancelled['id'] > latest['id'] and
                                             json.loads(cancelled['payload_json']).get('attempt_ids') == attempts):
                event(db, task_id, 'REVIEWER_VERIFICATION_WAIT_CANCELLED', dict(attempt_ids=attempts))
        db.commit()


def _review(db, task_id, attempt_id):
    return db.execute(
        "SELECT n.assignment_ref,t.revision,t.plan_json FROM runtime_nodes n "
        "JOIN delegation_provisioning p ON p.task_id=n.task_id AND p.idempotency_key=n.assignment_ref "
        "JOIN runtime_tasks t ON t.task_id=n.task_id "
        "WHERE n.task_id=? AND p.attempt_id=? AND n.revision=t.revision AND n.stage='review' "
        "AND n.status='running' AND t.state='REVIEWING'", (task_id, attempt_id)).fetchone()


def observation(db, task_id, command_ref):
    rows = db.execute(
        "SELECT payload_json FROM runtime_events WHERE task_id=? AND actor='runtime' AND type='CHECK_EXECUTION_FINISHED' "
        "AND json_extract(payload_json,'$.command_ref')=?", (task_id, command_ref)).fetchall()
    return json.loads(rows[0][0]) if len(rows) == 1 else None


def _usable(db, task_id, check):
    if not check or check.get('complete') is not True or type(check.get('exit_code')) is not int:
        return False
    from .. import task_workspace
    if check.get('profile', 'restricted') != 'restricted':
        return False
    if check.get('cwd') not in (None, task_workspace.task_root(db, task_id)):
        return False
    run_id = check.get('run_id')
    invalid = db.execute(
        "SELECT 1 FROM runtime_events WHERE task_id=? AND "
        "(type='SPECULATIVE_VERIFICATION_DISCARDED' OR "
        "(type='SPECULATIVE_VERIFICATION_FINISHED' AND "
        "(json_extract(payload_json,'$.stale')=1 OR json_extract(payload_json,'$.blocker') IS NOT NULL))) "
        "AND json_extract(payload_json,'$.run_id')=?", (task_id, run_id)).fetchone()
    if invalid:
        return False
    starts = db.execute(
        "SELECT id,payload_json FROM runtime_events WHERE task_id=? AND actor='runtime' "
        "AND type='CHECK_EXECUTION_STARTED' AND json_extract(payload_json,'$.command_ref')=?",
        (task_id, check.get('command_ref'))).fetchall()
    if len(starts) != 1 or starts[0]['id'] != check.get('started_event_id'):
        return False
    start = json.loads(starts[0]['payload_json'])
    if any(start.get(k) != check.get(k) for k in ('argv', 'revision_digest', 'command_ref', 'cwd', 'profile')):
        return False
    saved = db.execute(
        "SELECT * FROM large_outputs WHERE id=? AND task_id=?",
        (check.get('large_output_id'), task_id)).fetchone()
    from .check_closure import _retained_output_complete
    return bool(saved and saved['owner_scope'] == 'coordinator' and
                saved['source_ref'] == check.get('command_ref') and not saved['truncated'] and
                saved['exit_code'] == check['exit_code'] and saved['failure'] is None and
                type(saved['byte_count']) is int and saved['byte_count'] >= 0 and
                _retained_output_complete(db, task_id, saved))


def validate_reuse(db, task_id, attempt_id, receipt):
    """Verify runtime provenance without accepting an executor's assertion."""
    if receipt.get('profile', 'restricted') != 'restricted':
        return False
    from .. import task_workspace
    if receipt.get('cwd') not in (None, task_workspace.task_root(db, task_id)):
        return False
    origin = receipt.get('runtime_verification')
    if not isinstance(origin, dict):
        return False
    check = observation(db, task_id, origin.get('command_ref'))
    if not _usable(db, task_id, check):
        return False
    reviewer = db.execute(
        "SELECT n.stage FROM runtime_nodes n JOIN delegation_provisioning p "
        "ON p.task_id=n.task_id AND p.idempotency_key=n.assignment_ref "
        "WHERE n.task_id=? AND p.attempt_id=?", (task_id, attempt_id)).fetchone()
    if not reviewer or reviewer['stage'] != 'review':
        return False
    alias = db.execute("SELECT * FROM large_outputs WHERE id=? AND task_id=?",
                       (receipt.get('large_output_id'), task_id)).fetchone()
    original = db.execute("SELECT * FROM large_outputs WHERE id=? AND task_id=?",
                          (check['large_output_id'], task_id)).fetchone()
    return bool(alias and all(alias[k] == original[k] for k in
                             ('filename', 'byte_count', 'source_kind', 'signal', 'failure', 'exit_code', 'truncated')) and
                origin == dict(source='runtime', command_ref=check['command_ref'],
                               large_output_id=check['large_output_id'], run_id=check['run_id']) and
                all(receipt.get(k) == check.get(k) for k in ('argv', 'exit_code', 'complete', 'revision_digest')))


def reuse(store, channel, root, args, ref, owner):
    from .gateway import _authority
    from pathlib import Path
    if args.get('cwd') is not None and args['cwd'] != str(Path(root).resolve()):
        return None
    deadline = time.monotonic() + min(WAIT_SECONDS, args.get('timeout_seconds', WAIT_SECONDS))
    with store.connection() as db:
        review = _review(db, channel.task_id, channel.attempt_id)
        if not review or args['argv'] not in json.loads(review['plan_json'])['verification']:
            return None
        started = db.execute(
            "SELECT id,payload_json FROM runtime_events WHERE task_id=? AND actor='runtime' AND type='CHECK_EXECUTION_STARTED' "
            "AND json_extract(payload_json,'$.revision')=? AND json_extract(payload_json,'$.argv')=? "
            "ORDER BY id DESC LIMIT 1",
            (channel.task_id, review['revision'], json.dumps(args['argv'], ensure_ascii=False, separators=(',', ':')))).fetchone()
    if not started:
        return None
    started_id = started['id']
    started = json.loads(started['payload_json'])
    if 'run_id' not in started:
        try:
            started['run_id'] = int(started['command_ref'].split(':')[2])
        except (KeyError, ValueError, IndexError):
            return None
    current = revision.capture_revision(str(root))
    if (current.get('dirty_digest') or current.get('digest')) != started['revision_digest']:
        return None
    while True:
        with store.connection() as db:
            stopped = db.execute(
                "SELECT 1 FROM runtime_events WHERE task_id=? AND actor='runtime' "
                "AND type='REVIEWER_VERIFICATION_WAIT_CANCELLED' AND id>? "
                "AND EXISTS (SELECT 1 FROM json_each(payload_json,'$.attempt_ids') WHERE value=?)",
                (channel.task_id, started_id, channel.attempt_id)).fetchone()
            if stopped:
                raise StoreError('gateway_runtime_stopping')
            _authority(db, channel)
            live = _review(db, channel.task_id, channel.attempt_id)
            if not live or live['revision'] != review['revision']:
                return None
            check = observation(db, channel.task_id, started['command_ref'])
            aborted = db.execute(
                "SELECT 1 FROM runtime_events WHERE task_id=? AND type IN "
                "('CHECK_EXECUTION_ABORTED','SPECULATIVE_VERIFICATION_DISCARDED') "
                "AND json_extract(payload_json,'$.run_id')=?", (channel.task_id, started['run_id'])).fetchone()
            if aborted:
                return None
            if check:
                if not _usable(db, channel.task_id, check):
                    return None
                break
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            return None
        time.sleep(min(POLL_SECONDS, remaining))
    current = revision.capture_revision(str(root))
    if (current.get('dirty_digest') or current.get('digest')) != check['revision_digest']:
        return None
    # A scoped metadata alias to the same immutable output file, not a second
    # execution or a copy of raw logs. Existing output paging remains scoped.
    output_id = 'lo_' + uuid.uuid4().hex[:12]
    with store.connection() as db:
        db.execute('BEGIN IMMEDIATE')
        _authority(db, channel)
        live = _review(db, channel.task_id, channel.attempt_id)
        if (not live or live['revision'] != review['revision'] or
                live['assignment_ref'] != review['assignment_ref'] or
                live['plan_json'] != review['plan_json']):
            return None
        if db.execute(
                "SELECT 1 FROM runtime_events WHERE task_id=? AND actor='runtime' "
                "AND type='REVIEWER_VERIFICATION_WAIT_CANCELLED' AND id>? "
                "AND EXISTS (SELECT 1 FROM json_each(payload_json,'$.attempt_ids') WHERE value=?)",
                (channel.task_id, started_id, channel.attempt_id)).fetchone():
            raise StoreError('gateway_runtime_stopping')
        if not _usable(db, channel.task_id, check):
            return None
        db.execute(
            "INSERT INTO large_outputs (id,task_id,owner_scope,source_kind,source_ref,filename,byte_count,"
            "truncated,exit_code,signal,failure,created_at) "
            "SELECT ?,task_id,?,source_kind,?,filename,byte_count,truncated,exit_code,signal,failure,? "
            "FROM large_outputs WHERE id=? AND task_id=?",
            (output_id, owner, ref, store._now(), check['large_output_id'], channel.task_id))
        db.commit()
    return dict(tool_call_ref=ref, argv=check['argv'], cwd=str(Path(root).resolve()), profile='restricted',
                exit_code=check['exit_code'], complete=True,
                failure=None, revision_digest=check['revision_digest'], large_output_id=output_id,
                output='', output_truncated=False, output_inline_omitted=True,
                runtime_verification=dict(source='runtime', command_ref=check['command_ref'],
                                          large_output_id=check['large_output_id'], run_id=check['run_id']))
