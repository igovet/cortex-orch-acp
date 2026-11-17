import assert from "node:assert/strict";
import test from "node:test";
import { STOP_COMMAND_CONFIG_ID } from "../shared/command-stop.ts";
import { commandStopSender, unconfirmedStopError } from "./command-stop-registry.ts";
import { stopCommandHandler } from "./command-stop.ts";
import { wrapCortexConnection } from "./provider.ts";

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
