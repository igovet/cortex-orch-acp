import { existsSync } from "node:fs";
import { spawnSync } from "node:child_process";
import type { PluginHandlerContext } from "@getpaseo/plugin/server";
import { readWorkerHistory } from "../shared/worker-history.js";

const PYTHON = String.raw`
import json, sqlite3, sys
from urllib.parse import quote
import re
root, task_id, attempt_id, cursor, limit, assignment_cursor, assignment_ref = sys.argv[1], sys.argv[2], sys.argv[3] or None, int(sys.argv[4] or 0), min(max(int(sys.argv[5] or 200), 1), 200), sys.argv[6] or '', sys.argv[7] or None
db_path = root + '/.cortex/cortex.sqlite3'
def scrub(value):
    try:
        value = re.sub(r'(?i)(?:bearer|token|password|secret|api[_ -]?key)\s*[:=]\s*\S+', '[redacted]', value)
        return re.sub(r'(?<![A-Za-z0-9])/(?:home|tmp|private|Users)/[^\s,;]+', '[private path]', value)
    except re.error:
        return '[redacted]'
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
    assignments = []
    truncated = False
    more_assignments = False
    for task in task_rows:
        if assignment_ref:
            rows = db.execute('select * from work_assignments where task_id=? and (assignment_id=? or idempotency_key=?) order by assignment_id limit 2', (task['id'], assignment_ref, assignment_ref)).fetchall()
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
                'select assignment_ref,payload_json from task_events where task_id=? and assignment_ref in (?,?) and event_type=? order by id',
                (task['id'], a['assignment_id'], a['idempotency_key'], 'delegation_requested'),
            ).fetchall()
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
            if attempt_id:
                attempts = db.execute('select * from work_assignment_attempts where assignment_id=? and attempt_id=? order by attempt_no, attempt_id', (a['assignment_id'], attempt_id)).fetchall()
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
                evs = db.execute(f'''select id,event_type,created_at,payload_json from task_events where task_id=? and assignment_ref in ({placeholders}) and id>? order by id limit ?''', (task['id'], *refs, cursor, limit + 1)).fetchall()
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
                    for field in ('activity_ref', 'input', 'output', 'status', 'error', 'diff', 'tool_kind', 'title', 'locations', 'content', 'raw_input', 'raw_output'):
                        value = payload.get(field)
                        if value is None:
                            continue
                        # Runtime worker_activity already stores redacted
                        # structured envelopes. Keep their shape intact;
                        # only legacy/plain strings need the final display
                        # scrubber. No synthetic diff or payload is created.
                        event_result[field] = scrub(value) if isinstance(value, str) else value
                    event_out.append(event_result)
                # The history page is intentionally bounded, so its visible
                # events cannot be used to infer terminal state: a stop/result
                # marker may be older than the first 200 rows. Read the latest
                # terminal marker independently for these exact attempt refs.
                terminal_rows = db.execute(f'''select id,event_type from task_events where task_id=? and assignment_ref in ({placeholders}) and event_type in ('worker_result','assignment_failed','assignment_cancelled') order by id''', (task['id'], *refs)).fetchall()
                # A late compatibility failure/cancellation must not
                # downgrade a real worker result.  The replay path uses the
                # same precedence: worker_result is stronger than either
                # negative terminal marker, while the latest marker of the
                # same kind remains authoritative.
                terminal_type = None
                terminal_priority = {'worker_result': 2, 'assignment_failed': 1, 'assignment_cancelled': 1}
                for terminal_row in terminal_rows:
                    if terminal_type is None or terminal_priority[terminal_row['event_type']] >= terminal_priority[terminal_type]:
                        terminal_type = terminal_row['event_type']
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
                        attempt_route = {'provider': route_row['engine'], 'model': route_row['model'], 'effort': effort if isinstance(effort, str) and effort else None}
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
                if attempt_route is not None:
                    attempt_result['route'] = attempt_route
                attempt_out.append(attempt_result)
            # An exact assignment lookup can race delegate: the durable
            # assignment row is present before its first attempt is bound.
            # Return that one assignment as pending so the panel can keep the
            # selection and resolve it on the next bounded poll. Broad
            # catalogue reads still omit empty assignments (no phantom rows).
            if attempt_out or assignment_ref:
                assignment_result = {'taskId': task['id'], 'id': a['assignment_id'], 'assignmentRef': a['idempotency_key'], 'nodeKey': a['node_key'], 'status': attempt_out[-1]['status'] if attempt_out else (a['status'] or 'pending'), 'attempts': attempt_out}
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

export async function getWorkerHistory(input: { workspaceId: string; agentId: string; attemptId?: string; assignmentRef?: string; cursor?: number; limit?: number; assignmentCursor?: string }, context: PluginHandlerContext) {
  const workspaceHandle = context.paseo.workspaces.ref(input.workspaceId);
  await workspaceHandle.refresh();
  const projectRoot = workspaceHandle.directory;
  if (!projectRoot || !projectRoot.startsWith("/") || projectRoot.includes("\0")) {
    return { assignments: [], available: false, truncated: false, nextCursor: null, nextAssignmentCursor: null, message: "Worker history is unavailable for this workspace." };
  }
  const dbPath = `${projectRoot}/.cortex/cortex.sqlite3`;
  if (!existsSync(dbPath)) {
    return { assignments: [], available: false, truncated: false, nextCursor: null, nextAssignmentCursor: null, message: "No Cortex store is present in this workspace." };
  }
  const agentHandle = context.paseo.agents.ref(input.agentId);
  await agentHandle.refresh();
  const agent = agentHandle.current();
  const sessionId = parsePaseoRuntimeSessionId(agent?.runtimeInfo);
  if (!sessionId || agent?.workspaceId !== input.workspaceId) return { assignments: [], available: false, truncated: false, nextCursor: null, nextAssignmentCursor: null, message: "Worker history is unavailable for this agent." };
  const taskId = await readTaskId(projectRoot, sessionId);
  if (!taskId) return { assignments: [], available: false, truncated: false, nextCursor: null, nextAssignmentCursor: null, message: "No durable Cortex task is bound to this agent." };
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
