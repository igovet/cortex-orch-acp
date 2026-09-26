import assert from "node:assert/strict";
import { mkdtempSync, mkdirSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { loadServerModuleForTest } from "./test-bundle.ts";
import { execSqlite, sqlValues } from "./test-fixture-db.ts";

const { getPipeline } = await loadServerModuleForTest<typeof import("./pipeline.ts")>(resolve("server/pipeline.ts"));

const SESSION_ID = "acps_0123456789ab";
const WORKSPACE_ID = "workspace-1";
const AGENT_ID = "agent-1";
const TASK_ID = "t_test0000003";

function wrappedSessionId(sessionId: string): string {
  return `plugin:${JSON.stringify({ version: 1, data: { sessionId } })}`;
}

function makeFixture(): { root: string; cleanup: () => void } {
  const root = mkdtempSync(join(tmpdir(), "cortex-pipeline-test-"));
  mkdirSync(join(root, ".cortex"));
  const dbPath = join(root, ".cortex", "cortex.sqlite3");
  execSqlite(dbPath, `
    create table acp_host_sessions (session_id text primary key, task_id text, project_root text);
    create table tasks (id text primary key, project_root text, created_at text);
    create table work_assignments (assignment_id text primary key, task_id text, node_key text, idempotency_key text, status text);
    create table work_assignment_attempts (attempt_id text primary key, assignment_id text, attempt_no integer, status text, failure_reason text);
    create table task_events (id integer primary key autoincrement, task_id text, event_type text, assignment_ref text, payload_json text, created_at text);
    insert into acp_host_sessions (session_id, task_id, project_root) values ${sqlValues([SESSION_ID, TASK_ID, root])};
    insert into tasks (id, project_root, created_at) values ${sqlValues([TASK_ID, root, "2026-01-01T00:00:00Z"])};
    insert into task_events (task_id, event_type, assignment_ref, payload_json, created_at) values ${sqlValues([TASK_ID, "user_message", null, JSON.stringify({ content: "Please fix the bug." }), "2026-01-01T00:00:01Z"])};
  `);
  return { root, cleanup: () => rmSync(root, { recursive: true, force: true }) };
}

function mockContext(workspaceId: string, agentId: string, directory: string, sessionId: string | null): any {
  return {
    paseo: {
      workspaces: { ref: (id: string) => ({ refresh: async () => {}, directory: id === workspaceId ? directory : null }) },
      agents: {
        ref: (id: string) => ({
          refresh: async () => {},
          current: () => id === agentId ? { workspaceId, runtimeInfo: sessionId ? { sessionId: wrappedSessionId(sessionId) } : null } : null,
        }),
      },
    },
  };
}

// A resolved task returns its user message and an empty worker list, never
// a thrown error for a task with no delegations yet.
{
  const { root, cleanup } = makeFixture();
  try {
    const context = mockContext(WORKSPACE_ID, AGENT_ID, root, SESSION_ID);
    const pipeline = await getPipeline({ workspaceId: WORKSPACE_ID, agentId: AGENT_ID }, context);
    assert.equal(pipeline.available, true);
    assert.equal(pipeline.task?.id, TASK_ID);
    assert.equal(pipeline.messages.length, 1);
    assert.equal(pipeline.messages[0].text, "Please fix the bug.");
    assert.deepEqual(pipeline.workers, []);
  } finally {
    cleanup();
  }
}

// An unresolvable agent returns `available: false` with a message.
{
  const { root, cleanup } = makeFixture();
  try {
    const context = mockContext(WORKSPACE_ID, AGENT_ID, root, null);
    const pipeline = await getPipeline({ workspaceId: WORKSPACE_ID, agentId: AGENT_ID }, context);
    assert.equal(pipeline.available, false);
    assert.ok(pipeline.message);
  } finally {
    cleanup();
  }
}

// Weak/legacy evidence (`evidence_gap`) and a closed historical failure
// (`closed_by`) -- both already stored by delegation.evidence.accept_checks
// -- pass through both the delegation result and the plan-node result
// projections, camelCased, without re-deriving anything.
{
  const root = mkdtempSync(join(tmpdir(), "cortex-pipeline-checks-test-"));
  mkdirSync(join(root, ".cortex"));
  const dbPath = join(root, ".cortex", "cortex.sqlite3");
  const checks = JSON.stringify([
    { name: "unit tests", outcome: "failed", closed_by: "gateway:cmd:2" },
    { name: "typecheck", outcome: "unverified", claimed_outcome: "passed", evidence_gap: "tool_call_ref_missing" },
  ]);
  execSqlite(dbPath, `
    create table acp_host_sessions (session_id text primary key, task_id text, project_root text);
    create table tasks (id text primary key, project_root text, created_at text);
    create table task_events (id integer primary key autoincrement, task_id text, event_type text, assignment_ref text, payload_json text, created_at text);
    create table delegation_provisioning (idempotency_key text primary key, task_id text, assignment_id text, attempt_id text, role_id text, step text, status text, owner_epoch integer, created_at text, updated_at text);
    create table runtime_tasks (task_id text primary key, state text, revision integer, goal text, plan_json text, synthesis text, review_cycle integer, blocker text, created_at text, updated_at text);
    create table runtime_nodes (task_id text, revision integer, node_id text, role text, objective text, dependencies_json text, scope_json text, stage text, status text, assignment_ref text, result_json text);
    insert into acp_host_sessions (session_id, task_id, project_root) values ${sqlValues([SESSION_ID, TASK_ID, root])};
    insert into tasks (id, project_root, created_at) values ${sqlValues([TASK_ID, root, "2026-01-01T00:00:00Z"])};
    insert into task_events (task_id, event_type, assignment_ref, payload_json, created_at) values ${sqlValues([TASK_ID, "delegation_requested", "req-1", "{}", "2026-01-01T00:00:01Z"])};
    insert into delegation_provisioning (idempotency_key, task_id, assignment_id, attempt_id, role_id, step, status, owner_epoch, created_at, updated_at) values ${sqlValues(["req-1", TASK_ID, "wa-1", "xa-1", "worker-fullstack-dev", "execution", "provisioned", 1, "2026-01-01T00:00:00Z", "2026-01-01T00:00:00Z"])};
    insert into task_events (task_id, event_type, assignment_ref, payload_json, created_at) values ${sqlValues([TASK_ID, "worker_result", "xa-1", JSON.stringify({ summary: "Done", checks: JSON.parse(checks) }), "2026-01-01T00:05:00Z"])};
    insert into runtime_tasks (task_id, state, revision, goal, review_cycle, created_at, updated_at) values ${sqlValues([TASK_ID, "EXECUTING", 1, "Goal", 0, "2026-01-01T00:00:00Z", "2026-01-01T00:00:00Z"])};
    insert into runtime_nodes (task_id, revision, node_id, role, objective, dependencies_json, scope_json, stage, status, assignment_ref, result_json) values ${sqlValues([
      TASK_ID, 1, "n1", "worker-fullstack-dev", "Objective", "[]", "[]", "execution", "completed", "req-2",
      JSON.stringify({ summary: "Done", checks: JSON.parse(checks) }),
    ])};
  `);
  try {
    const context = mockContext(WORKSPACE_ID, AGENT_ID, root, SESSION_ID);
    const pipeline = await getPipeline({ workspaceId: WORKSPACE_ID, agentId: AGENT_ID }, context);
    const expected = [
      { name: "unit tests", outcome: "failed", closedBy: "gateway:cmd:2" },
      { name: "typecheck", outcome: "unverified", claimedOutcome: "passed", evidenceGap: "tool_call_ref_missing" },
    ];
    assert.deepEqual(pipeline.delegations["req-1"]?.result?.checks, expected);
    assert.deepEqual(pipeline.plan?.nodes[0]?.result?.checks, expected);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
}

console.log("pipeline: ok");
