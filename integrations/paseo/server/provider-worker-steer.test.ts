/**
 * Worker cards across steering, through the real installed ACP bridge.
 *
 * The provider is assembled exactly as Paseo loads it (`createCortexProvider`:
 * replay connector, bridge transformers, presentation wrapper) around a
 * scripted ACP agent fixture. Each prompt's text is the fixture's script.
 * No Cortex runtime, engine, network or paid model call is involved.
 */
import assert from "node:assert/strict";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { ProviderEventSchema } from "@getpaseo/plugin/server/provider";
import { createCortexProvider } from "./provider.ts";
import { WorkerCardFence } from "./worker-card-fence.ts";

// -- The fence itself -------------------------------------------------------
{
  const fence = new WorkerCardFence();
  const frame = (update: Record<string, unknown>) => ({ jsonrpc: "2.0", method: "session/update",
    params: { sessionId: "s", update: { sessionUpdate: "tool_call_update", ...update } } });
  fence.observe(frame({ toolCallId: "worker:a", status: "in_progress" }));
  fence.observe(frame({ toolCallId: "native-read", status: "in_progress" }));
  // An update without a status leaves Cortex's status unchanged.
  fence.observe(frame({ toolCallId: "worker:a", title: "Worker: x — y" }));
  assert.equal(fence.status("worker:a"), "in_progress");
  assert.equal(fence.status("native-read"), undefined, "only worker cards are tracked");
  const snapshot = { id: "worker:a", title: "Worker", status: "failed" as const, input: null, output: null, locations: [] };
  assert.equal(fence.toolCall(snapshot).status, "in_progress", "a merged bridge settlement is restored");
  assert.equal(fence.toolCall({ ...snapshot, id: "native-read" }).status, "failed", "other calls are untouched");
  assert.equal(fence.isSynthesizedTerminal({ type: "tool_call", callId: "worker:a", status: "failed" }), true);
  assert.equal(fence.isSynthesizedTerminal({ type: "tool_call", callId: "worker:a", status: "completed" }), true);
  // Bridge 0.11.x settles a cancelled turn's open calls as `canceled`.
  assert.equal(fence.isSynthesizedTerminal({ type: "tool_call", callId: "worker:a", status: "canceled" }), true);
  assert.equal(fence.isSynthesizedTerminal({ type: "tool_call", callId: "worker:a", status: "running" }), false);
  assert.equal(fence.isSynthesizedTerminal({ type: "tool_call", callId: "worker:unknown", status: "failed" }), false);
  assert.equal(fence.isSynthesizedTerminal({ type: "tool_call", callId: "native-read", status: "failed" }), false);
  fence.observe(frame({ toolCallId: "worker:a", status: "failed" }));
  assert.equal(fence.isSynthesizedTerminal({ type: "tool_call", callId: "worker:a", status: "failed" }), false,
    "Cortex's own failure is never withheld");
  assert.equal(fence.toolCall(snapshot).status, "failed");
  // Terminal is final: a stale or duplicated frame never reopens or flips it.
  fence.observe(frame({ toolCallId: "worker:a", status: "in_progress" }));
  fence.observe(frame({ toolCallId: "worker:a", status: "completed" }));
  assert.equal(fence.status("worker:a"), "failed");
  assert.equal(fence.toolCall({ ...snapshot, status: "in_progress" as const }).status, "failed");
}

// Both opposite-terminal directions preserve the first outcome, not just its status.
for (const [first, opposite] of [["failed", "completed"], ["completed", "failed"]] as const) {
  const fence = new WorkerCardFence();
  const frame = (status: string) => ({ method: "session/update", params: {
    update: { sessionUpdate: "tool_call_update", toolCallId: "worker:terminal", status },
  } });
  fence.observe(frame(first));
  const original = { id: "worker:terminal", status: first, title: "Worker",
    input: null, output: { summary: "First outcome" }, error: first === "failed" ? "First error" : null, locations: [] };
  assert.deepEqual(fence.toolCall(original), original);
  fence.observe(frame(opposite));
  assert.equal(fence.status(original.id), first);
  const stale = { ...original, status: opposite, output: { summary: "Contradiction" }, error: "Stale error" };
  assert.deepEqual(fence.toolCall(stale), original);
  assert.deepEqual(fence.toolCall({ ...stale, status: "in_progress" }), original);
  const enriched = { ...original, output: { summary: "Later detail for the same outcome" } };
  assert.deepEqual(fence.toolCall(enriched), enriched, "matching terminal updates may add detail");
  assert.deepEqual(fence.toolCall(stale), enriched, "a contradiction cannot replace the enriched outcome");
}

// -- Real bridge fixture ------------------------------------------------------
const dir = mkdtempSync(join(tmpdir(), "cortex-worker-steer-"));
const fixture = join(dir, "agent.py");
writeFileSync(fixture, String.raw`
import json, os, sys
if os.environ.get("CORTEX_MODEL_EFFORTS") == "1":
    print("{}", flush=True)
    sys.exit(0)

def send(frame): print(json.dumps(frame, ensure_ascii=False), flush=True)
def update(sid, value): send({"jsonrpc": "2.0", "method": "session/update", "params": {"sessionId": sid, "update": value}})

def card(cid, title, *rest):
    return [{"sessionUpdate": "tool_call", "toolCallId": cid, "title": title, "kind": "other", "status": "pending", "rawInput": {}}, *rest]

JOURNALS = {
    "journal-running": card("worker:w5", "Worker: explorer — Running at reload",
        {"sessionUpdate": "tool_call_update", "toolCallId": "worker:w5", "status": "in_progress"}),
    "journal-contradictory": card("worker:w8", "Worker: tester — First failure",
        {"sessionUpdate": "tool_call_update", "toolCallId": "worker:w8", "status": "failed",
         "rawOutput": "Original worker failure"},
        {"sessionUpdate": "tool_call_update", "toolCallId": "worker:w8", "status": "completed",
         "rawOutput": {"summary": "Stale success", "checks": []}},
        {"sessionUpdate": "tool_call_update", "toolCallId": "worker:w8"}),
    "journal-settled": card("worker:w7", "Worker: explorer — Settled at reload",
        {"sessionUpdate": "tool_call_update", "toolCallId": "worker:w7", "status": "in_progress"},
        {"sessionUpdate": "tool_call_update", "toolCallId": "worker:w7", "status": "completed",
         "rawOutput": {"summary": "Done before reload.", "checks": []}}),
}
pending = None
after_cancel = []

def run(req, sid, steps):
    global pending, after_cancel
    for step in steps:
        if step[0] == "update":
            update(sid, step[1])
        elif step[0] == "wait_cancel":
            pending = (req["id"], sid)
            after_cancel = step[1] if len(step) > 1 else []
            return
        elif step[0] == "end":
            send({"jsonrpc": "2.0", "id": req["id"], "result": {"stopReason": step[1]}})
            return
        elif step[0] == "error":
            send({"jsonrpc": "2.0", "id": req["id"], "error": {"code": -32603, "message": step[1]}})
            return

for line in sys.stdin:
    req = json.loads(line)
    method = req.get("method")
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": req["id"], "result": {"protocolVersion": 1, "agentCapabilities": {"loadSession": True}}})
    elif method == "session/new":
        send({"jsonrpc": "2.0", "id": req["id"], "result": {"sessionId": "native-steer"}})
    elif method == "session/load":
        sid = req["params"]["sessionId"]
        for value in JOURNALS[sid]:
            update(sid, value)
        send({"jsonrpc": "2.0", "id": req["id"], "result": {}})
    elif method == "session/prompt":
        run(req, req["params"]["sessionId"], json.loads(req["params"]["prompt"][0]["text"]))
    elif method == "session/cancel":
        if pending is not None:
            rid, sid = pending
            pending = None
            send({"jsonrpc": "2.0", "id": rid, "result": {"stopReason": "cancelled"}})
            for value in after_cancel:
                update(sid, value)
            after_cancel = []
    elif method == "session/set_config_option":
        # Cortex's durable steering capture (cortex_queued_message).
        send({"jsonrpc": "2.0", "id": req["id"], "result": {"configOptions": []}})
    elif "id" in req:
        send({"jsonrpc": "2.0", "id": req["id"], "result": {}})
`);

delete process.env.CORTEX_PASEO_SOURCE_ROOT;
process.env.CORTEX_PASEO_SHIM = fileURLToPath(new URL("./acp-load-shim.mjs", import.meta.url));
process.env.CORTEX_ACP_COMMAND = JSON.stringify(["python3", "-B", fixture]);

type Event = ReturnType<typeof ProviderEventSchema.parse>;
type Step = ["update", Record<string, unknown>] | ["wait_cancel", Record<string, unknown>[]?] | ["end", string] | ["error", string];

async function waitFor(predicate: () => boolean, what: string, timeoutMs = 10000) {
  const deadline = Date.now() + timeoutMs;
  while (!predicate()) {
    if (Date.now() > deadline) throw new Error(`timed out waiting for ${what}`);
    await new Promise((resolve) => setTimeout(resolve, 10));
  }
}

async function connect(open: { native?: string } = {}) {
  const connection = await createCortexProvider().connect({
    versions: [1], capabilities: ["session.persistence", "prompt.message", "prompt.steer", "session.configure"],
  } as any);
  const events: Event[] = [];
  // Paseo parses every provider event with this schema.
  connection.onEvent((event) => events.push(ProviderEventSchema.parse(event)));
  await connection.send({ type: "session.open", requestId: "open", sessionId: "boundary",
    history: open.native ? "replay" : "skip",
    ...(open.native ? { persistence: { version: 1, data: { sessionId: open.native } } } : {}),
    config: { cwd: dir, env: {}, mcpServers: {}, settings: {}, persist: false } } as any);
  await waitFor(() => events.some((event) => event.type === "session.ready"
    || (event.type === "request.failed" && event.requestId === "open")), "session ready");
  assert.ok(events.some((event) => event.type === "session.ready"), "session opened");
  const prompt = (id: string, steps: Step[], delivery: "auto" | "steer" = "auto") => connection.send({
    type: "session.prompt", sessionId: "boundary",
    prompt: { clientMessageId: id, delivery, input: { type: "message", content: [{ type: "text", text: JSON.stringify(steps) }] } },
  } as any);
  const cards = (callId: string) => events.flatMap((event) =>
    event.type === "timeline.item" && event.item.type === "tool_call" && event.item.callId === callId ? [event.item] : []);
  const turnEnded = (turnId: string, state: string) => events.some((event) =>
    event.type === "session.turn" && event.turnId === turnId && event.state === state);
  return { connection, events, prompt, cards, turnEnded };
}

const created = (callId: string, title: string): Step[] => [
  ["update", { sessionUpdate: "tool_call", toolCallId: callId, title, kind: "other", status: "pending", rawInput: {} }],
  ["update", { sessionUpdate: "tool_call_update", toolCallId: callId, status: "in_progress" }],
];
const progress = (callId: string, text: string): Step =>
  ["update", { sessionUpdate: "tool_call_update", toolCallId: callId, content: [{ type: "content", content: { type: "text", text } }] }];

try {
  // A steered worker keeps one running card until its true outcome.
  {
    const { connection, prompt, cards, turnEnded, events } = await connect();
    const title = "Worker: debugger · claude · Opus · high — Fix the check submission";
    await prompt("m1", [...created("worker:w1", title), ["wait_cancel"]]);
    await waitFor(() => cards("worker:w1").some((item) => item.status === "running"), "running worker card");

    // Default durable steer: captured, the turn and its worker keep running.
    await prompt("steer-1", [], "steer");
    await waitFor(() => events.some((event) => event.type === "session.prompt_result" && event.clientMessageId === "steer-1"),
      "durable steer result");
    const steered = events.find((event) => event.type === "session.prompt_result" && event.clientMessageId === "steer-1");
    assert.deepEqual(steered && "result" in steered ? steered.result : undefined, { type: "steer", turnId: "acp:m1" });
    assert.ok(!events.some((event) => event.type === "session.turn" && event.state !== "started"), "durable steer ends no turn");

    // Paseo's cancel -> prompt pairing: the coordinator turn is cancelled,
    // the worker is not. Twice, then a turn that ends while it still runs.
    await prompt("m2", [progress("worker:w1", "Reading the gateway"), ["wait_cancel"]]);
    await waitFor(() => turnEnded("acp:m1", "canceled"), "first steering cancel");
    await prompt("m3", [progress("worker:w1", "Editing the check"), ["end", "end_turn"]]);
    await waitFor(() => turnEnded("acp:m2", "canceled") && turnEnded("acp:m3", "completed"), "second steering and turn end");
    const whileRunning = cards("worker:w1");
    assert.ok(whileRunning.length >= 4, "progress reached the same card");
    for (const item of whileRunning) {
      assert.equal(item.status, "running", `a still-running worker is never shown ${item.status}: ${item.error}`);
      assert.equal(item.error, null);
    }

    // The same card later reports the worker's real outcome; a stale frame
    // after it does not reopen it.
    await prompt("m4", [
      ["update", { sessionUpdate: "tool_call_update", toolCallId: "worker:w1", status: "completed",
        rawOutput: { summary: "Fixed.", checks: [{ name: "unit tests", outcome: "passed" }] } }],
      ["update", { sessionUpdate: "tool_call_update", toolCallId: "worker:w1", status: "in_progress" }],
      ["end", "end_turn"],
    ]);
    await waitFor(() => turnEnded("acp:m4", "completed"), "outcome turn");
    const all = cards("worker:w1");
    const first = all.findIndex((item) => item.status === "completed");
    assert.ok(first > 0, "the worker's real completion reached its card");
    assert.ok(all.slice(first).every((item) => item.status === "completed"), "a stale frame never reopens the card");
    assert.ok(all.every((item) => item.status !== "failed"));
    assert.equal((all[first].metadata as any)?.cortexWorkerResult?.summary, "Fixed.");
    assert.ok(all.every((item) => item.callId === "worker:w1" && item.name === title), "one card identity");

    // Genuine failures keep their status and reason.
    const crashed = "Worker: tester — Crashes";
    await prompt("m5", [...created("worker:w2", crashed),
      ["update", { sessionUpdate: "tool_call_update", toolCallId: "worker:w2", status: "failed", rawOutput: "Worker crashed: engine exited 1" }],
      ["end", "end_turn"]]);
    await waitFor(() => turnEnded("acp:m5", "completed"), "crash turn");
    assert.equal(cards("worker:w2").at(-1)?.status, "failed");
    assert.equal(cards("worker:w2").at(-1)?.error, "Worker crashed: engine exited 1");
    const bare = "Worker: tester — Fails without output";
    await prompt("m6", [...created("worker:w3", bare),
      ["update", { sessionUpdate: "tool_call_update", toolCallId: "worker:w3", status: "failed" }], ["end", "end_turn"]]);
    await waitFor(() => turnEnded("acp:m6", "completed"), "bare failure turn");
    assert.equal(cards("worker:w3").at(-1)?.status, "failed");
    assert.equal(cards("worker:w3").at(-1)?.error, `Tool call failed: ${bare}`);

    // An explicit lone Stop: the turn ends; the runtime's own pause then
    // cancels the worker with its real reason (a distinct card outcome).
    const stopped = "Worker: explorer — Stopped";
    await prompt("m7", [...created("worker:w4", stopped), ["wait_cancel", [{ sessionUpdate: "tool_call_update",
      toolCallId: "worker:w4", status: "failed", title: `${stopped} · Cancelled`, rawOutput: "Cancelled: turn_interrupted" }]]]);
    await waitFor(() => cards("worker:w4").some((item) => item.status === "running"), "stoppable worker");
    await connection.send({ type: "session.interrupt", requestId: "stop", sessionId: "boundary" } as any);
    await waitFor(() => cards("worker:w4").at(-1)?.error === "Cancelled: turn_interrupted", "real cancellation");
    assert.equal(cards("worker:w4").at(-1)?.status, "failed");
    assert.equal(cards("worker:w4").at(-1)?.name, `${stopped} · Cancelled`);

    // A failed prompt (agent error, transport loss) proves nothing about
    // the worker: the bridge's failure is kept.
    const lost = "Worker: explorer — Lost with its turn";
    await prompt("m8", [...created("worker:w6", lost), ["error", "agent prompt failed"]]);
    await waitFor(() => turnEnded("acp:m8", "failed"), "failed turn");
    assert.equal(cards("worker:w6").at(-1)?.status, "failed");
    assert.equal(cards("worker:w6").at(-1)?.error, `Tool call failed: ${lost}`);
    await connection.close();
  }

  // History reload replays Cortex's saved cards unchanged.
  {
    const running = await connect({ native: "journal-running" });
    assert.ok(running.cards("worker:w5").length >= 2);
    assert.ok(running.cards("worker:w5").every((item) => item.status === "running"));
    await running.connection.close();
    const settled = await connect({ native: "journal-settled" });
    assert.equal(settled.cards("worker:w7").at(-1)?.status, "completed");
    assert.equal((settled.cards("worker:w7").at(-1)?.metadata as any)?.cortexWorkerResult?.summary, "Done before reload.");
    await settled.connection.close();
    const contradictory = await connect({ native: "journal-contradictory" });
    const replayed = contradictory.cards("worker:w8");
    const firstFailure = replayed.findIndex((item) => item.status === "failed");
    assert.ok(firstFailure >= 0);
    const outcomes = replayed.slice(firstFailure);
    assert.ok(outcomes.length >= 3);
    for (const item of outcomes) {
      assert.equal(item.status, "failed");
      assert.equal(item.error, "Original worker failure");
      assert.equal((item.metadata as any)?.cortexWorkerResult, undefined, "no stale success payload");
    }
    await contradictory.connection.close();
  }
} finally {
  rmSync(dir, { recursive: true, force: true });
}
