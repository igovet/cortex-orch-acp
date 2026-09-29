import { defineRpc } from "@getpaseo/plugin";
import { z } from "zod";

/**
 * Typed plugin RPC contracts for Cortex's owner configuration API
 * (``cortex_runtime.configuration.management``, API version 1).
 *
 * The Cortex JSON files and the project store stay the only source of truth:
 * these contracts never read or write Paseo's own plugin settings document.
 * Requests name what to change, never where: the project is resolved from
 * the Paseo workspace, the owner configuration home from the Cortex launch
 * this daemon runs, and no request carries a file path or a command to run.
 * Configuration field names are the API's own (snake_case); Paseo-side
 * identities (``workspaceId``) and the envelope are camelCase like the
 * plugin's other RPCs. Unknown request fields are refused.
 */

export const CONFIG_API_VERSION = 1;

export const configSurfaces = ["user_defaults", "project_ceiling", "routing", "mcp_servers", "connections"] as const;
export type ConfigSurface = typeof configSurfaces[number];

/** The marker a redacted credential part reads as in a response. */
export const REDACTED = "[redacted]";

const workspaceId = z.string().min(1).max(512);
const revision = z.string().regex(/^(?:absent|sha256:[0-9a-f]{64})$/);
const text = (max = 256) => z.string().max(max);
const nonNegativeInt = z.number().int().nonnegative();
const stringSet = z.array(text().min(1)).max(1000).nullable();

// --- request pieces ---------------------------------------------------------

const ceilingInput = z.strictObject({
  allowed_routes: stringSet.optional(),
  allowed_connections: stringSet.optional(),
  allowed_data_recipients: stringSet.optional(),
  permission_level: text(64).nullable().optional(),
  limits: z.record(text(64), nonNegativeInt).optional(),
  spend_limit: nonNegativeInt.nullable().optional(),
  spend_limit_enforceable: z.boolean().optional(),
  fallback_policy: z.strictObject({
    allowed_target_routes: stringSet.optional(),
    allowed_failure_classes: stringSet.optional(),
    budget: nonNegativeInt.nullable().optional(),
    data_recipient_consent: z.boolean().optional(),
  }).optional(),
});

// A key maps to its new value; null removes it. The route default is set or
// cleared as the whole route_id/route_connection_id/route_model group.
const defaultsPatch = z.strictObject({
  route_id: text(80).nullable().optional(),
  route_connection_id: text(32).nullable().optional(),
  route_model: text().nullable().optional(),
  parameters: z.record(text(64), text(128)).nullable().optional(),
  language: z.enum(["en", "ru"]).nullable().optional(),
});

// The six keys the routing loader supports; an omitted key is removed.
const routingRules = z.strictObject({
  pin: z.record(text(128), text()).optional(),
  deny: z.array(text()).max(1000).optional(),
  auto_allow: z.array(text()).max(1000).optional(),
  tiers: z.record(text(), text(64)).optional(),
  providers: z.array(text(64)).max(16).optional(),
  engine_affinity: z.record(text(64), text(64).nullable()).optional(),
});

// An ``env`` value is a new string or ``{"keep": true}`` for the value
// already stored; stored values are never returned.
const envValue = z.union([z.string().max(16384), z.strictObject({ keep: z.literal(true) })]);
const toolList = z.array(text().min(1)).max(512);
const mcpServerInput = z.strictObject({
  command: z.string().min(1).max(4096),
  args: z.array(z.string().max(16384)).max(256).optional(),
  env: z.record(z.string().regex(/^[A-Za-z_][A-Za-z0-9_]{0,127}$/), envValue).optional(),
  enabled: z.boolean().optional(),
  allowed_tools: toolList.optional(),
  read_only_tools: toolList.optional(),
  coordinator_tools: toolList.optional(),
  always_load_tools: toolList.optional(),
  read_paths: z.array(text(4096)).max(256).optional(),
  write_paths: z.array(text(4096)).max(256).optional(),
  network: z.boolean().optional(),
  runtime_dir_env: z.array(text(128)).max(64).optional(),
  roles: z.array(z.enum(["coordinator", "worker"])).max(2).optional(),
  engines: z.array(z.enum(["codex-acp", "claude-agent-acp", "opencode"])).max(3).optional(),
});
const serverName = text(64).min(1);
const mcpOperation = z.discriminatedUnion("op", [
  // The whole entry. A command/argument part still reading "[redacted]"
  // keeps the stored value it stands for; an env value omitted is dropped.
  z.strictObject({ op: z.literal("upsert"), name: serverName, server: mcpServerInput }),
  z.strictObject({ op: z.literal("remove"), name: serverName }),
  z.strictObject({ op: z.literal("set_enabled"), name: serverName, enabled: z.boolean() }),
]);
export type McpOperation = z.infer<typeof mcpOperation>;

// --- responses --------------------------------------------------------------

const ceiling = z.object({
  allowed_routes: z.array(z.string()).nullable(),
  allowed_connections: z.array(z.string()).nullable(),
  allowed_data_recipients: z.array(z.string()).nullable(),
  permission_level: z.string().nullable(),
  limits: z.record(z.string(), z.number()),
  spend_limit: z.number().nullable(),
  spend_limit_enforceable: z.boolean(),
  fallback_policy: z.object({
    allowed_target_routes: z.array(z.string()).nullable(),
    allowed_failure_classes: z.array(z.string()).nullable(),
    budget: z.number().nullable(),
    data_recipient_consent: z.boolean(),
  }),
});

const problem = z.object({ code: z.string(), message: z.string() });
const fieldError = z.object({ field: z.string(), code: z.string(), message: z.string() });

export const configError = z.object({
  code: z.string(),
  message: z.string(),
  field: z.string().nullable(),
  // Every invalid setting of a refused write; nothing was written.
  errors: z.array(fieldError).optional(),
  // A revision_conflict names the revision to re-read.
  currentRevision: z.string().optional(),
});

const writeMeta = {
  changed: z.boolean().optional(),
  notes: z.array(z.string()).optional(),
};

export const userDefaultsResult = z.object({
  surface: z.literal("user_defaults"),
  path: z.string(),
  revision,
  exists: z.boolean(),
  defaults: z.record(z.string(), z.unknown()),
  editable_keys: z.array(z.string()),
  preserved_keys: z.array(z.string()),
  languages: z.array(z.string()),
  ceiling,
  ceiling_set: z.boolean(),
  ...writeMeta,
});

export const projectCeilingResult = z.object({
  surface: z.literal("project_ceiling"),
  project_root: z.string(),
  revision,
  exists: z.boolean(),
  user_ceiling: ceiling,
  ceiling: ceiling.nullable(),
  project_defaults: z.record(z.string(), z.unknown()),
  effective_ceiling: ceiling,
  narrows_user_ceiling: z.boolean(),
  violation: z.object({ code: z.string(), field: z.string().nullable(), message: z.string() }).nullable(),
  updated_at: z.string().nullable(),
  ...writeMeta,
});

const routingSource = z.object({
  source: z.enum(["project", "env", "user"]),
  path: z.string(),
  writable: z.boolean(),
  exists: z.boolean(),
  revision: revision.nullable(),
  valid: z.boolean().optional(),
  error: z.object({ code: z.string(), message: z.string() }).optional(),
  rules: z.record(z.string(), z.unknown()).optional(),
  ignored_keys: z.array(z.string()).optional(),
});

export const routingResult = z.object({
  surface: z.literal("routing"),
  project_root: z.string().nullable(),
  precedence: z.array(z.string()),
  sources: z.array(routingSource),
  // The file the loader uses (files never merge), or the built-in policy.
  effective_source: z.enum(["project", "env", "user", "builtin"]),
  effective_valid: z.boolean(),
  effective_rules: z.record(z.string(), z.unknown()).nullable(),
  builtin: z.object({
    supported_engines: z.array(z.string()),
    optional_engines: z.array(z.string()),
    default_auto_deny_engines: z.array(z.string()),
    default_auto_deny_families: z.array(z.string()),
    tiers: z.array(z.string()),
    efforts: z.array(z.string()),
    work_kinds: z.array(z.string()),
    default_user_deny: z.array(z.string()),
  }),
  target: z.enum(["user", "project"]).optional(),
  written_revision: revision.optional(),
  // The source that still wins when the written file is not effective.
  shadowed_by: z.string().nullable().optional(),
  ...writeMeta,
});

// ``env`` is only ever names and whether each value is set.
const redactedEnv = z.array(z.object({ name: z.string().nullable(), value_set: z.boolean() })).nullable();

export const mcpServersResult = z.object({
  surface: z.literal("mcp_servers"),
  path: z.string(),
  revision,
  exists: z.boolean(),
  root_key: z.string(),
  version: z.unknown(),
  servers: z.array(z.object({
    name: z.string(),
    enabled: z.boolean().nullable(),
    valid: z.boolean(),
    problem: problem.nullable(),
    // Configuration as stored, except: env values never appear, and a
    // credential-looking command/argument part reads "[redacted]".
    config: z.object({ env: redactedEnv.optional() }).catchall(z.unknown()).nullable(),
    redacted: z.boolean(),
    unsupported_keys: z.array(z.string()),
  })),
  editable_keys: z.array(z.string()),
  engines: z.array(z.string()),
  ...writeMeta,
});

const connection = z.object({
  id: z.string(),
  name: z.string(),
  engine: z.string(),
  provider: z.string(),
  auth_mode: z.string(),
  key_kind: z.string().nullable(),
  endpoint: z.string().nullable(),
  launch_command: z.array(z.string()).nullable(),
  // The credential's kind only: never a value, never a path.
  credential: z.object({ kind: z.string(), owned: z.boolean().optional() }),
  state: z.string(),
  qualified: z.boolean(),
  max_concurrent_sessions: z.number(),
  optional_engine: z.boolean(),
  created_at: z.string(),
  updated_at: z.string(),
});

export const connectionsResult = z.object({
  surface: z.literal("connections"),
  revision,
  exists: z.boolean(),
  connections: z.array(connection),
  engines: z.array(z.object({
    engine: z.string(),
    optional: z.boolean(),
    onboarding: z.string(),
    auth_modes: z.record(z.string(), z.boolean()),
    qualified_versions: z.union([z.literal("any"), z.array(z.string())]),
    connection_ids: z.array(z.string()),
  })),
  operations: z.array(z.string()),
  max_concurrent_sessions_limit: z.number(),
  connection: connection.optional(),
  opencode: z.object({
    version: z.string(),
    next_action: z.string().nullable(),
    automatic_routing: z.string(),
  }).optional(),
  ...writeMeta,
});

export const openCodeDiscoveryResult = z.object({
  found: z.boolean(),
  binary: z.string().optional(),
  version: z.string().optional(),
  qualified: z.boolean().optional(),
  qualified_versions: z.union([z.literal("any"), z.array(z.string())]),
  onboarded_connection_ids: z.array(z.string()),
});

const surfaceOutcome = <Result extends z.ZodType>(result: Result) => z.union([
  z.object({ ok: z.literal(true), result }),
  z.object({ ok: z.literal(false), error: configError }),
]);

// One outcome per surface: a surface that cannot be read reports its own
// error without hiding the others.
export const allSurfacesResult = z.object({
  user_defaults: surfaceOutcome(userDefaultsResult),
  routing: surfaceOutcome(routingResult),
  mcp_servers: surfaceOutcome(mcpServersResult),
  connections: surfaceOutcome(connectionsResult),
  project_ceiling: surfaceOutcome(projectCeilingResult).optional(),
});

export const surfaceResults = {
  user_defaults: userDefaultsResult,
  project_ceiling: projectCeilingResult,
  routing: routingResult,
  mcp_servers: mcpServersResult,
  connections: connectionsResult,
} as const;

/** Where a response was bound, in display form only (``~`` for the owner's
 * home, ``<project>`` for the workspace's project), never raw paths. */
export const configBinding = z.object({
  apiVersion: z.literal(CONFIG_API_VERSION),
  configHome: z.string(),
  project: z.string().nullable(),
});

const envelope = <Result extends z.ZodType>(result: Result) => z.union([
  z.object({ ok: z.literal(true), binding: configBinding, result }),
  z.object({ ok: z.literal(false), binding: configBinding.nullable(), error: configError }),
]);

export const configReadOutput = envelope(z.union([
  allSurfacesResult,
  userDefaultsResult,
  projectCeilingResult,
  routingResult,
  mcpServersResult,
  connectionsResult,
]));
export const configUpdateOutput = envelope(z.union([
  userDefaultsResult,
  projectCeilingResult,
  routingResult,
  mcpServersResult,
  connectionsResult,
]));
export const openCodeDiscoverOutput = envelope(openCodeDiscoveryResult);
export const openCodeOnboardOutput = envelope(connectionsResult);

// --- contracts --------------------------------------------------------------

export const readConfig = defineRpc({
  name: "cortex.config.read",
  input: z.strictObject({
    // Binds the project surfaces (project ceiling, project routing file) to
    // this workspace's Cortex project. Without it only owner surfaces read.
    workspaceId: workspaceId.optional(),
    surface: z.enum(["all", ...configSurfaces]),
  }),
  output: configReadOutput,
});

export const updateConfig = defineRpc({
  name: "cortex.config.update",
  input: z.discriminatedUnion("surface", [
    z.strictObject({
      surface: z.literal("user_defaults"),
      expected_revision: revision,
      defaults: defaultsPatch.optional(),
      // An object replaces the user ceiling, null clears it, omitted keeps it.
      ceiling: ceilingInput.nullable().optional(),
    }),
    z.strictObject({
      surface: z.literal("project_ceiling"),
      workspaceId,
      expected_revision: revision,
      // Must narrow the user ceiling; null removes it.
      ceiling: ceilingInput.nullable(),
    }),
    z.strictObject({
      surface: z.literal("routing"),
      target: z.enum(["user", "project"]),
      // Required for the project file; for the user file it lets the result
      // report whether a project file still shadows it.
      workspaceId: workspaceId.optional(),
      expected_revision: revision,
      rules: routingRules,
    }),
    z.strictObject({
      surface: z.literal("mcp_servers"),
      expected_revision: revision,
      operations: z.array(mcpOperation).min(1).max(64),
    }),
    z.strictObject({
      surface: z.literal("connections"),
      expected_revision: revision,
      connection_id: text(32),
      operation: z.enum(["rename", "set_max_concurrent_sessions", "validate", "remove"]),
      name: text(120).optional(),
      max_concurrent_sessions: z.number().int().optional(),
    }),
  ]),
  output: configUpdateOutput,
});

/** Looks for a trusted OpenCode binary in the standard locations only
 * ($CORTEX_OPENCODE_BINARY, ~/.opencode/bin/opencode, PATH). A client never
 * names a binary: that would let a caller pick an executable to run. */
export const discoverOpenCode = defineRpc({
  name: "cortex.config.opencode.discover",
  input: z.strictObject({}),
  output: openCodeDiscoverOutput,
});

/** Registers the optional OpenCode v2 engine. Only an explicit opt-in is
 * accepted; routing is never touched. */
export const onboardOpenCode = defineRpc({
  name: "cortex.config.opencode.onboard",
  input: z.strictObject({
    expected_revision: revision,
    explicit_opt_in: z.literal(true),
    name: text(120).min(1).optional(),
  }),
  output: openCodeOnboardOutput,
});

export type ConfigError = z.infer<typeof configError>;
export type ConfigBinding = z.infer<typeof configBinding>;
export type ConfigReadInput = z.infer<typeof readConfig["input"]>;
export type ConfigUpdateInput = z.infer<typeof updateConfig["input"]>;
export type ConfigReadOutput = z.infer<typeof configReadOutput>;
export type ConfigUpdateOutput = z.infer<typeof configUpdateOutput>;
export type OpenCodeDiscoverOutput = z.infer<typeof openCodeDiscoverOutput>;
export type OpenCodeOnboardOutput = z.infer<typeof openCodeOnboardOutput>;
