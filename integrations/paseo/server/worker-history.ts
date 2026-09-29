import { existsSync } from "node:fs";
import type { PluginHandlerContext } from "@getpaseo/plugin/server";
import { readWorkerHistory } from "../shared/worker-history.js";
import { runPython } from "./python-runner.js";

/** Helpers shared by every Python store reader: the display scrubber (credentials
 * and private absolute paths never reach the client) and token totals.
 * Needs `import json, re`. */
export const STORE_PYTHON_HELPERS = String.raw`
def scrub(value):
    try:
        value = re.sub(r'(?i)(?:bearer|token|password|secret|api[_ -]?key)\s*[:=]\s*\S+', '[redacted]', value)
        return re.sub(r'(?<![A-Za-z0-9])/(?:home|tmp|private|Users)/[^\s,;]+', '[private path]', value)
    except re.error:
        return '[redacted]'
def sum_tokens(rows):
    # One per-turn ACP PromptResponse.usage row per prompt. Context fill is separate.
    totals = {'input': 0, 'output': 0, 'cachedRead': 0, 'cachedWrite': 0,
              'total': 0, 'knownCalls': 0, 'unknownCalls': 0}
    for row in rows:
        try: t = json.loads(row['tokens_json'] or '{}')
        except (ValueError, TypeError): t = {}
        if row['basis'] == 'unknown' or not isinstance(t, dict) or not t:
            totals['unknownCalls'] += 1
            continue
        count = lambda key: t.get(key) if isinstance(t.get(key), int) and not isinstance(t.get(key), bool) and t.get(key) >= 0 else 0
        totals['knownCalls'] += 1
        totals['input'] += count('inputTokens')
        totals['output'] += count('outputTokens')
        totals['cachedRead'] += count('cachedReadTokens')
        totals['cachedWrite'] += count('cachedWriteTokens')
        if isinstance(t.get('totalTokens'), int) and not isinstance(t.get('totalTokens'), bool) and t['totalTokens'] >= 0:
            totals['total'] += t['totalTokens']
        else:
            totals['total'] += count('inputTokens') + count('outputTokens') + count('cachedReadTokens') + count('cachedWriteTokens')
    return totals if totals['knownCalls'] or totals['unknownCalls'] else None
def usage_fallback_refs(db, task_id, execution_refs):
    # A routed retry replaces delegation_provisioning.attempt_id. Follow its
    # audited predecessor chain so the worker card still includes real spend
    # from every earlier route hop. This affects usage only, not lifecycle.
    refs = set(execution_refs)
    columns = {row[1] for row in db.execute("pragma table_info(attempt_route_audit)")}
    if 'predecessor_attempt_id' not in columns:
        return sorted(refs)
    for start in execution_refs:
        current = start
        for _ in range(50):
            row = db.execute(
                'select predecessor_attempt_id from attempt_route_audit '
                'where task_id=? and attempt_id=? order by recorded_at desc, id desc limit 1',
                (task_id, current),
            ).fetchone()
            previous = row['predecessor_attempt_id'] if row is not None else None
            if not previous or previous in refs:
                break
            refs.add(previous)
            current = previous
    return sorted(refs)
`;

const PYTHON = String.raw`
import json, os, sqlite3, sys
from urllib.parse import quote
import re
root, task_id, attempt_id, cursor, limit, assignment_cursor, assignment_ref = sys.argv[1], sys.argv[2], sys.argv[3] or None, int(sys.argv[4] or 0), min(max(int(sys.argv[5] or 200), 1), 200), sys.argv[6] or '', sys.argv[7] or None
db_path = root + '/.cortex/cortex.sqlite3'
${STORE_PYTHON_HELPERS}def process_identity_absent(row):
    """Return True only when the durable worker identity is provably gone."""
    if row['terminated_at'] is not None:
        return True
    try:
        with open('/proc/sys/kernel/random/boot_id', 'r', encoding='utf-8') as handle:
            current_boot = handle.read().strip()
        if not current_boot or current_boot != row['boot_id']:
            return True
        with open('/proc/%s/stat' % row['pid'], 'r', encoding='utf-8') as handle:
            stat = handle.read()
        close = stat.rfind(')')
        if close < 0:
            return False
        fields = stat[close + 1:].split()
        # fields[19] is /proc stat field 22 (starttime).
        if len(fields) < 20:
            return False
        return fields[19] != str(row['start_ticks'])
    except (OSError, ValueError, TypeError):
        # An unreadable identity is indeterminate, never a false failure.
        return False
# Mirrors acp/messages.py's worker_interrupted_notice and select_language.
INTERRUPTED_REASON = {
    'en': 'Interrupted when the host connection closed; no result yet. '
          'The next message reconciles this worker from saved state.',
    'ru': 'Прервано закрытием соединения с хостом; результата ещё нет. '
          'Следующее сообщение восстановит этого исполнителя из сохранённого состояния.',
}
def task_language(db, tables, task_id):
    if 'task_language' in tables:
        row = db.execute('select language from task_language where task_id=?', (task_id,)).fetchone()
        if row is not None and row['language'] in INTERRUPTED_REASON:
            return row['language']
    for row in db.execute("select payload_json from task_events where task_id=? and event_type='user_message' order by id desc limit 20", (task_id,)):
        try:
            text = json.loads(row['payload_json']).get('content')
        except (ValueError, TypeError, AttributeError):
            continue
        if isinstance(text, str) and text:
            letters = [ch for ch in text if ch.isalpha()]
            cyrillic = sum(1 for ch in letters if '\u0400' <= ch <= '\u04ff')
            return 'ru' if letters and cyrillic / len(letters) >= 0.30 else 'en'
    return 'en'
try:
    db = sqlite3.connect('file:' + quote(db_path, safe='/') + '?mode=ro', uri=True)
    db.row_factory = sqlite3.Row
    tables = {r[0] for r in db.execute("select name from sqlite_master where type='table'")}
    required = {'tasks', 'work_assignments', 'work_assignment_attempts', 'task_events'}
    if not required.issubset(tables):
        print(json.dumps({'assignments': [], 'available': False, 'truncated': False, 'nextCursor': None, 'message': 'Worker history is unavailable in this workspace store.'}))
        raise SystemExit
    task_rows = db.execute('select id from tasks where id=? and project_root=? order by id', (task_id, root)).fetchall()
    has_route_audit = 'attempt_route_audit' in tables
    has_provisioning = 'delegation_provisioning' in tables
    has_process_registry = 'attempt_process_registry' in tables
    assignments = []
    truncated = False
    more_assignments = False
    for task in task_rows:
        # Workers a closed host connection interrupted (e.g. a plugin
        # update): recovery reconciles them, so an absent process must not
        # read as a crash. Mirrors replay_store.py's
        # terminal_worker_card_reconciliation.
        # Recovery's recovery_worker_lost write-off of such a worker is the
        # same interruption, not a generic failure.
        disconnected_attempts = set()
        for row in db.execute(
                "select event_type,assignment_ref,payload_json from task_events where task_id=? and "
                "((event_type='progress' and payload_json like '%host_disconnected%') or "
                "(event_type='assignment_failed' and payload_json like '%recovery_worker_lost%'))", (task['id'],)):
            try:
                payload = json.loads(row['payload_json'])
            except (ValueError, TypeError):
                continue
            if not isinstance(payload, dict):
                continue
            if row['event_type'] == 'assignment_failed':
                # The stale-coordinator-channel write-off names only the assignment.
                ref = payload.get('attempt_id') or row['assignment_ref']
                if payload.get('error') == 'recovery_worker_lost' and ref:
                    disconnected_attempts.add(ref)
                continue
            if payload.get('kind') != 'host_disconnected':
                continue
            for worker in payload.get('interrupted_workers') or ():
                ref = worker.get('attempt_id') if isinstance(worker, dict) else None
                if ref:
                    disconnected_attempts.add(ref)
        interrupted_reason = INTERRUPTED_REASON[task_language(db, tables, task['id'])]
        if assignment_ref:
            rows = db.execute('''select a.* from work_assignments a where a.task_id=? and
                (a.assignment_id=? or a.idempotency_key=? or exists (
                    select 1 from task_events e where e.task_id=a.task_id
                    and e.event_type='delegation_requested' and e.assignment_ref=?
                    and (case when json_valid(e.payload_json) then
                        json_extract(e.payload_json, '$.plan_assignment_ref')=a.assignment_id
                        or json_extract(e.payload_json, '$.work_attempt_ref') in
                            (select attempt_id from work_assignment_attempts where assignment_id=a.assignment_id)
                        else 0 end)
                )) order by a.assignment_id limit 2''',
                (task['id'], assignment_ref, assignment_ref, assignment_ref)).fetchall()
            if len(rows) > 1:
                print(json.dumps({'assignments': [], 'available': False, 'truncated': False, 'nextCursor': None, 'nextAssignmentCursor': None, 'message': 'Worker assignment identity is ambiguous.'}))
                raise SystemExit
        else:
            rows = db.execute('select * from work_assignments where task_id=? and assignment_id>? order by assignment_id limit 101', (task['id'], assignment_cursor)).fetchall()
        assignment_page_more = len(rows) > 100
        if assignment_page_more: rows = rows[:100]
        more_assignments = more_assignments or assignment_page_more
        truncated = truncated or assignment_page_more
        for a in rows:
            title = None
            role = None
            title_rows = db.execute(
                '''select assignment_ref,payload_json from task_events where task_id=?
                and event_type='delegation_requested' and (assignment_ref in (?,?) or
                    (case when json_valid(payload_json) then
                        json_extract(payload_json, '$.plan_assignment_ref')=? or
                        json_extract(payload_json, '$.work_attempt_ref') in
                            (select attempt_id from work_assignment_attempts where assignment_id=?)
                        else 0 end)) order by id''',
                (task['id'], a['assignment_id'], a['idempotency_key'], a['assignment_id'], a['assignment_id']),
            ).fetchall()
            selected_request = next((r for r in title_rows if r['assignment_ref'] == assignment_ref), None)
            selected_attempt_id = attempt_id
            if selected_request is not None:
                try:
                    selected_payload = json.loads(selected_request['payload_json'])
                except (ValueError, TypeError):
                    selected_payload = {}
                bound_attempt_id = selected_payload.get('work_attempt_ref') if isinstance(selected_payload, dict) else None
                if bound_attempt_id:
                    if attempt_id and attempt_id != bound_attempt_id:
                        continue
                    selected_attempt_id = bound_attempt_id
            for title_row in title_rows:
                try:
                    request_payload = json.loads(title_row['payload_json'])
                    candidate_title = request_payload.get('title')
                    candidate_role = request_payload.get('role')
                except Exception:
                    candidate_title = None
                    candidate_role = None
                if title is None and isinstance(candidate_title, str) and candidate_title.strip():
                    title = candidate_title.strip()
                if role is None and isinstance(candidate_role, str) and candidate_role.strip():
                    # Match the coordinator's display convention. The
                    # canonical worker-* prefix is a storage identity, not
                    # part of the name shown to the user.
                    role = candidate_role.strip()
                    if role.startswith('worker-'):
                        role = role[len('worker-'):]
            if selected_attempt_id:
                attempts = db.execute('select * from work_assignment_attempts where assignment_id=? and attempt_id=? order by attempt_no, attempt_id', (a['assignment_id'], selected_attempt_id)).fetchall()
            else:
                attempts = db.execute('select * from work_assignment_attempts where assignment_id=? order by attempt_no, attempt_id', (a['assignment_id'],)).fetchall()
            attempt_out = []
            latest_route = None
            for at in attempts:
                if attempt_id and at['attempt_id'] != attempt_id:
                    continue
                # Worker events are keyed by the exact attempt id. The
                # assignment lifecycle rows use the graph assignment id (or
                # its public idempotency key), and belong to the initial
                # attempt only; including them for a revision would leak the
                # prior attempt's history into the selected attempt.
                refs = [at['attempt_id']]
                execution_attempt_refs = []
                if has_provisioning:
                    # Work-graph attempt ids and execution attempt ids are
                    # deliberately different namespaces. The delegation
                    # request's work_attempt_ref is the authoritative bridge.
                    for request_row in title_rows:
                        try:
                            request_payload = json.loads(request_row['payload_json'])
                        except Exception:
                            request_payload = {}
                        if request_payload.get('work_attempt_ref') != at['attempt_id']:
                            continue
                        refs.append(request_row['assignment_ref'])
                        provisioning_row = db.execute(
                            'select attempt_id from delegation_provisioning where task_id=? and idempotency_key=?',
                            (task['id'], request_row['assignment_ref']),
                        ).fetchone()
                        if provisioning_row is not None and isinstance(provisioning_row['attempt_id'], str) and provisioning_row['attempt_id']:
                            execution_attempt_refs.append(provisioning_row['attempt_id'])
                    refs.extend(execution_attempt_refs)
                if at['attempt_no'] == 1:
                    refs.extend((a['assignment_id'], a['idempotency_key']))
                placeholders = ','.join('?' for _ in refs)
                # Context fill rows are status, not transcript; the latest one
                # is read below independently of the event page.
                evs = db.execute(f'''select id,event_type,created_at,payload_json from task_events where task_id=? and assignment_ref in ({placeholders}) and id>? and (event_type in ('worker_result','worker_question','assignment_failed','assignment_cancelled','worker_activity') or (json_valid(payload_json) and json_extract(payload_json,'$.kind')='worker_activity')) and not (json_valid(payload_json) and coalesce(json_extract(payload_json,'$.activity_ref'),'')='context' and coalesce(json_extract(payload_json,'$.kind'),'')='worker_activity') order by id limit ?''', (task['id'], *refs, cursor, limit + 1)).fetchall()
                more = len(evs) > limit
                if more: evs = evs[:limit]
                truncated = truncated or more
                event_out = []
                for ev in evs:
                    try:
                        payload = json.loads(ev['payload_json'])
                    except Exception:
                        payload = {}
                    # A task assignment shares the task event ledger with
                    # coordinator lifecycle notifications. Only project the
                    # worker's durable activity/result channel into the
                    # worker transcript; coordinator decisions and generic
                    # progress markers are not worker chat.
                    event_kind = payload.get('kind') if isinstance(payload, dict) else None
                    if ev['event_type'] not in ('worker_result', 'worker_question', 'assignment_failed', 'assignment_cancelled', 'worker_activity') and event_kind != 'worker_activity':
                        continue
                    parts = [payload.get('title'), payload.get('status'), payload.get('message') or payload.get('summary') or payload.get('text') or payload.get('response') or payload.get('reason') or payload.get('detail'), payload.get('error')]
                    parts = [p if isinstance(p, str) else json.dumps(p, ensure_ascii=False) for p in parts if p not in (None, '')]
                    text = ' · '.join(parts) or ev['event_type']
                    if not isinstance(text, str): text = json.dumps(text, ensure_ascii=False)
                    event_result = {'id': ev['id'], 'type': ev['event_type'], 'createdAt': ev['created_at'], 'text': scrub(text)}
                    for field in ('activity_ref', 'input', 'output', 'status', 'error', 'diff', 'tool_kind', 'title', 'locations', 'content', 'raw_input', 'raw_output', 'message_id'):
                        value = payload.get(field)
                        if value is None:
                            continue
                        # Runtime worker_activity already stores redacted
                        # structured envelopes. Keep their shape intact;
                        # only legacy/plain strings need the final display
                        # scrubber. No synthetic diff or payload is created.
                        event_result[field] = scrub(value) if isinstance(value, str) else value
                    read_title = str(event_result.get('title') or '')
                    match = re.match(r'^mcp(?:__|\.)([^.]+?)(?:__|\.)([\w-]+)', read_title, re.I)
                    read_only_row = (match and match[1].lower() == 'cortex' and match[2].lower() in ('project_read','read_skill')) or (not match and (event_result.get('tool_kind') == 'read' or re.match(r'^(read|view|cat)(?:\s|$)',read_title,re.I)))
                    if read_only_row:
                        for key in ('output','raw_output','content','error','diff'):
                            event_result.pop(key,None)
                        event_result['text'] = read_title
                    event_out.append(event_result)
                # The history page is intentionally bounded, so its visible
                # events cannot be used to infer terminal state: a stop/result
                # marker may be older than the first 200 rows. Read the latest
                # terminal marker independently for these exact attempt refs.
                terminal_rows = db.execute(f'''select id,event_type,created_at from task_events where task_id=? and assignment_ref in ({placeholders}) and event_type in ('worker_result','assignment_failed','assignment_cancelled') order by id''', (task['id'], *refs)).fetchall()
                # A late compatibility failure/cancellation must not
                # downgrade a real worker result.  The replay path uses the
                # same precedence: worker_result is stronger than either
                # negative terminal marker, while the latest marker of the
                # same kind remains authoritative.
                terminal_type = None
                completed_at = None
                terminal_priority = {'worker_result': 2, 'assignment_failed': 1, 'assignment_cancelled': 1}
                for terminal_row in terminal_rows:
                    if terminal_type is None or terminal_priority[terminal_row['event_type']] >= terminal_priority[terminal_type]:
                        terminal_type = terminal_row['event_type']
                        completed_at = terminal_row['created_at']
                process_absent = False
                if has_process_registry and execution_attempt_refs:
                    process_rows = db.execute(
                        'select * from attempt_process_registry where task_id=? and role=? and attempt_id in (%s)' % ','.join('?' for _ in execution_attempt_refs),
                        (task['id'], 'worker', *execution_attempt_refs),
                    ).fetchall()
                    # Only conclude failure when every recorded execution
                    # identity is present and provably absent.  Missing or
                    # unreadable identity data stays indeterminate.
                    process_absent = (
                        len(process_rows) == len(set(execution_attempt_refs))
                        and bool(process_rows)
                        and all(process_identity_absent(row) for row in process_rows)
                    )
                attempt_route = None
                if has_route_audit:
                    route_row = None
                    for execution_attempt_ref in execution_attempt_refs:
                        candidate_route = db.execute(
                            'select engine,model,decision_detail_json from attempt_route_audit where task_id=? and attempt_id=? order by recorded_at desc, id desc limit 1',
                            (task['id'], execution_attempt_ref),
                        ).fetchone()
                        if candidate_route is not None:
                            route_row = candidate_route
                            break
                    if route_row is None:
                        route_row = db.execute(
                            'select engine,model,decision_detail_json from attempt_route_audit where task_id=? and attempt_id=? order by recorded_at desc, id desc limit 1',
                            (task['id'], at['attempt_id']),
                        ).fetchone()
                    if route_row is not None and isinstance(route_row['engine'], str) and route_row['engine'] and isinstance(route_row['model'], str) and route_row['model']:
                        try:
                            detail = json.loads(route_row['decision_detail_json'] or '{}')
                        except Exception:
                            detail = {}
                        effort = detail.get('effort') if isinstance(detail, dict) else None
                        # This route's real, engine-observed model name
                        # (live_adapters._resolve_route_model_name's own
                        # evidence, written into the audit's own detail) --
                        # absent when the engine gave no evidence beyond the
                        # bare alias/id in 'model' itself; the client then
                        # shows 'model' explicitly marked as an alias rather
                        # than presenting it as the concrete model.
                        model_name = detail.get('model_name') if isinstance(detail, dict) else None
                        attempt_route = {
                            'provider': route_row['engine'], 'model': route_row['model'],
                            'modelName': model_name if isinstance(model_name, str) and model_name else None,
                            'effort': effort if isinstance(effort, str) and effort else None,
                        }
                        latest_route = attempt_route
                failure = at['failure_reason']
                if isinstance(failure, str):
                    failure = scrub(failure)
                attempt_result = {'id': at['attempt_id'], 'attemptNo': at['attempt_no'], 'status': at['status'], 'failureReason': failure, 'events': event_out}
                # The graph status can lag the durable worker outcome. A
                # retained result on this exact attempt is the authoritative
                # terminal signal for the panel; only fall back to the graph
                # status when no terminal event was recorded.
                if terminal_type == 'worker_result':
                    attempt_result['status'] = 'completed'
                elif terminal_type == 'assignment_failed':
                    attempt_result['status'] = 'failed'
                elif terminal_type == 'assignment_cancelled':
                    attempt_result['status'] = 'cancelled'
                interrupted = False
                lost = any(ref in disconnected_attempts for ref in (*execution_attempt_refs, *refs))
                if lost and terminal_type in ('assignment_failed', 'assignment_cancelled'):
                    # Written off after its host was lost, or cancelled by the
                    # replan that followed: an interruption, never a failure.
                    interrupted = True
                    attempt_result['status'] = 'interrupted'
                    attempt_result['failureReason'] = interrupted_reason
                non_terminal = str(attempt_result['status']).lower() in ('planned', 'running', 'in_progress', 'working', 'ready', 'provisioning')
                # No signal reached the old host (SIGKILL, OOM): its launching
                # Cortex process is provably gone, so nothing runs this worker
                # any more. A live or unidentifiable owner never qualifies.
                if non_terminal and not lost and has_process_registry and execution_attempt_refs:
                    owner_rows = db.execute(
                        'select * from attempt_process_registry where task_id=? and role=? and attempt_id in (%s)' % ','.join('?' for _ in execution_attempt_refs),
                        (task['id'], 'acp_host', *('acp_host:' + ref for ref in execution_attempt_refs)),
                    ).fetchall()
                    lost = bool(owner_rows) and all(process_identity_absent(row) for row in owner_rows)
                # A recorded host disconnect is, on its own, the durable answer: real
                # recovery (recovery.worker_reconciliation) marks such a worker
                # recovery_worker_lost on the coordinator's next turn regardless of
                # whether its OS process can still be probed as alive (there is no
                # channel secret in a fresh process to safely resume it either way).
                # Display must not withhold "Interrupted" waiting for a stronger,
                # OS-level "absent" signal that recovery itself never requires.
                if non_terminal and lost:
                    interrupted = True
                    attempt_result['status'] = 'interrupted'
                    if not attempt_result['failureReason']:
                        attempt_result['failureReason'] = interrupted_reason
                elif process_absent and non_terminal:
                    attempt_result['status'] = 'failed'
                    if not attempt_result['failureReason']:
                        attempt_result['failureReason'] = 'Worker process exited before reporting a result.'
                # Runtime verdict outranks report publication. A submitted
                # report with rejected checks is a completed worker turn with
                # issues, while a failed node without a result is an execution
                # failure. This is the same distinction as the coordinator
                # ACP card; both derive it from runtime_nodes. A durably
                # interrupted attempt keeps that status here: the graph row
                # can still read "running" until recovery reconciles it, and
                # that stale word must never resurrect a live spinner for a
                # worker that is not running.
                if 'runtime_nodes' in tables and execution_attempt_refs and not interrupted:
                    runtime_rows = db.execute(
                        'select n.status,n.result_json from runtime_nodes n join delegation_provisioning p on p.task_id=n.task_id and p.idempotency_key=n.assignment_ref where n.task_id=? and p.attempt_id in (%s)' % ','.join('?' for _ in execution_attempt_refs),
                        (task['id'], *execution_attempt_refs),
                    ).fetchall()
                    if len(runtime_rows) == 1:
                        runtime_status, runtime_result = runtime_rows[0]['status'], runtime_rows[0]['result_json']
                        attempt_result['status'] = 'completed_with_issues' if runtime_status == 'failed' and runtime_result is not None else runtime_status
                        try:
                            result = json.loads(runtime_result or '{}') if runtime_result is not None else {}
                        except (TypeError, ValueError):
                            result = {}
                        if not isinstance(result, dict):
                            result = {}
                        checks = result.get('checks')
                        if isinstance(checks, list) and all(isinstance(check, dict) and isinstance(check.get('name'), str) and isinstance(check.get('outcome'), str) for check in checks):
                            attempt_result['checks'] = [{'name': check['name'], 'outcome': check['outcome'],
                                **({'blocker': check['blocker'][:600]} if isinstance(check.get('blocker'), str) and check['blocker'].strip() else {}),
                                # closedBy: an exact later rerun closed this
                                # failure (it stays as run history, resolved).
                                # claimedOutcome/evidenceGap: the runtime kept
                                # an insufficiently-evidenced "passed" claim as
                                # unverified/failed -- both already stored by
                                # delegation.evidence.accept_checks, never
                                # re-derived here.
                                **({'closedBy': check['closed_by']} if isinstance(check.get('closed_by'), str) and check['closed_by'].strip() else {}),
                                **({'claimedOutcome': check['claimed_outcome']} if isinstance(check.get('claimed_outcome'), str) and check['claimed_outcome'].strip() else {}),
                                **({'evidenceGap': check['evidence_gap']} if isinstance(check.get('evidence_gap'), str) and check['evidence_gap'].strip() else {})}
                                for check in checks]
                        if runtime_rows[0]['status'] == 'failed':
                            unresolved = [check.get('name', 'check') + ': ' + check.get('outcome', 'unverified') for check in result.get('checks', []) if check.get('outcome') != 'passed']
                            prefix = 'Completed with issues. Runtime did not accept worker checks. ' if runtime_result is not None else 'Worker failed. '
                            attempt_result['failureReason'] = scrub(prefix + '; '.join(unresolved))
                    elif len(runtime_rows) > 1:
                        attempt_result['status'] = 'failed'
                        attempt_result['failureReason'] = 'Ambiguous runtime assignment identity; outcome is unverified.'
                context_row = db.execute(f'''select payload_json from task_events where task_id=? and assignment_ref in ({placeholders}) and event_type='progress' and json_valid(payload_json) and json_extract(payload_json,'$.activity_ref')='context' and json_extract(payload_json,'$.kind')='worker_activity' order by id desc limit 1''', (task['id'], *refs)).fetchone()
                if context_row is not None:
                    content = json.loads(context_row['payload_json']).get('content')
                    if isinstance(content, dict) and all(isinstance(content.get(k), int) and content.get(k) >= 0 for k in ('used', 'size')) and content['size'] > 0:
                        attempt_result['context'] = {'used': content['used'], 'size': content['size']}
                if 'usage_records' in tables and execution_attempt_refs:
                    usage_refs = usage_fallback_refs(db, task['id'], execution_attempt_refs) if has_route_audit else execution_attempt_refs
                    tokens = sum_tokens(db.execute('select basis,tokens_json from usage_records where task_id=? and attempt_id in (%s)' % ','.join('?' for _ in usage_refs), (task['id'], *usage_refs)).fetchall())
                    if tokens is not None:
                        attempt_result['tokens'] = tokens
                if completed_at is not None:
                    attempt_result['completedAt'] = completed_at
                if attempt_route is not None:
                    attempt_result['route'] = attempt_route
                attempt_out.append(attempt_result)
            # An exact assignment lookup can race delegate: the durable
            # assignment row is present before its first attempt is bound.
            # Return that one assignment as pending so the panel can keep the
            # selection and resolve it on the next bounded poll. Broad
            # catalogue reads still omit empty assignments (no phantom rows).
            if attempt_out or assignment_ref:
                public_ref = selected_request['assignment_ref'] if selected_request is not None else (title_rows[-1]['assignment_ref'] if title_rows else a['idempotency_key'])
                assignment_result = {'taskId': task['id'], 'id': a['assignment_id'], 'assignmentRef': public_ref, 'nodeKey': a['node_key'], 'status': attempt_out[-1]['status'] if attempt_out else (a['status'] or 'pending'), 'attempts': attempt_out}
                if title is not None:
                    assignment_result['title'] = title
                if role is not None:
                    assignment_result['role'] = role
                if latest_route is not None:
                    assignment_result['route'] = latest_route
                assignments.append(assignment_result)
    next_cursor = max((event['id'] for assignment in assignments for at in assignment['attempts'] for event in at['events']), default=None) if truncated else None
    next_assignment_cursor = assignments[-1]['id'] if more_assignments and assignments else None
    print(json.dumps({'assignments': assignments, 'available': True, 'truncated': truncated, 'nextCursor': next_cursor, 'nextAssignmentCursor': next_assignment_cursor}, ensure_ascii=False))
except (sqlite3.Error, OSError) as exc:
    print(json.dumps({'assignments': [], 'available': False, 'truncated': False, 'nextCursor': None, 'message': 'Worker history could not be read.'}))
`;

/** The runtime keys its one store by the logical project root of the session
 * cwd (cortex_runtime.project_storage.logical_project_root): a verified linked
 * Git worktree shares its main repository's `.cortex`, so a worktree workspace
 * directory has no store of its own. This mirrors that resolution read-only;
 * any ambiguous, forged or unsafe layout exits nonzero instead of guessing. */
export const PROJECT_ROOT_PYTHON = String.raw`
import os, re, stat, sys
from pathlib import Path
class Conflict(Exception):
    pass
def canonical(value):
    project = Path(value)
    if not project.is_absolute() or not project.is_dir() or str(project.resolve(strict=True)) != str(project):
        raise Conflict
    return project
def read_line(path):
    # One small owner-private regular file, never following a symlink.
    info = path.lstat()
    if stat.S_ISLNK(info.st_mode) or not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid() or info.st_nlink != 1:
        raise Conflict
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW)
    try:
        data = os.read(fd, 4097)
    finally:
        os.close(fd)
    lines = data.decode('utf-8').splitlines()
    if len(data) > 4096 or len(lines) != 1:
        raise Conflict
    return lines[0]
def within_uid(base, raw):
    candidate = Path(raw)
    if not candidate.is_absolute():
        candidate = base / candidate
    resolved = candidate.resolve(strict=True)
    current = Path(resolved.anchor)
    for part in resolved.parts[1:-1]:
        current = current / part
        info = current.lstat()
        if stat.S_ISLNK(info.st_mode) or not stat.S_ISDIR(info.st_mode):
            raise Conflict
    info = resolved.lstat()
    if stat.S_ISLNK(info.st_mode) or info.st_uid != os.getuid():
        raise Conflict
    return resolved
def logical_root(value):
    candidate = canonical(value)
    marker = candidate / '.git'
    try:
        info = marker.lstat()
    except FileNotFoundError:
        return candidate
    if stat.S_ISLNK(info.st_mode):
        raise Conflict
    if stat.S_ISDIR(info.st_mode):
        if info.st_uid != os.getuid():
            raise Conflict
        return candidate
    match = re.fullmatch(r'gitdir: (.+)', read_line(marker))
    if match is None:
        raise Conflict
    gitdir = within_uid(candidate, match.group(1))
    if not gitdir.is_dir():
        raise Conflict
    common = within_uid(gitdir, read_line(gitdir / 'commondir'))
    if not common.is_dir() or not (common / 'HEAD').exists() or not (common / 'worktrees').is_dir():
        raise Conflict
    # The worktree metadata must point back to this exact ".git" file and
    # live directly under the common directory's own "worktrees/".
    if within_uid(gitdir, read_line(gitdir / 'gitdir')) != marker.resolve():
        raise Conflict
    if gitdir.parent.name != 'worktrees' or gitdir.parent.parent != common:
        raise Conflict
    project = within_uid(common, '..')
    if within_uid(project, '.git') != common:
        raise Conflict
    return canonical(str(project))
try:
    print(logical_root(sys.argv[1]))
except (Conflict, OSError, RuntimeError, ValueError):
    raise SystemExit(1)
`;

/** The runtime's logical project root for one workspace directory, or null. */
export async function resolveProjectRoot(directory: string): Promise<string | null> {
  const result = await runPython(["-B", "-c", PROJECT_ROOT_PYTHON, directory], { timeoutMs: 5000 });
  const root = result.status === 0 && result.stdout.endsWith("\n") ? result.stdout.slice(0, -1) : "";
  return root.startsWith("/") && !root.includes("\0") && !root.includes("\n") ? root : null;
}

const MAX_RUNTIME_SESSION_WRAPPER_LENGTH = 512;
const MAX_RUNTIME_SESSION_PAYLOAD_LENGTH = 480;
const ACP_SESSION_ID = /^acps_[0-9a-f]{12}$/;

/**
 * Paseo 0.8 stores the ACP identity in a versioned plugin-owned wrapper.
 * Keep this boundary deliberately narrow: runtimeInfo is client supplied and
 * must never become a source of project roots, task ids, or arbitrary SQL
 * identities. Unknown versions/shapes and conflicting identities fail closed.
 */
export function parsePaseoRuntimeSessionId(runtimeInfo: unknown): string | null {
  if (!isRecord(runtimeInfo) || !Object.prototype.hasOwnProperty.call(runtimeInfo, "sessionId")) {
    return null;
  }
  const wrapped = runtimeInfo.sessionId;
  if (typeof wrapped !== "string" || wrapped.length === 0 || wrapped.length > MAX_RUNTIME_SESSION_WRAPPER_LENGTH) {
    return null;
  }
  const prefix = "plugin:";
  if (!wrapped.startsWith(prefix)) return null;
  const payloadText = wrapped.slice(prefix.length);
  if (payloadText.length === 0 || payloadText.length > MAX_RUNTIME_SESSION_PAYLOAD_LENGTH) return null;
  // Paseo's v1 wrapper is emitted as plain JSON with these literal keys.
  // Reject escapes before JSON.parse: otherwise `\u0073essionId` can become
  // a second logical key after parsing while evading raw duplicate checks.
  if (payloadText.includes("\\")) return null;
  // JSON.parse keeps the last value for duplicate object keys. Reject those
  // known identity keys before parsing so an ambiguous wrapper cannot choose
  // a different session by ordering.
  for (const key of ["version", "data", "sessionId"]) {
    const occurrences = payloadText.match(new RegExp(`\\"${key}\\"\\s*:`, "g"))?.length ?? 0;
    if (occurrences !== 1) return null;
  }

  let payload: unknown;
  try {
    payload = JSON.parse(payloadText);
  } catch {
    return null;
  }
  if (!isRecord(payload) || payload.version !== 1 || !isRecord(payload.data)) return null;

  const payloadKeys = Object.keys(payload);
  const dataKeys = Object.keys(payload.data);
  if (payloadKeys.length !== 2 || !payloadKeys.includes("version") || !payloadKeys.includes("data")) return null;
  if (dataKeys.length !== 1 || dataKeys[0] !== "sessionId") return null;

  const sessionId = payload.data.sessionId;
  return typeof sessionId === "string" && ACP_SESSION_ID.test(sessionId) ? sessionId : null;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

// `resolveAgentTask`'s two identity spawns (`resolveProjectRoot`,
// `readTaskId`) are invariant for a given (workspaceId, agentId) for the
// life of the agent's bound session (measured: ~60% of a single call's
// cost). Cached here, keyed by that pair and invalidated whenever the
// workspace directory or the agent's bound session id changes underneath
// it, so a stale or reassigned agent never reads another session's task.
type AgentTaskCacheEntry = { directory: string; sessionId: string; projectRoot: string; taskId: string };
const agentTaskCache = new Map<string, AgentTaskCacheEntry>();

function agentTaskCacheKey(workspaceId: string, agentId: string): string {
  return `${workspaceId}\u0000${agentId}`;
}

/** Resolve the exact Cortex task bound to this Paseo agent, or say why not. */
export async function resolveAgentTask(input: { workspaceId: string; agentId: string }, context: PluginHandlerContext): Promise<{ projectRoot: string; taskId: string } | { message: string }> {
  const workspaceHandle = context.paseo.workspaces.ref(input.workspaceId);
  await workspaceHandle.refresh();
  const directory = workspaceHandle.directory;
  if (!directory || !directory.startsWith("/") || directory.includes("\0")) return { message: "Worker history is unavailable for this workspace." };
  const agentHandle = context.paseo.agents.ref(input.agentId);
  await agentHandle.refresh();
  const agent = agentHandle.current();
  const sessionId = parsePaseoRuntimeSessionId(agent?.runtimeInfo);
  if (!sessionId || agent?.workspaceId !== input.workspaceId) return { message: "Worker history is unavailable for this agent." };

  const cacheKey = agentTaskCacheKey(input.workspaceId, input.agentId);
  const cached = agentTaskCache.get(cacheKey);
  if (cached && cached.directory === directory && cached.sessionId === sessionId) {
    return { projectRoot: cached.projectRoot, taskId: cached.taskId };
  }

  const projectRoot = await resolveProjectRoot(directory);
  if (!projectRoot) return { message: "Worker history is unavailable for this workspace." };
  if (!existsSync(`${projectRoot}/.cortex/cortex.sqlite3`)) return { message: "No Cortex store is present in this workspace." };
  const taskId = await readTaskId(projectRoot, sessionId);
  if (!taskId) return { message: "No durable Cortex task is bound to this agent." };
  agentTaskCache.set(cacheKey, { directory, sessionId, projectRoot, taskId });
  return { projectRoot, taskId };
}

export async function getWorkerHistory(input: { workspaceId: string; agentId: string; attemptId?: string; assignmentRef?: string; cursor?: number; limit?: number; assignmentCursor?: string }, context: PluginHandlerContext) {
  const resolved = await resolveAgentTask(input, context);
  if ("message" in resolved) return { assignments: [], available: false, truncated: false, nextCursor: null, nextAssignmentCursor: null, message: resolved.message };
  const { projectRoot, taskId } = resolved;
  const result = await runPython(["-B", "-c", PYTHON, projectRoot, taskId, input.attemptId ?? "", String(input.cursor ?? 0), String(input.limit ?? 200), input.assignmentCursor ?? "", input.assignmentRef ?? ""], {
    timeoutMs: 5000,
    maxBuffer: 64 * 1024 * 1024,
  });
  if (result.status !== 0 || !result.stdout) {
    return { assignments: [], available: false, truncated: false, nextCursor: null, nextAssignmentCursor: null, message: "Worker history could not be read." };
  }
  try {
    return readWorkerHistory.output.parse(JSON.parse(result.stdout));
  } catch {
    return { assignments: [], available: false, truncated: false, nextCursor: null, nextAssignmentCursor: null, message: "Worker history returned an invalid response." };
  }
}

async function readTaskId(projectRoot: string, sessionId: string): Promise<string | null> {
  const result = await runPython(["-B", "-c", "import sqlite3,sys; from urllib.parse import quote; db=sqlite3.connect('file:'+quote(sys.argv[1]+'/.cortex/cortex.sqlite3',safe='/')+'?mode=ro',uri=True); r=db.execute('select task_id from acp_host_sessions where session_id=? and project_root=?',(sys.argv[2],sys.argv[1])).fetchone(); print(r[0] if r and r[0] else '')", projectRoot, sessionId], { timeoutMs: 5000 });
  return result.status === 0 && result.stdout.trim() ? result.stdout.trim() : null;
}
