import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { runInNewContext } from "node:vm";
import test from "node:test";
import { ProviderEventSchema } from "@getpaseo/plugin/server/provider";
import { runAcpProvider } from "@getpaseo/plugin/server/acp";
import { wrapCortexConnection } from "./provider.ts";
import { WorkerCardFence } from "./worker-card-fence.ts";

// Optionally exercise the actual installed client reducer, without running
// the app, daemon or an engine. The default model captures its adjacency rule.
const bundlePath = process.argv[2];
let installed: any;
if (bundlePath) {
  const bundle = readFileSync(bundlePath, "utf8");
  const at = bundle.indexOf("_e.applyStreamEvent=function");
  assert.ok(at >= 0, "Installed stream reducer must be observable");
  const start = bundle.lastIndexOf("__d(", at);
  const end = bundle.indexOf("\n__d(", at);
  assert.ok(start >= 0 && end > at);
  let factory: any;
  let ids: number[] = [];
  runInNewContext(bundle.slice(start, end), {
    __d(fn: any, _id: number, imports: number[]) { factory = fn; ids = imports; },
  }, { timeout: 1000 });
  assert.equal(ids.length, 3);
  installed = {};
  const dependencies = [
    { __esModule: true, default: (value: any, keys: string[]) =>
      Object.fromEntries(Object.entries(value).filter(([key]) => !keys.includes(key))) },
    { timelineItemIdentity: (item: any) => item.type === "plugin" ? `${item.pluginId}/${item.id}` : item.callId },
    { extractTaskEntriesFromToolCall: () => null },
  ];
  factory({}, (id: number) => {
    const index = ids.indexOf(id);
    assert.ok(index >= 0);
    return dependencies[index];
  }, undefined, undefined, {}, installed, ids);
}

function hostRows(events: any[]) {
  if (installed) {
    let state: any = { tail: [], head: [] };
    const snapshots = new Map<string, string>();
    for (const event of events) {
      // The daemon adapts cumulative provider snapshots to client deltas.
      let item = event.item;
      if (item?.type === "assistant_message") {
        const previous = snapshots.get(item.id) ?? "";
        snapshots.set(item.id, item.text);
        item = { ...item, text: item.text.slice(previous.length) };
      }
      const host = event.type === "timeline.item" ? { type: "timeline", provider: "cortex", item }
        : event.type === "session.turn" && event.state !== "started" ? { type: `turn_${event.state}` } : null;
      if (host) state = installed.applyStreamEvent({ ...state, event: host, timestamp: new Date(0) });
    }
    return [...state.tail, ...state.head];
  }
  const rows: any[] = [];
  const snapshots = new Map<string, string>();
  for (const event of events) {
    if (event.type !== "timeline.item") continue;
    const item = event.item;
    if (item.type === "assistant_message") {
      const delta = item.text.slice((snapshots.get(item.id) ?? "").length);
      snapshots.set(item.id, item.text);
      const last = rows.at(-1);
      if (last?.kind === "assistant_message" && last.messageId === item.messageId) last.text += delta;
      else rows.push({ kind: "assistant_message", messageId: item.messageId, text: delta });
    } else {
      const key = item.type === "plugin" ? item.id : item.callId;
      const index = rows.findIndex(row => row.key === key);
      const row = { kind: item.type, key, data: item.data };
      if (index < 0) rows.push(row);
      else rows[index] = row;
    }
  }
  return rows;
}

function fixture(fence?: WorkerCardFence) {
  const listeners = new Set<(event: any) => void>();
  const wrapped = wrapCortexConnection({
    capabilities: ["prompt.message"], async send() {}, async close() {},
    onEvent(listener: (event: any) => void) { listeners.add(listener); return () => listeners.delete(listener); },
  } as any, ["prompt.message"], fence);
  const seen: any[] = [];
  wrapped.onEvent(event => seen.push(ProviderEventSchema.parse(event)));
  const emit = (event: any) => listeners.forEach(listener => listener(event));
  const text = (value: string, id = "m", sessionId = "s") => emit({
    type: "timeline.item", sessionId, item: { type: "assistant_message", id, messageId: id, text: value },
  });
  const card = (id = "a", status = "running", sessionId = "s") => emit({
    type: "timeline.item", sessionId, item: { type: "tool_call", id: `worker:${id}`, callId: `worker:${id}`,
      name: "Worker: qa-engineer", status, error: null, detail: { type: "unknown", input: null, output: null } },
  });
  const open = (sessionId = "s") => emit({ type: "session.opened", sessionId, nativeSessionId: sessionId,
    capabilities: ["prompt.message"], config: {}, restoration: "core", cwd: "/fixture" });
  open();
  return { wrapped, seen, emit, text, card, open };
}

test("a worker insertion or update cannot split token-sized coordinator prose", async () => {
  const f = fixture();
  const prose = "Сверяю **результаты** и завершаю проверку.\n\n- exact  \n";
  f.card("existing");
  let value = "";
  for (const [index, token] of [...prose].entries()) {
    value += token;
    f.text(value);
    assert.equal(f.seen.at(-1).item.text, value, "text is never buffered");
    if (index === 3) f.card("a");
    if (index === 5) f.emit({ type: "session.usage", sessionId: "s", usage: { inputTokens: 1 } });
    if (index === 10) f.card("b");
    if (index === 21) f.card("existing", "completed");
    if (index === 25) f.card("a", "completed");
  }
  f.emit({ type: "session.turn", sessionId: "s", turnId: "t", state: "completed" });
  const rows = hostRows(f.seen);
  assert.deepEqual(rows.filter(row => row.kind === "assistant_message").map(row => row.text), [prose]);
  assert.equal(rows.filter(row => row.kind === "plugin").length, 3);
  const cards = f.seen.filter(event => event.type === "timeline.item" && event.item.type === "plugin");
  assert.deepEqual(cards.map(event => event.item.data.callId),
    ["worker:existing", "worker:a", "worker:b", "worker:existing", "worker:a"]);
  // Replaying the exact provider records gives the same rows, not extra text.
  assert.deepEqual(hostRows(JSON.parse(JSON.stringify(f.seen))), rows);
  await f.wrapped.close();
});

test("a completed coordinator message releases workers before the long ACP turn ends", async () => {
  const f = fixture();
  f.text("Delegating ");
  f.card("early");
  f.text("Delegating now.");
  // This zero-text runtime marker ends an engine message, not the ACP
  // prompt: the task can keep running workers for hours without another row.
  f.emit({ type: "timeline.item", sessionId: "s", item: {
    type: "assistant_message", id: "__cortex_message_boundary__:first",
    messageId: "__cortex_message_boundary__:first", text: "",
  } });
  f.card("next");
  f.card("early", "completed");
  const items = f.seen.filter(event => event.type === "timeline.item");
  assert.deepEqual(items.filter(event => event.item.type === "plugin").map(event => event.item.data.callId),
    ["worker:early", "worker:next", "worker:early"]);
  assert.deepEqual(hostRows(f.seen).filter(row => row.kind === "assistant_message").map(row => row.text),
    ["Delegating now."]);
  // No turn completion, new text, timer, reload or close is needed to see them.
  await f.wrapped.close();
});

for (const boundary of ["completed", "canceled", "failed", "ready", "closed", "runtime_failed", "message", "tool", "close"]) {
  test(`pending cards flush on ${boundary} without dropping or duplicating text`, async () => {
    const f = fixture();
    f.text("before ");
    f.card();
    f.text("before and after");
    if (boundary === "message") f.text("new message", "m2");
    else if (boundary === "tool") f.emit({ type: "timeline.item", sessionId: "s",
      item: { type: "tool_call", id: "read", callId: "read", name: "Read", status: "completed",
        error: null, detail: { type: "unknown", input: null, output: null } } });
    else if (boundary === "ready") f.emit({ type: "session.ready", sessionId: "s" });
    else if (boundary === "closed") f.emit({ type: "session.closed", sessionId: "s" });
    else if (boundary === "runtime_failed") f.emit({ type: "session.runtime_failed", sessionId: "s", error: { message: "fixture" } });
    else if (boundary === "close") await f.wrapped.close();
    else f.emit({ type: "session.turn", sessionId: "s", turnId: "t", state: boundary,
      ...(boundary === "failed" ? { error: { message: "fixture" } } : {}) });
    const items = f.seen.filter(event => event.type === "timeline.item");
    assert.deepEqual(items.slice(0, 3).map(event => event.item.type), ["assistant_message", "assistant_message", "plugin"]);
    assert.equal(items.filter(event => event.item.type === "plugin").length, 1);
    if (boundary !== "close") await f.wrapped.close();
  });
}

test("boundaries are session-local, idempotent and never discard nonempty prose", async () => {
  const f = fixture();
  f.text("still writing", "m", "s");
  f.card("waiting");
  f.open("other");
  const boundary = (sessionId: string) => f.emit({ type: "timeline.item", sessionId, item: {
    type: "assistant_message", id: "__cortex_message_boundary__:isolation",
    messageId: "__cortex_message_boundary__:isolation", text: "",
  } });
  boundary("other");
  assert.equal(f.seen.filter(event => event.item?.type === "plugin").length, 0);
  boundary("s");
  boundary("s");
  assert.equal(f.seen.filter(event => event.item?.type === "plugin").length, 1);
  f.text("not a control marker", "__cortex_message_boundary__:nonempty");
  assert.equal(f.seen.at(-1).item.text, "not a control marker");
  await f.wrapped.close();
});

test("the installed SDK carries an empty boundary without a displayed message or turn end", async () => {
  let stream!: ReadableStreamDefaultController<any>;
  const provider = runAcpProvider({ id: "cortex", label: "Cortex", connector: () => ({
    readable: new ReadableStream({ start(controller) { stream = controller; } }),
    writable: new WritableStream({ write(frame: any) {
      const result = frame.method === "initialize" ? { protocolVersion: 1, agentCapabilities: {} }
        : frame.method === "session/new" ? { sessionId: "native" } : {};
      if (frame.id !== undefined) stream.enqueue({ jsonrpc: "2.0", id: frame.id, result });
    } }),
  }) });
  const connection = wrapCortexConnection(await provider.connect({ versions: [1], capabilities: ["prompt.message"] }), []);
  const seen: any[] = [];
  let ready!: () => void;
  let arrived!: () => void;
  const opened = new Promise<void>(resolve => { ready = resolve; });
  const released = new Promise<void>(resolve => { arrived = resolve; });
  connection.onEvent(event => {
    seen.push(ProviderEventSchema.parse(event));
    if (event.type === "session.ready") ready();
    if (event.type === "timeline.item" && event.item.type === "plugin"
      && (event.item.data as { callId?: string }).callId === "worker:late") arrived();
  });
  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    await connection.send({ type: "session.open", requestId: "open", sessionId: "s", history: "replay",
      config: { cwd: "/fixture", env: {}, mcpServers: {}, settings: {}, persist: false } });
    await opened;
    for (const update of [
      { sessionUpdate: "agent_message_chunk", messageId: "m", content: { type: "text", text: "One " } },
      { sessionUpdate: "tool_call", toolCallId: "worker:early", title: "Worker: qa-engineer", kind: "other", status: "pending" },
      { sessionUpdate: "agent_message_chunk", messageId: "m", content: { type: "text", text: "message." } },
      { sessionUpdate: "agent_message_chunk", messageId: "__cortex_message_boundary__:sdk", content: { type: "text", text: "" } },
      { sessionUpdate: "tool_call", toolCallId: "worker:late", title: "Worker: qa-engineer", kind: "other", status: "pending" },
    ]) stream.enqueue({ jsonrpc: "2.0", method: "session/update", params: { sessionId: "native", update } });
    await Promise.race([released, new Promise((_, reject) => {
      timer = setTimeout(() => reject(new Error("Workers stayed hidden after the engine message ended")), 3000);
    })]);
    assert.equal(seen.filter(event => event.item?.type === "plugin").length, 2);
    assert.deepEqual(hostRows(seen).filter(row => row.kind === "assistant_message").map(row => row.text), ["One message."]);
    assert.ok(!seen.some(event => event.item?.messageId?.startsWith("__cortex_message_boundary__:")));
  } finally {
    clearTimeout(timer);
    await connection.close();
  }
});

test("ordering preserves the worker status fence across cancellation and later outcome", async () => {
  const fence = new WorkerCardFence();
  fence.observe({ method: "session/update", params: { update: {
    sessionUpdate: "tool_call", toolCallId: "worker:a", status: "in_progress",
  } } });
  const f = fixture(fence);
  f.text("one ");
  f.card();
  f.text("one sentence");
  f.card("a", "canceled"); // bridge-only turn settlement, not a real worker cancellation
  f.emit({ type: "session.turn", sessionId: "s", turnId: "t", state: "canceled" });
  await Promise.resolve(); // the fence's fallback microtask must not duplicate it
  const cards = () => f.seen.filter(event => event.item?.type === "plugin");
  assert.deepEqual(cards().map(event => event.item.data.status), ["running"]);
  fence.observe({ method: "session/update", params: { update: {
    sessionUpdate: "tool_call_update", toolCallId: "worker:a", status: "completed",
  } } });
  f.card("a", "completed");
  assert.deepEqual(cards().map(event => event.item.data.status), ["running", "completed"]);
  assert.deepEqual(hostRows(f.seen).filter(row => row.kind === "assistant_message").map(row => row.text), ["one sentence"]);
  await f.wrapped.close();
});

test("replay before opened and interleaved sessions use the same ordering", async () => {
  const f = fixture();
  f.text("A", "m2", "replay");
  f.card("r", "completed", "replay");
  f.text("A **B**\n\nC", "m2", "replay");
  f.card("live");
  f.emit({ type: "timeline.item", sessionId: "replay", item: {
    type: "assistant_message", id: "__cortex_message_boundary__:replay",
    messageId: "__cortex_message_boundary__:replay", text: "",
  } });
  f.open("replay");
  assert.equal(f.seen.filter(event => event.sessionId === "replay" && event.item?.type === "plugin").length, 1,
    "the saved boundary releases cards before session.ready");
  f.emit({ type: "session.ready", sessionId: "replay" });
  const rows = hostRows(f.seen.filter(event => event.sessionId === "replay"));
  assert.deepEqual(rows.filter(row => row.kind === "assistant_message").map(row => row.text), ["A **B**\n\nC"]);
  assert.equal(rows.filter(row => row.kind === "plugin").length, 1);
  assert.equal(f.seen.filter(event => event.sessionId === "s" && event.item?.type === "plugin").length, 1);
  await f.wrapped.close();
});
