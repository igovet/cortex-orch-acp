import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { existsSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";
import { build } from "esbuild";

const pluginRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const RealReact = createRequire(join(pluginRoot, "package.json"))("react") as { Component: new (props: unknown) => unknown };

/**
 * The client bundle exactly as Paseo 0.9's daemon compiles it
 * (`compileTarget(entry, "client")`: esbuild CJS, es2020, JSX automatic,
 * async lowered, host modules external; then its Hermes interop rewrite and
 * CommonJS wrapper). The phone app evaluates this string from source on
 * Hermes, bypassing Metro/Babel, so every syntax feature in it must be one
 * Hermes itself supports.
 */
async function compileClientLikePaseo(): Promise<string> {
  const result = await build({
    entryPoints: [join(pluginRoot, "index.client.tsx")],
    bundle: true,
    format: "cjs",
    jsx: "automatic",
    platform: "neutral",
    target: "es2020",
    supported: { "async-await": false },
    external: [
      "@getpaseo/plugin", "@getpaseo/plugin/client", "@getpaseo/plugin/client/react-native", "@getpaseo/plugin/client/ui",
      "@tanstack/react-query", "react", "react/jsx-runtime", "react-native", "zod",
    ],
    treeShaking: true,
    metafile: true,
    write: false,
    logLevel: "silent",
  });
  // React Native's Metro/Jest tooling is installed for compatibility checks,
  // not shipped in this client artifact. Keep vulnerable glob walkers out.
  for (const input of Object.keys(result.metafile!.inputs)) {
    assert.doesNotMatch(input, /node_modules\/(?:braces|micromatch|metro(?:-[^/]+)?|@jest\/[^/]+|jest-[^/]+)\//);
  }
  const code = result.outputFiles[0].text.replaceAll("get: () => from[key]", "value: from[key]");
  return `(function(require) {\nconst module = { exports: {} };\nconst exports = module.exports;\n${code}\nreturn module.exports;\n})`;
}

/** React Native 0.81's own Hermes compiler (the engine the phone app runs). */
function hermesCompiler(): string | null {
  const sdk = join(pluginRoot, "node_modules", "react-native", "sdks", "hermesc");
  if (process.platform === "linux" && process.arch === "x64") return join(sdk, "linux64-bin", "hermesc");
  if (process.platform === "darwin") return join(sdk, "osx-bin", "hermesc");
  if (process.platform === "win32") return join(sdk, "win64-bin", "hermesc.exe");
  return null;
}

const compiler = hermesCompiler();

test("client bundle compiles under the phone's Hermes with no unsupported syntax", {
  skip: compiler ? false : `not_applicable: react-native ships no hermesc for ${process.platform}/${process.arch}`,
}, async () => {
  // Hermes has no `class` support: its compiler rejects a class expression
  // here, but the phone's lazy from-source evaluation instead compiles it to
  // `undefined`, which surfaced as "Element type is invalid ... got:
  // undefined" (the error boundary) and `new undefined` (the poller).
  assert.ok(existsSync(compiler!), `hermesc missing at ${compiler}; run npm ci`);
  const dir = mkdtempSync(join(tmpdir(), "cortex-hermes-bundle-"));
  try {
    const source = join(dir, "client.js");
    writeFileSync(source, await compileClientLikePaseo());
    const run = spawnSync(compiler!, ["-emit-binary", "-out", join(dir, "client.hbc"), source], { encoding: "utf8" });
    assert.equal(run.error, undefined);
    assert.equal(run.status, 0, `hermesc rejected the client bundle:\n${run.stderr}`);
    assert.doesNotMatch(run.stderr, /error:/);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

// ---------------------------------------------------------------------------
// Render walk: evaluate the same bundle with host-module shims, drive
// `contribute()`, and render every surface the phone and desktop show,
// asserting no element anywhere has an undefined type.

type Element = { type: unknown; props: Record<string, unknown> };
type EffectSlot = { deps?: unknown[]; cleanup?: (() => void) | void };

function createRuntime() {
  const hooks = new Map<string, unknown[]>();
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
    Component: RealReact.Component,
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

  function render(node: unknown, path: string, onElement: (element: Element, path: string) => void): void {
    if (node === null || node === undefined || typeof node === "boolean" || typeof node === "string" || typeof node === "number") return;
    if (Array.isArray(node)) {
      node.forEach((child, index) => render(child, `${path}[${index}]`, onElement));
      return;
    }
    assert.equal(typeof node, "object", `unexpected render output at ${path}`);
    const element = node as Element;
    const type = element.type;
    assert.ok(
      typeof type === "string" || typeof type === "function" || (typeof type === "object" && type !== null),
      `element type at ${path} is ${String(type)} (expected a string or a class/function)`,
    );
    onElement(element, path);
    if (typeof type !== "function") {
      render(element.props?.children, `${path}/${String(type)}`, onElement);
      return;
    }
    const name = (type as { displayName?: string }).displayName ?? type.name ?? "anonymous";
    const childPath = `${path}/${name}`;
    if ((type as { prototype?: { isReactComponent?: unknown } }).prototype?.isReactComponent) {
      const instance = new (type as new (props: unknown) => { props: unknown; render(): unknown })(element.props);
      render(instance.render(), childPath, onElement);
      return;
    }
    current = { path: childPath, index: 0 };
    const output = (type as (props: unknown) => unknown)(element.props);
    current = null;
    render(output, childPath, onElement);
  }

  // Every string child and accessibility label of the last render pass:
  // what a reader or assistive technology could see, not pixels.
  let lastText: string[] = [];
  // The current text of every polite/assertive live region, in render order.
  let lastLive: string[] = [];
  // The `active` prop of every executing-indicator pulse in render order.
  let lastPulses: boolean[] = [];
  let labeledPresses = new Map<string, () => void>();
  let currentRoot: (() => unknown) | null = null;
  const flatText = (node: unknown): string => {
    if (typeof node === "string" || typeof node === "number") return String(node);
    if (Array.isArray(node)) return node.map(flatText).join("");
    return "";
  };
  const textOf = (element: Element, out: string[]) => {
    const label = element.props?.accessibilityLabel;
    if (typeof label === "string") out.push(label);
    const children = element.props?.children;
    for (const child of Array.isArray(children) ? children.flat(Infinity) : [children]) {
      if (typeof child === "string" || typeof child === "number") out.push(String(child));
    }
  };

  let count = 0;
  let presses: Array<() => void> = [];
  const pass = () => {
    count = 0;
    presses = [];
    labeledPresses = new Map();
    const texts: string[] = [];
    const live: string[] = [];
    const pulses: boolean[] = [];
    render(currentRoot!(), "root", (element) => {
      count += 1;
      if (typeof element.type === "function" && /^WorkingPulse\d*$/.test(element.type.name)) pulses.push(element.props.active === true);
      textOf(element, texts);
      if (element.props?.accessibilityLiveRegion && element.props.accessibilityLiveRegion !== "none") live.push(flatText(element.props.children));
      const onPress = element.props?.onPress;
      if (typeof onPress === "function") {
        const press = () => (onPress as () => void)();
        presses.push(press);
        if (typeof element.props.accessibilityLabel === "string") labeledPresses.set(element.props.accessibilityLabel, press);
      }
    });
    lastText = texts;
    lastLive = live;
    lastPulses = pulses;
  };
  /** Re-render the mounted root until state and effects settle. */
  const settle = async () => {
    for (let round = 0; round < 20; round += 1) {
      dirty = false;
      pass();
      const effects = pending;
      pending = [];
      for (const effect of effects) effect();
      await new Promise((done) => setTimeout(done, 0));
      if (!dirty && pending.length === 0) return;
    }
  };

  /** Render to a settled state: run effects, let async RPCs land, re-render
   * while state changed; then (unless `press: false`) press every Pressable
   * once and settle again so expandable sections render too. Returns the
   * element count seen. */
  async function mount(root: () => unknown, options: { press?: boolean } = {}): Promise<number> {
    // Each surface mounts alone, as on the phone: unsubscribe the previous
    // one so shared pollers start a fresh group with an immediate read.
    unmountAll();
    currentRoot = root;
    await settle();
    if (options.press === false) return count;
    for (const press of presses) press();
    await settle();
    return count;
  }

  /** Press the one rendered Pressable with this accessible name, then settle. */
  async function press(label: string | RegExp): Promise<void> {
    const names = [...labeledPresses.keys()].filter((name) => typeof label === "string" ? name === label : label.test(name));
    assert.ok(names.length <= 1, `ambiguous pressable ${String(label)}: ${names.join(" | ")}`);
    const action = names.length ? labeledPresses.get(names[0]) : undefined;
    assert.ok(action, `no pressable named ${label}; rendered: ${[...labeledPresses.keys()].join(" | ")}`);
    action();
    await settle();
  }

  function unmountAll(): void {
    for (const values of hooks.values()) {
      for (const value of values) (value as EffectSlot | undefined)?.cleanup?.();
    }
    hooks.clear();
  }

  return { react, mount, press, settle, unmountAll, text: () => lastText.join("\n"), liveRegions: () => [...lastLive], pulses: () => [...lastPulses] };
}

const colors = new Proxy({}, { get: () => "#808080" });
const theme = { colors, spacing: new Proxy({}, { get: () => 4 }) };

const check = { name: "npm run test:pipeline", outcome: "passed" };
const pipelineFixture = {
  available: true,
  tokens: { total: null, coordinator: null },
  task: { id: "t_1", createdAt: "2026-09-26T00:00:00Z", state: "EXECUTING", waitReason: null, waitDetail: null },
  messages: [{ text: "Fix the phone pipeline", createdAt: "2026-09-26T00:00:00Z" }],
  requirements: [{ id: "req1", statement: "Pipeline renders on the phone", criteria: ["no undefined element"] }],
  delegations: { ref1: { order: 0, result: { summary: "Fixed.", checks: [check] }, decision: { decision: "accepted", rationale: null } } },
  plan: {
    state: "EXECUTING",
    blocker: null,
    nodes: [
      { id: "n1", role: "worker-debugger", objective: "Fix it", stage: "execution", status: "completed", assignmentRef: "ref1", waiting: null,
        result: { summary: "Fixed.", checks: [check, { name: "typecheck", outcome: "failed", blocker: "tsc error" }] } },
      { id: "n2", role: "worker-reviewer", objective: "Review it", stage: "review", status: "pending", assignmentRef: null, waiting: "n1", result: null },
    ],
  },
  workers: [],
};

// A minimal, schema-shaped `cortex.config.read` fixture for `surface:
// "user_defaults"` only: enough for the Settings screen's default section to
// reach its "ready" state during the render walk below. Every other surface
// is left unresolved (the section shows "Loading…", which is still a
// defined element tree) -- this walk checks for undefined element types, not
// full section coverage, which is exercised by `settings-model.test.ts`.
const configBinding = { apiVersion: 1, configHome: "~", project: null as string | null };
const userDefaultsFixture = {
  surface: "user_defaults", path: "~/.config/cortex/defaults.json", revision: "absent", exists: false,
  defaults: {}, editable_keys: ["route_id", "route_connection_id", "route_model", "parameters", "language"],
  preserved_keys: [], languages: ["en", "ru"],
  ceiling: {
    allowed_routes: null, allowed_connections: null, allowed_data_recipients: null, permission_level: null,
    limits: {}, spend_limit: null, spend_limit_enforceable: false,
    fallback_policy: { allowed_target_routes: null, allowed_failure_classes: null, budget: null, data_recipient_consent: false },
  },
  ceiling_set: false,
};
async function configReadFixture(input: { surface: string }): Promise<unknown> {
  if (input.surface === "user_defaults") return { ok: true, binding: configBinding, result: userDefaultsFixture };
  return new Promise<never>(() => {});
}

type RpcCall = { name: string; input: Record<string, unknown> };

async function loadClient(platformOs: "android" | "ios" | "web", rpcOverrides: Record<string, (input: Record<string, unknown>) => Promise<unknown>> = {},
  options: { reduceMotion?: boolean } = {}) {
  const rpcCalls: RpcCall[] = [];
  // Native Animated loops the bundle starts and stops (web pulses are CSS).
  const animation = { started: 0, stopped: 0 };
  const runtime = createRuntime();
  const component = (name: string) => name;
  const reactNative = new Proxy({}, {
    get(_target, key) {
      if (key === "Platform") return { OS: platformOs, select: (options: Record<string, unknown>) => options[platformOs] ?? options.default };
      if (key === "useWindowDimensions") return () => ({ width: 390, height: 844 });
      if (key === "StyleSheet") return { create: (styles: unknown) => styles, hairlineWidth: 1 };
      if (key === "AccessibilityInfo" && options.reduceMotion !== undefined) {
        return { isReduceMotionEnabled: async () => options.reduceMotion, addEventListener: () => ({ remove() {} }) };
      }
      if (key === "Animated") {
        return {
          Value: function Value(this: Record<string, unknown>) { this.setValue = () => {}; this.stopAnimation = () => {}; },
          View: "Animated.View",
          loop: () => ({ start() { animation.started += 1; }, stop() { animation.stopped += 1; } }),
          sequence: () => ({}),
          timing: () => ({}),
        };
      }
      return component(String(key));
    },
  });
  const rpcs = new Map<string, (input: unknown) => Promise<unknown>>();
  // Shared between the `@getpaseo/plugin/client` module shim's `usePaseo()`
  // and the top-level `client.paseo` the harness passes to `contribute()`,
  // exactly like the real host: one workspace/agent catalogue either way.
  const paseoApi = {
    workspaces: { list: async () => ({ entries: [{ id: "ws1", title: null, name: "ws1" }] }), subscribe: () => () => {} },
    agents: {
      list: async () => ({ entries: [{ agent: { id: "a1", workspaceId: "ws1", status: "running", provider: "cortex" } }], pageInfo: { hasMore: false } }),
      subscribe: () => () => {},
    },
  };
  const pluginClient = {
    useAgent: (_agentId: string, select: (agent: { workspaceId: string }) => unknown) => select({ workspaceId: "ws1" }),
    usePaseo: () => paseoApi,
    useRpc(definition: { name: string }) {
      let call = rpcs.get(definition.name);
      if (!call) {
        const target = rpcOverrides[definition.name]
          ?? (definition.name === "cortex.pipeline.read"
          ? async () => pipelineFixture
          : definition.name === "cortex.config.read"
          ? (input: unknown) => configReadFixture(input as { surface: string })
          : () => new Promise<never>(() => {}));
        call = (input: unknown) => {
          rpcCalls.push({ name: definition.name, input: input as Record<string, unknown> });
          return target(input as Record<string, unknown>);
        };
        rpcs.set(definition.name, call);
      }
      return call;
    },
  };
  const modules: Record<string, unknown> = {
    react: runtime.react,
    "react/jsx-runtime": {
      jsx: (type: unknown, props: Record<string, unknown>) => ({ type, props }),
      jsxs: (type: unknown, props: Record<string, unknown>) => ({ type, props }),
      Fragment: "Fragment",
    },
    "react-native": reactNative,
    "@getpaseo/plugin": await import("@getpaseo/plugin"),
    "@getpaseo/plugin/client": pluginClient,
    "@getpaseo/plugin/client/react-native": { Icon: component("Icon"), ScrollView: component("ScrollView"), Modal: component("Modal") },
    zod: await import("zod"),
  };
  const evaluate = new Function(`return ${await compileClientLikePaseo()}`)() as (require: (name: string) => unknown) => { default: (client: unknown) => () => void };
  const exports = evaluate((name) => {
    assert.ok(name in modules, `bundle required unexpected module ${name}`);
    return modules[name];
  });
  const captured: Record<string, { Component: (props: Record<string, unknown>) => unknown }> = {};
  const commands: Record<string, { title: string; context: string; onSelect: (capabilities: { openSettings: (id: string) => void }) => void }> = {};
  let pill: { button: { behavior: { kind: string; Content?: (props: Record<string, unknown>) => unknown } } } | undefined;
  const registration = { remove() {} };
  const transformers: Record<string, { transform(input: { item: Record<string, unknown> }): unknown }> = {};
  const client = {
    addTimelineTransformer: (transformer: { id: string; transform(input: { item: Record<string, unknown> }): unknown }) => {
      transformers[transformer.id] = transformer;
      return registration;
    },
    addTimelineRenderer: (renderer: { kind: string; Component: (props: Record<string, unknown>) => unknown }) => { captured[renderer.kind] = renderer; return registration; },
    addWorkspacePanel: (panel: { id: string; Component: (props: Record<string, unknown>) => unknown }) => { captured[panel.id] = panel; return registration; },
    addSettingsScreen: (screen: { id: string; Component: (props: Record<string, unknown>) => unknown }) => { captured[screen.id] = screen; return registration; },
    addCommandCenterItem: (item: { id: string; title: string; context: string; onSelect: (capabilities: { openSettings: (id: string) => void }) => void }) => {
      commands[item.id] = item;
      return registration;
    },
    addComposerPill: (contribution: typeof pill) => { pill = contribution; return registration; },
    openPanel() {},
    paseo: paseoApi,
  };
  const cleanup = exports.default(client);
  for (let tick = 0; tick < 5 && !pill; tick += 1) await new Promise((done) => setTimeout(done, 0));
  return { runtime, captured, commands, transformers, pill: () => pill, cleanup, rpcCalls, animation };
}

test("a native running indicator stays static under the reduce-motion setting", async () => {
  const history = () => Promise.resolve({ available: true, truncated: false, nextCursor: null, nextAssignmentCursor: null,
    assignments: [{ taskId: "t_1", id: "wa_1", assignmentRef: "ref1", nodeKey: "n1", title: "Fix it", role: "debugger", status: "running",
      attempts: [{ id: "xa_1", attemptNo: 1, status: "running", failureReason: null, events: [], openIssues: 0 }] }] });
  const { runtime, captured, cleanup, animation } = await loadClient("android", { "cortex.worker-history.read": history }, { reduceMotion: true });
  try {
    await runtime.mount(() => ({ type: captured["worker-row"].Component, props: {
      theme, host: { id: "h1", label: "Local" }, layout: { compact: true, platform: "android" }, agentId: "a1",
      item: { id: "worker-row:worker:ref1", kind: "worker-row", version: 1, data: { callId: "worker:ref1", assignmentRef: "ref1", title: "Worker: debugger — Fix it", status: "running" } },
    } }), { press: false });
    assert.deepEqual(runtime.pulses(), [true, true], "still executing");
    // Effects ran with the setting already read: any loop started before it
    // landed has been stopped, and none is left running.
    assert.equal(animation.started - animation.stopped, 0);
    assert.match(runtime.text(), /Working/);
  } finally {
    runtime.unmountAll();
    cleanup();
  }
});

// The coordinator timeline card's props, from the worker items the provider
// emits (server/provider-worker-steer.test.ts drives the real bridge): a
// steered worker stays one running row, then shows its real outcome; a
// genuine failure keeps its reason. Inputs only, not rendered pixels.
test("worker card props follow the provider's worker items across steering", async () => {
  const { transformers, cleanup } = await loadClient("web");
  try {
    const row = transformers["worker-tool-call-row"];
    assert.ok(row, "worker row transformer registered");
    const title = "Worker: debugger · claude · Opus · high — Fix the check submission";
    const card = (item: Record<string, unknown>) => (row.transform({ item: {
      type: "tool_call", id: "worker:w1", callId: "worker:w1", name: title, detail: { type: "plan", text: title }, ...item,
    } }) as { items: Array<{ id: string; kind: string; data: Record<string, unknown> }> }).items[0];
    const running = card({ status: "running", error: null });
    assert.equal(running.id, "worker-row:worker:w1");
    assert.equal(running.kind, "worker-row");
    assert.equal(running.data.status, "running");
    assert.equal(running.data.assignmentRef, "w1");
    assert.ok(!("failureReason" in running.data), "a running card shows no failure line");
    const done = card({ status: "completed", error: null, metadata: { cortexWorkerOutcome: { openIssues: 0 } } });
    assert.equal(done.id, running.id, "the outcome updates the same row");
    assert.equal(done.data.status, "completed");
    assert.equal(done.data.openIssues, 0);
    assert.ok(!("result" in done.data));
    assert.ok(!("failureReason" in done.data));
    // An item persisted by an older plugin still carries the full result:
    // only its open-issue count reaches the card.
    const legacy = card({ status: "completed", error: null, metadata: { cortexWorkerResult: { summary: "LEGACY SUMMARY",
      checks: [{ name: "LEGACY CHECK", outcome: "not_run", blocker: "LEGACY BLOCKER" }, { name: "ok", outcome: "passed" }] } } });
    assert.equal(legacy.id, running.id, "replay keeps one row identity");
    assert.equal(legacy.data.openIssues, 1);
    assert.doesNotMatch(JSON.stringify(legacy), /LEGACY/);
    const crashed = card({ status: "failed", error: "Worker crashed: engine exited 1" });
    assert.equal(crashed.data.status, "failed");
    assert.equal(crashed.data.failureReason, "Worker crashed: engine exited 1");
  } finally {
    cleanup();
  }
});

// The worker-history fixture both views read: the card's compact read gets
// status/usage/open-issue count only; the worker tab gets the full attempt.
function workerHistoryFixture(input: Record<string, unknown>) {
  const full = {
    summary: "WORKER SUMMARY BODY " + "long ".repeat(400),
    checks: [
      { name: "CHECK-NAME-PASSED", outcome: "passed" },
      { name: "CHECK-NAME-WEAK", outcome: "unverified", claimedOutcome: "passed", evidenceGap: "EVIDENCE-GAP-TEXT", blocker: "BLOCKER-TEXT" },
    ],
  };
  return Promise.resolve({
    available: true, truncated: false, nextCursor: null, nextAssignmentCursor: null,
    assignments: [{ taskId: "t_1", id: "wa_1", assignmentRef: "ref1", nodeKey: "n1", title: "Fix it", role: "debugger", status: "completed_with_issues",
      attempts: [{ id: "xa_1", attemptNo: 1, status: "completed_with_issues", failureReason: "Completed with issues. Runtime did not accept worker checks. CHECK-NAME-WEAK: unverified",
        context: { used: 330000, size: 1000000 },
        events: input.compact ? [] : [{ id: 1, type: "worker_activity", createdAt: "2026-10-21T00:00:00Z", text: "ACTIVITY ROW" }],
        ...(input.compact ? { openIssues: 1 } : full) }] }],
  });
}

for (const platformOs of ["android", "web"] as const) {
  test(`coordinator card renders no result body or checks, even from a legacy payload (${platformOs})`, async () => {
    const { runtime, captured, cleanup, rpcCalls } = await loadClient(platformOs, { "cortex.worker-history.read": workerHistoryFixture });
    const host = { theme, host: { id: "h1", label: "Local" }, layout: { compact: platformOs !== "web", platform: platformOs } };
    try {
      const title = "Worker: debugger · claude · Opus 5.5 · high · flagship · by user_request · usage 25% — " + "Very long task title ".repeat(40) + "· Completed with issues";
      for (const data of [
        { callId: "worker:ref1", assignmentRef: "ref1", title, status: "failed", failureReason: "Completed with issues. Runtime did not accept worker checks. CHECK-NAME-WEAK: unverified",
          // A replayed legacy item's data can still carry `result`: never rendered.
          result: { summary: "LEGACY CARD SUMMARY", checks: [{ name: "LEGACY-CHECK", outcome: "not_run", blocker: "LEGACY-BLOCKER" }] } },
        { callId: "worker:ref1", assignmentRef: "ref1", title, status: "completed", openIssues: 1 },
      ]) {
        rpcCalls.length = 0;
        await runtime.mount(() => ({ type: captured["worker-row"].Component, props: {
          ...host, agentId: "a1", item: { id: "worker-row:worker:ref1", kind: "worker-row", version: 1, data },
        } }));
        const text = runtime.text();
        for (const hidden of ["LEGACY", "WORKER SUMMARY BODY", "CHECK-NAME", "EVIDENCE-GAP", "BLOCKER-TEXT", "Runtime did not accept"]) {
          assert.ok(!text.includes(hidden), `coordinator card must not render ${hidden}`);
        }
        // How the route was chosen is its own line; the plan's usage never
        // reads as a bare `usage` beside the worker's own context meter.
        assert.match(text, /^flagship · by user_request · plan usage 25%$/m);
        assert.doesNotMatch(text, /(?<!plan )usage 25%/);
        assert.match(text, /Completed with issues\n · 1/);
        assert.match(text, /1 open issue · details in the worker tab/);
        assert.match(text, /claude · Opus 5\.5 · high/);
        assert.match(text, /Context 33%, 330k \/ 1M tokens/);
        // The visible Open worker action was removed; the whole card opens the
        // worker, and only its accessible name still says so.
        assert.doesNotMatch(text, /^Open worker$/m);
        assert.match(text, /Open worker$/m);
        assert.ok(text.includes("Very long task title ".repeat(40).trim()), "the accessible name keeps the full title");
        // Only the compact card read, never the worker tab's full read or the
        // retired per-card result fetch.
        assert.ok(rpcCalls.length > 0);
        for (const call of rpcCalls) {
          assert.equal(call.name, "cortex.worker-history.read");
          assert.equal(call.input.compact, true);
          assert.equal(call.input.limit, 1);
        }
      }
    } finally {
      runtime.unmountAll();
      cleanup();
    }
  });

  test(`only an executing worker's indicators pulse, and a settled or unmounted card stops (${platformOs})`, async () => {
    let durable = "running";
    const history = (input: Record<string, unknown>) => Promise.resolve({
      available: true, truncated: false, nextCursor: null, nextAssignmentCursor: null,
      assignments: [{ taskId: "t_1", id: "wa_1", assignmentRef: "ref1", nodeKey: "n1", title: "Fix it", role: "debugger", status: durable,
        attempts: [{ id: "xa_1", attemptNo: 1, status: durable, failureReason: null, context: { used: 1000, size: 100000 },
          events: input.compact ? [] : [{ id: 1, type: "worker_activity", createdAt: "2026-10-21T00:00:00Z", text: "ACTIVITY ROW" }],
          ...(input.compact ? { openIssues: 0 } : {}) }] }],
    });
    const { runtime, captured, cleanup, animation } = await loadClient(platformOs, { "cortex.worker-history.read": history });
    const host = { theme, host: { id: "h1", label: "Local" }, layout: { compact: platformOs !== "web", platform: platformOs } };
    const title = "Worker: debugger · claude · Opus 5.5 · high — Fix it";
    const card = (status: string) => () => ({ type: captured["worker-row"].Component, props: {
      ...host, agentId: "a1", item: { id: "worker-row:worker:ref1", kind: "worker-row", version: 1, data: { callId: "worker:ref1", assignmentRef: "ref1", title, status } },
    } });
    try {
      await runtime.mount(card("running"), { press: false });
      // The worker marker and the status icon, nothing else on the card.
      assert.deepEqual(runtime.pulses(), [true, true]);
      assert.match(runtime.text(), /Working/);
      if (platformOs !== "web") assert.equal(animation.started - animation.stopped, 2, "one native loop per indicator");

      // A durable attempt still provisioning is a wait, not work.
      durable = "provisioning";
      await runtime.mount(card("running"), { press: false });
      assert.deepEqual(runtime.pulses(), [false, false]);
      assert.match(runtime.text(), /Working/);

      // Settled: no pulse, and every loop started was stopped.
      for (const [status, final] of [["completed", "completed"], ["failed", "failed"], ["canceled", "cancelled"], ["running", "interrupted"]] as const) {
        durable = final;
        await runtime.mount(card(status), { press: false });
        assert.deepEqual(runtime.pulses(), [false, false], `${status}/${final}`);
      }
      runtime.unmountAll();
      assert.equal(animation.started, animation.stopped, "no animation outlives its card");

      // The worker tab's selected-attempt status icon pulses while executing.
      // (The tab restores its saved view, so open the attempt only when the
      // list is showing.)
      const openTab = async () => {
        await runtime.mount(() => ({ type: captured.workers.Component, props: { ...host, workspaceId: "ws1", agentId: "a1", context: "agent" } }), { press: false });
        if (!runtime.text().includes("All workers")) await runtime.press(/^Open worker /);
      };
      durable = "running";
      await openTab();
      assert.deepEqual(runtime.pulses(), [true]);
      durable = "completed";
      await openTab();
      assert.deepEqual(runtime.pulses(), [false]);
      runtime.unmountAll();
      assert.equal(animation.started, animation.stopped);
    } finally {
      runtime.unmountAll();
      cleanup();
    }
  });

  test(`worker tab renders the selected attempt's complete result and checks (${platformOs})`, async () => {
    const { runtime, captured, cleanup, rpcCalls } = await loadClient(platformOs, { "cortex.worker-history.read": workerHistoryFixture });
    const host = { theme, host: { id: "h1", label: "Local" }, layout: { compact: platformOs !== "web", platform: platformOs } };
    try {
      await runtime.mount(() => ({ type: captured.workers.Component, props: { ...host, workspaceId: "ws1", agentId: "a1", context: "agent" } }));
      const text = runtime.text();
      for (const shown of ["Result", "WORKER SUMMARY BODY", "Checks · 2 · 1 open", "CHECK-NAME-PASSED", "Passed", "CHECK-NAME-WEAK", "Unverified",
        "WEAK EVIDENCE", "BLOCKER-TEXT", "Evidence gap: EVIDENCE-GAP-TEXT", "ACTIVITY ROW"]) {
        assert.ok(text.includes(shown), `worker tab must render ${shown}`);
      }
      // Result and Checks come before the retained activity.
      assert.ok(text.indexOf("WORKER SUMMARY BODY") < text.indexOf("ACTIVITY ROW"));
      assert.ok(rpcCalls.every((call) => call.name !== "cortex.worker-history.read" || call.input.compact === undefined), "the tab never uses the card's compact read");
    } finally {
      runtime.unmountAll();
      cleanup();
    }
  });
}

const panelHost = { theme, host: { id: "h1", label: "Local" }, layout: { compact: false, platform: "web" } };
const workerRow = (data: Record<string, unknown>) => ({ ...panelHost, agentId: "a1",
  item: { id: `worker-row:${String(data.callId)}`, kind: "worker-row", version: 1, data } });
const unavailableHistory = { available: false, assignments: [], truncated: false, nextCursor: null, nextAssignmentCursor: null,
  message: "Worker history is unavailable in this workspace store." };

test("a failed card shows only a category; the worker tab shows its reason when no attempt explains it", async () => {
  const reason = "Исполнитель не запустился: CODEX_AUTH_MISSING; lint: failed";
  const { runtime, captured, cleanup } = await loadClient("web", { "cortex.worker-history.read": (input) => Promise.resolve({
    available: true, truncated: false, nextCursor: null, nextAssignmentCursor: null,
    assignments: input.assignmentRef === "ref9" ? [{ taskId: "t_1", id: "wa_9", assignmentRef: "ref9", nodeKey: "n9", title: "Start it", status: "failed", attempts: [] }] : [],
  }) });
  try {
    await runtime.mount(() => ({ type: captured["worker-row"].Component, props: workerRow({
      callId: "worker:ref9", assignmentRef: "ref9", title: "Worker: debugger — Start it", status: "failed", failureReason: reason }) }), { press: false });
    const card = runtime.text();
    assert.match(card, /Failure details in the worker tab/);
    for (const hidden of ["CODEX_AUTH_MISSING", "Исполнитель", "lint"]) assert.ok(!card.includes(hidden), `card must not render ${hidden}`);
    await runtime.press(/Open worker$/);
    await runtime.mount(() => ({ type: captured.workers.Component, props: { ...panelHost, workspaceId: "ws1", agentId: "a1", context: "agent" } }), { press: false });
    const tab = runtime.text();
    assert.match(tab, /Selected worker has no attempt history/);
    assert.ok(tab.includes(reason), "the worker tab keeps the reason reachable");
  } finally {
    runtime.unmountAll();
    cleanup();
  }
});

test("an initial history read failure keeps the selection, explains itself and retries", async () => {
  let fail = true;
  const { runtime, captured, cleanup, rpcCalls } = await loadClient("web", { "cortex.worker-history.read": (input) =>
    input.compact || !fail ? workerHistoryFixture(input) : Promise.reject(new Error("socket closed at /home/someone/private")) });
  try {
    await runtime.mount(() => ({ type: captured["worker-row"].Component, props: workerRow({
      callId: "worker:ref1", assignmentRef: "ref1", title: "Worker: debugger — Fix it", status: "completed", openIssues: 1 }) }), { press: false });
    await runtime.press(/Open worker$/);
    rpcCalls.length = 0;
    await runtime.mount(() => ({ type: captured.workers.Component, props: { ...panelHost, workspaceId: "ws1", agentId: "a1", context: "agent" } }), { press: false });
    const failed = runtime.text();
    assert.match(failed, /Worker history could not be read\./);
    assert.match(failed, /Retry reading worker history/);
    for (const hidden of ["No worker assignments recorded yet", "Loading worker history", "socket closed", "private"]) {
      assert.ok(!failed.includes(hidden), `a failed read must not render ${hidden}`);
    }
    assert.ok(rpcCalls.length > 0 && rpcCalls.every((call) => call.input.assignmentRef === "ref1"), "the read stays scoped to the selected worker");
    fail = false;
    await runtime.press("Retry reading worker history");
    const recovered = runtime.text();
    assert.match(recovered, /WORKER SUMMARY BODY/);
    assert.ok(!recovered.includes("could not be read"));
  } finally {
    runtime.unmountAll();
    cleanup();
  }
});

for (const failure of ["rejected", "unavailable"] as const) {
  test(`${failure === "rejected" ? "a rejected" : "an unavailable"} refresh of a terminal worker keeps its details, marks them stale and retries`, async () => {
    let mode: "ok" | "failing" = "ok";
    const { runtime, captured, cleanup } = await loadClient("web", { "cortex.worker-history.read": (input) =>
      mode === "ok" ? workerHistoryFixture(input) : failure === "rejected" ? Promise.reject(new Error("boom")) : Promise.resolve(unavailableHistory) });
    const panel = () => ({ type: captured.workers.Component, props: { ...panelHost, workspaceId: "ws1", agentId: "a1", context: "agent" } });
    try {
      await runtime.mount(panel, { press: false });
      await runtime.press(/^Open worker /);
      assert.match(runtime.text(), /WORKER SUMMARY BODY/);
      // A terminal worker no longer polls: reopening the tab restores its
      // retained view and its one refresh fails.
      mode = "failing";
      await runtime.mount(panel, { press: false });
      const stale = runtime.text();
      for (const shown of ["WORKER SUMMARY BODY", "CHECK-NAME-WEAK", "ACTIVITY ROW", "Showing the last retained history", "Retry reading worker history"]) {
        assert.ok(stale.includes(shown), `a stale view keeps ${shown}`);
      }
      assert.ok(stale.includes(failure === "rejected" ? "Worker history could not be read." : unavailableHistory.message));
      assert.ok(!stale.includes("boom"));
      mode = "ok";
      await runtime.press("Retry reading worker history");
      const fresh = runtime.text();
      assert.match(fresh, /WORKER SUMMARY BODY/);
      assert.ok(!fresh.includes("Showing the last retained history"));
    } finally {
      runtime.unmountAll();
      cleanup();
    }
  });
}

test("an available:false first read explains itself instead of an empty worker list", async () => {
  const { runtime, captured, cleanup } = await loadClient("web", { "cortex.worker-history.read": () => Promise.resolve(unavailableHistory) });
  try {
    await runtime.mount(() => ({ type: captured.workers.Component, props: { ...panelHost, workspaceId: "ws1", agentId: "a1", context: "agent" } }), { press: false });
    const text = runtime.text();
    assert.equal(text.split(unavailableHistory.message).length - 1, 1, "the reason is shown once");
    assert.match(text, /Retry reading worker history/);
    assert.ok(!text.includes("No worker assignments recorded yet"));
  } finally {
    runtime.unmountAll();
    cleanup();
  }
});

test("status announcements name the worker, once per transition, never for usage polls", async () => {
  const durable: Record<string, string> = { w1: "running", w2: "running" };
  let reads = 0;
  const { runtime, captured, cleanup } = await loadClient("web", { "cortex.worker-history.read": (input) => {
    reads += 1;
    const ref = String(input.assignmentRef);
    return Promise.resolve({ available: true, truncated: false, nextCursor: null, nextAssignmentCursor: null,
      assignments: [{ taskId: "t_1", id: `wa_${ref}`, assignmentRef: ref, nodeKey: ref, status: durable[ref], attempts: [{ id: `xa_${ref}`, attemptNo: 1,
        status: durable[ref], failureReason: null, events: [], context: { used: 1000 + reads, size: 100000 } }] }] });
  } });
  const data: Record<string, Record<string, unknown>> = {
    w1: { callId: "worker:w1", assignmentRef: "w1", title: "Worker: debugger · claude · Opus — Fix one", status: "running" },
    w2: { callId: "worker:w2", assignmentRef: "w2", title: "Worker: reviewer · codex · gpt — Review two", status: "running" },
  };
  const Card = captured["worker-row"].Component;
  try {
    await runtime.mount(() => [{ type: Card, props: workerRow(data.w1) }, { type: Card, props: workerRow(data.w2) }], { press: false });
    assert.deepEqual(runtime.liveRegions(), ["", ""], "mounting and the first durable reads announce nothing");
    const readsBefore = reads;

    durable.w1 = "completed";
    data.w1 = { ...data.w1, status: "completed" };
    await runtime.settle();
    assert.ok(reads > readsBefore, "the settled card re-read its usage");
    assert.deepEqual(runtime.liveRegions(), ["Worker debugger, Fix one: Completed", ""]);
    // A further usage-only change says nothing new.
    await runtime.settle();
    assert.deepEqual(runtime.liveRegions(), ["Worker debugger, Fix one: Completed", ""]);

    durable.w2 = "failed";
    data.w2 = { ...data.w2, status: "failed" };
    await runtime.settle();
    assert.deepEqual(runtime.liveRegions(), ["Worker debugger, Fix one: Completed", "Worker reviewer, Review two: Failed"]);
    // The visible status pill is no longer a live region of its own.
    assert.equal(runtime.liveRegions().length, 2);
  } finally {
    runtime.unmountAll();
    cleanup();
  }
});

test("Cortex settings registers a native screen and a global command that opens it", async () => {
  const { captured, commands, cleanup } = await loadClient("web");
  try {
    assert.ok(captured["cortex-settings"], "native settings screen registered");
    const command = commands["open-cortex-settings"];
    assert.ok(command, "settings command registered");
    assert.equal(command.title, "Open Cortex settings");
    assert.equal(command.context, "global");
    const opened: string[] = [];
    command.onSelect({ openSettings: (id) => {
      assert.ok(captured[id], `native settings screen ${id} is registered`);
      opened.push(id);
    } });
    assert.deepEqual(opened, ["cortex-settings"]);
  } finally {
    cleanup();
  }
});

for (const platformOs of ["android", "web"] as const) {
  test(`every rendered surface has defined element types (${platformOs})`, async () => {
    const { runtime, captured, pill, cleanup } = await loadClient(platformOs);
    const host = { theme, host: { id: "h1", label: "Local" }, layout: { compact: platformOs !== "web", platform: platformOs } };
    try {
      const card = captured["worker-row"];
      assert.ok(card, "worker-row timeline renderer registered");
      for (const data of [
        { callId: "worker:ref1", assignmentRef: "ref1", title: "worker-debugger · Fix it", status: "running" },
        { callId: "worker:ref1", assignmentRef: "ref1", title: "worker-debugger · Fix it", status: "completed", openIssues: 0 },
      ]) {
        const count = await runtime.mount(() => ({ type: card.Component, props: {
          ...host, agentId: "a1", item: { id: "worker-row:worker:ref1", kind: "worker-row", version: 1, data },
        } }));
        assert.ok(count > 5, `timeline card (${data.status}) rendered a real tree`);
      }

      const panelProps = { ...host, workspaceId: "ws1", agentId: "a1", context: "agent" };
      assert.ok((await runtime.mount(() => ({ type: captured.pipeline.Component, props: panelProps }))) > 20, "pipeline panel rendered its steps");
      assert.ok((await runtime.mount(() => ({ type: captured.workers.Component, props: panelProps }))) > 1, "workers panel rendered");

      const settings = captured["cortex-settings"];
      assert.ok(settings, "Cortex settings screen registered");
      assert.ok((await runtime.mount(() => ({ type: settings.Component, props: host }))) > 15, "settings screen rendered its default section");

      const registered = pill();
      assert.ok(registered, "Pipeline composer pill registered");
      const behavior = registered.button.behavior;
      if (platformOs === "web") {
        assert.equal(behavior.kind, "action");
      } else {
        // The phone's Pipeline bottom sheet.
        assert.equal(behavior.kind, "popover");
        const Content = behavior.Content!;
        assert.ok((await runtime.mount(() => ({ type: Content, props: { ...host, close() {} } }))) > 20, "pipeline popover rendered its steps");
      }
    } finally {
      runtime.unmountAll();
      cleanup();
    }
  });
}

// The selected attempt's assignment prompt is reachable in the worker tab
// (collapsed to its state and a preview, full on demand), and the attempt's
// Result paragraph is not repeated by its identical worker_result row.
test("worker tab shows the attempt's sent prompt and its result once", async () => {
  const summary = "RESULT PARAGRAPH reconciled everything.";
  const brief = JSON.stringify({ objective: "PROMPT OBJECTIVE first line\n\nPROMPT OBJECTIVE DETAIL", scope: ["SCOPE-PATH"], dependency_reports: [], original_goal: "OWNER GOAL", review_evidence: [] });
  const { runtime, captured, cleanup, rpcCalls } = await loadClient("web", { "cortex.worker-history.read": (input) => Promise.resolve({
    available: true, truncated: false, nextCursor: null, nextAssignmentCursor: null,
    assignments: [{ taskId: "t_1", id: "wa_1", assignmentRef: "ref1", nodeKey: "n1", title: "Fix it", role: "debugger", status: "completed",
      attempts: [{ id: "xa_1", attemptNo: 1, status: "completed", failureReason: null, ...(input.compact ? { openIssues: 0, events: [] } : {
        summary, checks: [],
        prompt: { state: "sent", sentAt: "2026-10-21T18:47:05Z", brief, acceptance: "ACCEPTANCE LINE" },
        events: [
          { id: 1, type: "worker_result", createdAt: "2026-10-21T00:00:00Z", text: summary },
          { id: 2, type: "worker_result", createdAt: "2026-10-21T00:00:01Z", text: summary },
          { id: 3, type: "worker_activity", createdAt: "2026-10-21T00:00:02Z", text: "FINAL RESPONSE ROW" },
        ] }) }] }],
  }) });
  try {
    await runtime.mount(() => ({ type: captured["worker-row"].Component, props: workerRow({
      callId: "worker:ref1", assignmentRef: "ref1", title: "Worker: debugger — Fix it", status: "completed", openIssues: 0 }) }), { press: false });
    const card = runtime.text();
    for (const hidden of ["PROMPT OBJECTIVE", "ACCEPTANCE LINE", "RESULT PARAGRAPH", "Assignment prompt"]) {
      assert.ok(!card.includes(hidden), `coordinator card must not render ${hidden}`);
    }
    await runtime.press(/Open worker$/);
    await runtime.mount(() => ({ type: captured.workers.Component, props: { ...panelHost, workspaceId: "ws1", agentId: "a1", context: "agent" } }), { press: false });
    const collapsed = runtime.text();
    assert.match(collapsed, /Assignment prompt/);
    assert.match(collapsed, /Sent to this attempt · 2026-10-21T18:47:05Z/);
    assert.match(collapsed, /PROMPT OBJECTIVE first line/);
    assert.ok(!collapsed.includes("PROMPT OBJECTIVE DETAIL"), "collapsed until opened");
    assert.equal(collapsed.split("RESULT PARAGRAPH").length - 1, 1, "the result paragraph is shown once");
    assert.match(collapsed, /FINAL RESPONSE ROW/);
    await runtime.press("Show assignment prompt");
    const open = runtime.text();
    for (const shown of ["PROMPT OBJECTIVE DETAIL", "SCOPE-PATH", "OWNER GOAL", "ACCEPTANCE LINE", "Hide assignment prompt"]) {
      assert.ok(open.includes(shown), `opened prompt shows ${shown}`);
    }
    // Prompt before Result before activity.
    assert.ok(open.indexOf("ACCEPTANCE LINE") < open.indexOf("RESULT PARAGRAPH"));
    assert.ok(open.indexOf("RESULT PARAGRAPH") < open.indexOf("FINAL RESPONSE ROW"));
    assert.equal(open.split("RESULT PARAGRAPH").length - 1, 1);
    assert.ok(rpcCalls.some((call) => call.input.assignmentRef === "ref1" && call.input.compact === undefined));
  } finally {
    runtime.unmountAll();
    cleanup();
  }
});

test("the error boundary is a real class component and renders its fallback", async () => {
  const { runtime, captured, cleanup } = await loadClient("android");
  try {
    const root = captured.pipeline.Component({ theme, workspaceId: "ws1", agentId: "a1" }) as Element;
    const Boundary = root.type as { prototype: { isReactComponent?: unknown; render(): unknown }; getDerivedStateFromError(error: unknown): unknown };
    assert.equal(typeof Boundary, "function", "boundary element type is defined");
    assert.ok(Boundary.prototype.isReactComponent, "React treats the boundary as a class component");
    const error = new Error("boom");
    assert.deepEqual(Boundary.getDerivedStateFromError(error), { error });
    const instance = new (Boundary as unknown as new (props: unknown) => { state: unknown; render(): unknown })(root.props);
    assert.deepEqual(instance.state, { error: undefined });
    assert.equal(instance.render(), root.props.children, "renders children while healthy");
    instance.state = { error };
    let text = "";
    await runtime.mount(() => instance.render());
    const walk = (node: unknown): void => {
      if (typeof node === "string") text += node;
      else if (node && typeof node === "object" && "props" in node) walk((node as Element).props.children);
    };
    walk(instance.render());
    assert.equal(text, "Pipeline panel could not be shown (boom). Reopen it to try again.");
  } finally {
    runtime.unmountAll();
    cleanup();
  }
});
