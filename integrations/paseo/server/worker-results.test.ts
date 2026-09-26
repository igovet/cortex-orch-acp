import assert from "node:assert/strict";
import { mkdtempSync, mkdirSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { loadServerModuleForTest } from "./test-bundle.ts";
import { execSqlite, sqlValues } from "./test-fixture-db.ts";

const { getWorkerResults } = await loadServerModuleForTest<typeof import("./worker-results.ts")>(resolve("server/worker-results.ts"));

const SESSION_ID = "acps_0123456789ab";
const WORKSPACE_ID = "workspace-1";
const AGENT_ID = "agent-1";
const TASK_ID = "t_test0000002";

function wrappedSessionId(sessionId: string): string {
  return `plugin:${JSON.stringify({ version: 1, data: { sessionId } })}`;
}

function makeFixture(): { root: string; cleanup: () => void } {
  const root = mkdtempSync(join(tmpdir(), "cortex-worker-results-test-"));
  mkdirSync(join(root, ".cortex"));
  const dbPath = join(root, ".cortex", "cortex.sqlite3");
  // One completed delegation, its provisioning binding an execution attempt
  // id distinct from the public delegation ref (the real ref/attempt split).
  execSqlite(dbPath, `
    create table acp_host_sessions (session_id text primary key, task_id text, project_root text);
    create table tasks (id text primary key, project_root text);
    create table work_assignments (assignment_id text primary key, task_id text, node_key text, idempotency_key text, status text);
    create table work_assignment_attempts (attempt_id text primary key, assignment_id text, attempt_no integer, status text, failure_reason text);
    create table task_events (id integer primary key autoincrement, task_id text, event_type text, assignment_ref text, payload_json text, created_at text);
    create table delegation_provisioning (task_id text, idempotency_key text, attempt_id text);
    insert into acp_host_sessions (session_id, task_id, project_root) values ${sqlValues([SESSION_ID, TASK_ID, root])};
    insert into tasks (id, project_root) values ${sqlValues([TASK_ID, root])};
    insert into delegation_provisioning (task_id, idempotency_key, attempt_id) values ${sqlValues([TASK_ID, "delegation-ref-1", "xa_exec_1"])};
    insert into task_events (task_id, event_type, assignment_ref, payload_json, created_at) values ${sqlValues([TASK_ID, "worker_result", "xa_exec_1", JSON.stringify({
      summary: "Implemented the fix and ran tests.",
      checks: [{ name: "pytest", outcome: "passed" }, { name: "lint", outcome: "failed", blocker: "unused import" }],
    }), "2026-01-01T00:05:00Z"])};
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

// A known delegation ref resolves to its terminal result, clipped and
// scrubbed checks intact; an unknown ref in the same call resolves to null
// rather than being omitted (the client indexes the response by ref).
{
  const { root, cleanup } = makeFixture();
  try {
    const context = mockContext(WORKSPACE_ID, AGENT_ID, root, SESSION_ID);
    const result = await getWorkerResults({ workspaceId: WORKSPACE_ID, agentId: AGENT_ID, assignmentRefs: ["delegation-ref-1", "unknown-ref"] }, context);
    assert.equal(result.available, true);
    assert.deepEqual(Object.keys(result.results).sort(), ["delegation-ref-1", "unknown-ref"]);
    assert.equal(result.results["unknown-ref"], null);
    const known = result.results["delegation-ref-1"];
    assert.ok(known);
    assert.equal(known!.summary, "Implemented the fix and ran tests.");
    assert.equal(known!.checks.length, 2);
    assert.deepEqual(known!.checks[1], { name: "lint", outcome: "failed", blocker: "unused import" });
  } finally {
    cleanup();
  }
}

// An agent with no bound task resolves every requested ref to null with
// `available: false`, never a thrown error.
{
  const { root, cleanup } = makeFixture();
  try {
    const context = mockContext(WORKSPACE_ID, AGENT_ID, root, null);
    const result = await getWorkerResults({ workspaceId: WORKSPACE_ID, agentId: AGENT_ID, assignmentRefs: ["delegation-ref-1"] }, context);
    assert.equal(result.available, false);
    assert.equal(result.results["delegation-ref-1"], null);
  } finally {
    cleanup();
  }
}

// An empty ref list is a trivial no-op read, not an error.
{
  const { root, cleanup } = makeFixture();
  try {
    const context = mockContext(WORKSPACE_ID, AGENT_ID, root, SESSION_ID);
    const result = await getWorkerResults({ workspaceId: WORKSPACE_ID, agentId: AGENT_ID, assignmentRefs: [] }, context);
    assert.equal(result.available, true);
    assert.deepEqual(result.results, {});
  } finally {
    cleanup();
  }
}

console.log("worker-results: ok");
