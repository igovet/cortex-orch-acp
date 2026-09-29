import assert from "node:assert/strict";
import test from "node:test";
import {
  buildCeiling,
  buildConnectionMaxSessions,
  buildConnectionRename,
  buildMcpUpsert,
  buildOnboard,
  buildProjectCeilingUpdate,
  buildRoutingRules,
  buildRoutingUpdate,
  buildUserDefaultsUpdate,
  ceilingToForm,
  composePin,
  connectionAction,
  describeConfigError,
  errorAt,
  errorFor,
  fieldErrorsFromConfigError,
  isRevisionConflict,
  mcpServerToForm,
  mcpUpdate,
  newMcpServerForm,
  parsePin,
  parseWholeNumber,
  preservedDefaultKeys,
  projectCeilingRemoval,
  projectCeilingToForm,
  routingRulesToForm,
  routingTarget,
  routingVocabulary,
  userDefaultsToForm,
  type CeilingView,
  type ConnectionsView,
  type McpServerView,
  type ProjectCeilingView,
  type RoutingView,
  type UserDefaultsView,
} from "./settings-model.ts";

const REV = (letter: string) => `sha256:${letter.repeat(64)}`;

const OPEN_CEILING: CeilingView = {
  allowed_routes: null, allowed_connections: null, allowed_data_recipients: null, permission_level: null,
  limits: {}, spend_limit: null, spend_limit_enforceable: false,
  fallback_policy: { allowed_target_routes: null, allowed_failure_classes: null, budget: null, data_recipient_consent: false },
};

const RICH_CEILING: CeilingView = {
  allowed_routes: ["codex-acp/gpt-5", "opencode/anthropic/claude-sonnet-4-5"], allowed_connections: [], allowed_data_recipients: null,
  permission_level: "STANDARD", limits: { tokens: 100000, attempts: 3 }, spend_limit: 50, spend_limit_enforceable: true,
  fallback_policy: { allowed_target_routes: ["codex-acp/gpt-5"], allowed_failure_classes: ["engine_failure"], budget: 2, data_recipient_consent: true },
};

function userDefaults(overrides: Partial<UserDefaultsView> = {}): UserDefaultsView {
  return {
    surface: "user_defaults", path: "~/.config/cortex/defaults.json", revision: REV("a"), exists: true,
    defaults: {
      route_id: "xr_0123456789ab", route_connection_id: "cn_0123456789ab", route_model: "gpt-5", language: "en",
      parameters: { tone: "brief", depth: 3 }, legacy_flag: true,
    },
    editable_keys: ["route_id", "route_connection_id", "route_model", "parameters", "language"],
    preserved_keys: ["legacy_flag"], languages: ["en", "ru"], ceiling: OPEN_CEILING, ceiling_set: false,
    ...overrides,
  };
}

function routing(rules: Record<string, unknown> | undefined, extra: Partial<RoutingView["sources"][number]> = {}): RoutingView {
  return {
    surface: "routing", project_root: "<project>", precedence: ["project", "env", "user"],
    sources: [
      { source: "project", path: "<project>/.cortex/routing.json", writable: true, exists: false, revision: "absent" },
      { source: "env", path: "(unset)", writable: false, exists: false, revision: null },
      { source: "user", path: "~/.config/cortex/routing.json", writable: true, exists: true, revision: REV("b"), valid: true, rules, ignored_keys: ["comment"], ...extra },
    ],
    effective_source: "user", effective_valid: true, effective_rules: rules ?? null,
    builtin: {
      supported_engines: ["codex-acp", "claude-agent-acp"], optional_engines: ["opencode"], default_auto_deny_engines: ["opencode"],
      default_auto_deny_families: [], tiers: ["light", "standard", "heavy"], efforts: ["low", "medium", "high"],
      work_kinds: ["code", "review"], default_user_deny: [],
    },
  };
}

const CONNECTIONS: ConnectionsView = {
  surface: "connections", revision: REV("c"), exists: true, connections: [], engines: [],
  operations: ["rename", "set_max_concurrent_sessions", "validate", "remove"], max_concurrent_sessions_limit: 8,
};

// --- errors -------------------------------------------------------------------

test("isRevisionConflict matches only the revision_conflict code", () => {
  assert.equal(isRevisionConflict({ code: "revision_conflict" }), true);
  assert.equal(isRevisionConflict({ code: "invalid" }), false);
});

test("describeConfigError folds field and per-item problems into one line", () => {
  assert.equal(describeConfigError({ code: "invalid", message: "Bad value", field: null }), "Bad value");
  assert.equal(describeConfigError({ code: "invalid", message: "Bad value", field: "route_id" }), "Bad value — (field: route_id)");
  assert.equal(
    describeConfigError({ code: "invalid", message: "Refused", field: null, errors: [{ field: "limits.tokens", code: "negative", message: "must be >= 0" }] }),
    "Refused — limits.tokens: must be >= 0",
  );
});

test("API field errors map onto form keys, per entry and per control", () => {
  const errors = fieldErrorsFromConfigError({
    code: "invalid_configuration", message: "Refused", field: null,
    errors: [
      { field: "operations[0].server.env.API_KEY", code: "unknown_env_value", message: "has no stored value to keep" },
      { field: "operations[0].name", code: "invalid_server_name", message: "bad name" },
    ],
  }, ["operations[0].server.", "operations[0]."]);
  assert.deepEqual(errors, { "env.API_KEY": "has no stored value to keep", name: "bad name" });
  assert.equal(errorAt(errors, "env"), "has no stored value to keep");
  assert.equal(errorFor({ "rules.pin[1]": "row", "rules.pin.worker": "named" }, "rules.pin"), "row; named");
  assert.equal(errorAt({ "rules.pin[1]": "row", "rules.pin.worker": "named" }, "rules.pin"), "named");
  assert.deepEqual(fieldErrorsFromConfigError({ code: "x", message: "only", field: "name" }), { name: "only" });
});

test("whole-number fields accept blank as unset and enforce bounds", () => {
  assert.deepEqual(parseWholeNumber(" "), { ok: true, value: null });
  assert.deepEqual(parseWholeNumber("12"), { ok: true, value: 12 });
  assert.equal(parseWholeNumber("-1").ok, false);
  assert.equal(parseWholeNumber("1.5").ok, false);
  assert.equal(parseWholeNumber("0", { min: 1 }).ok, false);
  assert.equal(parseWholeNumber("9", { max: 8 }).ok, false);
});

// --- ceilings -------------------------------------------------------------------

test("a ceiling round-trips through its typed form unchanged", () => {
  for (const ceiling of [OPEN_CEILING, RICH_CEILING]) {
    const built = buildCeiling(ceilingToForm(ceiling));
    assert.equal(built.ok, true);
    if (built.ok) assert.deepEqual(built.value, ceiling);
  }
});

test("ceiling fields are validated with field-level errors", () => {
  const form = ceilingToForm(RICH_CEILING);
  form.spend_limit_enforceable = false;
  form.limits.tokens = "lots";
  form.fallback_allowed_failure_classes = ["refusal", ""];
  form.allowed_routes = ["a", "a"];
  form.permission_level = "ROOT";
  const built = buildCeiling(form);
  assert.equal(built.ok, false);
  if (built.ok) return;
  assert.match(built.errors["ceiling.spend_limit"], /enforceable/);
  assert.match(built.errors["ceiling.limits.tokens"], /whole number/);
  assert.match(built.errors["ceiling.fallback_policy.allowed_failure_classes[0]"], /never/);
  assert.match(built.errors["ceiling.fallback_policy.allowed_failure_classes[1]"], /Enter a value/);
  assert.match(built.errors["ceiling.allowed_routes[1]"], /Duplicate/);
  assert.match(built.errors["ceiling.permission_level"], /READ_ONLY/);
});

// --- user defaults ----------------------------------------------------------------

test("user defaults: an untouched form sends nothing", () => {
  const view = userDefaults();
  assert.deepEqual(buildUserDefaultsUpdate(view, userDefaultsToForm(view)), { ok: true, value: null });
  assert.deepEqual(preservedDefaultKeys(view), ["legacy_flag"]);
});

test("user defaults: only the changed field is patched, with the read revision", () => {
  const view = userDefaults();
  const form = { ...userDefaultsToForm(view), language: "ru" };
  assert.deepEqual(buildUserDefaultsUpdate(view, form), {
    ok: true, value: { surface: "user_defaults", expected_revision: REV("a"), defaults: { language: "ru" } },
  });
  // A stored non-string parameter is only rewritten once the owner edits it.
  const automatic = { ...userDefaultsToForm(view), language: "" };
  assert.deepEqual(buildUserDefaultsUpdate(view, automatic), {
    ok: true, value: { surface: "user_defaults", expected_revision: REV("a"), defaults: { language: null } },
  });
});

test("user defaults: the route is set or cleared as a validated group", () => {
  const view = userDefaults();
  const partial = { ...userDefaultsToForm(view), route_model: "" };
  const refused = buildUserDefaultsUpdate(view, partial);
  assert.equal(refused.ok, false);
  if (!refused.ok) assert.match(refused.errors["defaults.route_model"], /together/);

  const cleared = { ...userDefaultsToForm(view), route_id: "", route_connection_id: "", route_model: "" };
  assert.deepEqual(buildUserDefaultsUpdate(view, cleared), {
    ok: true,
    value: { surface: "user_defaults", expected_revision: REV("a"), defaults: { route_id: null, route_connection_id: null, route_model: null } },
  });

  const bad = { ...userDefaultsToForm(view), route_id: "codex-acp/gpt-5" };
  const badBuilt = buildUserDefaultsUpdate(view, bad);
  assert.equal(badBuilt.ok, false);
  if (!badBuilt.ok) assert.match(badBuilt.errors["defaults.route_id"], /xr_/);

  const changed = { ...userDefaultsToForm(view), route_model: "anthropic/claude-sonnet-4-5" };
  assert.deepEqual(buildUserDefaultsUpdate(view, changed), {
    ok: true,
    value: {
      surface: "user_defaults", expected_revision: REV("a"),
      defaults: { route_id: "xr_0123456789ab", route_connection_id: "cn_0123456789ab", route_model: "anthropic/claude-sonnet-4-5" },
    },
  });
});

test("user defaults: parameters are a validated map; removing every row deletes it", () => {
  const view = userDefaults();
  const edited = { ...userDefaultsToForm(view), parameters: [{ key: "tone", value: "brief" }, { key: "depth", value: "4" }] };
  assert.deepEqual(buildUserDefaultsUpdate(view, edited), {
    ok: true, value: { surface: "user_defaults", expected_revision: REV("a"), defaults: { parameters: { tone: "brief", depth: "4" } } },
  });
  const removed = { ...userDefaultsToForm(view), parameters: [] };
  assert.deepEqual(buildUserDefaultsUpdate(view, removed), {
    ok: true, value: { surface: "user_defaults", expected_revision: REV("a"), defaults: { parameters: null } },
  });
  const invalid = { ...userDefaultsToForm(view), parameters: [{ key: "Tone", value: "" }, { key: "Tone", value: "x" }] };
  const built = buildUserDefaultsUpdate(view, invalid);
  assert.equal(built.ok, false);
  if (!built.ok) {
    assert.match(built.errors["defaults.parameters[0]"], /lowercase/);
    assert.match(built.errors["defaults.parameters[0]"], /Enter a value/);
  }
});

test("user defaults: the owner ceiling is set, kept, changed or cleared explicitly", () => {
  const unset = userDefaults();
  const enable = { ...userDefaultsToForm(unset), ceiling_set: true };
  assert.deepEqual(buildUserDefaultsUpdate(unset, enable), {
    ok: true, value: { surface: "user_defaults", expected_revision: REV("a"), ceiling: OPEN_CEILING },
  });

  const set = userDefaults({ ceiling: RICH_CEILING, ceiling_set: true });
  assert.deepEqual(buildUserDefaultsUpdate(set, userDefaultsToForm(set)), { ok: true, value: null });
  const narrowed = userDefaultsToForm(set);
  narrowed.ceiling = { ...narrowed.ceiling, permission_level: "READ_ONLY" };
  const built = buildUserDefaultsUpdate(set, narrowed);
  assert.equal(built.ok, true);
  if (built.ok) assert.deepEqual(built.value?.ceiling, { ...RICH_CEILING, permission_level: "READ_ONLY" });
  assert.deepEqual(buildUserDefaultsUpdate(set, { ...userDefaultsToForm(set), ceiling_set: false }), {
    ok: true, value: { surface: "user_defaults", expected_revision: REV("a"), ceiling: null },
  });
});

// --- project ceiling --------------------------------------------------------------

test("project ceiling: starts from the owner ceiling and writes with the workspace and revision", () => {
  const view: ProjectCeilingView = {
    surface: "project_ceiling", project_root: "<project>", revision: "absent", exists: false, user_ceiling: RICH_CEILING,
    ceiling: null, project_defaults: {}, effective_ceiling: RICH_CEILING, narrows_user_ceiling: true, violation: null, updated_at: null,
  };
  const form = projectCeilingToForm(view);
  assert.deepEqual(buildProjectCeilingUpdate(view, "ws1", form), {
    ok: true, value: { surface: "project_ceiling", workspaceId: "ws1", expected_revision: "absent", ceiling: RICH_CEILING },
  });
  assert.deepEqual(projectCeilingRemoval({ ...view, revision: REV("d") }, "ws1"), {
    surface: "project_ceiling", workspaceId: "ws1", expected_revision: REV("d"), ceiling: null,
  });
});

// --- routing ----------------------------------------------------------------------

test("pins split into engine, full model id and per-model effort, and compose back exactly", () => {
  for (const value of ["opencode/anthropic/claude-sonnet-4-5:high", "codex-acp/gpt-5", "claude-agent-acp/sonnet:low", "bare-model"]) {
    assert.equal(composePin(parsePin("*", value)), value);
  }
  assert.deepEqual(parsePin("worker", "opencode/anthropic/claude-sonnet-4-5:high"), {
    role: "worker", engine: "opencode", model: "anthropic/claude-sonnet-4-5", effort: "high",
  });
});

test("routing rules round-trip through the typed form, including absent keys", () => {
  const rules = {
    pin: { "*": "codex-acp/gpt-5:medium", reviewer: "opencode/anthropic/claude-sonnet-4-5:high" },
    deny: ["claude-agent-acp/opus"],
    auto_allow: ["opencode/anthropic/claude-sonnet-4-5"],
    tiers: { "gpt-5": "standard" },
    providers: ["codex-acp", "opencode"],
    engine_affinity: { code: "codex-acp", review: null },
  };
  const view = routing(rules);
  const parsed = routingRulesToForm(rules);
  assert.equal(parsed.ok, true);
  if (!parsed.ok) return;
  const built = buildRoutingRules(parsed.form, routingVocabulary(view));
  assert.deepEqual(built, { ok: true, value: rules });

  const sparse = routingRulesToForm({ deny: [] });
  assert.equal(sparse.ok, true);
  if (sparse.ok) assert.deepEqual(buildRoutingRules(sparse.form, routingVocabulary(view)), { ok: true, value: { deny: [] } });
});

test("routing writes target one file with that file's own revision", () => {
  const view = routing({ pin: { "*": "opencode/anthropic/claude-sonnet-4-5:high" } });
  const state = routingTarget(view, "user");
  assert.equal(state.editable, true);
  assert.deepEqual(state.ignoredKeys, ["comment"]);
  const form = { ...state.form!, pin: [{ role: "*", engine: "opencode", model: "anthropic/claude-sonnet-4-5", effort: "medium" }] };
  assert.deepEqual(buildRoutingUpdate(view, "user", "ws1", form), {
    ok: true,
    value: { surface: "routing", target: "user", workspaceId: "ws1", expected_revision: REV("b"), rules: { pin: { "*": "opencode/anthropic/claude-sonnet-4-5:medium" } } },
  });
  const project = routingTarget(view, "project");
  assert.equal(project.editable, true);
  assert.equal(project.revision, "absent");
  assert.deepEqual(project.form, { pin: null, deny: null, auto_allow: null, tiers: null, providers: null, engine_affinity: null });
});

test("routing validation mirrors the API's OpenCode and vocabulary rules", () => {
  const view = routing({});
  const vocabulary = routingVocabulary(view);
  const built = buildRoutingRules({
    pin: [
      { role: "*", engine: "opencode", model: "sonnet", effort: "" },
      { role: "*", engine: "codex-acp", model: "gpt-5", effort: "extreme" },
    ],
    deny: ["anthropic/claude-sonnet-4-5"],
    auto_allow: ["opencode/sonnet"],
    tiers: [{ model: "gpt 5", tier: "mega" }],
    providers: [],
    engine_affinity: [{ kind: "chat", engine: "codex-acp" }],
  }, vocabulary);
  assert.equal(built.ok, false);
  if (built.ok) return;
  assert.match(built.errors["rules.pin[0]"], /exact provider\/model/);
  assert.match(built.errors["rules.pin[1]"], /Each role/);
  assert.match(built.errors["rules.pin[1]"], /Effort/);
  assert.match(built.errors["rules.deny[0]"], /engine/);
  assert.match(built.errors["rules.auto_allow[0]"], /exact opencode/);
  assert.match(built.errors["rules.tiers[0]"], /spaces/);
  assert.match(built.errors["rules.tiers[0]"], /Tier/);
  assert.match(built.errors["rules.providers"], /at least one/);
  assert.match(built.errors["rules.engine_affinity[0]"], /Work kind/);
});

test("routing sources Cortex cannot represent or write are read-only, never rewritten", () => {
  assert.equal(routingRulesToForm({ pin: { "*": 5 } }).ok, false);
  assert.equal(routingRulesToForm({ deny: "codex-acp" }).ok, false);
  const broken = routing(undefined, { valid: false, error: { code: "invalid_routing_overrides", message: "bad JSON" } });
  const state = routingTarget(broken, "user");
  assert.equal(state.editable, false);
  assert.match(state.reason ?? "", /bad JSON/);
  assert.equal(buildRoutingUpdate(broken, "user", null, { pin: null, deny: null, auto_allow: null, tiers: null, providers: null, engine_affinity: null }).ok, false);
  const noProject = { ...routing({}), sources: routing({}).sources.filter((source) => source.source !== "project") };
  assert.equal(routingTarget(noProject, "project").editable, false);
});

// --- MCP servers --------------------------------------------------------------------

const SEARCH_SERVER: McpServerView = {
  name: "search", enabled: true, valid: true, problem: null,
  config: { command: "npx", args: ["-y", "search-mcp", "--token", "[redacted]"], env: [{ name: "API_KEY", value_set: true }], allowed_tools: ["find"], network: true },
  redacted: true, unsupported_keys: [],
};

test("MCP: a stored entry round-trips, keeping redacted parts and env values", () => {
  const parsed = mcpServerToForm(SEARCH_SERVER);
  assert.equal(parsed.ok, true);
  if (!parsed.ok) return;
  assert.deepEqual(buildMcpUpsert("search", parsed.form, { isNew: false, existingNames: ["search"] }), {
    ok: true,
    value: {
      op: "upsert", name: "search",
      server: { command: "npx", args: ["-y", "search-mcp", "--token", "[redacted]"], env: { API_KEY: { keep: true } }, allowed_tools: ["find"], network: true },
    },
  });
});

test("MCP: env values are replaced or removed only explicitly", () => {
  const parsed = mcpServerToForm(SEARCH_SERVER);
  assert.equal(parsed.ok, true);
  if (!parsed.ok) return;
  const replaced = { ...parsed.form, env: [{ ...parsed.form.env![0], mode: "replace" as const, value: "new-secret" }, { name: "REGION", stored: false, valueSet: false, mode: "replace" as const, value: "eu" }] };
  const built = buildMcpUpsert("search", replaced, { isNew: false, existingNames: ["search"] });
  assert.equal(built.ok, true);
  if (built.ok && built.value.op === "upsert") assert.deepEqual(built.value.server.env, { API_KEY: "new-secret", REGION: "eu" });
  const removed = buildMcpUpsert("search", { ...parsed.form, env: null }, { isNew: false, existingNames: ["search"] });
  assert.equal(removed.ok, true);
  if (removed.ok && removed.value.op === "upsert") assert.equal("env" in removed.value.server, false);
  const keepNew = buildMcpUpsert("search", { ...parsed.form, env: [{ name: "NEW", stored: false, valueSet: false, mode: "keep", value: "" }] }, { isNew: false, existingNames: [] });
  assert.equal(keepNew.ok, false);
  if (!keepNew.ok) assert.match(keepNew.errors["env[0]"], /no stored value/);
});

test("MCP: entries with unmanaged keys or unexpected shapes are read-only", () => {
  const legacy = { ...SEARCH_SERVER, name: "legacy", unsupported_keys: ["cwd"] };
  const parsed = mcpServerToForm(legacy);
  assert.equal(parsed.ok, false);
  if (!parsed.ok) assert.match(parsed.reason, /cwd/);
  assert.equal(mcpServerToForm({ ...SEARCH_SERVER, config: { command: "x", roles: ["admin"] } }).ok, false);
  assert.equal(mcpServerToForm({ ...SEARCH_SERVER, config: null }).ok, false);
  const view = {
    surface: "mcp_servers" as const, path: "~", revision: REV("e"), exists: true, root_key: "servers", version: 1,
    servers: [legacy], editable_keys: [], engines: [],
  };
  assert.deepEqual(mcpUpdate(view, { op: "set_enabled", name: "legacy", enabled: false }), {
    surface: "mcp_servers", expected_revision: REV("e"), operations: [{ op: "set_enabled", name: "legacy", enabled: false }],
  });
});

test("MCP: a new server needs a valid unique name and a command", () => {
  const empty = buildMcpUpsert("cortex", newMcpServerForm(), { isNew: true, existingNames: [] });
  assert.equal(empty.ok, false);
  if (!empty.ok) {
    assert.match(empty.errors.name, /cortex/);
    assert.match(empty.errors.command, /command/);
  }
  const duplicate = buildMcpUpsert("search", { ...newMcpServerForm(), command: "x" }, { isNew: true, existingNames: ["search"] });
  assert.equal(duplicate.ok, false);
  const created = buildMcpUpsert("files", { ...newMcpServerForm(), command: "files-mcp", roles: ["worker"], engines: ["opencode"], enabled: false },
    { isNew: true, existingNames: ["search"] });
  assert.deepEqual(created, {
    ok: true, value: { op: "upsert", name: "files", server: { command: "files-mcp", enabled: false, roles: ["worker"], engines: ["opencode"] } },
  });
});

// --- connections --------------------------------------------------------------------

test("connections: rename and session limits are validated before a write", () => {
  assert.deepEqual(buildConnectionRename(CONNECTIONS, "cn_0123456789ab", "  Work  "), {
    ok: true, value: { surface: "connections", expected_revision: REV("c"), connection_id: "cn_0123456789ab", operation: "rename", name: "Work" },
  });
  assert.equal(buildConnectionRename(CONNECTIONS, "cn_0123456789ab", "   ").ok, false);
  assert.equal(buildConnectionMaxSessions(CONNECTIONS, "cn_0123456789ab", "9").ok, false);
  assert.equal(buildConnectionMaxSessions(CONNECTIONS, "cn_0123456789ab", "0").ok, false);
  assert.deepEqual(buildConnectionMaxSessions(CONNECTIONS, "cn_0123456789ab", "3"), {
    ok: true,
    value: { surface: "connections", expected_revision: REV("c"), connection_id: "cn_0123456789ab", operation: "set_max_concurrent_sessions", max_concurrent_sessions: 3 },
  });
  assert.deepEqual(connectionAction(CONNECTIONS, "cn_0123456789ab", "validate"), {
    surface: "connections", expected_revision: REV("c"), connection_id: "cn_0123456789ab", operation: "validate",
  });
});

test("OpenCode v2 registration is sent only with an explicit opt-in", () => {
  assert.equal(buildOnboard(CONNECTIONS, false, "").ok, false);
  assert.deepEqual(buildOnboard(CONNECTIONS, true, ""), { ok: true, value: { expected_revision: REV("c"), explicit_opt_in: true } });
  assert.deepEqual(buildOnboard(CONNECTIONS, true, " OpenCode "), { ok: true, value: { expected_revision: REV("c"), explicit_opt_in: true, name: "OpenCode" } });
});
