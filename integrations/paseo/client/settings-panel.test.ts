import assert from "node:assert/strict";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";
import { build } from "esbuild";

/**
 * Interaction tests for the Cortex Settings screen: the real
 * `settings-panel.tsx` (bundled like the plugin client) is rendered with a
 * small hook runtime and host-module shims, then driven through its typed
 * controls. Each test checks the exact `cortex.config.update` payload a
 * control produces, revision-conflict handling, accessibility labelling, and
 * that no raw JSON entry field exists anywhere on the screen.
 *
 * Run with the pure form-model tests (`settings-model.test.ts`) as
 * `npm --prefix integrations/paseo run test:settings`.
 */

const pluginRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");

async function compilePanel(): Promise<string> {
  const result = await build({
    entryPoints: [join(pluginRoot, "client", "settings-panel.tsx")],
    bundle: true,
    format: "cjs",
    jsx: "automatic",
    platform: "neutral",
    target: "es2020",
    external: [
      "@getpaseo/plugin", "@getpaseo/plugin/client", "@getpaseo/plugin/client/react-native", "react", "react/jsx-runtime", "react-native", "zod",
    ],
    write: false,
    logLevel: "silent",
  });
  return `(function(require) {\nconst module = { exports: {} };\nconst exports = module.exports;\n${result.outputFiles[0].text}\nreturn module.exports;\n})`;
}

type Element = { type: unknown; props: Record<string, unknown> };
type EffectSlot = { deps?: unknown[]; cleanup?: (() => void) | void };

/** A minimal hook runtime: state is keyed by component path, effects run
 * after each render pass, and `settle` re-renders until quiescent. */
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
    const childPath = `${path}/${type.name || "anonymous"}`;
    current = { path: childPath, index: 0 };
    const output = (type as (props: unknown) => unknown)(element.props);
    current = null;
    render(output, childPath);
  }

  let root: () => unknown = () => null;
  async function settle(): Promise<void> {
    for (let round = 0; round < 40; round += 1) {
      dirty = false;
      elements = [];
      render(root(), "root");
      const effects = pending;
      pending = [];
      for (const effect of effects) effect();
      await new Promise((done) => setTimeout(done, 0));
      if (!dirty && pending.length === 0) return;
    }
    throw new Error("settings screen did not settle");
  }
  return {
    react,
    settle,
    async mount(next: () => unknown) { root = next; await settle(); },
    elements: () => elements,
  };
}

function textOf(node: unknown): string {
  if (typeof node === "string" || typeof node === "number") return String(node);
  if (Array.isArray(node)) return node.map(textOf).join("");
  if (node && typeof node === "object" && "props" in node) return textOf((node as Element).props.children);
  return "";
}

// --- fixtures -------------------------------------------------------------------

const REV = (letter: string) => `sha256:${letter.repeat(64)}`;
const OPEN_CEILING = {
  allowed_routes: null, allowed_connections: null, allowed_data_recipients: null, permission_level: null,
  limits: {}, spend_limit: null, spend_limit_enforceable: false,
  fallback_policy: { allowed_target_routes: null, allowed_failure_classes: null, budget: null, data_recipient_consent: false },
};

function fixtures() {
  return {
    user_defaults: {
      surface: "user_defaults", path: "~/.config/cortex/defaults.json", revision: REV("a"), exists: true,
      defaults: { route_id: "xr_0123456789ab", route_connection_id: "cn_0123456789ab", route_model: "gpt-5", language: "en", parameters: { tone: "brief" }, legacy_flag: true },
      editable_keys: ["route_id", "route_connection_id", "route_model", "parameters", "language"], preserved_keys: ["legacy_flag"],
      languages: ["en", "ru"], ceiling: OPEN_CEILING, ceiling_set: true,
    },
    project_ceiling: {
      surface: "project_ceiling", project_root: "<project>", revision: REV("p"), exists: true, user_ceiling: OPEN_CEILING,
      ceiling: { ...OPEN_CEILING, permission_level: "STANDARD", limits: { tokens: 1000 } }, project_defaults: { route_id: "xr_0123456789ab" },
      effective_ceiling: { ...OPEN_CEILING, permission_level: "STANDARD", limits: { tokens: 1000 } }, narrows_user_ceiling: true, violation: null,
      updated_at: "2026-09-01T00:00:00Z",
    },
    routing: {
      surface: "routing", project_root: null, precedence: ["project", "env", "user"],
      sources: [
        { source: "env", path: "(unset)", writable: false, exists: false, revision: null },
        {
          source: "user", path: "~/.config/cortex/routing.json", writable: true, exists: true, revision: REV("b"), valid: true,
          rules: { pin: { "*": "opencode/anthropic/claude-sonnet-4-5:high" }, deny: ["claude-agent-acp/opus"], providers: ["codex-acp", "opencode"] },
          ignored_keys: ["comment"],
        },
      ],
      effective_source: "user", effective_valid: true, effective_rules: null,
      builtin: {
        supported_engines: ["codex-acp", "claude-agent-acp"], optional_engines: ["opencode"], default_auto_deny_engines: ["opencode"],
        default_auto_deny_families: [], tiers: ["light", "standard", "heavy"], efforts: ["low", "medium", "high"], work_kinds: ["code", "review"],
        default_user_deny: [],
      },
    },
    mcp_servers: {
      surface: "mcp_servers", path: "~/.config/cortex/mcp-servers.json", revision: REV("m"), exists: true, root_key: "servers", version: 1,
      servers: [
        {
          name: "search", enabled: true, valid: true, problem: null, redacted: true, unsupported_keys: [],
          config: { command: "npx", args: ["-y", "search-mcp", "--token", "[redacted]"], env: [{ name: "API_KEY", value_set: true }], allowed_tools: ["find"] },
        },
        { name: "legacy", enabled: false, valid: true, problem: null, redacted: false, unsupported_keys: ["cwd"], config: { command: "legacy-mcp" } },
      ],
      editable_keys: [], engines: ["codex-acp", "claude-agent-acp", "opencode"],
    },
    connections: {
      surface: "connections", revision: REV("c"), exists: true,
      connections: [{
        id: "cn_0123456789ab", name: "Codex", engine: "codex-acp", provider: "openai", auth_mode: "native", key_kind: null, endpoint: null,
        launch_command: null, credential: { kind: "native_dir", owned: true }, state: "ready", qualified: true, max_concurrent_sessions: 2,
        optional_engine: false, created_at: "2026-09-01T00:00:00Z", updated_at: "2026-09-01T00:00:00Z",
      }],
      engines: [
        { engine: "codex-acp", optional: false, onboarding: "native", auth_modes: { native: true }, qualified_versions: ["1.13.0"], connection_ids: ["cn_0123456789ab"] },
        { engine: "opencode", optional: true, onboarding: "explicit_opt_in", auth_modes: {}, qualified_versions: ["2.0.19"], connection_ids: [] },
      ],
      operations: ["rename", "set_max_concurrent_sessions", "validate", "remove"], max_concurrent_sessions_limit: 8,
    },
  } as Record<string, Record<string, unknown>>;
}

const binding = { apiVersion: 1, configHome: "~/.config/cortex", project: null };
const colors = new Proxy({}, { get: () => "#808080" });
const theme = { colors, spacing: new Proxy({}, { get: () => 4 }) };

async function openScreen() {
  const runtime = createRuntime();
  const data = fixtures();
  const reads: Array<Record<string, unknown>> = [];
  const updates: Array<Record<string, unknown>> = [];
  const onboards: Array<Record<string, unknown>> = [];
  const replies: unknown[] = [];
  const rpcs: Record<string, (input: Record<string, unknown>) => Promise<unknown>> = {
    "cortex.config.read": async (input) => {
      reads.push(input);
      return { ok: true, binding, result: data[input.surface as string] };
    },
    "cortex.config.update": async (input) => {
      updates.push(input);
      const reply = replies.shift();
      return reply ?? { ok: true, binding, result: data[input.surface as string] };
    },
    "cortex.config.opencode.discover": async () => ({
      ok: true, binding, result: { found: true, version: "2.0.19", qualified: true, qualified_versions: ["2.0.19"], onboarded_connection_ids: [] },
    }),
    "cortex.config.opencode.onboard": async (input) => { onboards.push(input); return { ok: true, binding, result: data.connections }; },
  };
  const paseo = { workspaces: { list: async () => ({ entries: [{ id: "ws1", title: null, name: "ws1" }] }) } };
  const modules: Record<string, unknown> = {
    react: runtime.react,
    "react/jsx-runtime": {
      jsx: (type: unknown, props: Record<string, unknown>) => ({ type, props }),
      jsxs: (type: unknown, props: Record<string, unknown>) => ({ type, props }),
      Fragment: "Fragment",
    },
    "react-native": new Proxy({}, { get: (_target, key) => String(key) }),
    "@getpaseo/plugin": await import("@getpaseo/plugin"),
    "@getpaseo/plugin/client": { usePaseo: () => paseo, useRpc: (definition: { name: string }) => rpcs[definition.name] },
    "@getpaseo/plugin/client/react-native": { Icon: "Icon", ScrollView: "ScrollView" },
    zod: await import("zod"),
  };
  const evaluate = new Function(`return ${await compilePanel()}`)() as (require: (name: string) => unknown) => { SettingsScreen: (props: unknown) => unknown };
  const { SettingsScreen } = evaluate((name) => {
    assert.ok(name in modules, `settings panel required unexpected module ${name}`);
    return modules[name];
  });
  await runtime.mount(() => ({ type: SettingsScreen, props: { theme } }));

  const find = (label: string, type?: string): Element => {
    const found = runtime.elements().filter((element) => element.props?.accessibilityLabel === label && (!type || element.type === type));
    assert.ok(found.length > 0, `no control labelled ${JSON.stringify(label)}`);
    return found[0];
  };
  const has = (label: string) => runtime.elements().some((element) => element.props?.accessibilityLabel === label);
  const press = async (label: string) => { (find(label, "Pressable").props.onPress as () => void)(); await runtime.settle(); };
  const type = async (label: string, value: string) => { (find(label, "TextInput").props.onChangeText as (v: string) => void)(value); await runtime.settle(); };
  const toggle = async (label: string, value: boolean) => { (find(label, "Switch").props.onValueChange as (v: boolean) => void)(value); await runtime.settle(); };
  const texts = () => runtime.elements().filter((element) => element.type === "Text").map((element) => textOf(element.props.children));
  const section = async (title: string) => press(`Section: ${title}`);
  return { runtime, data, reads, updates, onboards, replies, find, has, press, type, toggle, texts, section };
}

// --- tests ------------------------------------------------------------------------

test("no section offers a raw JSON entry field, and every control is labelled", async () => {
  const screen = await openScreen();
  await screen.press("Project (for project ceiling and project routing): ws1");
  const visits: Array<[string, () => Promise<void>]> = [
    ["User defaults", async () => {}],
    ["Project ceiling", async () => {}],
    ["Routing", async () => {}],
    ["MCP servers", async () => { await screen.press("Edit search"); await screen.press("Add MCP server"); }],
    ["Connections", async () => { await screen.press("Check for OpenCode"); }],
  ];
  for (const [title, extra] of visits) {
    await screen.section(title);
    await extra();
    const elements = screen.runtime.elements();
    const inputs = elements.filter((element) => element.type === "TextInput");
    assert.ok(inputs.length > 0, `${title}: rendered editable fields`);
    for (const input of inputs) {
      assert.notEqual(input.props.multiline, true, `${title}: ${String(input.props.accessibilityLabel)} is a multi-line (JSON-style) editor`);
      assert.ok(typeof input.props.accessibilityLabel === "string" && input.props.accessibilityLabel.length > 0, `${title}: unlabelled text field`);
      assert.doesNotMatch(String(input.props.accessibilityLabel), /json/i, `${title}: a field asks for JSON`);
    }
    for (const element of elements) {
      if (element.type === "Pressable") {
        assert.ok(typeof element.props.accessibilityLabel === "string" && element.props.accessibilityLabel.length > 0, `${title}: unlabelled button`);
        assert.ok(["button", "radio", "checkbox"].includes(String(element.props.accessibilityRole)), `${title}: ${String(element.props.accessibilityLabel)} has no role`);
        if (element.props.accessibilityRole !== "button") {
          const state = element.props.accessibilityState as { checked?: unknown } | undefined;
          assert.equal(typeof state?.checked, "boolean", `${title}: ${String(element.props.accessibilityLabel)} does not expose its checked state`);
        }
      }
      if (element.type === "Switch") {
        assert.equal(element.props.accessibilityRole, "switch");
        assert.ok(typeof element.props.accessibilityLabel === "string" && element.props.accessibilityLabel.length > 0, `${title}: unlabelled switch`);
      }
    }
    assert.ok(!screen.texts().some((text) => /\bJSON\b/.test(text)), `${title}: visible text still mentions JSON entry`);
  }
});

test("user defaults: a selector change saves exactly that field against the read revision", async () => {
  const screen = await openScreen();
  assert.deepEqual(screen.find("Language: en", "Pressable").props.accessibilityState, { checked: true, disabled: false });
  await screen.press("Language: ru");
  await screen.press("Save");
  assert.deepEqual(screen.updates, [{ surface: "user_defaults", expected_revision: REV("a"), defaults: { language: "ru" } }]);
  assert.ok(screen.texts().includes("Saved."));
  assert.ok(screen.texts().some((text) => text.includes("legacy_flag")), "preserved key shown read-only");
});

test("user defaults: invalid input is flagged on the field and nothing is written", async () => {
  const screen = await openScreen();
  await screen.type("Route id", "codex-acp/gpt-5");
  await screen.type("Limit: tokens", "many");
  await screen.press("Save");
  assert.equal(screen.updates.length, 0);
  assert.match(String(screen.find("Route id", "TextInput").props.accessibilityHint), /xr_/);
  assert.match(String(screen.find("Limit: tokens", "TextInput").props.accessibilityHint), /whole number/);
  const alerts = screen.runtime.elements().filter((element) => element.type === "Text" && element.props.accessibilityRole === "alert");
  assert.ok(alerts.length >= 2, "field errors are announced");
});

test("user defaults: API field errors land on the field they name", async () => {
  const screen = await openScreen();
  screen.replies.push({
    ok: false, binding,
    error: { code: "invalid_configuration", message: "Refused", field: null, errors: [{ field: "defaults.route_model", code: "invalid_value", message: "must be the route's exact model id" }] },
  });
  await screen.type("Model (exact id)", "gpt 5");
  await screen.press("Save");
  assert.equal(screen.updates.length, 1);
  assert.equal(screen.find("Model (exact id)", "TextInput").props.accessibilityHint, "must be the route's exact model id");
  assert.ok(screen.texts().some((text) => text.startsWith("Not saved:")));
});

test("a revision conflict writes nothing more and offers a reload of the latest file", async () => {
  const screen = await openScreen();
  screen.replies.push({
    ok: false, binding,
    error: { code: "revision_conflict", message: "defaults.json changed after it was read.", field: "expected_revision", currentRevision: REV("f") },
  });
  await screen.press("Language: ru");
  await screen.press("Save");
  assert.ok(screen.texts().some((text) => text.startsWith("Changed elsewhere since this was loaded")));
  const readsBefore = screen.reads.filter((read) => read.surface === "user_defaults").length;
  screen.data.user_defaults = { ...screen.data.user_defaults, revision: REV("f"), defaults: { ...(screen.data.user_defaults.defaults as object), language: "ru" } };
  await screen.press("Reload latest (discards edits here)");
  assert.equal(screen.reads.filter((read) => read.surface === "user_defaults").length, readsBefore + 1);
  assert.equal(screen.updates.length, 1, "the conflict is never retried automatically");
  assert.deepEqual(screen.find("Language: ru", "Pressable").props.accessibilityState, { checked: true, disabled: false });
  await screen.press("Language: en");
  await screen.press("Save");
  assert.deepEqual(screen.updates[1], { surface: "user_defaults", expected_revision: REV("f"), defaults: { language: "en" } });
});

test("routing: a per-model effort is edited as a typed pin and saved to its own file revision", async () => {
  const screen = await openScreen();
  await screen.section("Routing");
  assert.ok(screen.texts().some((text) => text.includes("comment")), "unmanaged routing keys are listed as kept");
  assert.equal(screen.find("Pin 1 model (full id)", "TextInput").props.value, "anthropic/claude-sonnet-4-5");
  await screen.press("Pin 1 effort: medium");
  await screen.press("Save user routing");
  assert.deepEqual(screen.updates, [{
    surface: "routing", target: "user", expected_revision: REV("b"),
    rules: { pin: { "*": "opencode/anthropic/claude-sonnet-4-5:medium" }, deny: ["claude-agent-acp/opus"], providers: ["codex-acp", "opencode"] },
  }]);
});

test("routing: adding a deny entry and reordering engines produce the typed lists", async () => {
  const screen = await openScreen();
  await screen.section("Routing");
  await screen.press("Add denied route or engine");
  await screen.type("Denied route or engine 2", "opencode/anthropic/claude-opus-4");
  await screen.press("Move opencode up");
  await screen.press("Save user routing");
  const rules = screen.updates[0].rules as Record<string, unknown>;
  assert.deepEqual(rules.deny, ["claude-agent-acp/opus", "opencode/anthropic/claude-opus-4"]);
  assert.deepEqual(rules.providers, ["opencode", "codex-acp"]);
});

test("MCP: editing keeps stored env values and redacted parts; replacing is explicit", async () => {
  const screen = await openScreen();
  await screen.section("MCP servers");
  await screen.press("Edit search");
  await screen.press("Save server");
  assert.deepEqual(screen.updates[0], {
    surface: "mcp_servers", expected_revision: REV("m"),
    operations: [{ op: "upsert", name: "search", server: {
      command: "npx", args: ["-y", "search-mcp", "--token", "[redacted]"], env: { API_KEY: { keep: true } }, allowed_tools: ["find"],
    } }],
  });

  const again = await openScreen();
  await again.section("MCP servers");
  await again.press("Edit search");
  await again.press("API_KEY value: Replace value");
  assert.equal(again.find("New value for API_KEY", "TextInput").props.secureTextEntry, true);
  await again.type("New value for API_KEY", "rotated");
  await again.press("Save server");
  const server = ((again.updates[0].operations as Array<Record<string, unknown>>)[0].server) as Record<string, unknown>;
  assert.deepEqual(server.env, { API_KEY: "rotated" });
});

test("MCP: an entry with unmanaged keys is read-only but can still be toggled safely", async () => {
  const screen = await openScreen();
  await screen.section("MCP servers");
  assert.equal((screen.find("Edit legacy", "Pressable").props.accessibilityState as { disabled: boolean }).disabled, true);
  assert.ok(screen.texts().some((text) => text.includes("cwd")), "the unmanaged key is named");
  await screen.toggle("legacy enabled", true);
  assert.deepEqual(screen.updates, [{ surface: "mcp_servers", expected_revision: REV("m"), operations: [{ op: "set_enabled", name: "legacy", enabled: true }] }]);
});

test("MCP: a new server is built from typed fields with field-level validation", async () => {
  const screen = await openScreen();
  await screen.section("MCP servers");
  await screen.press("Add MCP server");
  await screen.press("Save server");
  assert.equal(screen.updates.length, 0);
  assert.match(String(screen.find("Command", "TextInput").props.accessibilityHint), /command/);
  await screen.type("Server name", "files");
  await screen.type("Command", "files-mcp");
  await screen.toggle("Limit to roles", true);
  await screen.press("Roles: worker");
  await screen.toggle("Environment variables", true);
  await screen.press("Add environment variable");
  await screen.type("Environment variable 1 name", "ROOT");
  await screen.type("Environment variable 1 value", "/srv");
  await screen.press("Save server");
  assert.deepEqual(screen.updates[0], {
    surface: "mcp_servers", expected_revision: REV("m"),
    operations: [{ op: "upsert", name: "files", server: { command: "files-mcp", roles: ["worker"], env: { ROOT: "/srv" } } }],
  });
});

test("connections: numeric session limits are validated; OpenCode v2 needs an explicit opt-in", async () => {
  const screen = await openScreen();
  await screen.section("Connections");
  const sessions = "Max concurrent sessions for Codex (1-8)";
  await screen.type(sessions, "99");
  await screen.press("Apply max concurrent sessions for Codex");
  assert.equal(screen.updates.length, 0);
  assert.match(String(screen.find(sessions, "TextInput").props.accessibilityHint), /at most 8/);
  await screen.type(sessions, "3");
  await screen.press("Apply max concurrent sessions for Codex");
  assert.deepEqual(screen.updates, [{
    surface: "connections", expected_revision: REV("c"), connection_id: "cn_0123456789ab", operation: "set_max_concurrent_sessions", max_concurrent_sessions: 3,
  }]);

  await screen.press("Check for OpenCode");
  assert.equal((screen.find("Enable OpenCode v2", "Pressable").props.accessibilityState as { disabled: boolean }).disabled, true);
  await screen.toggle("I want to enable OpenCode v2 for this owner", true);
  await screen.press("Enable OpenCode v2");
  assert.deepEqual(screen.onboards, [{ expected_revision: REV("c"), explicit_opt_in: true }]);
});

test("project ceiling: typed ceiling fields save with the workspace and revision", async () => {
  const screen = await openScreen();
  await screen.press("Project (for project ceiling and project routing): ws1");
  await screen.section("Project ceiling");
  assert.equal(screen.find("Limit: tokens", "TextInput").props.value, "1000");
  await screen.press("Project permission level: READ_ONLY");
  await screen.toggle("Restrict allowed routes", true);
  await screen.press("Add allowed routes");
  await screen.type("Allowed routes 1", "codex-acp/gpt-5");
  await screen.press("Save");
  assert.deepEqual(screen.updates, [{
    surface: "project_ceiling", workspaceId: "ws1", expected_revision: REV("p"),
    ceiling: { ...OPEN_CEILING, allowed_routes: ["codex-acp/gpt-5"], permission_level: "READ_ONLY", limits: { tokens: 1000 } },
  }]);
});
