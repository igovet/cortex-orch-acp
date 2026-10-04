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

  /** Render to a settled state: run effects, let async RPCs land, re-render
   * while state changed; then press every Pressable once and settle again so
   * expandable sections render too. Returns the element count seen. */
  async function mount(root: () => unknown): Promise<number> {
    // Each surface mounts alone, as on the phone: unsubscribe the previous
    // one so shared pollers start a fresh group with an immediate read.
    unmountAll();
    let count = 0;
    let presses: Array<() => void> = [];
    const pass = () => {
      count = 0;
      presses = [];
      render(root(), "root", (element) => {
        count += 1;
        const onPress = element.props?.onPress;
        if (typeof onPress === "function") presses.push(() => (onPress as () => void)());
      });
    };
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
    await settle();
    for (const press of presses) press();
    await settle();
    return count;
  }

  function unmountAll(): void {
    for (const values of hooks.values()) {
      for (const value of values) (value as EffectSlot | undefined)?.cleanup?.();
    }
    hooks.clear();
  }

  return { react, mount, unmountAll };
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

async function loadClient(platformOs: "android" | "ios" | "web") {
  const runtime = createRuntime();
  const component = (name: string) => name;
  const reactNative = new Proxy({}, {
    get(_target, key) {
      if (key === "Platform") return { OS: platformOs, select: (options: Record<string, unknown>) => options[platformOs] ?? options.default };
      if (key === "useWindowDimensions") return () => ({ width: 390, height: 844 });
      if (key === "StyleSheet") return { create: (styles: unknown) => styles, hairlineWidth: 1 };
      if (key === "Animated") {
        return {
          Value: function Value(this: Record<string, unknown>) { this.setValue = () => {}; this.stopAnimation = () => {}; },
          View: "Animated.View",
          loop: () => ({ start() {}, stop() {} }),
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
        call = definition.name === "cortex.pipeline.read"
          ? async () => pipelineFixture
          : definition.name === "cortex.config.read"
          ? (input: unknown) => configReadFixture(input as { surface: string })
          : () => new Promise<never>(() => {});
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
  const client = {
    addTimelineTransformer: () => registration,
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
  return { runtime, captured, commands, pill: () => pill, cleanup };
}

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
        { callId: "worker:ref1", assignmentRef: "ref1", title: "worker-debugger · Fix it", status: "completed", result: { summary: "Fixed.", checks: [check] } },
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
