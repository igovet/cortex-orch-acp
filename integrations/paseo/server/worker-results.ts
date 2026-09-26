import type { PluginHandlerContext } from "@getpaseo/plugin/server";
import { readWorkerResults } from "../shared/worker-results.js";
import { resolveAgentTask, STORE_PYTHON_HELPERS } from "./worker-history.js";
import { runPython } from "./python-runner.js";

// Deliberately narrow: only the terminal `worker_result` rows for the exact
// requested assignment refs, one spawn, no ``getWorkerHistory`` call. This is
// the fallback path (see ``server/provider.ts``'s ``presentTimeline``) for a
// worker card whose ACP item predates the embedded result projection.
const PYTHON = String.raw`
import json, re, sqlite3, sys
from urllib.parse import quote
root, task_id, refs_json = sys.argv[1], sys.argv[2], sys.argv[3]
${STORE_PYTHON_HELPERS}
def clip(value, limit):
    if not isinstance(value, str): return None
    value = scrub(value.strip())
    return value if len(value) <= limit else value[:limit - 1] + '…'
refs = [r for r in json.loads(refs_json) if isinstance(r, str)]
results = {ref: None for ref in refs}
db = sqlite3.connect('file:' + quote(root + '/.cortex/cortex.sqlite3', safe='/') + '?mode=ro', uri=True)
db.row_factory = sqlite3.Row
tables = {r[0] for r in db.execute("select name from sqlite_master where type='table'")}
if refs and 'delegation_provisioning' in tables and 'task_events' in tables:
    placeholders = ','.join('?' for _ in refs)
    for row in db.execute(f'''select p.idempotency_key ref, e.payload_json from task_events e
            join delegation_provisioning p on p.task_id=e.task_id and p.attempt_id=e.assignment_ref
            where e.task_id=? and e.event_type='worker_result' and p.idempotency_key in ({placeholders})
            order by e.id''', (task_id, *refs)):
        if row['ref'] not in results:
            continue
        try:
            payload = json.loads(row['payload_json'])
        except (ValueError, TypeError):
            payload = {}
        if not isinstance(payload, dict):
            payload = {}
        checks = [{'name': clip(c.get('name'), 300) or 'check', 'outcome': str(c.get('outcome') or 'unverified'),
            **({'blocker': clip(c.get('blocker'), 600)} if isinstance(c.get('blocker'), str) and c.get('blocker').strip() else {})}
            for c in payload.get('checks') or [] if isinstance(c, dict)]
        results[row['ref']] = {'summary': clip(payload.get('summary'), 600), 'checks': checks[:30]}
print(json.dumps({'available': True, 'results': results}, ensure_ascii=False))
`;

function unavailable(assignmentRefs: readonly string[]) {
  return { available: false, results: Object.fromEntries(assignmentRefs.map((ref) => [ref, null])) };
}

export async function getWorkerResults(
  input: { workspaceId: string; agentId: string; assignmentRefs: string[] },
  context: PluginHandlerContext,
): Promise<{ available: boolean; results: Record<string, { summary: string | null; checks: { name: string; outcome: string; blocker?: string }[] } | null> }> {
  const resolved = await resolveAgentTask(input, context);
  if ("message" in resolved) return unavailable(input.assignmentRefs);
  const result = await runPython(["-B", "-c", PYTHON, resolved.projectRoot, resolved.taskId, JSON.stringify(input.assignmentRefs)], {
    timeoutMs: 5000,
    maxBuffer: 4 * 1024 * 1024,
  });
  if (result.status !== 0 || !result.stdout) return unavailable(input.assignmentRefs);
  try {
    return readWorkerResults.output.parse(JSON.parse(result.stdout));
  } catch {
    return unavailable(input.assignmentRefs);
  }
}
