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

type Element = { key?: string; type: unknown; props: Record<string, unknown> };
type EffectSlot = { deps?: unknown[]; cleanup?: (() => void) | void };

/** A minimal hook runtime: state is keyed by component path, effects run
 * after each render pass, and `settle` re-renders until quiescent. */
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
  async function settle(): Promise<void> {
    for (let round = 0; round < 40; round += 1) {
      dirty = false;
      elements = [];
      mounted.clear();
      render(root(), "root");
      for (const [path, values] of hooks) {
        if (mounted.has(path)) continue;
        for (const value of values) {
          if (value && typeof value === "object" && "cleanup" in value) (value as EffectSlot).cleanup?.();
        }
        hooks.delete(path);
      }
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

const CONN = {
  codex: ["cn_0123456789ab", "Codex"], claude: ["cn_1123456789ab", "Claude"], opencode: ["cn_2123456789ab", "OpenCode"],
} as const;

function catalogRoute(id: string, engine: string, model: string, name: string, connection: readonly [string, string], extra: Record<string, unknown> = {}) {
  return {
    route_id: `xr_${id.repeat(12)}`, engine, model, model_name: name, connection_id: connection[0], connection_name: connection[1],
    connection_state: "configured", qualified: true, observed: true, tier: "standard", selectable: true, in_selector: true,
    disabled: false, deny_tokens: [], enabled: true, engine_disabled: false, default_auto_excluded: null, auto_allow_tokens: [], automatic: "eligible", auto_selected: engine !== "opencode",
    superseded_by: null, preferred_for: [], preferred_inactive: false, matches: { user: { deny: [], auto_allow: [] } }, ...extra,
  };
}

function catalogEngine(engine: string, connection: readonly [string, string] | null, extra: Record<string, unknown> = {}) {
  return {
    engine, optional: engine === "opencode", disabled: false, automatic: true, default_auto_excluded: engine === "opencode", route_count: 1,
    connections: connection ? [{ id: connection[0], name: connection[1], state: "configured", qualified: true, catalog_observed_at: "2026-09-30T08:00:00+00:00" }] : [],
    enabled_models: engine === "opencode" ? ["anthropic/claude-sonnet-4-5"] : null, enabled_default: engine !== "opencode", unmatched_enabled: [],
    matches: { user: { disabled: false, automatic: true } }, ...extra,
  };
}

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
          rules: { model_policy_version: 2, pin: { "*": "opencode/anthropic/claude-sonnet-4-5:high" }, deny: ["claude-agent-acp/opus"], providers: ["codex-acp", "opencode"],
            enabled: { "codex-acp": ["gpt-6-luna"], "claude-agent-acp": ["opus[1m]", "claude-sonnet-5"], opencode: ["anthropic/claude-sonnet-4-5"] } },
          ignored_keys: ["comment"],
        },
      ],
      effective_source: "user", effective_valid: true, effective_rules: null,
      builtin: {
        supported_engines: ["codex-acp", "claude-agent-acp"], optional_engines: ["opencode"], default_auto_deny_engines: ["opencode"],
        default_auto_deny_families: [], tiers: ["light", "standard", "heavy"], efforts: ["low", "medium", "high"], work_kinds: ["code", "review"],
        work_kind_labels: { code: "Code", review: "Review" }, work_kind_roles: { code: [], review: [] },
        builtin_affinity: { code: "codex-acp", review: "claude-agent-acp" },
        default_user_deny: [],
      },
    },
    routing_profiles: {
      surface: "routing_profiles", path: "~/.config/cortex/routing-profiles.json", revision: REV("q"),
      format: 2, migrated_from: null, active: "mix", state: "modified", applied: null,
      pending: null, problems: [], modified_sections: ["routing"], differences: [{ section: "routing", key: "providers" }],
      current_revisions: { routing: REV("b"), user_defaults: REV("a"), mcp_servers: REV("m"), connections: REV("c") }, user_routing_revision: REV("b"), shadowed_by: null,
      matches_profile: [], profiles: [{
        name: "mix", description: "All engines", revision: REV("r"),
        sections: ["routing", "user_defaults", "mcp_servers", "connections"], settings: { routing: { providers: ["codex-acp", "opencode"] } }, created_at: "2026-09-01T00:00:00Z", updated_at: "2026-09-01T00:00:00Z",
        summary: { providers: ["codex-acp", "opencode"], denied_engines: [], selected_models: {}, default_model: null, language: "en", enabled_mcp_servers: 1, ceiling_set: true },
      }],
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
    route_catalog: {
      surface: "route_catalog", project_root: null, precedence: ["env", "user"], effective_source: "user", effective_valid: true,
      providers: ["codex-acp", "opencode"],
      engines: [
        catalogEngine("codex-acp", CONN.codex),
        catalogEngine("claude-agent-acp", CONN.claude, { automatic: false, route_count: 2, matches: { user: { disabled: false, automatic: false } } }),
        catalogEngine("opencode", CONN.opencode),
      ],
      routes: [
        catalogRoute("a", "codex-acp", "gpt-6-luna", "GPT-6 Luna", CONN.codex, { tier: "light" }),
        catalogRoute("b", "claude-agent-acp", "opus[1m]", "Opus 5.5", CONN.claude, {
          in_selector: false, disabled: true, deny_tokens: ["claude-agent-acp/opus"], automatic: "disabled", tier: null,
          matches: { user: { deny: ["claude-agent-acp/opus"], auto_allow: [] } },
        }),
        catalogRoute("c", "claude-agent-acp", "claude-sonnet-5", "Sonnet 5", CONN.claude, { automatic: "engine_excluded_from_automatic_routing", tier: null }),
        catalogRoute("d", "opencode", "anthropic/claude-sonnet-4-5", "Claude Sonnet 4.5", CONN.opencode, {
          default_auto_excluded: "optional_engine", automatic: "excluded_by_default", tier: null,
        }),
      ],
      catalog_problems: [], not_shown: ["recorded_unavailability", "quota"],
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
        { engine: "opencode", optional: true, onboarding: "explicit_opt_in", auth_modes: {}, qualified_versions: ["2.x.x"], connection_ids: [] },
      ],
      operations: ["rename", "set_max_concurrent_sessions", "validate", "remove"], max_concurrent_sessions_limit: 8,
    },
  } as Record<string, Record<string, unknown>>;
}

const binding = { apiVersion: 1, configHome: "~/.config/cortex", project: null };
const colors = new Proxy({}, { get: () => "#808080" });
const theme = { colors, spacing: new Proxy({}, { get: () => 4 }) };

async function openScreen(data = fixtures()) {
  const runtime = createRuntime();
  const reads: Array<Record<string, unknown>> = [];
  const updates: Array<Record<string, unknown>> = [];
  const onboards: Array<Record<string, unknown>> = [];
  const onboardReplies: unknown[] = [];
  const replies: unknown[] = [];
  const refreshes: Array<Record<string, unknown>> = [];
  const rpcs: Record<string, (input: Record<string, unknown>) => Promise<unknown>> = {
    "cortex.config.read": async (input) => {
      reads.push(input);
      return { ok: true, binding, result: structuredClone(data[input.surface as string]) };
    },
    "cortex.config.update": async (input) => {
      updates.push(input);
      const reply = replies.shift();
      return reply ?? { ok: true, binding, result: structuredClone(data[input.surface as string]) };
    },
    "cortex.config.catalog.refresh": async (input) => {
      refreshes.push(input);
      return { ok: true, binding, result: data.route_catalog };
    },
    "cortex.config.opencode.discover": async () => ({
      ok: true, binding, result: { found: true, version: "2.0.20", qualified: true, qualified_versions: ["2.x.x"], onboarded_connection_ids: [] },
    }),
    "cortex.config.opencode.onboard": async (input) => {
      onboards.push(input);
      const reply = onboardReplies.shift();
      return reply ?? { ok: true, binding, result: data.connections };
    },
  };
  const paseo = { workspaces: { list: async () => ({ entries: [{ id: "ws1", title: null, name: "ws1" }] }) } };
  const modules: Record<string, unknown> = {
    react: runtime.react,
    "react/jsx-runtime": {
      jsx: (type: unknown, props: Record<string, unknown>, key?: string) => ({ type, props, key }),
      jsxs: (type: unknown, props: Record<string, unknown>, key?: string) => ({ type, props, key }),
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
  return { runtime, data, reads, updates, onboards, onboardReplies, replies, refreshes, find, has, press, type, toggle, texts, section };
}

// --- tests ------------------------------------------------------------------------

test("no section offers a raw JSON entry field, and every control is labelled", async () => {
  const screen = await openScreen();
  await screen.press("Project (for project ceiling and project routing): ws1");
  const visits: Array<[string, () => Promise<void>]> = [
    ["User defaults", async () => {}],
    ["Models", async () => {}],
    ["Project ceiling", async () => {}],
    ["Routing", async () => {}],
    ["Profiles", async () => {}],
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
  assert.ok(screen.texts().some((text) => text.includes("profile was not updated")));
  assert.ok(screen.texts().some((text) => text.includes("legacy_flag")), "preserved key shown read-only");
});

test("user defaults: invalid input is flagged on the field and nothing is written", async () => {
  const screen = await openScreen();
  await screen.type("Route id", "codex-acp/gpt-5");
  assert.ok(screen.texts().some((text) => text.includes("default route") && text.includes("saved owner profiles")), "field help explains effect and profile scope");
  assert.ok(screen.texts().some((text) => text.includes("Example:") && text.includes("route")), "field help includes an example");
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
    rules: { model_policy_version: 2, pin: { "*": "opencode/anthropic/claude-sonnet-4-5:medium" }, deny: ["claude-agent-acp/opus"], providers: ["codex-acp", "opencode"],
      enabled: { "codex-acp": ["gpt-6-luna"], "claude-agent-acp": ["opus[1m]", "claude-sonnet-5"], opencode: ["anthropic/claude-sonnet-4-5"] } },
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
  assert.ok(screen.texts().some((text) => text.includes("Found version 2.0.20") && text.includes("supported versions: 2.x.x")));
  assert.equal((screen.find("Enable OpenCode v2", "Pressable").props.accessibilityState as { disabled: boolean }).disabled, true);
  await screen.toggle("I want to enable OpenCode v2 for this owner", true);
  await screen.press("Enable OpenCode v2");
  assert.deepEqual(screen.onboards, [{ expected_revision: REV("c"), explicit_opt_in: true }]);
});

test("connections: a successful OpenCode onboard shows its own next-step guidance, not the reloaded connections view", async () => {
  // Regression test: `read_connections` (what `surface.reload()` re-fetches after onboarding)
  // never carries an `opencode` key -- only the one-shot onboard response does. The panel used
  // to read `result.opencode` off the reloaded view, so the next_action banner never rendered.
  const screen = await openScreen();
  screen.onboardReplies.push({
    ok: true, binding, result: {
      ...screen.data.connections,
      opencode: { version: "2.0.19", next_action: "make_credential_dir_private", automatic_routing: "excluded_until_routing_opts_in" },
    },
  });
  await screen.section("Connections");
  await screen.press("Check for OpenCode");
  await screen.toggle("I want to enable OpenCode v2 for this owner", true);
  await screen.press("Enable OpenCode v2");
  assert.deepEqual(screen.onboards, [{ expected_revision: REV("c"), explicit_opt_in: true }]);
  assert.ok(screen.texts().some((text) => text.includes("Onboarded OpenCode 2.0.19") && text.includes("login directory is not private")),
    "expected the banner to show the onboard responses own next_action, not the reloaded connections view");
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

test("engines & models: every engine and model is a labelled switch with full identity and its current state", async () => {
  const screen = await openScreen();
  await screen.section("Engines & models");
  const elements = screen.runtime.elements();
  const switches = elements.filter((element) => element.type === "Switch");
  assert.deepEqual(switches.map((element) => element.props.accessibilityLabel).filter((label) => !String(label).includes(" — ")), [
    "Use Codex (codex-acp) in Cortex", "Automatic routing may use Codex (codex-acp)", "Use codex-acp/gpt-6-luna for automatic ranking",
    "Use Claude (claude-agent-acp) in Cortex", "Automatic routing may use Claude (claude-agent-acp)",
    "Use claude-agent-acp/claude-sonnet-5 for automatic ranking",
    "Use OpenCode v2 (opencode) in Cortex", "Automatic routing may use OpenCode v2 (opencode)",
    "Use opencode/anthropic/claude-sonnet-4-5 for automatic ranking", "Allow opencode/anthropic/claude-sonnet-4-5 in automatic routing",
  ], "only models switched on under Models are listed; opus[1m] is switched off");
  assert.ok(switches.some((element) => element.props.accessibilityLabel === "Review — opencode/anthropic/claude-sonnet-4-5"));
  for (const element of switches) {
    assert.equal(element.props.accessibilityRole, "switch");
    assert.equal(typeof (element.props.accessibilityState as { checked?: unknown }).checked, "boolean");
  }
  for (const element of elements.filter((item) => item.type === "Pressable")) {
    assert.ok(typeof element.props.accessibilityLabel === "string" && element.props.accessibilityLabel.length > 0, "unlabelled button");
    assert.ok(["button", "radio", "checkbox"].includes(String(element.props.accessibilityRole)));
  }
  assert.ok(!elements.some((element) => element.type === "TextInput" && element.props.multiline === true));
  assert.ok(!screen.has("Use claude-agent-acp/opus[1m] for automatic ranking"));
  assert.equal(screen.find("Automatic routing may use Claude (claude-agent-acp)", "Switch").props.value, false);
  assert.equal(screen.find("Use Claude (claude-agent-acp) in Cortex", "Switch").props.value, true, "automatic routing is not disabling");
  const texts = screen.texts().join("\n");
  assert.match(texts, /route xr_aaaaaaaaaaaa/);
  assert.doesNotMatch(texts, /route xr_bbbbbbbbbbbb/);
  assert.match(texts, /model selector: shown · automatic routing: no \(engine switched off for automatic routing\)/);
  assert.match(texts, /automatic routing: no \(OpenCode v2 routes need an opt-in\)/);
  assert.match(texts, /not enforced by this Cortex version/);
  assert.ok(!screen.texts().some((text) => /\bJSON\b/.test(text)));
});

test("engines & models: switches save exact routing entries to the chosen file, then re-read the effective state", async () => {
  const screen = await openScreen();
  await screen.section("Engines & models");
  const catalogReads = () => screen.reads.filter((read) => read.surface === "route_catalog").length;
  const before = catalogReads();
  await screen.toggle("Use Codex (codex-acp) in Cortex", false);
  assert.ok(!screen.has("Use codex-acp/gpt-6-luna for automatic ranking"), "the engine switch covers its models");
  await screen.toggle("Automatic routing may use Claude (claude-agent-acp)", true);
  await screen.toggle("Allow opencode/anthropic/claude-sonnet-4-5 in automatic routing", true);
  assert.ok(screen.texts().includes("Unsaved changes to the user routing file."));
  assert.equal(screen.updates.length, 0, "nothing is written before Save");
  await screen.press("Save engine and model switches to the user routing file");
  assert.deepEqual(screen.updates, [{
    surface: "routing", target: "user", expected_revision: REV("b"),
    rules: {
      model_policy_version: 2,
      pin: { "*": "opencode/anthropic/claude-sonnet-4-5:high" },
      deny: ["claude-agent-acp/opus", "codex-acp/*"],
      enabled: { "codex-acp": ["gpt-6-luna"], "claude-agent-acp": ["opus[1m]", "claude-sonnet-5"], opencode: ["anthropic/claude-sonnet-4-5"] },
      auto_allow: ["opencode/anthropic/claude-sonnet-4-5"],
      providers: ["codex-acp", "opencode", "claude-agent-acp"],
    },
  }]);
  assert.ok(catalogReads() > before, "the effective state is read again after saving");
  assert.ok(screen.texts().some((text) => text.startsWith("Applied to user routing")));
  assert.deepEqual(screen.onboards, [], "no switch registers OpenCode");
});

test("models: removing a broader entry is explicit; a conflict writes nothing and reload re-reads both views", async () => {
  const screen = await openScreen();
  await screen.section("Models");
  const opus = "Opus 5.5 (claude-agent-acp/opus[1m])";
  assert.equal(screen.find(opus, "Switch").props.value, false);
  assert.equal(screen.find(opus, "Switch").props.disabled, true, "a family entry is never cleared by one model's switch");
  await screen.press("Remove deny rule claude-agent-acp/opus (covers claude-agent-acp/opus[1m])");
  assert.equal(screen.find(opus, "Switch").props.value, true);
  screen.replies.push({
    ok: false, binding,
    error: { code: "revision_conflict", message: "routing (user) changed after it was read.", field: "expected_revision", currentRevision: REV("f") },
  });
  await screen.press("Save model switches to the user routing file");
  assert.deepEqual((screen.updates[0].rules as { deny?: string[] }).deny, []);
  assert.ok(screen.texts().some((text) => text.startsWith("Changed elsewhere since this was loaded")));
  const reads = (surface: string) => screen.reads.filter((read) => read.surface === surface).length;
  const [routingBefore, catalogBefore] = [reads("routing"), reads("route_catalog")];
  await screen.press("Reload latest (discards edits here)");
  assert.equal(reads("routing"), routingBefore + 1);
  assert.equal(reads("route_catalog"), catalogBefore + 1);
  assert.equal(screen.updates.length, 1, "a conflict is never retried automatically");
  assert.equal(screen.find(opus, "Switch").props.value, false, "edits here were discarded");
});

test("models: explicitly listed engines update exact enabled lists, and the lists refresh without a prompt", async () => {
  const screen = await openScreen();
  const catalog = screen.data.route_catalog;
  screen.data.route_catalog = {
    ...catalog,
    routes: [...(catalog.routes as unknown[]), catalogRoute("e", "opencode", "nvidia/nemotron-ultra", "Nemotron Ultra", CONN.opencode, {
      enabled: false, disabled: true, in_selector: false, default_auto_excluded: "optional_engine", automatic: "disabled", tier: null,
    })],
  };
  await screen.section("Models");
  const texts = screen.texts().join("\n");
  assert.match(texts, /Codex \(codex-acp\) — 1 of 1 on/);
  assert.match(texts, /OpenCode v2 \(opencode\) — 1 of 2 on/);
  assert.match(texts, /off until you switch them on here/);
  assert.equal(screen.find("Nemotron Ultra (opencode/nvidia/nemotron-ultra)", "Switch").props.value, false);
  await screen.type("Filter models", "nemotron");
  assert.ok(!screen.has("GPT-6 Luna (codex-acp/gpt-6-luna)"), "the filter hides other models");
  await screen.type("Filter models", "");
  await screen.toggle("Nemotron Ultra (opencode/nvidia/nemotron-ultra)", true);
  await screen.toggle("GPT-6 Luna (codex-acp/gpt-6-luna)", false);
  await screen.toggle("Claude Sonnet 4.5 (opencode/anthropic/claude-sonnet-4-5)", false);
  await screen.press("Save model switches to the user routing file");
  const rules = screen.updates[0].rules as { deny: string[]; enabled: Record<string, string[]> };
  assert.deepEqual(rules.deny, ["claude-agent-acp/opus"]);
  assert.deepEqual(rules.enabled, { "codex-acp": [], "claude-agent-acp": ["opus[1m]", "claude-sonnet-5"], opencode: ["nvidia/nemotron-ultra"] });

  screen.data.route_catalog = { ...screen.data.route_catalog, refresh: [
    { connection_id: CONN.codex[0], success: true, reason: null },
    { connection_id: CONN.claude[0], success: false, reason: "authorization_required" },
  ] };
  await screen.press("Read every engine's model list again");
  assert.deepEqual(screen.refreshes, [{}]);
  const after = screen.texts().join("\n");
  assert.match(after, /model list updated/);
  assert.match(after, /not updated \(not signed in to this engine\); the previous list is kept/);
  assert.equal(screen.updates.length, 1, "a refresh never writes routing");
});

test("engines & models: a shadowing project file is named; an unregistered OpenCode v2 is never activated from here", async () => {
  const screen = await openScreen();
  const routing = screen.data.routing;
  screen.data.routing = {
    ...routing, project_root: "<project>", precedence: ["project", "env", "user"], effective_source: "project",
    sources: [
      { source: "project", path: "<project>/.cortex/routing.json", writable: true, exists: true, revision: REV("p"), valid: true, rules: { deny: [] }, ignored_keys: [] },
      ...(routing.sources as unknown[]),
    ],
  };
  const catalog = screen.data.route_catalog;
  screen.data.route_catalog = {
    ...catalog,
    engines: (catalog.engines as Array<Record<string, unknown>>).map((engine) => (engine.engine === "opencode" ? { ...engine, connections: [], route_count: 0 } : engine)),
    routes: (catalog.routes as Array<Record<string, unknown>>).filter((route) => route.engine !== "opencode"),
  };
  await screen.press("Project (for project ceiling and project routing): ws1");
  await screen.section("Engines & models");
  const checked = (label: string) => (screen.find(label, "Pressable").props.accessibilityState as { checked: boolean }).checked;
  assert.equal(checked("Save switches to: Project routing file"), true, "the file in use is edited by default");
  await screen.press("Save switches to: User routing file");
  assert.equal(checked("Save switches to: User routing file"), true);
  assert.ok(screen.texts().some((text) => text.startsWith("The project routing file takes precedence")));
  assert.ok(screen.texts().some((text) => text.startsWith("Not registered. OpenCode v2 is only added by an explicit opt-in")));
  assert.ok(!screen.has("Allow opencode/anthropic/claude-sonnet-4-5 in automatic routing"));
  await screen.toggle("Use Claude (claude-agent-acp) in Cortex", false);
  await screen.press("Save engine and model switches to the user routing file");
  assert.deepEqual(screen.onboards, []);
  assert.equal(screen.updates.length, 1);
  assert.deepEqual([screen.updates[0].surface, screen.updates[0].target, screen.updates[0].workspaceId], ["routing", "user", "ws1"]);
  assert.deepEqual((screen.updates[0].rules as { deny?: string[] }).deny, ["claude-agent-acp/opus", "claude-agent-acp/*"]);
});

test("purpose choices require explicit auto selection and save OpenCode preferences", async () => {
  const screen = await openScreen();
  await screen.section("Engines & models");
  const purpose = "Review — opencode/anthropic/claude-sonnet-4-5";
  assert.equal(screen.find(purpose, "Switch").props.disabled, true);
  await screen.toggle("Use opencode/anthropic/claude-sonnet-4-5 for automatic ranking", true);
  assert.equal(screen.find(purpose, "Switch").props.disabled, false);
  await screen.toggle(purpose, true);
  await screen.press("Save engine and model switches to the user routing file");
  const rules = screen.updates[0].rules as any;
  assert.deepEqual(rules.models.opencode.auto, ["anthropic/claude-sonnet-4-5"]);
  assert.deepEqual(rules.models.opencode.policy["anthropic/claude-sonnet-4-5"].preferred_for, ["review"]);
});

test("engines & models: a model can be placed in several tiers and back to unplaced", async () => {
  const screen = await openScreen();
  await screen.section("Engines & models");
  const checked = (label: string) => (screen.find(label, "Pressable").props.accessibilityState as { checked: boolean }).checked;
  const tier = (name: string) => `Automatic tiers for codex-acp/gpt-6-luna: ${name}`;
  assert.equal(checked(tier("light")), true, "the tier Cortex uses now is shown");
  await screen.press(tier("standard"));
  assert.deepEqual([checked(tier("light")), checked(tier("standard")), checked(tier("deep"))], [true, true, false]);
  await screen.press("Save engine and model switches to the user routing file");
  assert.deepEqual((screen.updates[0].rules as any).models["codex-acp"].tiers, { "gpt-6-luna": ["light", "standard"] });
  const fresh = await openScreen();
  await fresh.section("Engines & models");
  await fresh.press(tier("standard"));
  await fresh.press(tier("light"));
  await fresh.press(tier("standard"));
  await fresh.press("Save engine and model switches to the user routing file");
  assert.equal((fresh.updates[0].rules as any).models?.["codex-acp"]?.tiers, undefined, "no tier left: no placement");
});

test("profiles show modified sections, confirm revert, and retain revision conflicts", async () => {
  const screen = await openScreen();
  assert.ok(screen.texts().some((text) => text.includes("User routing (providers)")));
  await screen.section("Profiles");
  await screen.press("Saved profile: mix");
  await screen.type("New profile name", "new-profile");
  await screen.type("Rename profile to", "renamed");
  assert.equal(screen.find("New profile name", "TextInput").props.value, "new-profile");
  screen.replies.push({ ok: false, binding, error: { code: "revision_conflict", message: "Profile changed", field: "expected_revision", currentRevision: REV("s") } });
  await screen.press("Activate profile");
  assert.equal(screen.updates.length, 0);
  await screen.press("Discard changes and switch");
  assert.deepEqual(screen.updates[0], {
    surface: "routing_profiles", action: "activate", expected_revision: REV("q"),
    expected_revisions: screen.data.routing_profiles.current_revisions, name: "mix", discard_modifications: true,
  });
  assert.ok(screen.has("Reload latest (discards edits here)"));
  await screen.press("Reload latest (discards edits here)");
  assert.equal(screen.find("Current profile: mix", "Pressable").props.accessibilityState.checked, true);
});

test("routing and ceilings say what each setting actually does", async () => {
  const screen = await openScreen();
  await screen.section("Routing");
  const texts = screen.texts();
  assert.ok(texts.some((text) => text.startsWith("Automatic worker routing, failover and pins use only these engines")));
  assert.ok(!texts.some((text) => /built-in deny list|engine preference/i.test(text)));
  await screen.press("Project (for project ceiling and project routing): ws1");
  await screen.section("Project ceiling");
  await screen.toggle("Restrict allowed connections", true);
  assert.ok(screen.texts().some((text) => text.startsWith("Recorded and checked for narrowing only")));
});


test("profiles: explicit save and create capture all applied sections, independently of rename", async () => {
  const screen = await openScreen();
  const expected = screen.data.routing_profiles.current_revisions;
  await screen.press("Save to current profile");
  assert.deepEqual(screen.updates[0], { surface: "routing_profiles", action: "save", name: "mix", expected_revision: REV("q"), expected_revisions: expected });
  await screen.type("New profile name", "fresh");
  await screen.type("New profile description", "New snapshot");
  await screen.press("Save as new profile");
  assert.deepEqual(screen.updates[1], { surface: "routing_profiles", action: "create", name: "fresh", description: "New snapshot", expected_revision: REV("q"), expected_revisions: expected });
  await screen.section("Profiles");
  await screen.press("Saved profile: mix");
  await screen.type("Rename profile to", "renamed");
  await screen.type("Profile description", "");
  await screen.press("Save profile name and description");
  assert.deepEqual(screen.updates[2], { surface: "routing_profiles", action: "update", name: "mix", rename_to: "renamed", description: "", expected_revision: REV("q") });
  assert.equal(screen.find("New profile name", "TextInput").props.value, "fresh");
});

test("profiles: save then switch uses the save result's revisions, cancel writes nothing", async () => {
  const data = fixtures();
  const other = { ...(data.routing_profiles.profiles as any[])[0], name: "other" };
  (data.routing_profiles.profiles as any[]).push(other);
  const screen = await openScreen(data);
  await screen.press("Current profile: other");
  await screen.press("Cancel switch");
  assert.equal(screen.updates.length, 0);
  await screen.press("Current profile: other");
  const saved = { ...data.routing_profiles, state: "in_sync", revision: REV("s"), current_revisions: { routing: REV("t"), user_defaults: REV("u"), mcp_servers: REV("v"), connections: REV("w") } };
  screen.replies.push({ ok: true, binding, result: saved });
  await screen.press("Save changes and switch");
  assert.equal(screen.updates.length, 2);
  assert.equal(screen.updates[0].action, "save");
  assert.deepEqual(screen.updates[1], { surface: "routing_profiles", action: "activate", name: "other", expected_revision: REV("s"), expected_revisions: saved.current_revisions });
});

test("profiles: failed save before switching never activates", async () => {
  const screen = await openScreen();
  await screen.press("Revert to saved");
  screen.replies.push({ ok: false, binding, error: { code: "revision_conflict", message: "Changed" } });
  await screen.press("Save changes and switch");
  assert.deepEqual(screen.updates.map((item) => item.action), ["save"]);
  assert.ok(screen.has("Reload latest (discards edits here)"));
});

test("profiles: section saves refresh profile differences while project settings stay independent", async () => {
  const screen = await openScreen();
  const reads = () => screen.reads.filter((item) => item.surface === "routing_profiles").length;
  const before = reads();
  screen.data.routing_profiles.modified_sections = ["user_defaults"];
  screen.data.routing_profiles.differences = [{ section: "user_defaults", key: "language" }];
  await screen.press("Language: ru");
  await screen.press("Save");
  assert.ok(reads() > before);
  assert.ok(screen.texts().some((text) => text.includes("User defaults (language)")));
  assert.equal(screen.updates.some((item) => item.surface === "routing_profiles"), false);
  await screen.section("MCP servers");
  const beforeMcp = reads();
  await screen.toggle("search enabled", false);
  assert.ok(reads() > beforeMcp);
  await screen.section("Connections");
  const beforeConnections = reads();
  await screen.type("Max concurrent sessions for Codex (1-8)", "2");
  await screen.press("Apply max concurrent sessions for Codex");
  assert.ok(reads() > beforeConnections);
});

for (const section of ["User defaults", "Routing", "Models", "Engines & models", "MCP servers", "Connections", "Project ceiling"]) {
  test(`profiles: activation reloads the mounted ${section} form and all later sections`, async () => {
    const screen = await openScreen();
    await screen.press("Project (for project ceiling and project routing): ws1");
    await screen.section(section);
    const before = screen.reads.length;
    screen.data.user_defaults.defaults = { ...(screen.data.user_defaults.defaults as object), language: "ru" };
    await screen.press("Revert to saved");
    await screen.press("Discard changes and switch");
    const surface = section === "User defaults" ? "user_defaults" : section === "MCP servers" ? "mcp_servers"
      : section === "Connections" ? "connections" : section === "Project ceiling" ? "project_ceiling" : "routing";
    assert.ok(screen.reads.slice(before).some((item) => item.surface === surface), `${surface} re-read after activation`);
    await screen.section("User defaults");
    assert.equal(screen.find("Language: ru", "Pressable").props.accessibilityState.checked, true);
  });
}

test("profiles: none, in-sync, partial, interrupted and unavailable have observable controls", async () => {
  for (const state of ["none", "in_sync", "interrupted", "unavailable"]) {
    const data = fixtures();
    data.routing_profiles.state = state;
    if (state === "none") data.routing_profiles.active = null;
    if (state === "interrupted") data.routing_profiles.pending = { operation: "activate", profile: "mix", at: "now" };
    if (state === "unavailable") {
      data.routing_profiles.current_revisions = { ...(data.routing_profiles.current_revisions as object), mcp_servers: null };
      data.routing_profiles.problems = [{ section: "mcp_servers", code: "invalid", message: "Invalid policy" }];
    }
    (data.routing_profiles.profiles as any[])[0].sections = ["routing"];
    const screen = await openScreen(data);
    assert.ok(screen.texts().includes(state));
    if (state !== "none") assert.ok(screen.texts().some((text) => text.includes("stores only User routing")));
    if (state === "none" || state === "unavailable") assert.equal(screen.find("Save to current profile", "Pressable").props.disabled, true);
    if (state === "unavailable") assert.ok(screen.texts().some((text) => text.includes("MCP servers: Invalid policy")));
    if (state === "interrupted") {
      assert.ok(screen.has("Re-activate mix"));
      await screen.press("Save current settings to mix");
      assert.equal(screen.updates[0].action, "save");
      await screen.press("Detach");
      assert.equal(screen.updates[1].action, "deactivate");
    }
    if (state === "none") {
      await screen.type("New profile name", "new");
      await screen.press("Save as new profile");
      assert.equal(screen.updates[0].action, "create");
    }
  }
});


test("profiles: activation failure refreshes forms and reload exposes the pending recovery state", async () => {
  const screen = await openScreen();
  await screen.press("Language: ru");
  await screen.press("Revert to saved");
  screen.data.routing_profiles.state = "interrupted";
  screen.data.routing_profiles.pending = { operation: "activate", profile: "mix", at: "now" };
  screen.replies.push({ ok: false, binding, error: { code: "activation_failed", message: "Could not finish activation" } });
  const before = screen.reads.length;
  await screen.press("Discard changes and switch");
  assert.ok(screen.reads.slice(before).some((item) => item.surface === "user_defaults"));
  assert.ok(screen.texts().some((text) => text.includes("Could not finish activation")));
  assert.equal(screen.find("Language: en", "Pressable").props.accessibilityState.checked, true);
  await screen.press("Reload profile state");
  assert.ok(screen.has("Re-activate mix"));
  assert.ok(screen.texts().some((text) => text.includes('Activation of "mix" did not finish')));
});

test("profiles: detach does not discard unapplied form edits", async () => {
  const screen = await openScreen();
  await screen.press("Language: ru");
  await screen.press("Current profile: No profile");
  assert.equal(screen.updates[0].action, "deactivate");
  assert.equal(screen.find("Language: ru", "Pressable").props.accessibilityState.checked, true);
});


for (const kind of ["inactive", "selected", "last"] as const) test(`delete ${kind} profile confirms exact name, cancels without mutation and keeps working settings`, async () => {
  const data = fixtures();
  if (kind !== "last") data.routing_profiles.profiles.push({ ...data.routing_profiles.profiles[0], name: "spare" });
  const name = kind === "inactive" ? "spare" : "mix";
  const before = structuredClone(data.user_defaults);
  const screen = await openScreen(data);
  await screen.section("Profiles");
  await screen.press(`Saved profile: ${name}`);
  await screen.press("Delete profile");
  assert.ok(screen.texts().some((text) => text.includes(`Delete profile "${name}"?`) && text.includes("Current working settings stay unchanged")));
  await screen.press("Cancel delete");
  assert.equal(screen.updates.length, 0);
  await screen.press("Delete profile");
  const result = { ...data.routing_profiles, revision: REV("e"), profiles: data.routing_profiles.profiles.filter((profile) => profile.name !== name),
    active: kind === "inactive" ? "mix" : null, state: kind === "inactive" ? "modified" : "none", applied: null };
  screen.replies.push({ ok: true, binding, result });
  await screen.press("Confirm delete profile");
  assert.deepEqual(screen.updates, [{ surface: "routing_profiles", action: "delete", name, expected_revision: REV("q") }]);
  assert.deepEqual(data.user_defaults, before);
  assert.ok(!screen.has(`Saved profile: ${name}`));
  if (kind === "last") assert.ok(screen.texts().some((text) => text.startsWith("No saved profiles.")));
  assert.equal(screen.updates.some((input) => input.action === "activate"), false);
});

for (const code of ["revision_conflict", "profile_delete_failed"]) test(`delete ${code} preserves state and requires reload before another submission`, async () => {
  const screen = await openScreen();
  await screen.section("Profiles");
  await screen.press("Saved profile: mix");
  await screen.press("Delete profile");
  screen.replies.push({ ok: false, binding, error: { code, message: "Nothing was deleted; reload profile state.", field: "expected_revision" } });
  await screen.press("Confirm delete profile");
  assert.equal(screen.updates.length, 1);
  assert.ok(screen.has("Saved profile: mix"));
  assert.equal(screen.find("Delete profile", "Pressable").props.disabled, true);
  await screen.press("Reload profile state");
  assert.equal(screen.find("Delete profile", "Pressable").props.disabled, false);
  assert.equal(screen.updates.length, 1);
});

test("profile deletion gates duplicate submissions while pending", async () => {
  const screen = await openScreen();
  await screen.section("Profiles"); await screen.press("Saved profile: mix"); await screen.press("Delete profile");
  let finish!: (value: unknown) => void;
  screen.replies.push(new Promise((resolve) => { finish = resolve; }));
  const press = screen.find("Confirm delete profile", "Pressable").props.onPress as () => void;
  press(); press(); await screen.runtime.settle();
  assert.equal(screen.updates.length, 1);
  assert.equal(screen.find("Confirm delete profile", "Pressable").props.disabled, true);
  finish({ ok: false, binding, error: { code: "profile_delete_failed", message: "Reload" } });
  await screen.runtime.settle();
});

test("exact model policy controls edit arbitrary withdrawn IDs and preserve unsupported attributes", async () => {
  const data = fixtures();
  const rules = data.routing.sources.find((source) => source.source === "user")!.rules as any;
  rules.models = { "codex-acp": { auto: ["ARBITRARY@9"], tiers: { "ARBITRARY@9": ["light", "standard"] }, policy: { "ARBITRARY@9": {
    effort_floor: "medium", max_effort: "high", effort_ladder: ["medium", "high"], also_standard_for: ["review"], preferred_for: ["review"], request_names: ["Owner choice"], future: { retain: true },
  } } } };
  const screen = await openScreen(data); await screen.section("Routing");
  await screen.press("effort_floor for codex-acp/ARBITRARY@9: high");
  await screen.press("Save user routing");
  const saved = screen.updates[0].rules as any;
  assert.deepEqual(saved.models["codex-acp"].policy["ARBITRARY@9"], { ...rules.models["codex-acp"].policy["ARBITRARY@9"], effort_floor: "high" });
  assert.equal(saved.model_policy_version, 2);
  assert.ok(screen.texts().some((text) => text.includes("future is not supported here; kept unchanged")));
});

test("portable ambiguity and missing policy diagnostics survive rendering without substituting a model", async () => {
  const data = fixtures() as any;
  data.user_defaults.defaults = { coordinator: { engine: "codex-acp", model: "ARBITRARY@9", parameters: { mode: "agent" } } };
  data.user_defaults.coordinator_resolution = { state: "unavailable", error: { code: "portable_coordinator_ambiguous", message: "Several exact matches; choose explicitly." } };
  const screen = await openScreen(data);
  assert.ok(screen.texts().some((text) => text.includes("portable_coordinator_ambiguous")));
  assert.equal(screen.find("Coordinator model (exact id)", "TextInput").props.value, "ARBITRARY@9");
  await screen.section("Routing");
  assert.ok(screen.texts().some((text) => text.includes("Model policy version")));
  delete data.routing.sources.find((source: any) => source.source === "user").rules.model_policy_version;
  const legacy = await openScreen(data);
  await legacy.section("Routing");
  assert.ok(legacy.texts().some((text) => text.includes("missing — explicit migration required")));
});


test("portable save re-reads resolution, keeps exact parameters and clearing constrained policy lists writes neutral null", async () => {
  const data = fixtures() as any;
  data.user_defaults.defaults = { coordinator: { engine: "codex-acp", model: "OLD@9", parameters: { mode: "agent" } } };
  const screen = await openScreen(data);
  await screen.type("Coordinator model (exact id)", "RENAMED@10");
  data.user_defaults.defaults.coordinator.model = "RENAMED@10";
  data.user_defaults.coordinator_resolution = { state: "unavailable", error: { code: "portable_coordinator_unavailable", message: "No exact observed match" } };
  const before = screen.reads.filter((input) => input.surface === "user_defaults").length;
  await screen.press("Save");
  assert.deepEqual(screen.updates[0], { surface: "user_defaults", expected_revision: REV("a"), defaults: { coordinator: { engine: "codex-acp", model: "RENAMED@10", parameters: { mode: "agent" } } } });
  assert.equal(screen.reads.filter((input) => input.surface === "user_defaults").length, before + 1);
  assert.ok(screen.texts().some((text) => text.includes("portable_coordinator_unavailable")));
  const rules = data.routing.sources.find((source: any) => source.source === "user").rules;
  rules.models = { "codex-acp": { auto: [], tiers: {}, policy: { "NEUTRAL@9": { effort_ladder: ["medium"], request_names: ["owner-choice"] } } } };
  await screen.section("Routing");
  await screen.press("Remove effort_ladder for codex-acp/NEUTRAL@9 1");
  await screen.press("Remove request_names for codex-acp/NEUTRAL@9 1");
  await screen.press("Save user routing");
  assert.deepEqual((screen.updates[1].rules as any).models["codex-acp"].policy["NEUTRAL@9"], { effort_ladder: null, request_names: null });
});


test("quota typed controls edit exact labels and scopes and retain unrelated data", async () => {
  const data = fixtures();
  const rules = data.routing.sources.find((source) => source.source === "user")!.rules as any;
  rules.quota_windows = { "codex-acp": [{ window: "Owner exact@7", models: ["WITHDRAWN@9"] }] };
  const screen = await openScreen(data); await screen.section("Routing");
  await screen.type("codex-acp window 1 exact label", "Renamed / Label@8");
  await screen.press("codex-acp window 1 scope: Shared / account");
  await screen.press("Save user routing");
  const saved = screen.updates[0].rules as any;
  assert.deepEqual(saved.quota_windows, { "codex-acp": [{ window: "Renamed / Label@8", models: null }] });
  assert.deepEqual(saved.models, rules.models);
  assert.ok(screen.texts().some((text) => text.includes("Unmapped labels remain conservatively shared")));
});

test("unknown quota rule shape remains read-only with no destructive save", async () => {
  const data = fixtures();
  const rules = data.routing.sources.find((source) => source.source === "user")!.rules as any;
  rules.quota_windows = { "codex-acp": [{ window: "future", models: null, future: true }] };
  const screen = await openScreen(data); await screen.section("Routing");
  assert.ok(screen.texts().some((text) => text.includes("cannot show without changing it")));
  assert.equal(screen.updates.length, 0);
  assert.equal(rules.quota_windows["codex-acp"][0].future, true);
});
