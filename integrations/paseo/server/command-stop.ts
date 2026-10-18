import type { PluginHandlerContext } from "@getpaseo/plugin/server";
import { readRunningCommands, type CommandStopResult } from "../shared/command-stop.ts";
import { commandStopSender, isUnconfirmedStop, type CommandStopSend } from "./command-stop-registry.ts";
import { parsePaseoRuntimeSessionId, resolveAgentTask, STORE_PYTHON_HELPERS } from "./worker-history.ts";
import { runPython } from "./python-runner.ts";

type RunningCommands = ReturnType<typeof readRunningCommands["output"]["parse"]>;

// Read-only mirror of command_control._live_commands: a started gateway
// command of a live worker channel, current owner epoch and non-terminal
// task, with no exit recorded. "Running" means running or unresolved; only
// Cortex's runner can confirm a stop.
const PYTHON = String.raw`
import json, re, shlex, sqlite3, sys
from datetime import datetime, timezone
from urllib.parse import quote
root, task_id, assignment_ref = sys.argv[1], sys.argv[2], sys.argv[3] or None
${STORE_PYTHON_HELPERS}
def clip(value, limit):
    return value if len(value) <= limit else value[:limit - 1] + '…'
db = sqlite3.connect('file:' + quote(root + '/.cortex/cortex.sqlite3', safe='/') + '?mode=ro', uri=True)
db.row_factory = sqlite3.Row
tables = {r[0] for r in db.execute("select name from sqlite_master where type='table'")}
if not {'runtime_events', 'trusted_channels', 'task_owners', 'runtime_tasks'}.issubset(tables):
    print(json.dumps({'available': False, 'commands': [], 'message': 'Running commands are unavailable in this workspace store.'}))
    raise SystemExit
provisioning = 'delegation_provisioning' in tables and 'channel_id' in {r[1] for r in db.execute('pragma table_info(delegation_provisioning)')}
assignment = 'p.idempotency_key' if provisioning else 'null'
join = ('LEFT JOIN delegation_provisioning p ON p.channel_id=c.channel_id AND p.task_id=c.task_id ' if provisioning else '')
now = datetime.now(timezone.utc).isoformat(timespec='microseconds')
rows = db.execute(
    "SELECT e.payload_json,e.created_at,c.attempt_id," + assignment + " AS assignment_ref FROM runtime_events e "
    "JOIN trusted_channels c ON c.channel_id=e.actor AND c.task_id=e.task_id "
    "JOIN task_owners o ON o.task_id=c.task_id AND o.epoch=c.owner_epoch "
    "JOIN runtime_tasks t ON t.task_id=e.task_id " + join +
    "WHERE e.task_id=? AND e.type='COMMAND_PROCESS_STARTED' "
    "AND c.role='worker' AND c.revoked_at IS NULL "
    "AND (c.expires_at IS NULL OR c.expires_at>?) "
    "AND t.state NOT IN ('COMPLETED','FAILED','CANCELLED') "
    "AND NOT EXISTS (SELECT 1 FROM runtime_events x WHERE x.task_id=e.task_id "
    "AND x.type='COMMAND_PROCESS_EXITED' AND json_extract(x.payload_json,'$.tool_call_ref')="
    "json_extract(e.payload_json,'$.tool_call_ref')) ORDER BY e.id",
    (task_id, now)).fetchall()
commands = []
for row in rows:
    try:
        data = json.loads(row['payload_json'])
    except (ValueError, TypeError):
        continue
    ref = data.get('tool_call_ref') if isinstance(data, dict) else None
    if not isinstance(ref, str) or not re.fullmatch(r'gateway:[0-9a-f]{32}', ref):
        continue
    if assignment_ref is not None and row['assignment_ref'] != assignment_ref:
        continue
    argv = data.get('argv')
    summary = clip(scrub(shlex.join([str(part) for part in argv])), 500) if isinstance(argv, list) else ''
    stop = db.execute("SELECT payload_json FROM runtime_events WHERE task_id=? AND type='COMMAND_STOP_REQUESTED' "
                      "AND json_extract(payload_json,'$.tool_call_ref')=? ORDER BY id LIMIT 1", (task_id, ref)).fetchone()
    stop_request = None
    if stop is not None:
        try:
            requested = json.loads(stop['payload_json'])
        except (ValueError, TypeError):
            requested = {}
        if isinstance(requested, dict) and isinstance(requested.get('reason'), str):
            origin = requested.get('origin')
            stop_request = {'reason': clip(requested['reason'], 2000), 'origin': origin if isinstance(origin, str) else None}
    commands.append({'toolCallRef': ref, 'operation': str(data.get('operation') or 'command_run'),
        'assignmentRef': row['assignment_ref'], 'attemptId': row['attempt_id'], 'startedAt': row['created_at'],
        'argvSummary': summary, 'stopRequest': stop_request})
print(json.dumps({'available': True, 'commands': commands[:50]}, ensure_ascii=False))
`;

function unavailable(message: string): RunningCommands {
  return { available: false, commands: [], message };
}

/** The running or unresolved worker commands of one task, read-only. */
export async function readRunningCommandsInStore(projectRoot: string, taskId: string, assignmentRef?: string): Promise<RunningCommands> {
  const result = await runPython(["-B", "-c", PYTHON, projectRoot, taskId, assignmentRef ?? ""], { timeoutMs: 5000, maxBuffer: 4 * 1024 * 1024 });
  if (result.status !== 0 || !result.stdout) return unavailable("Running commands could not be read.");
  try {
    return readRunningCommands.output.parse(JSON.parse(result.stdout));
  } catch {
    return unavailable("Running commands returned an invalid response.");
  }
}

export async function getRunningCommands(input: { workspaceId: string; agentId: string; assignmentRef?: string }, context: PluginHandlerContext): Promise<RunningCommands> {
  const resolved = await resolveAgentTask(input, context);
  if ("message" in resolved) return unavailable(resolved.message);
  return readRunningCommandsInStore(resolved.projectRoot, resolved.taskId, input.assignmentRef);
}

/** The native ACP session bound to this exact Paseo agent, or null. */
async function agentSession(input: { workspaceId: string; agentId: string }, context: PluginHandlerContext): Promise<string | null> {
  const handle = context.paseo.agents.ref(input.agentId);
  await handle.refresh();
  const agent = handle.current();
  if (!agent || agent.workspaceId !== input.workspaceId) return null;
  return parsePaseoRuntimeSessionId(agent.runtimeInfo);
}

/** A readable reason for a stop request Cortex did not accept. */
export function stopRefusalMessage(detail: string): string {
  const text = detail.trim().slice(0, 300);
  if (/command_stop_not_running/.test(text)) return "This command has already finished or is no longer a running worker command of this task. Nothing was stopped.";
  if (/command_stop_session_not_owner/.test(text)) return "This chat no longer owns the Cortex task. Nothing was stopped.";
  if (/request_key_conflict/.test(text)) return "This stop request conflicts with an earlier one. Edit the reason and try again.";
  return `Cortex did not accept the stop request${text ? ` (${text})` : ""}.`;
}

export async function stopCommandHandler(
  input: { workspaceId: string; agentId: string; toolCallRef: string; reason: string; requestKey: string },
  context: PluginHandlerContext,
  lookup: (sessionId: string) => CommandStopSend | undefined = commandStopSender,
): Promise<CommandStopResult> {
  const sessionId = await agentSession(input, context);
  if (!sessionId) return { status: "refused", message: "This chat is not bound to a Cortex session. Nothing was stopped." };
  const send = lookup(sessionId);
  if (!send) {
    return { status: "refused", message: "This chat has no live Cortex connection. Nothing was stopped. "
      + "Reopen the chat and try again, or use /cortex-kill with the command reference and a reason." };
  }
  try {
    await send({ toolCallRef: input.toolCallRef, reason: input.reason.trim(), requestKey: input.requestKey });
    return { status: "requested" };
  } catch (error) {
    if (isUnconfirmedStop(error)) {
      return { status: "unconfirmed", message: "Cortex did not confirm the stop request in time. It may still apply; retry to check." };
    }
    return { status: "refused", message: stopRefusalMessage(error instanceof Error ? error.message : "") };
  }
}
