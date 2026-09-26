import assert from "node:assert/strict";
import { mkdtempSync, mkdirSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { loadServerModuleForTest } from "./test-bundle.ts";
import { execSqlite, sqlValues } from "./test-fixture-db.ts";

/**
 * Reproducible load test/regression guard for the root-cause fix in
 * r_6df0ca1ceeb1: `getWorkerHistory` calls issued concurrently for
 * different agents (the real shape of "many worker cards" or several open
 * panels) must overlap rather than serialize one after another behind a
 * blocking `spawnSync`. This exercises the real, unmodified handler code
 * end-to-end (resolveAgentTask + the sqlite query subprocess) against a
 * real fixture store, not a synthetic stand-in for the spawn cost.
 */
const { getWorkerHistory } = await loadServerModuleForTest<typeof import("./worker-history.ts")>(resolve("server/worker-history.ts"));

const WORKSPACE_ID = "workspace-1";

function sessionIdFor(index: number): string {
  return `acps_${index.toString(16).padStart(12, "0")}`;
}

function wrappedSessionId(sessionId: string): string {
  return `plugin:${JSON.stringify({ version: 1, data: { sessionId } })}`;
}

function makeFixture(taskId: string, sessionId: string): { root: string; cleanup: () => void } {
  const root = mkdtempSync(join(tmpdir(), "cortex-history-concurrency-test-"));
  mkdirSync(join(root, ".cortex"));
  const dbPath = join(root, ".cortex", "cortex.sqlite3");
  execSqlite(dbPath, `
    create table acp_host_sessions (session_id text primary key, task_id text, project_root text);
    create table tasks (id text primary key, project_root text);
    create table work_assignments (assignment_id text primary key, task_id text, node_key text, idempotency_key text, status text);
    create table work_assignment_attempts (attempt_id text primary key, assignment_id text, attempt_no integer, status text, failure_reason text);
    create table task_events (id integer primary key autoincrement, task_id text, event_type text, assignment_ref text, payload_json text, created_at text);
    insert into tasks (id, project_root) values ${sqlValues([taskId, root])};
    insert into acp_host_sessions (session_id, task_id, project_root) values ${sqlValues([sessionId, taskId, root])};
  `);
  return { root, cleanup: () => rmSync(root, { recursive: true, force: true }) };
}

const AGENT_COUNT = 12;
const fixtures = Array.from({ length: AGENT_COUNT }, (_, i) => {
  const taskId = `t_conc${String(i).padStart(7, "0")}`;
  const sessionId = sessionIdFor(i);
  const fixture = makeFixture(taskId, sessionId);
  return { ...fixture, taskId, sessionId, agentId: `agent-${i}` };
});

function contextFor(fixture: (typeof fixtures)[number]): any {
  return {
    paseo: {
      workspaces: { ref: () => ({ refresh: async () => {}, directory: fixture.root }) },
      agents: {
        ref: () => ({
          refresh: async () => {},
          current: () => ({ workspaceId: WORKSPACE_ID, runtimeInfo: { sessionId: wrappedSessionId(fixture.sessionId) } }),
        }),
      },
    },
  };
}

try {
  const singleStart = Date.now();
  await getWorkerHistory({ workspaceId: WORKSPACE_ID, agentId: fixtures[0].agentId }, contextFor(fixtures[0]));
  const singleCostMs = Math.max(1, Date.now() - singleStart);

  const concurrentStart = Date.now();
  const results = await Promise.all(fixtures.map((fixture) => getWorkerHistory({ workspaceId: WORKSPACE_ID, agentId: fixture.agentId }, contextFor(fixture))));
  const concurrentTotalMs = Date.now() - concurrentStart;

  for (const result of results) assert.equal(result.available, true, result.message ?? "");

  const serializedFloorMs = AGENT_COUNT * singleCostMs;
  assert.ok(
    concurrentTotalMs < serializedFloorMs * 0.7,
    `expected ${AGENT_COUNT} concurrent getWorkerHistory calls to overlap: took ${concurrentTotalMs}ms, ` +
      `a fully serialized run (the spawnSync-era behavior) would take ~${serializedFloorMs}ms (single call ${singleCostMs}ms)`,
  );
} finally {
  for (const fixture of fixtures) fixture.cleanup();
}

console.log("worker-history-concurrency: ok");
