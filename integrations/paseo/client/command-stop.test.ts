import assert from "node:assert/strict";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";
import { build } from "esbuild";
import {
  startStop, stopRequestInput, stopResultState, stoppedCommand, submitStop, validateStopReason,
} from "./command-stop.ts";

/**
 * The worker panel's stop control for a running worker command: the reason
 * model, the exact `cortex.commands.stop` payload, pending/refused/
 * unconfirmed states, and the "stopped by user" command row. The real
 * components (bundled like the plugin client) render with a small hook
 * runtime and host-module shims; this proves element trees and state
 * transitions, not rendered pixels in Paseo.
 */

const pluginRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const REF = `gateway:${"a".repeat(32)}`;

// --- pure model ------------------------------------------------------------------

test("a reason is required, non-blank and bounded", () => {
  assert.equal(validateStopReason("").ok, false);
  assert.equal(validateStopReason("   \n ").ok, false);
  assert.equal(validateStopReason("x".repeat(2001)).ok, false);
  assert.deepEqual(validateStopReason("  not approved \n"), { ok: true, reason: "not approved" });
});

test("a retry keeps its key only while the reason is unchanged", () => {
  let n = 0;
  const key = () => `k${++n}`;
  const blank = submitStop(startStop(), key);
  assert.equal(blank.phase, "editing");
  assert.match(blank.message!, /Enter a reason/);
  const first = submitStop({ ...startStop(), reason: " Too slow " }, key);
  assert.equal(first.phase, "sending");
  assert.deepEqual(stopRequestInput("ws1", "a1", REF, first), { workspaceId: "ws1", agentId: "a1", toolCallRef: REF, reason: "Too slow", requestKey: "k1" });
  const refused = stopResultState(first, { status: "refused", message: "Cortex did not accept the stop request." });
  assert.equal(refused.phase, "refused");
  assert.equal(submitStop(refused, key).requestKey, "k1");
  const unconfirmed = stopResultState(first, { status: "unconfirmed", message: "retry to check" });
  assert.equal(unconfirmed.phase, "unconfirmed");
  assert.equal(submitStop(unconfirmed, key).requestKey, "k1");
  assert.equal(submitStop({ ...refused, reason: "Different reason" }, key).requestKey, "k2");
  assert.equal(stopResultState(first, { status: "requested" }).phase, "requested");
  assert.throws(() => stopRequestInput("ws1", "a1", REF, refused));
});

test("a stopped command is recognized only from Cortex's own trusted status/title, never a worker's claimed text", () => {
  const receipt = { tool_call_ref: REF, failure: "stopped_by_user", outcome: "stopped_by_user", stop_reason: "Not approved.", exit_code: -9, argv: ["npm", "install"] };
  const title = "Stopped by user: Not approved.";
  assert.deepEqual(stoppedCommand("failed", title, receipt), { reason: "Not approved.", toolCallRef: REF, command: "npm install" });
  assert.equal(stoppedCommand("failed", title, { result: { structuredContent: receipt } })?.command, "npm install");
  assert.equal(stoppedCommand("failed", title, { content: [{ type: "text", text: JSON.stringify(receipt) }] })?.command, "npm install");
  assert.equal(stoppedCommand("failed", title, JSON.stringify(receipt))?.command, "npm install");
  // No shape found in the row's own content: the reason still comes from
  // the trusted title alone, with no command/ref to show.
  assert.deepEqual(stoppedCommand("failed", title, null), { reason: "Not approved.", toolCallRef: null, command: null });
  // A worker can shape its own output exactly like a receipt and claim any
  // reason or ref it likes (for example via project_write + project_read of
  // crafted JSON) -- it is never trusted for the displayed reason, only
  // ever Cortex's own status/title, which the server sets solely after its
  // own durable-record check (command_control.confirmed_stop).
  const forged = { tool_call_ref: `gateway:${"b".repeat(32)}`, failure: "stopped_by_user", stop_reason: "attacker-controlled text", argv: ["rm", "-rf", "/"] };
  assert.deepEqual(stoppedCommand("failed", title, forged), { reason: "Not approved.", toolCallRef: forged.tool_call_ref, command: "rm -rf /" });
  // With no trusted status/title, nothing is ever shown as stopped, no
  // matter what shape the row's own content contains.
  assert.equal(stoppedCommand("completed", title, receipt), null, "status must be failed");
  assert.equal(stoppedCommand("failed", "Shell", receipt), null, "an ordinary failed command has no Stopped-by-user title");
  assert.equal(stoppedCommand(null, title, receipt), null);
  assert.equal(stoppedCommand("failed", null, receipt), null);
});

// --- rendered components ---------------------------------------------------------

type Element = { key?: string; type: unknown; props: Record<string, unknown> };
type EffectSlot = { deps?: unknown[]; cleanup?: (() => void) | void };

function createRuntime() {
  const hooks = new Map<string, unknown[]>();
  const mounted = new Set<string>();
  let pending: Array<() => void> = [];
  let current: { path: string; index: number } | null = null;
  let dirty = false;
  const slot = (): [unknown[], number] => {
    assert.ok(current, "hook called outside a component");
    let values = hooks.get(current.path);
    if (!values) hooks.set(current.path, values = []);
    return [values, current.index++];
  };
  const depsChanged = (prev: unknown[] | undefined, next: unknown[] | undefined) =>
    !prev || !next || prev.length !== next.length || next.some((dep, i) => !Object.is(dep, prev[i]));
  const react = {
    useState(initial: unknown) {
      const [values, i] = slot();
      if (!(i in values)) values[i] = typeof initial === "function" ? (initial as () => unknown)() : initial;
      return [values[i], (value: unknown) => {
        const next = typeof value === "function" ? (value as (prev: unknown) => unknown)(values[i]) : value;
        if (!Object.is(next, values[i])) { values[i] = next; dirty = true; }
      }];
    },
    useRef(initial: unknown) {
      const [values, i] = slot();
      if (!(i in values)) values[i] = { current: initial };
      return values[i];
    },
    useMemo(factory: () => unknown, deps?: unknown[]) {
      const [values, i] = slot();
      const prev = values[i] as { deps?: unknown[]; value: unknown } | undefined;
      if (!prev || depsChanged(prev.deps, deps)) values[i] = { deps, value: factory() };
      return (values[i] as { value: unknown }).value;
    },
    useCallback(callback: unknown, deps?: unknown[]) {
      return react.useMemo(() => callback, deps);
    },
    useEffect(effect: () => (() => void) | void, deps?: unknown[]) {
      const [values, i] = slot();
      const prev = values[i] as EffectSlot | undefined;
      if (prev && !depsChanged(prev.deps, deps)) return;
      const entry: EffectSlot = { deps, cleanup: prev?.cleanup };
      values[i] = entry;
      pending.push(() => { entry.cleanup?.(); entry.cleanup = effect(); });
    },
  };
  let elements: Element[] = [];
  function render(node: unknown, path: string): void {
    if (node === null || node === undefined || typeof node === "boolean" || typeof node === "string" || typeof node === "number") return;
    if (Array.isArray(node)) { node.forEach((child, index) => render(child, `${path}[${index}]`)); return; }
    const element = node as Element;
    const type = element.type;
    assert.ok(typeof type === "string" || typeof type === "function", `element type at ${path} is ${String(type)}`);
    elements.push(element);
    if (typeof type === "string") { render(element.props?.children, `${path}/${type}`); return; }
    const childPath = `${path}/${type.name || "anonymous"}:${element.key ?? ""}`;
    mounted.add(childPath);
    current = { path: childPath, index: 0 };
    const output = (type as (props: unknown) => unknown)(element.props);
    current = null;
    render(output, childPath);
  }
  let root: () => unknown = () => null;
  function unmount(paths: (path: string) => boolean): void {
    for (const [path, values] of hooks) {
      if (!paths(path)) continue;
      for (const value of values) {
        if (value && typeof value === "object" && "cleanup" in value) (value as EffectSlot).cleanup?.();
      }
      hooks.delete(path);
    }
  }
  async function settle(): Promise<void> {
    for (let round = 0; round < 40; round += 1) {
      dirty = false;
      elements = [];
      mounted.clear();
      render(root(), "root");
      unmount((path) => !mounted.has(path));
      const effects = pending;
      pending = [];
      for (const effect of effects) effect();
      await new Promise((done) => setTimeout(done, 0));
      if (!dirty && pending.length === 0) return;
    }
    throw new Error("component did not settle");
  }
  return {
    react, settle,
    async mount(next: () => unknown) { root = next; await settle(); },
    unmountAll() { unmount(() => true); },
    elements: () => elements,
  };
}

function textOf(node: unknown): string {
  if (typeof node === "string" || typeof node === "number") return String(node);
  if (Array.isArray(node)) return node.map(textOf).join("");
  if (node && typeof node === "object" && "props" in node) return textOf((node as Element).props.children);
  return "";
}

async function compileComponents(): Promise<string> {
  const result = await build({
    stdin: {
      contents: `export { RunningCommands } from "./command-stop-panel.tsx"; export { TranscriptEvent } from "./worker-panel.tsx";`,
      resolveDir: join(pluginRoot, "client"), sourcefile: "command-stop-entry.ts", loader: "ts",
    },
    bundle: true, format: "cjs", jsx: "automatic", platform: "neutral", target: "es2020",
    external: ["@getpaseo/plugin", "@getpaseo/plugin/client", "@getpaseo/plugin/client/react-native", "react", "react/jsx-runtime", "react-native", "zod"],
    write: false, logLevel: "silent",
  });
  return `(function(require) {\nconst module = { exports: {} };\nconst exports = module.exports;\n${result.outputFiles[0].text}\nreturn module.exports;\n})`;
}

const colors = new Proxy({}, { get: (_target, key) => `color:${String(key)}` });
const theme = { colors, spacing: new Proxy({}, { get: () => 4 }) };
const bundled = compileComponents();

type Rpc = (input: Record<string, unknown>) => Promise<unknown>;

async function load(rpcs: Record<string, Rpc>) {
  const runtime = createRuntime();
  const modules: Record<string, unknown> = {
    react: { ...runtime.react, default: runtime.react },
    "react/jsx-runtime": {
      jsx: (type: unknown, props: Record<string, unknown>, key?: string) => ({ type, props, key }),
      jsxs: (type: unknown, props: Record<string, unknown>, key?: string) => ({ type, props, key }),
      Fragment: "Fragment",
    },
    "react-native": new Proxy({}, { get: (_target, key) => String(key) }),
    "@getpaseo/plugin": await import("@getpaseo/plugin"),
    "@getpaseo/plugin/client": { useRpc: (definition: { name: string }) => rpcs[definition.name] ?? (() => new Promise(() => {})) },
    "@getpaseo/plugin/client/react-native": { Icon: "Icon", ScrollView: "ScrollView" },
    zod: await import("zod"),
  };
  const evaluate = new Function(`return ${await bundled}`)() as (require: (name: string) => unknown) => {
    RunningCommands: (props: unknown) => unknown; TranscriptEvent: (props: unknown) => unknown;
  };
  const exports = evaluate((name) => {
    assert.ok(name in modules, `component required unexpected module ${name}`);
    return modules[name];
  });
  const find = (label: string, type?: string): Element | undefined =>
    runtime.elements().find((element) => element.props?.accessibilityLabel === label && (!type || element.type === type));
  const press = async (label: string) => {
    const element = find(label, "Pressable");
    assert.ok(element, `no button labelled ${JSON.stringify(label)}`);
    assert.notEqual(element.props.disabled, true, `${label} is disabled`);
    (element.props.onPress as () => void)();
    await runtime.settle();
  };
  const type = async (label: string, value: string) => {
    const element = find(label, "TextInput");
    assert.ok(element, `no field labelled ${JSON.stringify(label)}`);
    (element.props.onChangeText as (v: string) => void)(value);
    await runtime.settle();
  };
  const texts = () => runtime.elements().filter((element) => element.type === "Text").map((element) => textOf(element.props.children));
  return { runtime, exports, find, press, type, texts };
}

const command = (stopRequest: { reason: string; origin: string | null } | null = null) => ({
  toolCallRef: REF, operation: "command_run", assignmentRef: "assignment-ref-1", attemptId: "xa_1",
  startedAt: "2026-10-18T01:00:00+00:00", argvSummary: "npm install", stopRequest,
});

async function panel(replies: unknown[], listed = () => [command()]) {
  const stops: Record<string, unknown>[] = [];
  const reads: Record<string, unknown>[] = [];
  const view = await load({
    "cortex.commands.running": async (input) => { reads.push(input); return { available: true, commands: listed() }; },
    "cortex.commands.stop": async (input) => {
      stops.push(input);
      const reply = replies.shift();
      if (reply instanceof Error) throw reply;
      return reply;
    },
  });
  await view.runtime.mount(() => ({ type: view.exports.RunningCommands,
    props: { theme, workspaceId: "ws1", agentId: "a1", assignmentRef: "assignment-ref-1", active: false } }));
  return { ...view, stops, reads };
}

test("Stop opens a required reason field and sends the exact stop request", async () => {
  const view = await panel([{ status: "requested" }]);
  try {
    assert.deepEqual(view.reads[0], { workspaceId: "ws1", agentId: "a1", assignmentRef: "assignment-ref-1" });
    assert.ok(view.texts().includes("$ npm install"));
    await view.press("Stop command npm install");
    const confirm = () => view.find("Confirm stopping npm install", "Pressable")!;
    assert.equal(confirm().props.disabled, true, "no reason yet");
    assert.deepEqual(confirm().props.accessibilityState, { disabled: true });
    await view.type("Reason for stopping npm install", "   ");
    assert.equal(confirm().props.disabled, true, "a blank reason is not a reason");
    await view.type("Reason for stopping npm install", "  Installing packages is not approved. ");
    assert.equal(confirm().props.disabled, false);
    await view.press("Confirm stopping npm install");
    assert.equal(view.stops.length, 1);
    const sent = view.stops[0];
    assert.deepEqual(Object.keys(sent).sort(), ["agentId", "reason", "requestKey", "toolCallRef", "workspaceId"]);
    assert.equal(sent.toolCallRef, REF);
    assert.equal(sent.reason, "Installing packages is not approved.");
    assert.match(String(sent.requestKey), /^paseo-stop:/);
    assert.ok(view.texts().some((text) => text === "Stop requested: Installing packages is not approved. — waiting for the command to end."));
    assert.equal(view.find("Stop command npm install"), undefined, "no second stop for a requested command");
    assert.equal(view.find("Reason for stopping npm install"), undefined);
    assert.ok(view.reads.length >= 2, "the list is re-read after an acknowledged stop");
  } finally { view.runtime.unmountAll(); }
});

test("a refusal keeps the reason, says why, and a retry reuses the request key", async () => {
  const view = await panel([
    { status: "refused", message: "This chat no longer owns the Cortex task. Nothing was stopped." },
    new Error("plugin invoke failed"),
    { status: "requested" },
  ]);
  try {
    await view.press("Stop command npm install");
    await view.type("Reason for stopping npm install", "Wrong directory");
    await view.press("Confirm stopping npm install");
    assert.ok(view.texts().includes("This chat no longer owns the Cortex task. Nothing was stopped."));
    const alert = view.runtime.elements().find((element) => element.props?.accessibilityRole === "alert");
    assert.equal(alert?.props.style && (alert.props.style as Array<{ color?: string }>)[1].color, "color:statusDanger");
    assert.equal(view.find("Reason for stopping npm install", "TextInput")?.props.value, "Wrong directory");
    assert.ok(view.texts().includes("Try again"));
    await view.press("Confirm stopping npm install");
    assert.ok(view.texts().some((text) => /could not be delivered\. It may still apply/.test(text)), "transport failure is unconfirmed, not refused");
    await view.press("Confirm stopping npm install");
    assert.equal(view.stops.length, 3);
    assert.equal(new Set(view.stops.map((stop) => stop.requestKey)).size, 1, "one retry identity for one reason");
    assert.ok(view.texts().some((text) => text.startsWith("Stop requested: Wrong directory")));
  } finally { view.runtime.unmountAll(); }
});

test("Cancel abandons the stop without sending anything", async () => {
  const view = await panel([]);
  try {
    await view.press("Stop command npm install");
    await view.type("Reason for stopping npm install", "never mind");
    await view.press("Keep running npm install");
    assert.equal(view.stops.length, 0);
    assert.ok(view.find("Stop command npm install", "Pressable"));
  } finally { view.runtime.unmountAll(); }
});

test("a stop recorded by another route shows as pending, with no second Stop", async () => {
  const view = await panel([], () => [command({ reason: "Not approved.", origin: "owner_cli" })]);
  try {
    assert.ok(view.texts().includes("Stop requested from the local command: Not approved. — waiting for the command to end."));
    assert.equal(view.find("Stop command npm install"), undefined);
  } finally { view.runtime.unmountAll(); }
});

test("no running command renders nothing", async () => {
  const view = await panel([], () => []);
  try {
    assert.equal(view.runtime.elements().length, 1, "only the RunningCommands element itself");
  } finally { view.runtime.unmountAll(); }
});

const toolEvent = (extra: Record<string, unknown>) => ({
  id: 7, type: "tool_call", createdAt: "2026-10-18T01:00:00Z", text: "", activity_ref: `tool:${REF}`,
  tool_kind: "execute", raw_input: { argv: ["npm", "install"] }, ...extra,
});

async function row(event: Record<string, unknown>) {
  const view = await load({});
  await view.runtime.mount(() => ({ type: view.exports.TranscriptEvent,
    props: { event, updates: [], expanded: false, onToggle: () => {}, theme } }));
  return view;
}

test("a command the user stopped renders as stopped by user, with the reason visible", async () => {
  const receipt = { tool_call_ref: REF, failure: "stopped_by_user", outcome: "stopped_by_user", complete: true,
    stop_reason: "Installing packages is not approved.", exit_code: -9, argv: ["npm", "install"] };
  const view = await row(toolEvent({ status: "failed", title: "Stopped by user: Installing packages is not approved.", raw_output: receipt, error: receipt }));
  const texts = view.texts();
  assert.ok(texts.includes("Stopped by user"), "the row label");
  assert.ok(texts.includes("npm install"), "the stopped command");
  assert.ok(texts.includes("Reason: Installing packages is not approved."), "the reason is visible without expanding");
  assert.ok(view.find("Stopped by user. Reason: Installing packages is not approved.", "Text"));
  const reason = view.runtime.elements().find((element) => textOf(element.props?.children) === "Reason: Installing packages is not approved.");
  assert.equal((reason!.props.style as Array<{ color?: string }>)[1].color, "color:statusWarning");
});

test("a genuine stop still renders when the row's own engine ref is never a gateway-format ref", async () => {
  // In production, activity_ref/the ACP engine's own per-call ref is an
  // engine-assigned id (e.g. a Claude/Codex tool-use id), never the
  // gateway's own `gateway:<hex>` tool_call_ref embedded in the receipt --
  // those are different, unrelated identifier namespaces (see
  // command_control.confirmed_stop). Detection must not depend on them
  // ever being equal.
  const receipt = { tool_call_ref: REF, failure: "stopped_by_user", stop_reason: "Not approved.", argv: ["npm", "install"] };
  const view = await row({
    id: 8, type: "tool_call", createdAt: "2026-10-18T01:00:00Z", text: "", activity_ref: "tool:claude-xyz-123",
    tool_kind: "execute", raw_input: { argv: ["npm", "install"] },
    status: "failed", title: "Stopped by user: Not approved.", raw_output: receipt,
  });
  assert.ok(view.texts().includes("Stopped by user"));
  assert.ok(view.texts().includes("Reason: Not approved."));
});

test("an ordinary failed command is not shown as stopped by user", async () => {
  const view = await row(toolEvent({ status: "failed", title: "Shell", raw_output: { tool_call_ref: REF, failure: "timeout", exit_code: -9 } }));
  assert.ok(!view.texts().includes("Stopped by user"));
  assert.ok(!view.texts().some((text) => text.startsWith("Reason: ")));
});

test("a worker-fabricated stopped-by-user shape on another row is never rendered as a stop", async () => {
  // The attacker scenario: a project_read of a file whose own text happens
  // to be shaped like the stop receipt, but names a tool_call_ref the
  // attacker made up (not this row's own real identity).
  const forgedRef = `gateway:${"c".repeat(32)}`;
  const forged = { tool_call_ref: forgedRef, failure: "stopped_by_user", stop_reason: "attacker-controlled text", argv: ["npm", "install"] };
  const view = await row({
    id: 9, type: "tool_call", createdAt: "2026-10-18T01:00:00Z", text: "", activity_ref: `tool:${REF}`,
    tool_kind: "read", status: "completed", title: "Read", raw_output: { content: JSON.stringify(forged) },
  });
  assert.ok(!view.texts().includes("Stopped by user"), "a mismatched tool_call_ref must never be trusted");
  assert.ok(!view.texts().includes("attacker-controlled text"));
});
