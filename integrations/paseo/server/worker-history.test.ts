import assert from "node:assert/strict";
import { mkdtempSync, mkdirSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { loadServerModuleForTest } from "./test-bundle.ts";
import { execSqlite, sqlValues } from "./test-fixture-db.ts";
import { tokensSummaryLabel } from "../client/worker-display.ts";

const { getWorkerHistory, parsePaseoRuntimeSessionId, resolveAgentTask } = await loadServerModuleForTest<typeof import("./worker-history.ts")>(resolve("server/worker-history.ts"));

const SESSION_ID = "acps_0123456789ab";
const WORKSPACE_ID = "workspace-1";
const AGENT_ID = "agent-1";
const TASK_ID = "t_test0000001";

function wrappedSessionId(sessionId: string): string {
  return `plugin:${JSON.stringify({ version: 1, data: { sessionId } })}`;
}

function makeFixture(): { root: string; cleanup: () => void } {
  const root = mkdtempSync(join(tmpdir(), "cortex-worker-history-test-"));
  mkdirSync(join(root, ".cortex"));
  const dbPath = join(root, ".cortex", "cortex.sqlite3");
  execSqlite(dbPath, `
    create table acp_host_sessions (session_id text primary key, task_id text, project_root text);
    create table tasks (id text primary key, project_root text);
    create table work_assignments (assignment_id text primary key, task_id text, node_key text, idempotency_key text, status text);
    create table work_assignment_attempts (attempt_id text primary key, assignment_id text, attempt_no integer, status text, failure_reason text);
    create table task_events (id integer primary key autoincrement, task_id text, event_type text, assignment_ref text, payload_json text, created_at text);
    insert into acp_host_sessions (session_id, task_id, project_root) values ${sqlValues([SESSION_ID, TASK_ID, root])};
    insert into tasks (id, project_root) values ${sqlValues([TASK_ID, root])};
    insert into work_assignments (assignment_id, task_id, node_key, idempotency_key, status) values ${sqlValues(["wa_1", TASK_ID, "node-1", "idem-1", "completed"])};
    insert into work_assignment_attempts (attempt_id, assignment_id, attempt_no, status, failure_reason) values ${sqlValues(["xa_1", "wa_1", 1, "completed", null])};
    insert into task_events (task_id, event_type, assignment_ref, payload_json, created_at) values ${sqlValues([TASK_ID, "delegation_requested", "wa_1", JSON.stringify({ title: "Fix the bug", role: "worker-fullstack-dev" }), "2026-01-01T00:00:00Z"])};
    insert into task_events (task_id, event_type, assignment_ref, payload_json, created_at) values ${sqlValues([TASK_ID, "worker_result", "xa_1", JSON.stringify({ summary: "Done", checks: [] }), "2026-01-01T00:05:00Z"])};
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

// `parsePaseoRuntimeSessionId` accepts the exact wrapped v1 shape and
// rejects malformed/ambiguous wrappers -- the identity boundary the rest of
// this module trusts.
{
  assert.equal(parsePaseoRuntimeSessionId({ sessionId: wrappedSessionId(SESSION_ID) }), SESSION_ID);
  assert.equal(parsePaseoRuntimeSessionId({ sessionId: "plugin:not-json" }), null);
  assert.equal(parsePaseoRuntimeSessionId({ sessionId: `plugin:${JSON.stringify({ version: 2, data: { sessionId: SESSION_ID } })}` }), null);
  assert.equal(parsePaseoRuntimeSessionId(null), null);
  assert.equal(parsePaseoRuntimeSessionId({}), null);
}

// End-to-end: resolveAgentTask finds the project root and bound task from a
// real (fixture) store, and getWorkerHistory reads back the recorded
// assignment with its title/role/status/events.
{
  const { root, cleanup } = makeFixture();
  try {
    const context = mockContext(WORKSPACE_ID, AGENT_ID, root, SESSION_ID);
    const resolved = await resolveAgentTask({ workspaceId: WORKSPACE_ID, agentId: AGENT_ID }, context);
    assert.ok(!("message" in resolved), `expected a resolved task, got ${JSON.stringify(resolved)}`);
    if (!("message" in resolved)) {
      assert.equal(resolved.projectRoot, root);
      assert.equal(resolved.taskId, TASK_ID);
    }

    const history = await getWorkerHistory({ workspaceId: WORKSPACE_ID, agentId: AGENT_ID }, context);
    assert.equal(history.available, true);
    assert.equal(history.assignments.length, 1);
    assert.equal(history.assignments[0].title, "Fix the bug");
    assert.equal(history.assignments[0].role, "fullstack-dev");
    assert.equal(history.assignments[0].status, "completed");
  } finally {
    cleanup();
  }
}

// An agent with no bound session is reported unavailable with a message,
// never a thrown error or an empty-but-`available: true` response.
{
  const { root, cleanup } = makeFixture();
  try {
    const context = mockContext(WORKSPACE_ID, AGENT_ID, root, null);
    const history = await getWorkerHistory({ workspaceId: WORKSPACE_ID, agentId: AGENT_ID }, context);
    assert.equal(history.available, false);
    assert.ok(history.message);
  } finally {
    cleanup();
  }
}

// The identity resolution cache: a second call for the same workspace+agent
// with an unchanged directory/session must not need any process disclosed
// by "message" mismatch or wrong project (regression guard for the cache
// added to remove the two now-redundant identity spawns per repeat call).
{
  const { root, cleanup } = makeFixture();
  try {
    const context = mockContext(WORKSPACE_ID, AGENT_ID, root, SESSION_ID);
    const first = await resolveAgentTask({ workspaceId: WORKSPACE_ID, agentId: AGENT_ID }, context);
    const second = await resolveAgentTask({ workspaceId: WORKSPACE_ID, agentId: AGENT_ID }, context);
    assert.deepEqual(first, second);
  } finally {
    cleanup();
  }
}

// A workspace whose directory changed underneath the same agent id (e.g. a
// re-created workspace) is not served the stale cached project root.
{
  const fixtureA = makeFixture();
  const fixtureB = makeFixture();
  try {
    const contextA = mockContext(WORKSPACE_ID, AGENT_ID, fixtureA.root, SESSION_ID);
    const first = await resolveAgentTask({ workspaceId: WORKSPACE_ID, agentId: AGENT_ID }, contextA);
    assert.ok(!("message" in first));

    const contextB = mockContext(WORKSPACE_ID, AGENT_ID, fixtureB.root, SESSION_ID);
    const second = await resolveAgentTask({ workspaceId: WORKSPACE_ID, agentId: AGENT_ID }, contextB);
    assert.ok(!("message" in second));
    if (!("message" in first) && !("message" in second)) {
      assert.equal(first.projectRoot, fixtureA.root);
      assert.equal(second.projectRoot, fixtureB.root);
    }
  } finally {
    fixtureA.cleanup();
    fixtureB.cleanup();
  }
}

// A worker process absent because a host disconnect (e.g. a plugin update)
// interrupted it is reported "interrupted", never the same "failed" pill as
// a genuine crash; a process absent with no recorded disconnect stays
// "failed" (regression). Closed/weak-evidence check fields (delegation.
// evidence.accept_checks's `closed_by`/`claimed_outcome`/`evidence_gap`)
// pass through the runtime_nodes projection unmodified.
function makeInterruptFixture(): { root: string; cleanup: () => void } {
  const root = mkdtempSync(join(tmpdir(), "cortex-worker-history-interrupt-test-"));
  mkdirSync(join(root, ".cortex"));
  const dbPath = join(root, ".cortex", "cortex.sqlite3");
  const provisioning = (idempotencyKey: string, attemptId: string) => sqlValues([
    idempotencyKey, TASK_ID, "wa_x", attemptId, "worker-fullstack-dev", "execution", "provisioned",
    1, "2026-01-01T00:00:00Z", "2026-01-01T00:00:00Z",
  ]);
  const process = (attemptId: string, terminated: string | null) => sqlValues([
    attemptId, TASK_ID, "worker", 1, 1, "1", "boot", "digest", "2026-01-01T00:00:00Z", terminated,
  ]);
  execSqlite(dbPath, `
    create table acp_host_sessions (session_id text primary key, task_id text, project_root text);
    create table tasks (id text primary key, project_root text);
    create table work_assignments (assignment_id text primary key, task_id text, node_key text, idempotency_key text, status text);
    create table work_assignment_attempts (attempt_id text primary key, assignment_id text, attempt_no integer, status text, failure_reason text);
    create table task_events (id integer primary key autoincrement, task_id text, event_type text, assignment_ref text, payload_json text, created_at text);
    create table delegation_provisioning (idempotency_key text primary key, task_id text, assignment_id text, attempt_id text, role_id text, step text, status text, owner_epoch integer, created_at text, updated_at text);
    create table attempt_process_registry (attempt_id text primary key, task_id text, role text, pid integer, pgid integer, start_ticks text, boot_id text, launch_command_digest text, recorded_at text, terminated_at text);
    create table runtime_tasks (task_id text primary key, state text, revision integer, goal text, plan_json text, synthesis text, review_cycle integer, blocker text, created_at text, updated_at text);
    create table runtime_nodes (task_id text, revision integer, node_id text, role text, objective text, dependencies_json text, scope_json text, stage text, status text, assignment_ref text, result_json text);
    insert into acp_host_sessions (session_id, task_id, project_root) values ${sqlValues([SESSION_ID, TASK_ID, root])};
    insert into tasks (id, project_root) values ${sqlValues([TASK_ID, root])};

    -- wa_interrupted: an execution attempt a host disconnect paused, process now provably absent.
    insert into work_assignments (assignment_id, task_id, node_key, idempotency_key, status) values ${sqlValues(["wa_interrupted", TASK_ID, "node-interrupted", "idem-interrupted", "running"])};
    insert into work_assignment_attempts (attempt_id, assignment_id, attempt_no, status, failure_reason) values ${sqlValues(["xa_interrupted", "wa_interrupted", 1, "running", null])};
    insert into task_events (task_id, event_type, assignment_ref, payload_json, created_at) values ${sqlValues([TASK_ID, "delegation_requested", "req-interrupted", JSON.stringify({ title: "Interrupted worker", role: "worker-fullstack-dev", work_attempt_ref: "xa_interrupted" }), "2026-01-01T00:00:00Z"])};
    insert into delegation_provisioning (idempotency_key, task_id, assignment_id, attempt_id, role_id, step, status, owner_epoch, created_at, updated_at) values ${provisioning("req-interrupted", "exec-interrupted")};
    insert into attempt_process_registry (attempt_id, task_id, role, pid, pgid, start_ticks, boot_id, launch_command_digest, recorded_at, terminated_at) values ${process("exec-interrupted", "2026-01-01T00:10:00Z")};
    insert into task_events (task_id, event_type, assignment_ref, payload_json, created_at) values ${sqlValues([TASK_ID, "progress", null, JSON.stringify({ kind: "host_disconnected", interrupted_workers: [{ attempt_id: "exec-interrupted" }] }), "2026-01-01T00:09:00Z"])};

    -- wa_reload_interrupted: an in-place plugin/page reload -- the host
    -- disconnected this worker's session, but no attempt_process_registry
    -- row was ever recorded absent for it (the daemon process was never
    -- confirmed dead, only the transport was). Recovery itself never waits
    -- for that stronger signal either, so display must not either.
    insert into work_assignments (assignment_id, task_id, node_key, idempotency_key, status) values ${sqlValues(["wa_reload_interrupted", TASK_ID, "node-reload-interrupted", "idem-reload-interrupted", "running"])};
    insert into work_assignment_attempts (attempt_id, assignment_id, attempt_no, status, failure_reason) values ${sqlValues(["xa_reload_interrupted", "wa_reload_interrupted", 1, "running", null])};
    insert into task_events (task_id, event_type, assignment_ref, payload_json, created_at) values ${sqlValues([TASK_ID, "delegation_requested", "req-reload-interrupted", JSON.stringify({ title: "Reload interrupted worker", role: "worker-fullstack-dev", work_attempt_ref: "xa_reload_interrupted" }), "2026-01-01T00:00:00Z"])};
    insert into delegation_provisioning (idempotency_key, task_id, assignment_id, attempt_id, role_id, step, status, owner_epoch, created_at, updated_at) values ${provisioning("req-reload-interrupted", "exec-reload-interrupted")};
    insert into task_events (task_id, event_type, assignment_ref, payload_json, created_at) values ${sqlValues([TASK_ID, "progress", null, JSON.stringify({ kind: "host_disconnected", interrupted_workers: [{ attempt_id: "exec-reload-interrupted" }] }), "2026-01-01T00:09:00Z"])};

    -- wa_crashed: an execution attempt whose process is absent with no recorded disconnect.
    insert into work_assignments (assignment_id, task_id, node_key, idempotency_key, status) values ${sqlValues(["wa_crashed", TASK_ID, "node-crashed", "idem-crashed", "running"])};
    insert into work_assignment_attempts (attempt_id, assignment_id, attempt_no, status, failure_reason) values ${sqlValues(["xa_crashed", "wa_crashed", 1, "running", null])};
    insert into task_events (task_id, event_type, assignment_ref, payload_json, created_at) values ${sqlValues([TASK_ID, "delegation_requested", "req-crashed", JSON.stringify({ title: "Crashed worker", role: "worker-fullstack-dev", work_attempt_ref: "xa_crashed" }), "2026-01-01T00:00:00Z"])};
    insert into delegation_provisioning (idempotency_key, task_id, assignment_id, attempt_id, role_id, step, status, owner_epoch, created_at, updated_at) values ${provisioning("req-crashed", "exec-crashed")};
    insert into attempt_process_registry (attempt_id, task_id, role, pid, pgid, start_ticks, boot_id, launch_command_digest, recorded_at, terminated_at) values ${process("exec-crashed", "2026-01-01T00:10:00Z")};

    -- wa_checks: a completed node whose retained checks carry closed_by/claimed_outcome/evidence_gap.
    insert into work_assignments (assignment_id, task_id, node_key, idempotency_key, status) values ${sqlValues(["wa_checks", TASK_ID, "node-checks", "idem-checks", "completed"])};
    insert into work_assignment_attempts (attempt_id, assignment_id, attempt_no, status, failure_reason) values ${sqlValues(["xa_checks", "wa_checks", 1, "completed", null])};
    insert into task_events (task_id, event_type, assignment_ref, payload_json, created_at) values ${sqlValues([TASK_ID, "delegation_requested", "req-checks", JSON.stringify({ title: "Checked worker", role: "worker-fullstack-dev", work_attempt_ref: "xa_checks" }), "2026-01-01T00:00:00Z"])};
    insert into delegation_provisioning (idempotency_key, task_id, assignment_id, attempt_id, role_id, step, status, owner_epoch, created_at, updated_at) values ${provisioning("req-checks", "exec-checks")};
    insert into task_events (task_id, event_type, assignment_ref, payload_json, created_at) values ${sqlValues([TASK_ID, "worker_result", "xa_checks", JSON.stringify({ summary: "Done with history", checks: [] }), "2026-01-01T00:05:00Z"])};
    insert into runtime_tasks (task_id, state, revision, goal, review_cycle, created_at, updated_at) values ${sqlValues([TASK_ID, "EXECUTING", 1, "Goal", 0, "2026-01-01T00:00:00Z", "2026-01-01T00:00:00Z"])};
    insert into runtime_nodes (task_id, revision, node_id, role, objective, dependencies_json, scope_json, stage, status, assignment_ref, result_json) values ${sqlValues([
      TASK_ID, 1, "n-checks", "worker-fullstack-dev", "Objective", "[]", "[]", "execution", "completed", "req-checks",
      JSON.stringify({ summary: "Done with history", checks: [
        { name: "unit tests", outcome: "failed", closed_by: "gateway:cmd:2" },
        { name: "typecheck", outcome: "unverified", claimed_outcome: "passed", evidence_gap: "tool_call_ref_missing" },
      ] }),
    ])};
  `);
  return { root, cleanup: () => rmSync(root, { recursive: true, force: true }) };
}

{
  const { root, cleanup } = makeInterruptFixture();
  try {
    const context = mockContext(WORKSPACE_ID, AGENT_ID, root, SESSION_ID);
    const history = await getWorkerHistory({ workspaceId: WORKSPACE_ID, agentId: AGENT_ID }, context);
    assert.equal(history.available, true);
    const byTitle = (title: string) => history.assignments.find((assignment) => assignment.title === title);

    const interrupted = byTitle("Interrupted worker");
    assert.ok(interrupted, "expected the interrupted assignment to be present");
    const interruptedAttempt = interrupted!.attempts[interrupted!.attempts.length - 1];
    assert.equal(interruptedAttempt.status, "interrupted");
    assert.match(interruptedAttempt.failureReason ?? "", /^Interrupted when the host connection closed/);

    const reloadInterrupted = byTitle("Reload interrupted worker");
    assert.ok(reloadInterrupted, "expected the reload-interrupted assignment to be present");
    const reloadInterruptedAttempt = reloadInterrupted!.attempts[reloadInterrupted!.attempts.length - 1];
    assert.equal(reloadInterruptedAttempt.status, "interrupted");
    assert.match(reloadInterruptedAttempt.failureReason ?? "", /^Interrupted when the host connection closed/);

    const crashed = byTitle("Crashed worker");
    assert.ok(crashed, "expected the crashed assignment to be present");
    const crashedAttempt = crashed!.attempts[crashed!.attempts.length - 1];
    assert.equal(crashedAttempt.status, "failed");
    assert.match(crashedAttempt.failureReason ?? "", /^Worker process exited before reporting a result\.$/);

    const checked = byTitle("Checked worker");
    assert.ok(checked, "expected the checked assignment to be present");
    const checkedAttempt = checked!.attempts[checked!.attempts.length - 1];
    assert.deepEqual(checkedAttempt.checks, [
      { name: "unit tests", outcome: "failed", closedBy: "gateway:cmd:2" },
      { name: "typecheck", outcome: "unverified", claimedOutcome: "passed", evidenceGap: "tool_call_ref_missing" },
    ]);
  } finally {
    cleanup();
  }
}

// A host lost with no signal (SIGKILL/OOM, no host_disconnected record):
// the launching Cortex process recorded beside the worker is provably gone,
// so the worker reads "interrupted", not a crash; a live launching process
// keeps it running. Recovery's recovery_worker_lost write-off and the replan
// cancellation after it stay "interrupted", in the task's language.
function makeHostLossFixture(): { root: string; cleanup: () => void } {
  const root = mkdtempSync(join(tmpdir(), "cortex-worker-history-host-loss-test-"));
  mkdirSync(join(root, ".cortex"));
  const dbPath = join(root, ".cortex", "cortex.sqlite3");
  const bootId = readFileSync("/proc/sys/kernel/random/boot_id", "utf8").trim();
  const stat = readFileSync(`/proc/${process.pid}/stat`, "utf8");
  const ownTicks = stat.slice(stat.lastIndexOf(")") + 2).split(" ")[19];
  const provisioning = (idempotencyKey: string, attemptId: string) => sqlValues([
    idempotencyKey, TASK_ID, "wa_x", attemptId, "worker-fullstack-dev", "execution", "provisioned",
    1, "2026-01-01T00:00:00Z", "2026-01-01T00:00:00Z",
  ]);
  const registry = (attemptId: string, role: string, pid: number, ticks: string, boot: string) => sqlValues([
    attemptId, TASK_ID, role, pid, pid, ticks, boot, role, "2026-01-01T00:00:00Z", null,
  ]);
  const running = (name: string, title: string) => `
    insert into work_assignments (assignment_id, task_id, node_key, idempotency_key, status) values ${sqlValues([`wa_${name}`, TASK_ID, `node-${name}`, `idem-${name}`, "running"])};
    insert into work_assignment_attempts (attempt_id, assignment_id, attempt_no, status, failure_reason) values ${sqlValues([`xa_${name}`, `wa_${name}`, 1, "running", null])};
    insert into task_events (task_id, event_type, assignment_ref, payload_json, created_at) values ${sqlValues([TASK_ID, "delegation_requested", `req-${name}`, JSON.stringify({ title, role: "worker-fullstack-dev", work_attempt_ref: `xa_${name}` }), "2026-01-01T00:00:00Z"])};
    insert into delegation_provisioning (idempotency_key, task_id, assignment_id, attempt_id, role_id, step, status, owner_epoch, created_at, updated_at) values ${provisioning(`req-${name}`, `exec-${name}`)};`;
  execSqlite(dbPath, `
    create table acp_host_sessions (session_id text primary key, task_id text, project_root text);
    create table tasks (id text primary key, project_root text);
    create table work_assignments (assignment_id text primary key, task_id text, node_key text, idempotency_key text, status text);
    create table work_assignment_attempts (attempt_id text primary key, assignment_id text, attempt_no integer, status text, failure_reason text);
    create table task_events (id integer primary key autoincrement, task_id text, event_type text, assignment_ref text, payload_json text, created_at text);
    create table delegation_provisioning (idempotency_key text primary key, task_id text, assignment_id text, attempt_id text, role_id text, step text, status text, owner_epoch integer, created_at text, updated_at text);
    create table attempt_process_registry (attempt_id text primary key, task_id text, role text, pid integer, pgid integer, start_ticks text, boot_id text, launch_command_digest text, recorded_at text, terminated_at text);
    insert into acp_host_sessions (session_id, task_id, project_root) values ${sqlValues([SESSION_ID, TASK_ID, root])};
    insert into tasks (id, project_root) values ${sqlValues([TASK_ID, root])};
    insert into task_events (task_id, event_type, assignment_ref, payload_json, created_at) values ${sqlValues([TASK_ID, "user_message", null, JSON.stringify({ content: "Запусти долгую команду" }), "2026-01-01T00:00:00Z"])};
    ${running("owner-lost", "Owner lost worker")}
    insert into attempt_process_registry (attempt_id, task_id, role, pid, pgid, start_ticks, boot_id, launch_command_digest, recorded_at, terminated_at) values ${registry("exec-owner-lost", "worker", 1, "1", "previous-boot")};
    insert into attempt_process_registry (attempt_id, task_id, role, pid, pgid, start_ticks, boot_id, launch_command_digest, recorded_at, terminated_at) values ${registry("acp_host:exec-owner-lost", "acp_host", 1, "1", "previous-boot")};
    ${running("owner-alive", "Owner alive worker")}
    insert into attempt_process_registry (attempt_id, task_id, role, pid, pgid, start_ticks, boot_id, launch_command_digest, recorded_at, terminated_at) values ${registry("exec-owner-alive", "worker", process.pid, ownTicks, bootId)};
    insert into attempt_process_registry (attempt_id, task_id, role, pid, pgid, start_ticks, boot_id, launch_command_digest, recorded_at, terminated_at) values ${registry("acp_host:exec-owner-alive", "acp_host", process.pid, ownTicks, bootId)};
    ${running("written-off", "Written off worker")}
    insert into task_events (task_id, event_type, assignment_ref, payload_json, created_at) values ${sqlValues([TASK_ID, "progress", null, JSON.stringify({ kind: "host_disconnected", interrupted_workers: [{ attempt_id: "exec-written-off" }] }), "2026-01-01T00:09:00Z"])};
    insert into task_events (task_id, event_type, assignment_ref, payload_json, created_at) values ${sqlValues([TASK_ID, "assignment_failed", "req-written-off", JSON.stringify({ error: "recovery_worker_lost", assignment_ref: "req-written-off", attempt_id: "exec-written-off", prior_state: "absent" }), "2026-01-01T00:10:00Z"])};
    insert into task_events (task_id, event_type, assignment_ref, payload_json, created_at) values ${sqlValues([TASK_ID, "assignment_cancelled", "req-written-off", JSON.stringify({ reason: "replanning" }), "2026-01-01T00:10:01Z"])};
  `);
  return { root, cleanup: () => rmSync(root, { recursive: true, force: true }) };
}

{
  const { root, cleanup } = makeHostLossFixture();
  try {
    const context = mockContext(WORKSPACE_ID, AGENT_ID, root, SESSION_ID);
    const history = await getWorkerHistory({ workspaceId: WORKSPACE_ID, agentId: AGENT_ID }, context);
    const latest = (title: string) => {
      const assignment = history.assignments.find((item) => item.title === title);
      assert.ok(assignment, `expected ${title}`);
      return assignment!.attempts[assignment!.attempts.length - 1];
    };
    const russian = "Прервано закрытием соединения с хостом; результата ещё нет. Следующее сообщение восстановит этого исполнителя из сохранённого состояния.";
    assert.equal(latest("Owner lost worker").status, "interrupted");
    assert.equal(latest("Owner lost worker").failureReason, russian);
    assert.equal(latest("Owner alive worker").status, "running");
    assert.equal(latest("Owner alive worker").failureReason, null);
    assert.equal(latest("Written off worker").status, "interrupted");
    assert.equal(latest("Written off worker").failureReason, russian);
    // The chat card polls this exact assignment and settles on it.
    const card = await getWorkerHistory({ workspaceId: WORKSPACE_ID, agentId: AGENT_ID, assignmentRef: "req-owner-lost", cursor: Number.MAX_SAFE_INTEGER, limit: 1 }, context);
    assert.equal(card.assignments[0]?.attempts.at(-1)?.status, "interrupted");
  } finally {
    cleanup();
  }
}

// Historical usage CHECK accepts the post-result marker in existing columns;
// the actual card projection suppresses only that marker, not genuine misses.
{
  const { root, cleanup } = makeFixture();
  const dbPath = join(root, ".cortex", "cortex.sqlite3");
  try {
    execSqlite(dbPath, `
      create table delegation_provisioning (idempotency_key text primary key, task_id text, attempt_id text);
      create table usage_records (task_id text, attempt_id text,
        basis text check(basis in ('observed','engine_reported','derived','estimated','unknown')),
        tokens_json text, stop_reason text, failed integer);
      insert into delegation_provisioning values ${sqlValues(["req-usage", TASK_ID, "exec-usage"])};
      insert into task_events (task_id,event_type,assignment_ref,payload_json,created_at) values
        ${sqlValues([TASK_ID, "delegation_requested", "req-usage", JSON.stringify({ work_attempt_ref: "xa_1" }), "now"])};
      insert into usage_records values ${sqlValues([TASK_ID, "exec-usage", "unknown", "{}", "released_after_result", 0])};
    `);
    const context = mockContext(WORKSPACE_ID, AGENT_ID, root, SESSION_ID);
    const card = async () => {
      const history = await getWorkerHistory({ workspaceId: WORKSPACE_ID, agentId: AGENT_ID, assignmentRef: "req-usage" }, context);
      assert.equal(history.available, true);
      assert.equal(history.assignments.length, 1);
      return history.assignments[0].attempts.at(-1)!;
    };
    assert.equal((await card()).tokens, undefined);
    assert.equal(tokensSummaryLabel((await card()).tokens), null);
    execSqlite(dbPath, `
      insert into usage_records values ${sqlValues([TASK_ID, "exec-usage", "engine_reported", '{"inputTokens":10,"outputTokens":5}', "released_after_result", 0])};
    `);
    assert.equal((await card()).tokens?.knownCalls, 1);
    assert.equal((await card()).tokens?.unknownCalls, 0);
    assert.doesNotMatch(tokensSummaryLabel((await card()).tokens)!, /unavailable/);
    execSqlite(dbPath, `
      insert into usage_records values ${sqlValues([TASK_ID, "exec-usage", "unknown", "{}", "end_turn", 0])};
      insert into usage_records values ${sqlValues([TASK_ID, "exec-usage", "unknown", "{}", "released_after_result", 1])};
    `);
    const tokens = (await card()).tokens;
    assert.equal(tokens?.total, 15);
    assert.equal(tokens?.knownCalls, 1);
    assert.equal(tokens?.unknownCalls, 2);
    assert.match(tokensSummaryLabel(tokens)!, /2 calls unavailable/);
  } finally {
    cleanup();
  }
}

console.log("worker-history: ok");
