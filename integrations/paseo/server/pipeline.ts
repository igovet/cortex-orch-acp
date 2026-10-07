import type { PluginHandlerContext } from "@getpaseo/plugin/server";
import { readPipeline, type Pipeline } from "../shared/pipeline.js";
import { getWorkerHistory, resolveAgentTask, STORE_PYTHON_HELPERS } from "./worker-history.js";
import { runPython } from "./python-runner.js";

// Read-only projection of one task's durable lifecycle: its orchestration
// state, the user's messages, the coordinator's requirements, and per
// delegation the worker's reported result and the coordinator's verdict.
// Workers themselves come from the worker-history reader, which already
// resolves their names, routes and runtime-verified status.
const PYTHON = String.raw`
import json, re, sqlite3, sys
from urllib.parse import quote
root, task_id = sys.argv[1], sys.argv[2]
${STORE_PYTHON_HELPERS}
def clip(value, limit):
    if not isinstance(value, str): return None
    value = scrub(value.strip())
    return value if len(value) <= limit else value[:limit - 1] + '…'
def load(text):
    try:
        value = json.loads(text)
        return value if isinstance(value, dict) else {}
    except (ValueError, TypeError):
        return {}
db = sqlite3.connect('file:' + quote(root + '/.cortex/cortex.sqlite3', safe='/') + '?mode=ro', uri=True)
db.row_factory = sqlite3.Row
tables = {r[0] for r in db.execute("select name from sqlite_master where type='table'")}
task = db.execute('select id, created_at from tasks where id=? and project_root=?', (task_id, root)).fetchone()
out = {'available': task is not None, 'tokens': {'total': None, 'coordinator': None}, 'task': None, 'messages': [], 'requirements': [], 'delegations': {}, 'plan': None}
if task is not None:
    out['task'] = {'id': task['id'], 'createdAt': task['created_at'], 'state': None, 'waitReason': None, 'waitDetail': None}
    if 'task_orchestration' in tables:
        row = db.execute('select state, wait_reason, wait_detail_json from task_orchestration where task_id=?', (task_id,)).fetchone()
        if row is not None:
            out['task'].update({'state': row['state'], 'waitReason': row['wait_reason'],
                'waitDetail': clip(load(row['wait_detail_json']).get('explanation'), 1200)})
    if 'work_requirements' in tables:
        for row in db.execute('''select r.requirement_id, v.statement, v.criteria_json from work_requirements r
                join work_requirement_revisions v on v.requirement_id=r.requirement_id and v.revision=r.current_revision
                where r.task_id=? order by r.created_at limit 50''', (task_id,)):
            try: criteria = [clip(c, 400) for c in json.loads(row['criteria_json']) if isinstance(c, str)]
            except (ValueError, TypeError): criteria = []
            out['requirements'].append({'id': row['requirement_id'], 'statement': clip(row['statement'], 1200) or '', 'criteria': criteria[:30]})
    if 'task_events' in tables:
        messages = []
        for row in db.execute("select payload_json, created_at from task_events where task_id=? and event_type='user_message' order by id", (task_id,)):
            text = clip(load(row['payload_json']).get('content'), 2000)
            if text: messages.append({'text': text, 'createdAt': row['created_at']})
        out['messages'] = messages[:1] + messages[1:][-19:]
        delegations = out['delegations']
        def entry(ref, order):
            return delegations.setdefault(ref, {'order': order, 'result': None, 'decision': None})
        for row in db.execute("select id, assignment_ref from task_events where task_id=? and event_type='delegation_requested' and assignment_ref is not null order by id", (task_id,)):
            entry(row['assignment_ref'], row['id'])
        # A worker reports under its execution attempt; provisioning binds
        # that attempt back to the delegation it was started for.
        if 'delegation_provisioning' in tables:
            for row in db.execute('''select p.idempotency_key ref, e.payload_json from task_events e
                    join delegation_provisioning p on p.task_id=e.task_id and p.attempt_id=e.assignment_ref
                    where e.task_id=? and e.event_type='worker_result' order by e.id''', (task_id,)):
                if row['ref'] not in delegations: continue
                payload = load(row['payload_json'])
                checks = [{'name': clip(c.get('name'), 300) or 'check', 'outcome': str(c.get('outcome') or 'unverified'),
                    **({'blocker': clip(c.get('blocker'), 600)} if isinstance(c.get('blocker'), str) and c.get('blocker').strip() else {}),
                    **({'closedBy': clip(c.get('closed_by'), 128)} if isinstance(c.get('closed_by'), str) and c.get('closed_by').strip() else {}),
                    **({'claimedOutcome': c.get('claimed_outcome')} if isinstance(c.get('claimed_outcome'), str) and c.get('claimed_outcome').strip() else {}),
                    **({'evidenceGap': c.get('evidence_gap')} if isinstance(c.get('evidence_gap'), str) and c.get('evidence_gap').strip() else {})}
                    for c in payload.get('checks') or [] if isinstance(c, dict)]
                delegations[row['ref']]['result'] = {'summary': clip(payload.get('summary'), 600), 'checks': checks[:30]}
        for row in db.execute("select assignment_ref, payload_json from task_events where task_id=? and event_type='coordinator_decision' and assignment_ref is not null order by id", (task_id,)):
            if row['assignment_ref'] not in delegations: continue
            payload = load(row['payload_json'])
            if isinstance(payload.get('decision'), str):
                delegations[row['assignment_ref']]['decision'] = {'decision': payload['decision'], 'rationale': clip(payload.get('rationale'), 600)}
    # The runtime's own plan: every node of the current revision, so a step
    # is visible from the moment the plan is accepted, not only once a
    # worker has started for it.
    if 'runtime_tasks' in tables and 'runtime_nodes' in tables:
        runtime = db.execute('select state, revision, blocker from runtime_tasks where task_id=?', (task_id,)).fetchone()
        if runtime is not None:
            rows = db.execute('''select node_id, role, objective, stage, status, assignment_ref, dependencies_json, result_json
                from runtime_nodes where task_id=? and revision=? order by rowid''', (task_id, runtime['revision'])).fetchall()
            finished = {r['node_id'] for r in rows if r['status'] == 'completed'}
            nodes = []
            for r in rows:
                try: dependencies = [d for d in json.loads(r['dependencies_json']) if isinstance(d, str)]
                except (ValueError, TypeError): dependencies = []
                waiting = None
                if r['status'] == 'pending':
                    unmet = [d for d in dependencies if d not in finished]
                    waiting = 'Waiting for ' + ', '.join(unmet) if unmet else 'Waiting to start'
                result = load(r['result_json'])
                checks = [{'name': clip(c.get('name'), 300) or 'check', 'outcome': str(c.get('outcome') or 'unverified'),
                    **({'blocker': clip(c.get('blocker'), 600)} if isinstance(c.get('blocker'), str) and c.get('blocker').strip() else {}),
                    **({'closedBy': clip(c.get('closed_by'), 128)} if isinstance(c.get('closed_by'), str) and c.get('closed_by').strip() else {}),
                    **({'claimedOutcome': c.get('claimed_outcome')} if isinstance(c.get('claimed_outcome'), str) and c.get('claimed_outcome').strip() else {}),
                    **({'evidenceGap': c.get('evidence_gap')} if isinstance(c.get('evidence_gap'), str) and c.get('evidence_gap').strip() else {})}
                    for c in result.get('checks') or [] if isinstance(c, dict)]
                node_result = {'summary': clip(result.get('summary'), 600), 'checks': checks[:30]} if r['result_json'] is not None else None
                nodes.append({'id': r['node_id'], 'role': r['role'] or '', 'objective': clip(r['objective'], 2000) or '',
                    'stage': r['stage'] or '', 'status': r['status'] or 'pending', 'assignmentRef': r['assignment_ref'], 'waiting': waiting, 'result': node_result})
            out['plan'] = {'state': runtime['state'], 'blocker': clip(runtime['blocker'], 600), 'nodes': nodes}
    if 'usage_records' in tables:
        rows = db.execute('select attempt_id, basis, tokens_json, stop_reason, failed from usage_records where task_id=?', (task_id,)).fetchall()
        out['tokens'] = {'total': sum_tokens(rows), 'coordinator': sum_tokens([r for r in rows if (r['attempt_id'] or '').startswith('co_')])}
print(json.dumps(out, ensure_ascii=False))
`;

function unavailable(message: string): Pipeline {
  return { available: false, message, tokens: { total: null, coordinator: null }, task: null, messages: [], requirements: [], delegations: {}, plan: null, workers: [] };
}

export async function getPipeline(input: { workspaceId: string; agentId: string }, context: PluginHandlerContext): Promise<Pipeline> {
  const resolved = await resolveAgentTask(input, context);
  if ("message" in resolved) return unavailable(resolved.message);
  const result = await runPython(["-B", "-c", PYTHON, resolved.projectRoot, resolved.taskId], {
    timeoutMs: 5000,
    maxBuffer: 16 * 1024 * 1024,
  });
  if (result.status !== 0 || !result.stdout) return unavailable("The pipeline could not be read.");
  // Only lifecycle status is needed here: a cursor past every event id keeps
  // the transcript out while terminal status is still read independently.
  const workers = await getWorkerHistory({ ...input, cursor: Number.MAX_SAFE_INTEGER, limit: 1 }, context);
  try {
    return readPipeline.output.parse({ ...JSON.parse(result.stdout), workers: workers.assignments });
  } catch {
    return unavailable("The pipeline returned an invalid response.");
  }
}
