import { existsSync } from "node:fs";
import { spawnSync } from "node:child_process";
import type { PluginHandlerContext } from "@getpaseo/plugin/server";
import { readWorkerHistory } from "../shared/worker-history.js";

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
    # Engine-reported ACP PromptResponse.usage rows; None when no call reported.
    totals, seen = {'input': 0, 'output': 0, 'cachedRead': 0, 'total': 0}, False
    for row in rows:
        try: t = json.loads(row['tokens_json'] or '{}')
        except (ValueError, TypeError): continue
        if not isinstance(t, dict) or not t: continue
        count = lambda key: t.get(key) if isinstance(t.get(key), int) and t.get(key) >= 0 else 0
        seen = True
        totals['input'] += count('inputTokens')
        totals['output'] += count('outputTokens')
        totals['cachedRead'] += count('cachedReadTokens')
        totals['total'] += count('totalTokens') or count('inputTokens') + count('outputTokens')
    return totals if seen else None
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
                elif process_absent and str(attempt_result['status']).lower() in ('planned', 'running', 'in_progress', 'working', 'ready', 'provisioning'):
                    attempt_result['status'] = 'failed'
                    if not attempt_result['failureReason']:
                        attempt_result['failureReason'] = 'Worker process exited before reporting a result.'
                # Runtime verdict outranks report publication. A submitted
                # report may contain failed/unverified checks; its existence
                # must never turn that exact attempt into a displayed success.
                if 'runtime_nodes' in tables and execution_attempt_refs:
                    runtime_rows = db.execute(
                        'select n.status,n.result_json from runtime_nodes n join delegation_provisioning p on p.task_id=n.task_id and p.idempotency_key=n.assignment_ref where n.task_id=? and p.attempt_id in (%s)' % ','.join('?' for _ in execution_attempt_refs),
                        (task['id'], *execution_attempt_refs),
                    ).fetchall()
                    if len(runtime_rows) == 1:
                        attempt_result['status'] = runtime_rows[0]['status']
                        if runtime_rows[0]['status'] == 'failed':
                            result = json.loads(runtime_rows[0]['result_json'] or '{}')
                            unresolved = [check.get('name', 'check') + ': ' + check.get('outcome', 'unverified') for check in result.get('checks', []) if check.get('outcome') != 'passed']
                            attempt_result['failureReason'] = scrub('Runtime did not accept worker checks. ' + '; '.join(unresolved))
                    elif len(runtime_rows) > 1:
                        attempt_result['status'] = 'failed'
                        attempt_result['failureReason'] = 'Ambiguous runtime assignment identity; outcome is unverified.'
                context_row = db.execute(f'''select payload_json from task_events where task_id=? and assignment_ref in ({placeholders}) and event_type='progress' and json_valid(payload_json) and json_extract(payload_json,'$.activity_ref')='context' and json_extract(payload_json,'$.kind')='worker_activity' order by id desc limit 1''', (task['id'], *refs)).fetchone()
                if context_row is not None:
                    content = json.loads(context_row['payload_json']).get('content')
                    if isinstance(content, dict) and all(isinstance(content.get(k), int) and content.get(k) >= 0 for k in ('used', 'size')) and content['size'] > 0:
                        attempt_result['context'] = {'used': content['used'], 'size': content['size']}
                if 'usage_records' in tables and execution_attempt_refs:
                    tokens = sum_tokens(db.execute('select tokens_json from usage_records where task_id=? and attempt_id in (%s)' % ','.join('?' for _ in execution_attempt_refs), (task['id'], *execution_attempt_refs)).fetchall())
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
function resolveProjectRoot(directory: string): string | null {
  const result = spawnSync("python3", ["-B", "-c", PROJECT_ROOT_PYTHON, directory], { encoding: "utf8", timeout: 5000 });
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

/** Resolve the exact Cortex task bound to this Paseo agent, or say why not. */
export async function resolveAgentTask(input: { workspaceId: string; agentId: string }, context: PluginHandlerContext): Promise<{ projectRoot: string; taskId: string } | { message: string }> {
  const workspaceHandle = context.paseo.workspaces.ref(input.workspaceId);
  await workspaceHandle.refresh();
  const directory = workspaceHandle.directory;
  if (!directory || !directory.startsWith("/") || directory.includes("\0")) return { message: "Worker history is unavailable for this workspace." };
  const projectRoot = resolveProjectRoot(directory);
  if (!projectRoot) return { message: "Worker history is unavailable for this workspace." };
  if (!existsSync(`${projectRoot}/.cortex/cortex.sqlite3`)) return { message: "No Cortex store is present in this workspace." };
  const agentHandle = context.paseo.agents.ref(input.agentId);
  await agentHandle.refresh();
  const agent = agentHandle.current();
  const sessionId = parsePaseoRuntimeSessionId(agent?.runtimeInfo);
  if (!sessionId || agent?.workspaceId !== input.workspaceId) return { message: "Worker history is unavailable for this agent." };
  const taskId = await readTaskId(projectRoot, sessionId);
  if (!taskId) return { message: "No durable Cortex task is bound to this agent." };
  return { projectRoot, taskId };
}

export async function getWorkerHistory(input: { workspaceId: string; agentId: string; attemptId?: string; assignmentRef?: string; cursor?: number; limit?: number; assignmentCursor?: string }, context: PluginHandlerContext) {
  const resolved = await resolveAgentTask(input, context);
  if ("message" in resolved) return { assignments: [], available: false, truncated: false, nextCursor: null, nextAssignmentCursor: null, message: resolved.message };
  const { projectRoot, taskId } = resolved;
  const result = spawnSync("python3", ["-B", "-c", PYTHON, projectRoot, taskId, input.attemptId ?? "", String(input.cursor ?? 0), String(input.limit ?? 200), input.assignmentCursor ?? "", input.assignmentRef ?? ""], {
    encoding: "utf8",
    timeout: 5000,
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
  const result = spawnSync("python3", ["-B", "-c", "import sqlite3,sys; from urllib.parse import quote; db=sqlite3.connect('file:'+quote(sys.argv[1]+'/.cortex/cortex.sqlite3',safe='/')+'?mode=ro',uri=True); r=db.execute('select task_id from acp_host_sessions where session_id=? and project_root=?',(sys.argv[2],sys.argv[1])).fetchone(); print(r[0] if r and r[0] else '')", projectRoot, sessionId], { encoding: "utf8", timeout: 5000 });
  return result.status === 0 && result.stdout.trim() ? result.stdout.trim() : null;
}
