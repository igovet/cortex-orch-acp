import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { STOP_COMMAND_CONFIG_ID } from "../shared/command-stop.ts";
import { commandStopSender, unconfirmedStopError } from "./command-stop-registry.ts";
import { readRunningCommandsInStore, stopCommandHandler } from "./command-stop.ts";
import { wrapCortexConnection } from "./provider.ts";
import { execSqlite, sqlValues } from "./test-fixture-db.ts";

/**
 * The Paseo-side stop of one running worker command: the panel RPC handler
 * resolves the agent's own ACP session, the provider sends Cortex's
 * `cortex_stop_command` config value through the bridge's concurrent config
 * channel, and Cortex's acknowledgement or refusal is reported back.
 * No Paseo, Cortex process or model is started.
 */

const REF = `gateway:${"a".repeat(32)}`;
const NATIVE = "acps_0123456789ab";

/** A stand-in for Paseo's ACP bridge; `configure` decides each outcome. */
function bridge(configure: (input: any, emit: (event: any) => void) => void = () => {}) {
  const sent: any[] = [];
  const listeners = new Set<(event: any) => void>();
  const emit = (event: any) => listeners.forEach((listener) => listener(event));
  const connection: any = {
    capabilities: ["prompt.message"],
    async send(input: any) {
      sent.push(input);
      if (input.type === "session.configure") configure(input, emit);
    },
    onEvent(listener: (event: any) => void) {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    async close() {},
  };
  const wrapped = wrapCortexConnection(connection, ["prompt.message", "prompt.steer"]);
  const seen: any[] = [];
  wrapped.onEvent((event) => seen.push(event));
  return { sent, emit, wrapped, seen };
}

function open(emit: (event: any) => void, boundary = "paseo-session-1", native = NATIVE) {
  emit({ type: "session.opened", sessionId: boundary, capabilities: [], restoration: "core",
    persistence: { version: 1, data: { sessionId: native } }, cwd: "/tmp" });
}

function context(native = NATIVE, workspaceId = "ws1"): any {
  const agent = { id: "a1", workspaceId, runtimeInfo: { sessionId: `plugin:${JSON.stringify({ version: 1, data: { sessionId: native } })}` } };
  return { paseo: { agents: { ref: () => ({ refresh: async () => null, current: () => agent }) } } };
}

const input = { workspaceId: "ws1", agentId: "a1", toolCallRef: REF, reason: "  Installing packages is not approved.  ", requestKey: "paseo-stop:k1" };

test("a stop request is Cortex's exact config value on the agent's own session, without interrupting it", async () => {
  const { sent, emit, wrapped, seen } = bridge((request, emit) =>
    queueMicrotask(() => emit({ type: "request.completed", requestId: request.requestId })));
  open(emit);
  emit({ type: "session.turn", sessionId: "paseo-session-1", turnId: "turn-1", state: "started" });
  const result = await stopCommandHandler(input, context());
  assert.deepEqual(result, { status: "requested" });
  assert.equal(sent.length, 1);
  assert.equal(sent[0].type, "session.configure");
  assert.equal(sent[0].sessionId, "paseo-session-1");
  assert.match(sent[0].requestId, /^cortex-stop-command:/);
  assert.deepEqual(Object.keys(sent[0].changes.settings), [STOP_COMMAND_CONFIG_ID]);
  const value = JSON.parse(sent[0].changes.settings[STOP_COMMAND_CONFIG_ID]);
  assert.deepEqual(Object.keys(value), ["toolCallRef", "reason", "requestKey"]);
  assert.deepEqual(value, { toolCallRef: REF, reason: "Installing packages is not approved.", requestKey: "paseo-stop:k1" });
  assert.ok(!sent.some((item) => item.type === "session.interrupt" || item.type === "session.prompt"));
  // The acknowledgement is this bridge's own; Paseo never sees it.
  assert.ok(!seen.some((event) => event.type === "request.completed" || event.type === "request.failed"));
  await wrapped.close();
});

test("Cortex's refusal is reported with its reason and the session is not touched further", async () => {
  const replies = ["Invalid params: command_stop_not_running", "Invalid params: command_stop_session_not_owner", "transport closed"];
  const { sent, emit, wrapped, seen } = bridge((request, emit) =>
    queueMicrotask(() => emit({ type: "request.failed", requestId: request.requestId, error: { message: replies.shift() } })));
  open(emit);
  const notRunning = await stopCommandHandler(input, context());
  assert.equal(notRunning.status, "refused");
  assert.match(notRunning.message!, /already finished/);
  assert.match(notRunning.message!, /Nothing was stopped/);
  const notOwner = await stopCommandHandler(input, context());
  assert.match(notOwner.message!, /no longer owns the Cortex task/);
  const other = await stopCommandHandler(input, context());
  assert.equal(other.status, "refused");
  assert.match(other.message!, /did not accept the stop request \(transport closed\)/);
  assert.equal(sent.length, 3);
  assert.ok(sent.every((item) => item.type === "session.configure"));
  assert.ok(!seen.some((event) => event.type === "request.failed"));
  await wrapped.close();
});

test("no live, matching Cortex session refuses without sending anything", async () => {
  const { sent, emit, wrapped } = bridge();
  const before = await stopCommandHandler(input, context());
  assert.equal(before.status, "refused");
  assert.match(before.message!, /no live Cortex connection/);
  open(emit);
  assert.ok(commandStopSender(NATIVE));
  const wrongWorkspace = await stopCommandHandler(input, context(NATIVE, "ws2"));
  assert.equal(wrongWorkspace.status, "refused");
  assert.match(wrongWorkspace.message!, /not bound to a Cortex session/);
  const otherSession = await stopCommandHandler(input, context("acps_ffffffffffff"));
  assert.match(otherSession.message!, /no live Cortex connection/);
  emit({ type: "session.closed", sessionId: "paseo-session-1" });
  assert.equal(commandStopSender(NATIVE), undefined);
  const closed = await stopCommandHandler(input, context());
  assert.match(closed.message!, /no live Cortex connection/);
  assert.equal(sent.length, 0);
  await wrapped.close();
});

test("closing the connection rejects an unacknowledged stop and forgets its sessions", async () => {
  const { emit, wrapped } = bridge();
  open(emit);
  const pending = stopCommandHandler(input, context());
  await new Promise((resolve) => setImmediate(resolve));
  await wrapped.close();
  const result = await pending;
  assert.equal(result.status, "refused");
  assert.match(result.message!, /closed before the stop request was acknowledged/);
  assert.equal(commandStopSender(NATIVE), undefined);
});

test("an unacknowledged stop is reported as unconfirmed, not as refused or stopped", async () => {
  const result = await stopCommandHandler(input, context(), () => async () => { throw unconfirmedStopError(); });
  assert.equal(result.status, "unconfirmed");
  assert.match(result.message!, /retry/);
});

function fixtureStore(withTables = true): { root: string; cleanup: () => void } {
  const root = mkdtempSync(join(tmpdir(), "cortex-command-stop-test-"));
  mkdirSync(join(root, ".cortex"));
  const db = join(root, ".cortex", "cortex.sqlite3");
  const started = (ref: string, channel: string, argv: string[], id: number) =>
    `insert into runtime_events (task_id, type, actor, payload_json, created_at) values ${sqlValues(["t_1", "COMMAND_PROCESS_STARTED", channel,
      JSON.stringify({ tool_call_ref: ref, operation: "command_run", argv }), `2026-10-18T01:0${id}:00+00:00`])};`;
  execSqlite(db, withTables ? `
    create table runtime_tasks (task_id text primary key, state text);
    create table task_owners (task_id text, epoch integer);
    create table trusted_channels (channel_id text primary key, task_id text, role text, revoked_at text, expires_at text, owner_epoch integer, assignment_id text, attempt_id text);
    create table delegation_provisioning (task_id text, channel_id text, idempotency_key text, attempt_id text);
    create table runtime_events (id integer primary key autoincrement, task_id text, type text, actor text, payload_json text, created_at text);
    insert into runtime_tasks values ${sqlValues(["t_1", "EXECUTING"])};
    insert into task_owners values ${sqlValues(["t_1", 1])};
    insert into trusted_channels values ${sqlValues(["ch_live", "t_1", "worker", null, null, 1, "wa_1", "xa_1"])};
    insert into trusted_channels values ${sqlValues(["ch_revoked", "t_1", "worker", "2026-10-18T00:00:00+00:00", null, 1, "wa_2", "xa_2"])};
    insert into delegation_provisioning values ${sqlValues(["t_1", "ch_live", "assignment-ref-1", "xa_1"])};
    ${started(REF, "ch_live", ["npm", "install", "--prefix", "/home/someone/project"], 1)}
    ${started(`gateway:${"b".repeat(32)}`, "ch_live", ["sleep", "1"], 2)}
    insert into runtime_events (task_id, type, actor, payload_json, created_at) values ${sqlValues(["t_1", "COMMAND_PROCESS_EXITED", "ch_live",
      JSON.stringify({ tool_call_ref: `gateway:${"b".repeat(32)}`, exit_code: 0 }), "2026-10-18T01:05:00+00:00"])};
    ${started(`gateway:${"c".repeat(32)}`, "ch_revoked", ["make"], 3)}
    insert into runtime_events (task_id, type, actor, payload_json, created_at) values ${sqlValues(["t_1", "COMMAND_STOP_REQUESTED", "user:owner_cli",
      JSON.stringify({ tool_call_ref: REF, reason: "Not approved.", origin: "owner_cli", request_key: "k" }), "2026-10-18T01:06:00+00:00"])};
  ` : "create table unrelated (id integer);");
  return { root, cleanup: () => rmSync(root, { recursive: true, force: true }) };
}

test("the running-command list is exact, scrubbed and carries a recorded stop", async () => {
  const { root, cleanup } = fixtureStore();
  try {
    const listed = await readRunningCommandsInStore(root, "t_1");
    assert.equal(listed.available, true);
    assert.equal(listed.commands.length, 1, "exited and revoked-channel commands are not running");
    const [command] = listed.commands;
    assert.equal(command.toolCallRef, REF);
    assert.equal(command.operation, "command_run");
    assert.equal(command.assignmentRef, "assignment-ref-1");
    assert.equal(command.attemptId, "xa_1");
    assert.equal(command.argvSummary, "npm install --prefix [private path]");
    assert.deepEqual(command.stopRequest, { reason: "Not approved.", origin: "owner_cli" });
    assert.equal((await readRunningCommandsInStore(root, "t_1", "assignment-ref-1")).commands.length, 1);
    assert.equal((await readRunningCommandsInStore(root, "t_1", "another-ref")).commands.length, 0);
    assert.equal((await readRunningCommandsInStore(root, "t_other")).commands.length, 0);
  } finally { cleanup(); }
});

test("an older store without command events is reported unavailable, not empty-and-fine", async () => {
  const { root, cleanup } = fixtureStore(false);
  try {
    const listed = await readRunningCommandsInStore(root, "t_1");
    assert.equal(listed.available, false);
    assert.deepEqual(listed.commands, []);
    assert.match(listed.message!, /unavailable/);
  } finally { cleanup(); }
});
