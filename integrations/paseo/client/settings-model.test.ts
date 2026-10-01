import assert from "node:assert/strict";
import test from "node:test";
import {
  projectChoices,
  buildProfileSave,
  buildProfileDelete, buildProfileCreate, buildProfileActivate, buildProfileDeactivate, buildProfileRename,
  profileSwitchDecision, profileIsPartial, profileStateText, validProfileName, type ProfilesView,
  automaticText,
  canAutoAllow,
  emptyRoutingForm,
  engineAutomaticIn,
  engineDisabledIn,
  engineModelsTarget,
  modelAutoAllowState,
  modelDenyState,
  modelGroups,
  removeRoutingToken,
  setEngineAutomatic,
  setEngineEnabled,
  setModelAutoAllowed,
  setModelEnabled,
  setModelAutoSelected,
  modelAutoSelected,
  setModelPreferredFor,
  shadowingSource,
  unmatchedTokens,
  type CatalogRoute,
  type RouteCatalogView,
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

function catalogRoute(engine: string, model: string, extra: Partial<CatalogRoute> = {}): CatalogRoute {
  return {
    route_id: "xr_aaaaaaaaaaaa", engine, model, model_name: model, connection_id: "cn_0123456789ab", connection_name: "Main",
    connection_state: "configured", qualified: true, observed: true, tier: "standard", selectable: true, in_selector: true,
    disabled: false, deny_tokens: [], engine_disabled: false, default_auto_excluded: null, auto_allow_tokens: [], automatic: "eligible", auto_selected: engine !== "opencode",
    superseded_by: null, matches: { user: { deny: [], auto_allow: [] } }, ...extra,
  };
}

function catalog(routes: CatalogRoute[]): RouteCatalogView {
  return {
    surface: "route_catalog", project_root: null, precedence: ["user"], effective_source: "user", effective_valid: true, providers: null,
    engines: ["codex-acp", "claude-agent-acp", "opencode"].map((engine) => ({
      engine, optional: engine === "opencode", connections: [], disabled: false, automatic: true,
      default_auto_excluded: engine === "opencode", route_count: 0, matches: {},
    })),
    routes, catalog_problems: [], not_shown: ["recorded_unavailability", "quota"],
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

// --- engines & models -----------------------------------------------------------

test("engines & models: engine switches write <engine>/* and the automatic-routing list, keeping every other rule", () => {
  const view = routing({ deny: ["gpt-5.5"], pin: { "*": "codex-acp/gpt-6-luna" }, tiers: { "gpt-6-luna": "light" } });
  const state = engineModelsTarget(view, "user");
  assert.equal(state.seededFrom, null);
  const engines = routingVocabulary(view).engines;
  let form = setEngineEnabled(state.form!, "claude-agent-acp", false);
  assert.equal(engineDisabledIn(form, "claude-agent-acp"), true);
  assert.equal(setEngineEnabled(form, "claude-agent-acp", false), form, "switching off twice writes nothing more");
  form = setEngineAutomatic(form, "codex-acp", false, engines);
  assert.deepEqual(form.providers, ["claude-agent-acp", "opencode"], "an unset list becomes every other engine");
  assert.equal(engineAutomaticIn(form, "codex-acp"), false);
  assert.equal(engineDisabledIn(form, "codex-acp"), false, "automatic routing is not disabling");
  const built = buildRoutingUpdate(view, "user", null, form);
  assert.ok(built.ok);
  if (built.ok) {
    assert.equal(built.value.expected_revision, REV("b"));
    assert.deepEqual(built.value.rules, {
      pin: { "*": "codex-acp/gpt-6-luna" }, deny: ["gpt-5.5", "claude-agent-acp/*"], tiers: { "gpt-6-luna": "light" },
      providers: ["claude-agent-acp", "opencode"],
    });
  }
  form = setEngineEnabled(setEngineAutomatic(form, "codex-acp", true, engines), "claude-agent-acp", true);
  assert.deepEqual(form.deny, ["gpt-5.5"]);
  assert.deepEqual(form.providers, ["claude-agent-acp", "opencode", "codex-acp"]);
  let none = form;
  for (const engine of engines) none = setEngineAutomatic(none, engine, false, engines);
  const refused = buildRoutingUpdate(view, "user", null, none);
  assert.equal(refused.ok, false, "automatic routing always keeps at least one engine");
});

test("engines & models: a model switch writes its exact entry; broader entries are named, never removed by it", () => {
  const view = routing({ deny: ["haiku", "codex-acp/gpt-6-sol"] });
  const form = engineModelsTarget(view, "user").form!;
  const haiku = catalogRoute("claude-agent-acp", "claude-haiku-4-5", {
    default_auto_excluded: "family", matches: { user: { deny: ["haiku"], auto_allow: [] } },
  });
  const sol = catalogRoute("codex-acp", "gpt-6-sol", { matches: { user: { deny: ["codex-acp/gpt-6-sol"], auto_allow: [] } } });
  const sonnet = catalogRoute("claude-agent-acp", "claude-sonnet-5");
  assert.deepEqual(modelDenyState(haiku, form, "user"), { on: true, exact: "claude-agent-acp/claude-haiku-4-5", blocking: ["haiku"] });
  assert.deepEqual(modelDenyState(sol, form, "user"), { on: true, exact: "codex-acp/gpt-6-sol", blocking: [] });
  let next = setModelEnabled(form, sol, true);
  assert.deepEqual(next.deny, ["haiku"]);
  next = setModelEnabled(next, sonnet, false);
  assert.deepEqual(next.deny, ["haiku", "claude-agent-acp/claude-sonnet-5"]);
  assert.equal(modelDenyState(sonnet, next, "user").on, true);
  assert.deepEqual(modelDenyState(sol, setEngineEnabled(next, "codex-acp", false), "user").blocking, ["codex-acp/*"]);
  assert.deepEqual(modelDenyState(sol, { ...next, deny: ["gpt-6-sol"] }, "user").blocking, ["gpt-6-sol"], "a bare id applies on every engine");
  assert.deepEqual(removeRoutingToken(next, "deny", "haiku").deny, ["claude-agent-acp/claude-sonnet-5"]);
  assert.equal(removeRoutingToken(next, "auto_allow", "haiku"), next);
  const cat = catalog([haiku, sol, sonnet]);
  assert.deepEqual(unmatchedTokens({ ...next, deny: [...next.deny!, "gpt-4o", "opencode/*"] }, "deny", cat, "user"), ["gpt-4o"]);
  const groups = modelGroups([sol, { ...sol, route_id: "xr_bbbbbbbbbbbb", connection_id: "cn_bbbbbbbbbbbb" }, sonnet], "codex-acp");
  assert.equal(groups.length, 1);
  assert.equal(groups[0].routes.length, 2, "one switch covers every connection offering the model");
});

test("engines & models: OpenCode v2 routes are opted into automatic routing only by their exact route", () => {
  const view = routing({});
  const form = engineModelsTarget(view, "user").form!;
  const sonnet = catalogRoute("opencode", "anthropic/claude-sonnet-4-5", { default_auto_excluded: "optional_engine", automatic: "excluded_by_default" });
  assert.equal(canAutoAllow(sonnet), true);
  assert.equal(canAutoAllow(catalogRoute("opencode", "big-pickle", { default_auto_excluded: "optional_engine" })), false);
  assert.equal(canAutoAllow(catalogRoute("codex-acp", "gpt-6-luna")), false, "only a model excluded by default has an opt-in");
  assert.match(automaticText(sonnet), /OpenCode v2 routes need an opt-in/);
  const allowed = setModelAutoAllowed(form, sonnet, true);
  assert.deepEqual(allowed.auto_allow, ["opencode/anthropic/claude-sonnet-4-5"]);
  assert.equal(modelAutoAllowState(sonnet, allowed, "user").on, true);
  const built = buildRoutingUpdate(view, "user", null, allowed);
  assert.ok(built.ok);
  if (built.ok) assert.deepEqual(built.value.rules, { auto_allow: ["opencode/anthropic/claude-sonnet-4-5"] }, "nothing else is written");
  assert.deepEqual(setModelAutoAllowed(allowed, sonnet, false).auto_allow, []);
});

test("engines & models: a new project file starts from the rules in use; a shadowing file is named", () => {
  const view = routing({ deny: ["gpt-5.5"], providers: ["codex-acp"] });
  const project = engineModelsTarget(view, "project");
  assert.equal(project.seededFrom, "user");
  assert.deepEqual(project.form?.deny, ["gpt-5.5"]);
  assert.equal(project.revision, "absent");
  assert.equal(routingTarget(view, "project").form?.deny, null, "the Routing section still shows the file as it is");
  assert.equal(shadowingSource(view, "user"), null);
  const built = buildRoutingUpdate(view, "project", "ws1", project.form!);
  assert.ok(built.ok);
  if (built.ok) {
    assert.deepEqual(built.value, {
      surface: "routing", target: "project", workspaceId: "ws1", expected_revision: "absent", rules: { deny: ["gpt-5.5"], providers: ["codex-acp"] },
    });
  }
  const withProject: RoutingView = {
    ...view, effective_source: "project", effective_rules: {},
    sources: view.sources.map((source) => (source.source === "project" ? { ...source, exists: true, revision: REV("p"), valid: true, rules: {} } : source)),
  };
  assert.equal(shadowingSource(withProject, "user"), "project");
  assert.equal(engineModelsTarget(withProject, "project").seededFrom, null);
  assert.equal(engineModelsTarget({ ...view, effective_valid: false }, "project").seededFrom, null, "an invalid file is never copied");
});

test("routing validation: the only wildcard is an exact <engine>/* selector", () => {
  const vocabulary = routingVocabulary(routing({}));
  assert.ok(buildRoutingRules({ ...emptyRoutingForm(), deny: ["codex-acp/*", "opencode/*"] }, vocabulary).ok);
  const bad = buildRoutingRules({ ...emptyRoutingForm(), deny: ["*", "gemini/*", "gpt-*"], auto_allow: ["opencode/*"] }, vocabulary);
  assert.equal(bad.ok, false);
  if (!bad.ok) assert.deepEqual(Object.keys(bad.errors).sort(), ["rules.deny[0]", "rules.deny[1]", "rules.deny[2]"]);
});

test("purpose preferences round-trip other policy attributes and require an auto-selected model", () => {
  const parsed = routingRulesToForm({ models: { opencode: {
    auto: ["zai-coding-plan/glm-5"], tiers: { "zai-coding-plan/glm-5": "standard" },
    policy: { "zai-coding-plan/glm-5": { preferred_for: ["review"], effort_floor: "medium" } },
  } } });
  assert.equal(parsed.ok, true);
  if (!parsed.ok) return;
  const view = routing();
  let form = setModelPreferredFor(parsed.form, "opencode", "zai-coding-plan/glm-5", "code", true);
  const built = buildRoutingRules(form, routingVocabulary(view));
  assert.equal(built.ok, true);
  if (built.ok) assert.deepEqual(built.value.models?.opencode?.policy?.["zai-coding-plan/glm-5"], {
    preferred_for: ["review", "code"], effort_floor: "medium",
  });
  form = setModelAutoSelected(form, "opencode", "zai-coding-plan/glm-5", false, ["zai-coding-plan/glm-5"]);
  const removed = buildRoutingRules(form, routingVocabulary(view));
  assert.equal(removed.ok, true);
  if (removed.ok) assert.deepEqual(removed.value.models?.opencode?.policy?.["zai-coding-plan/glm-5"], {
    preferred_for: [], effort_floor: "medium",
  });
});


function profileView(): ProfilesView {
  return {
    surface: "routing_profiles", path: "routing-profiles.json", revision: REV("a"), format: 2, migrated_from: null,
    active: "work", state: "modified", modified_sections: ["routing", "user_defaults", "mcp_servers", "connections"],
    differences: [{ section: "routing", key: "models" }, { section: "user_defaults", key: "language" },
      { section: "mcp_servers", key: "search" }, { section: "connections", key: "cn_0123456789ab" }],
    pending: null, problems: [], applied: null, profiles: [], shadowed_by: null, matches_profile: [], user_routing_revision: REV("b"),
    current_revisions: { routing: REV("b"), user_defaults: "absent", mcp_servers: REV("c"), connections: REV("d") },
  };
}

test("profiles: save, create and activate bind every backing revision without legacy fields", () => {
  const view = profileView();
  const cases = [buildProfileSave(view), buildProfileCreate(view, " new-work ", " description "), buildProfileActivate(view, "other", { discard: true })];
  for (const built of cases) {
    assert.equal(built.ok, true);
    if (!built.ok) continue;
    assert.deepEqual((built.value as any).expected_revisions, view.current_revisions);
    assert.equal(built.value.expected_revision, view.revision);
    assert.equal("from_current" in built.value, false);
    assert.equal("expected_routing_revision" in built.value, false);
    assert.equal("rules" in built.value, false);
  }
  assert.deepEqual(buildProfileSave(view), { ok: true, value: { surface: "routing_profiles", action: "save", expected_revision: view.revision, expected_revisions: view.current_revisions, name: "work" } });
  assert.deepEqual(buildProfileCreate(view, "new-work", " description "), { ok: true, value: { surface: "routing_profiles", action: "create", expected_revision: view.revision, expected_revisions: view.current_revisions, name: "new-work", description: "description" } });
  const activation = buildProfileActivate(view, "other", { discard: true });
  assert.equal(activation.ok && "discard_modifications" in activation.value && activation.value.discard_modifications, true);
});

test("profiles: validation blocks unreadable sections and invalid names", () => {
  const view = profileView();
  for (const section of ["routing", "user_defaults", "mcp_servers", "connections"] as const) {
    const unavailable = { ...view, current_revisions: { ...view.current_revisions, [section]: null } };
    for (const built of [buildProfileSave(unavailable), buildProfileCreate(unavailable, "new"), buildProfileActivate(unavailable, "other")]) {
      assert.equal(built.ok, false);
      if (!built.ok) assert.ok(built.errors[`current_revisions.${section}`]);
    }
  }
  assert.equal(buildProfileSave({ ...view, active: null }).ok, false);
  for (const name of ["", "UPPER", "-bad", "a".repeat(41), "has space"]) {
    assert.equal(validProfileName(name), false);
    assert.equal(buildProfileCreate(view, name).ok, false);
  }
  assert.equal(buildProfileCreate(view, "new", "a".repeat(257)).ok, false);
});

test("profiles: state text names every differing section and recovery state", () => {
  const view = profileView();
  const text = profileStateText(view);
  for (const label of ["User routing (models)", "User defaults (language)", "MCP servers (search)", "Connections (cn_0123456789ab)"]) assert.ok(text.includes(label));
  assert.match(profileStateText({ ...view, active: null, state: "none" }), /No profile/);
  assert.match(profileStateText({ ...view, state: "in_sync" }), /match profile "work"/);
  assert.match(profileStateText({ ...view, state: "interrupted", pending: { operation: "activate", profile: "other", at: "now" } }), /Activation of "other" did not finish/);
  assert.match(profileStateText({ ...view, state: "unavailable", problems: [{ section: "mcp_servers", code: "invalid", message: "Invalid policy" }] }), /MCP servers: Invalid policy/);
});

test("profiles: switching and reverting modified settings require confirmation; detach does not", () => {
  const view = profileView();
  for (const state of ["modified", "unavailable"] as const) {
    assert.equal(profileSwitchDecision({ ...view, state }, "other"), "confirm");
    assert.equal(profileSwitchDecision({ ...view, state }, "work"), "confirm");
  }
  assert.equal(profileSwitchDecision(view, ""), "direct");
  for (const state of ["none", "in_sync", "interrupted"] as const) assert.equal(profileSwitchDecision({ ...view, state }, "other"), "direct");
  assert.equal(profileIsPartial({ sections: ["routing"] }), true);
  assert.equal(profileIsPartial({ sections: ["routing", "user_defaults", "mcp_servers", "connections"] }), false);
});

test("profiles: rename is metadata-only and can clear description; detach keeps backing files", () => {
  const view = profileView();
  assert.deepEqual(buildProfileRename(view, "work", " renamed ", ""), { ok: true, value: { surface: "routing_profiles", action: "update", expected_revision: view.revision, name: "work", rename_to: "renamed", description: "" } });
  assert.equal(buildProfileRename(view, "work", "INVALID", "").ok, false);
  assert.deepEqual(buildProfileDeactivate(view), { ok: true, value: { surface: "routing_profiles", action: "deactivate", expected_revision: view.revision } });
});


test("v2 arbitrary identities round-trip complete policy, neutral attributes and disabled engines", () => {
  const rules = { model_policy_version: 2, enabled: { "codex-acp": ["RENAMED-abc@v9"], opencode: [] }, deny: ["opencode/*"],
    models: { "codex-acp": { auto: ["RENAMED-abc@v9"], tiers: { "RENAMED-abc@v9": ["light", "standard"] },
      policy: { "RENAMED-abc@v9": { effort_floor: "medium", max_effort: "high", effort_ladder: ["medium", "high"],
        also_standard_for: ["review"], preferred_for: ["review"], request_names: ["owner-choice"], future: { keep: true } },
        dormant: { effort_floor: null, max_effort: null, preferred_for: null, request_names: null } } } } };
  const parsed = routingRulesToForm(rules);
  assert.ok(parsed.ok);
  if (!parsed.ok) return;
  const built = buildRoutingRules(parsed.form, routingVocabulary(routing(rules)));
  assert.ok(built.ok);
  if (built.ok) assert.deepEqual(built.value, rules);
  assert.equal(modelAutoSelected(emptyRoutingForm(), "codex-acp", "RENAMED-abc@v9", true), false);
});

test("portable coordinator changes use exact IDs, preserve unrelated fields and reject bound-route conflicts", () => {
  const view = userDefaults({ defaults: { coordinator: { engine: "codex-acp", model: "RENAMED@9", parameters: { mode: "agent" } }, future: { keep: true } } });
  const form = userDefaultsToForm(view);
  assert.deepEqual(buildUserDefaultsUpdate(view, form), { ok: true, value: null });
  form.coordinator_model = "DifferentExactID";
  const built = buildUserDefaultsUpdate(view, form);
  assert.ok(built.ok);
  if (built.ok && built.value) assert.deepEqual(built.value.defaults, { coordinator: { engine: "codex-acp", model: "DifferentExactID", parameters: { mode: "agent" } } });
  form.route_id = "xr_0123456789ab"; form.route_connection_id = "cn_0123456789ab"; form.route_model = "Other";
  assert.equal(buildUserDefaultsUpdate(view, form).ok, false);
});

test("delete profile payload is closed, exact and valid for selected/last snapshots; pending activation blocks", () => {
  const view = profileView();
  for (const name of view.profiles.map((profile) => profile.name)) {
    assert.deepEqual(buildProfileDelete(view, name), { ok: true, value: { surface: "routing_profiles", action: "delete", name, expected_revision: view.revision } });
  }
  assert.equal(buildProfileDelete(view, "absent").ok, false);
  assert.equal(buildProfileDelete({ ...view, pending: { operation: "activate", profile: "mix", at: "now" } }, "mix").ok, false);
});


test("unsupported portable fields remain intact and cannot be dropped by a coordinator edit", () => {
  const view = userDefaults({ defaults: { coordinator: { engine: "codex-acp", model: "EXACT@9", future: { retain: true } } } });
  const form = userDefaultsToForm(view);
  form.language = "ru";
  const unrelated = buildUserDefaultsUpdate(view, form);
  assert.ok(unrelated.ok);
  if (unrelated.ok && unrelated.value) assert.deepEqual(unrelated.value.defaults, { language: "ru" });
  form.coordinator_model = "OTHER@9";
  assert.equal(buildUserDefaultsUpdate(view, form).ok, false);
  assert.deepEqual((view.defaults.coordinator as any).future, { retain: true });
});


test("quota window rules round-trip exact renamed identities and shared/empty scopes", () => {
  const rules = { model_policy_version: 2, quota_windows: { "codex-acp": [
    { window: "Owner window / EXACT@7", models: ["RENAMED@9", "WITHDRAWN@7"] },
    { window: "account", models: null }, { window: "excluded", models: [] },
  ] } };
  const parsed = routingRulesToForm(rules); assert.ok(parsed.ok);
  if (!parsed.ok) return;
  assert.deepEqual(buildRoutingRules(parsed.form, routingVocabulary(routing({}))), { ok: true, value: rules });
  parsed.form.quota_windows!["codex-acp"].push({ window: "account", models: ["RENAMED@9"] });
  const ambiguous = buildRoutingRules(parsed.form, routingVocabulary(routing({})));
  assert.equal(ambiguous.ok, false);
  assert.equal(routingRulesToForm({ quota_windows: { "codex-acp": [{ window: "account", models: null, future: true }] } }).ok, false);
  assert.deepEqual(rules.quota_windows["codex-acp"][0].models, ["RENAMED@9", "WITHDRAWN@7"]);
});

test("project picker lists one choice per project directory, named by project, never by task title", () => {
  const base = { projectDisplayName: "demo", projectRootPath: "/p/demo" };
  const choices = projectChoices([
    { ...base, id: "w1", name: "main", workspaceDirectory: "/p/demo", workspaceKind: "local_checkout", title: "Task one" } as never,
    { ...base, id: "w2", name: "main", workspaceDirectory: "/p/demo", workspaceKind: "local_checkout", title: "Task two" } as never,
    { ...base, id: "w3", name: "feature", worktreeSlug: "feature-x", workspaceDirectory: "/p/demo-wt/feature-x", workspaceKind: "worktree" },
    { ...base, id: "w4", name: "old", workspaceDirectory: "/p/demo-wt/old", workspaceKind: "worktree", archivingAt: "2026-10-01T00:00:00Z" },
    { id: "w5", projectDisplayName: "other", projectCustomName: "Other app", projectRootPath: "/p/other", workspaceDirectory: "/p/other", workspaceKind: "local_checkout" },
  ]);
  assert.deepEqual(choices, [
    { id: "w1", label: "demo — /p/demo" },
    { id: "w3", label: "demo (worktree feature-x) — /p/demo-wt/feature-x" },
    { id: "w5", label: "Other app — /p/other" },
  ]);
});
