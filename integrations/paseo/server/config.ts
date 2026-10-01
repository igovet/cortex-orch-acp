import { existsSync } from "node:fs";
import { homedir } from "node:os";
import { basename, dirname, isAbsolute, join } from "node:path";
import type { PluginHandlerContext } from "@getpaseo/plugin/server";
import {
  CONFIG_API_VERSION,
  REDACTED,
  allSurfacesResult,
  openCodeDiscoveryResult,
  readSurfaceResults,
  type ConfigBinding,
  type ConfigError,
  type ConfigReadSurface,
  type ConfigReadInput,
  type ConfigReadOutput,
  type ConfigSurface,
  type ConfigUpdateInput,
  type ConfigUpdateOutput,
  type McpOperation,
  type CatalogRefreshOutput,
  type OpenCodeDiscoverOutput,
  type OpenCodeOnboardOutput,
} from "../shared/config.js";
import { cortexLaunchTarget } from "./provider.js";
import { runProcess } from "./python-runner.js";
import { resolveProjectRoot } from "./worker-history.js";

/**
 * Plugin RPC handlers over Cortex's owner configuration API
 * (``cortex_runtime.configuration.management``). Each call runs that API's
 * stdin entry point once, with the exact runtime and environment this
 * plugin launches Cortex with, so a setting written here is the setting the
 * next Cortex session reads.
 *
 * Boundaries:
 * - Authentication: Paseo 0.10.0 (installed) dispatches
 *   ``plugin.rpc.invoke.request`` only for a daemon session holding
 *   ``daemon.manage``, and validates input/output against these contracts.
 * - Owner: the Cortex launch record's HOME/XDG overrides select the
 *   configuration home; a client never names it.
 * - Project: resolved from the Paseo workspace's directory through the same
 *   logical-root resolution the store readers use; a client never sends a
 *   path, and no request can name a binary or command to run.
 * - Writes need the ``expected_revision`` a read returned; the API compares it
 *   under its lock and refuses a stale write with ``revision_conflict``.
 *   Writes from this plugin process are additionally serialized.
 * - Responses never carry an MCP env value (the API redacts them), and this
 *   module also redacts credential-looking command/argument parts and shows
 *   paths only as ``~``/``<project>`` display forms. Error text is scrubbed
 *   the same way. Handlers never throw raw errors to the host.
 */

type LaunchTarget = { argv: string[]; env?: Record<string, string> };

type ConfigRuntime = {
  interpreter: string;
  srcDir: string;
  env: NodeJS.ProcessEnv;
  home: string;
  configHome: string;
};

type Scope = { home: string; projectRoot: string | null };

type Failure = { failure: ConfigError };

const PYTHON_INTERPRETER = /^python3(?:\.\d{1,2})?$/;
const READ_TIMEOUT_MS = 15000;
// Below Paseo's 30 s plugin request timeout, so a slow write still returns a
// structured error instead of a host timeout.
const WRITE_TIMEOUT_MS = 25000;
// Every connection's engine is started in parallel, each bounded at 20 s.
const REFRESH_TIMEOUT_MS = 60000;
const MAX_OUTPUT_BYTES = 8 * 1024 * 1024;

const BOOT = [
  "import sys",
  "sys.path.insert(0, sys.argv[1])",
  "from cortex_runtime.configuration.management import main",
  "raise SystemExit(main())",
].join("\n");

function failure(code: string, message: string, field: string | null = null): Failure {
  return { failure: { code, message, field } };
}

function isFailure(value: unknown): value is Failure {
  return typeof value === "object" && value !== null && "failure" in value;
}

/** The Cortex runtime and owner environment this plugin launches. */
export function configRuntime(target: LaunchTarget | undefined = cortexLaunchTarget()): ConfigRuntime | Failure {
  if (target === undefined) {
    return failure("config_api_unavailable", "No Cortex launch is configured for this Paseo daemon; run the Cortex Paseo setup first.");
  }
  const [interpreter, ...rest] = target.argv;
  if (!interpreter || !PYTHON_INTERPRETER.test(basename(interpreter)) || (interpreter.includes("/") && !isAbsolute(interpreter))) {
    return failure("config_api_unavailable", "The configured Cortex launch is not a Python entry point this settings API can use.");
  }
  const entry = rest.find((part) => isAbsolute(part) && basename(part) === "cortex_acp.py" && !part.includes("\0"));
  if (entry === undefined) {
    return failure("config_api_unavailable", "The configured Cortex launch names no Cortex entry point.");
  }
  const srcDir = dirname(entry);
  if (!existsSync(join(srcDir, "cortex_runtime", "configuration", "management.py"))) {
    return failure("config_api_unavailable", "The installed Cortex runtime predates the configuration API; update Cortex.");
  }
  const env: NodeJS.ProcessEnv = { ...process.env, ...(target.env ?? {}) };
  const home = env.HOME && isAbsolute(env.HOME) ? env.HOME : homedir();
  const configBase = env.XDG_CONFIG_HOME && isAbsolute(env.XDG_CONFIG_HOME) ? env.XDG_CONFIG_HOME : join(home, ".config");
  return { interpreter, srcDir, env, home, configHome: join(configBase, "cortex") };
}

// --- display and redaction ------------------------------------------------------

const PRIVATE_PATH = /^\/(?:home|tmp|private|Users|root|var\/folders|run\/user)(?:\/|$)/;

function within(value: string, root: string): boolean {
  return value === root || value.startsWith(root.endsWith("/") ? root : `${root}/`);
}

/** A path as the settings UI may show it: never the owner's home or a
 * private absolute location spelled out. */
export function displayPath(value: string, scope: Scope): string {
  if (scope.projectRoot && within(value, scope.projectRoot)) return `<project>${value.slice(scope.projectRoot.length)}`;
  if (scope.home && scope.home !== "/" && within(value, scope.home)) return `~${value.slice(scope.home.length)}`;
  if (PRIVATE_PATH.test(value)) return "[private path]";
  return value;
}

const CREDENTIAL_ASSIGNMENT = /((?:bearer|token|password|passwd|secret|api[_ -]?key|access[_ -]?key|auth(?:orization)?|credentials?)[A-Za-z0-9_-]*\s*[:=]\s*(?:bearer\s+)?)([^\s,;&"']+)/gi;
const BEARER = /\b(bearer\s+)([^\s,;"']+)/gi;
const URL_USERINFO = /(\b[a-z][a-z0-9+.-]*:\/\/[^\s:@/]+:)([^\s@/]+)(@)/gi;
const TOKEN_SHAPE = /\b(?:sk|pk|rk)-[A-Za-z0-9_-]{16,}|\bgh[pousr]_[A-Za-z0-9]{20,}|\bxox[abprs]-[A-Za-z0-9-]{10,}|\bAKIA[0-9A-Z]{16}\b/g;
const SECRET_FLAG = /^--?[A-Za-z0-9_-]*(?:token|secret|password|passwd|api[-_]?key|access[-_]?key|auth|credential)[A-Za-z0-9_-]*$/i;

/** ``value`` with every credential-looking part replaced by the marker. */
export function redactCredentials(value: string): string {
  return value
    .replace(URL_USERINFO, (_match, head: string, _secret: string, at: string) => `${head}${REDACTED}${at}`)
    .replace(CREDENTIAL_ASSIGNMENT, (_match, head: string) => `${head}${REDACTED}`)
    .replace(BEARER, (match, head: string, secret: string) => (secret === REDACTED ? match : `${head}${REDACTED}`))
    .replace(TOKEN_SHAPE, REDACTED);
}

/** An argv with credential parts redacted; a value following a
 * credential-named flag (``--token VALUE``) is redacted whole. */
export function redactArgv(argv: readonly unknown[]): unknown[] {
  return argv.map((part, index) => {
    if (typeof part !== "string") return part;
    const previous = index > 0 ? argv[index - 1] : undefined;
    if (typeof previous === "string" && SECRET_FLAG.test(previous) && !part.startsWith("-") && part.length > 0) return REDACTED;
    return redactCredentials(part);
  });
}

/** Free text (an error or a loader verdict) safe to show: credential parts
 * redacted and absolute paths in display form. */
export function scrubText(value: string, scope: Scope): string {
  return redactCredentials(value).replace(/(?<![A-Za-z0-9_.~-])\/[^\s,;:'"()<>]*/g, (path) => displayPath(path, scope));
}

function convertError(raw: unknown, scope: Scope): ConfigError {
  const error = typeof raw === "object" && raw !== null ? raw as Record<string, unknown> : {};
  const converted: ConfigError = {
    code: typeof error.code === "string" ? error.code : "configuration_error",
    message: scrubText(typeof error.message === "string" ? error.message : "The configuration request was refused.", scope),
    field: typeof error.field === "string" ? error.field : null,
  };
  if (Array.isArray(error.errors)) {
    converted.errors = error.errors.filter((item): item is Record<string, unknown> => typeof item === "object" && item !== null).map((item) => ({
      field: String(item.field ?? ""),
      code: String(item.code ?? "invalid_value"),
      message: scrubText(String(item.message ?? ""), scope),
    }));
  }
  if (typeof error.current_revision === "string") converted.currentRevision = error.current_revision;
  return converted;
}

function record(value: unknown): Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value) ? value as Record<string, unknown> : {};
}

function displayField(target: Record<string, unknown>, key: string, scope: Scope): void {
  if (typeof target[key] === "string") target[key] = displayPath(target[key] as string, scope);
}

function sanitizeMcpServer(raw: unknown, scope: Scope): Record<string, unknown> {
  const server = { ...record(raw) };
  let redacted = false;
  if (server.config !== null && typeof server.config === "object") {
    const config = { ...record(server.config) };
    // Defence in depth: the API already returns only names and whether set.
    if ("env" in config) {
      config.env = Array.isArray(config.env)
        ? config.env.map((item) => ({ name: typeof record(item).name === "string" ? record(item).name : null, value_set: record(item).value_set === true }))
        : null;
    }
    if (typeof config.command === "string") {
      const command = redactCredentials(config.command);
      redacted ||= command !== config.command;
      config.command = command;
    }
    if (Array.isArray(config.args)) {
      const args = redactArgv(config.args);
      redacted ||= args.some((part, index) => part !== (config.args as unknown[])[index]);
      config.args = args;
    }
    server.config = config;
  }
  const problem = server.problem;
  if (problem !== null && typeof problem === "object") {
    const verdict = { ...record(problem) };
    if (typeof verdict.message === "string") verdict.message = scrubText(verdict.message, scope);
    server.problem = verdict;
  }
  server.redacted = redacted;
  return server;
}

function sanitizeConnection(raw: unknown, scope: Scope): Record<string, unknown> {
  const connection = { ...record(raw) };
  if (Array.isArray(connection.launch_command)) {
    connection.launch_command = redactArgv(connection.launch_command)
      .map((part) => (typeof part === "string" && isAbsolute(part) ? displayPath(part, scope) : part));
  }
  if (typeof connection.endpoint === "string") connection.endpoint = redactCredentials(connection.endpoint);
  return connection;
}

/** Profile problems scrubbed; and, as defence in depth (the API refuses
 * such a profile), never an MCP command, argument or env value from a
 * stored profile. */
function sanitizeProfiles(result: Record<string, unknown>, scope: Scope): void {
  if (Array.isArray(result.problems)) {
    result.problems = result.problems.map((item) => {
      const problem = { ...record(item) };
      if (typeof problem.message === "string") problem.message = scrubText(problem.message, scope);
      return problem;
    });
  }
  if (Array.isArray(result.profiles)) {
    result.profiles = result.profiles.map((item) => {
      const profile = { ...record(item) };
      const settings = { ...record(profile.settings) };
      if (settings.mcp_servers !== undefined) {
        settings.mcp_servers = Object.fromEntries(Object.entries(record(settings.mcp_servers)).map(([name, policy]) => {
          const { command: _command, args: _args, env: _env, ...kept } = record(policy);
          return [name, kept];
        }));
      }
      profile.settings = settings;
      return profile;
    });
  }
}

/** A surface result as the settings UI may receive it. Editable values
 * (defaults, ceilings, rules, MCP tool/path grants) stay verbatim so they
 * round-trip; only metadata is put in display form. */
export function sanitizeSurface(surface: ConfigReadSurface, raw: unknown, scope: Scope): Record<string, unknown> {
  const result = { ...record(raw) };
  displayField(result, "path", scope);
  displayField(result, "project_root", scope);
  if (surface === "user_defaults" && result.coordinator_resolution !== undefined) {
    const resolution = { ...record(result.coordinator_resolution) };
    if (resolution.error !== undefined) resolution.error = convertError(resolution.error, scope);
    result.coordinator_resolution = resolution;
  }
  if (surface === "routing" && Array.isArray(result.sources)) {
    result.sources = result.sources.map((item) => {
      const source = { ...record(item) };
      displayField(source, "path", scope);
      if (source.error !== undefined) {
        const error = { ...record(source.error) };
        if (typeof error.message === "string") error.message = scrubText(error.message, scope);
        source.error = error;
      }
      return source;
    });
  }
  if (surface === "routing_profiles") sanitizeProfiles(result, scope);
  if (surface === "mcp_servers" && Array.isArray(result.servers)) {
    result.servers = result.servers.map((server) => sanitizeMcpServer(server, scope));
  }
  if (surface === "route_catalog" && Array.isArray(result.catalog_problems)) {
    result.catalog_problems = result.catalog_problems.map((item) => {
      const problem = { ...record(item) };
      if (typeof problem.message === "string") problem.message = scrubText(problem.message, scope);
      return problem;
    });
  }
  if (surface === "connections") {
    if (Array.isArray(result.connections)) result.connections = result.connections.map((item) => sanitizeConnection(item, scope));
    if (result.connection !== undefined) result.connection = sanitizeConnection(result.connection, scope);
  }
  return result;
}

// --- API transport ------------------------------------------------------------

type ApiOutcome = { ok: true; result: unknown } | { ok: false; error: unknown };

async function callApi(runtime: ConfigRuntime, request: Record<string, unknown>, timeoutMs: number): Promise<ApiOutcome | Failure> {
  const run = await runProcess(runtime.interpreter, ["-B", "-c", BOOT, runtime.srcDir], {
    timeoutMs,
    maxBuffer: MAX_OUTPUT_BYTES,
    input: JSON.stringify({ api_version: CONFIG_API_VERSION, ...request }),
    env: runtime.env,
    cwd: runtime.srcDir,
  });
  let response: unknown;
  try {
    response = JSON.parse(run.stdout);
  } catch {
    return failure(
      "config_api_failed",
      "The Cortex configuration API did not answer. If this was a change, re-read the settings before retrying: it may or may not have been applied.",
    );
  }
  const body = record(response);
  if (body.api_version !== CONFIG_API_VERSION) {
    return failure("unsupported_api_version", `The installed Cortex answers a different configuration API version; this plugin speaks version ${CONFIG_API_VERSION}.`);
  }
  if (body.ok === true && "result" in body) return { ok: true, result: body.result };
  if (body.ok === false) return { ok: false, error: body.error };
  return failure("invalid_response", "The Cortex configuration API returned an unexpected response.");
}

async function resolveWorkspaceProject(workspaceId: string, context: PluginHandlerContext): Promise<string | Failure> {
  try {
    const handle = context.paseo.workspaces.ref(workspaceId);
    await handle.refresh();
    const directory = handle.directory;
    if (!directory || !directory.startsWith("/") || directory.includes("\0")) {
      return failure("workspace_unavailable", "This Paseo workspace has no local project directory.", "workspaceId");
    }
    const root = await resolveProjectRoot(directory);
    return root ?? failure("workspace_unavailable", "This workspace's project root could not be resolved safely.", "workspaceId");
  } catch {
    return failure("workspace_unavailable", "This Paseo workspace could not be read.", "workspaceId");
  }
}

function binding(runtime: ConfigRuntime, scope: Scope): ConfigBinding {
  return {
    apiVersion: CONFIG_API_VERSION,
    configHome: displayPath(runtime.configHome, scope),
    project: scope.projectRoot ? "<project>" : null,
  };
}

let writeChain: Promise<unknown> = Promise.resolve();

/** One configuration write at a time from this plugin process; the API's
 * own lock and revision check still guard against every other writer. */
function serializeWrite<T>(task: () => Promise<T>): Promise<T> {
  const run = writeChain.then(task, task);
  writeChain = run.catch(() => undefined);
  return run;
}

type Prepared = { runtime: ConfigRuntime; scope: Scope; bound: ConfigBinding };

async function prepare(workspaceId: string | undefined, context: PluginHandlerContext): Promise<Prepared | { refusal: ConfigError; bound: ConfigBinding | null }> {
  const runtime = configRuntime();
  if (isFailure(runtime)) return { refusal: runtime.failure, bound: null };
  let projectRoot: string | null = null;
  if (workspaceId !== undefined) {
    const resolved = await resolveWorkspaceProject(workspaceId, context);
    if (isFailure(resolved)) return { refusal: resolved.failure, bound: binding(runtime, { home: runtime.home, projectRoot: null }) };
    projectRoot = resolved;
  }
  const scope = { home: runtime.home, projectRoot };
  return { runtime, scope, bound: binding(runtime, scope) };
}

function refused(bound: ConfigBinding | null, error: ConfigError) {
  return { ok: false as const, binding: bound, error };
}

/** Parse one sanitized surface result against its contract, or explain. */
function typedSurface(surface: ConfigReadSurface, raw: unknown, scope: Scope): { result: unknown } | Failure {
  const parsed = readSurfaceResults[surface].safeParse(sanitizeSurface(surface, raw, scope));
  return parsed.success ? { result: parsed.data } : failure("invalid_response", `The Cortex configuration API returned an unexpected ${surface} result.`);
}

function guarded<Input, Output>(handler: (input: Input, context: PluginHandlerContext) => Promise<Output>, fallback: () => Output) {
  return async (input: Input, context: PluginHandlerContext): Promise<Output> => {
    try {
      return await handler(input, context);
    } catch {
      // Never hand the host a raw exception message (it would reach the client).
      return fallback();
    }
  };
}

const internalError = () => refused(null, { code: "internal_error", message: "The configuration request failed.", field: null });

// --- handlers -------------------------------------------------------------------

export const readConfigHandler = guarded(async (input: ConfigReadInput, context: PluginHandlerContext): Promise<ConfigReadOutput> => {
  if (input.surface === "project_ceiling" && input.workspaceId === undefined) {
    return refused(null, { code: "workspace_required", message: "The project ceiling belongs to a workspace; open this from a workspace.", field: "workspaceId" });
  }
  const prepared = await prepare(input.workspaceId, context);
  if ("refusal" in prepared) return refused(prepared.bound, prepared.refusal);
  const { runtime, scope, bound } = prepared;
  const projectScoped = input.surface === "all" || input.surface === "project_ceiling" || input.surface === "routing" || input.surface === "routing_profiles"
    || input.surface === "route_catalog";
  const outcome = await callApi(runtime, {
    action: "read",
    surface: input.surface,
    ...(projectScoped && scope.projectRoot ? { project: scope.projectRoot } : {}),
  }, READ_TIMEOUT_MS);
  if (isFailure(outcome)) return refused(bound, outcome.failure);
  if (!outcome.ok) return refused(bound, convertError(outcome.error, scope));
  if (input.surface !== "all") {
    const typed = typedSurface(input.surface, outcome.result, scope);
    return isFailure(typed) ? refused(bound, typed.failure) : { ok: true, binding: bound, result: typed.result as never };
  }
  const all: Record<string, unknown> = {};
  for (const [surface, entry] of Object.entries(record(outcome.result))) {
    if (!(surface in readSurfaceResults) || surface === "route_catalog") continue;
    const item = record(entry);
    if (item.ok === true) {
      const typed = typedSurface(surface as ConfigSurface, item.result, scope);
      all[surface] = isFailure(typed) ? { ok: false, error: typed.failure } : { ok: true, result: typed.result };
    } else {
      all[surface] = { ok: false, error: convertError(item.error, scope) };
    }
  }
  const parsed = allSurfacesResult.safeParse(all);
  if (!parsed.success) return refused(bound, failure("invalid_response", "The Cortex configuration API returned an unexpected result.").failure);
  return { ok: true, binding: bound, result: parsed.data };
}, internalError);

/** Put the stored value back wherever an MCP upsert still carries the
 * redaction marker for it; a marker that no longer matches is refused. */
async function restoreRedacted(
  runtime: ConfigRuntime, scope: Scope, expected: string, operations: McpOperation[],
): Promise<McpOperation[] | Failure> {
  const needs = operations.some((operation) => operation.op === "upsert"
    && (operation.server.command.includes(REDACTED) || (operation.server.args ?? []).some((part) => part.includes(REDACTED))));
  if (!needs) return operations;
  const outcome = await callApi(runtime, { action: "read", surface: "mcp_servers" }, READ_TIMEOUT_MS);
  if (isFailure(outcome)) return outcome;
  if (!outcome.ok) return { failure: convertError(outcome.error, scope) };
  const current = record(outcome.result);
  if (current.revision !== expected) {
    return {
      failure: {
        code: "revision_conflict",
        message: "The mcp_servers configuration changed after it was read; nothing was written. Re-read it and apply the change again.",
        field: "expected_revision",
        ...(typeof current.revision === "string" ? { currentRevision: current.revision } : {}),
      },
    };
  }
  const stored = new Map<string, Record<string, unknown>>();
  for (const server of Array.isArray(current.servers) ? current.servers : []) {
    const item = record(server);
    if (typeof item.name === "string") stored.set(item.name, record(item.config));
  }
  const errors: { field: string; code: string; message: string }[] = [];
  const restored = operations.map((operation, index) => {
    if (operation.op !== "upsert") return operation;
    const previous = stored.get(operation.name) ?? {};
    const server = { ...operation.server };
    const field = `operations[${index}].server`;
    if (server.command.includes(REDACTED)) {
      const original = typeof previous.command === "string" ? previous.command : undefined;
      if (original !== undefined && redactCredentials(original) === server.command) server.command = original;
      else errors.push({ field: `${field}.command`, code: "redacted_value_changed", message: `still contains ${REDACTED} but does not match the stored value; enter the full value` });
    }
    if (server.args?.some((part) => part.includes(REDACTED))) {
      const originals = Array.isArray(previous.args) ? previous.args : [];
      const shown = redactArgv(originals);
      server.args = server.args.map((part, position) => {
        if (!part.includes(REDACTED)) return part;
        if (typeof originals[position] === "string" && shown[position] === part) return originals[position] as string;
        errors.push({ field: `${field}.args[${position}]`, code: "redacted_value_changed", message: `still contains ${REDACTED} but does not match the stored value; enter the full value` });
        return part;
      });
    }
    return { ...operation, server };
  });
  if (errors.length) {
    return { failure: { code: "invalid_configuration", message: "Correct the invalid setting(s); nothing was written.", field: errors[0].field, errors } };
  }
  return restored;
}

export const updateConfigHandler = guarded(async (input: ConfigUpdateInput, context: PluginHandlerContext): Promise<ConfigUpdateOutput> => {
  const workspaceId = "workspaceId" in input ? input.workspaceId : undefined;
  if (input.surface === "routing" && input.target === "project" && workspaceId === undefined) {
    return refused(null, { code: "workspace_required", message: "The project routing file belongs to a workspace; open this from a workspace.", field: "workspaceId" });
  }
  const prepared = await prepare(workspaceId, context);
  if ("refusal" in prepared) return refused(prepared.bound, prepared.refusal);
  const { runtime, scope, bound } = prepared;
  return serializeWrite(async () => {
    let request: Record<string, unknown>;
    switch (input.surface) {
      case "user_defaults":
        request = { expected_revision: input.expected_revision, defaults: input.defaults ?? null, ...("ceiling" in input && input.ceiling !== undefined ? { ceiling: input.ceiling } : {}) };
        break;
      case "project_ceiling":
        request = { project: scope.projectRoot, expected_revision: input.expected_revision, ceiling: input.ceiling };
        break;
      case "routing":
        request = { target: input.target, expected_revision: input.expected_revision, rules: input.rules, ...(scope.projectRoot ? { project: scope.projectRoot } : {}) };
        break;
      case "routing_profiles":
        request = {
          action: input.action, expected_revision: input.expected_revision,
          ...("name" in input ? { name: input.name } : {}),
          ...("description" in input && input.description !== undefined ? { description: input.description } : {}),
          ...("rename_to" in input && input.rename_to !== undefined ? { rename_to: input.rename_to } : {}),
          ...("expected_revisions" in input ? { expected_revisions: input.expected_revisions } : {}),
          ...("discard_modifications" in input && input.discard_modifications === true ? { discard_modifications: true } : {}),
          ...(scope.projectRoot ? { project: scope.projectRoot } : {}),
        };
        break;
      case "mcp_servers": {
        const operations = await restoreRedacted(runtime, scope, input.expected_revision, input.operations);
        if (isFailure(operations)) return refused(bound, operations.failure);
        request = { expected_revision: input.expected_revision, operations };
        break;
      }
      case "connections":
        request = {
          expected_revision: input.expected_revision, connection_id: input.connection_id, operation: input.operation,
          ...(input.name === undefined ? {} : { name: input.name }),
          ...(input.max_concurrent_sessions === undefined ? {} : { max_concurrent_sessions: input.max_concurrent_sessions }),
        };
        break;
    }
    const outcome = await callApi(runtime, { action: input.surface === "routing_profiles" ? input.action : "update", surface: input.surface, ...request }, WRITE_TIMEOUT_MS);
    if (isFailure(outcome)) return refused(bound, outcome.failure);
    if (!outcome.ok) return refused(bound, convertError(outcome.error, scope));
    const typed = typedSurface(input.surface, outcome.result, scope);
    return isFailure(typed) ? refused(bound, typed.failure) : { ok: true, binding: bound, result: typed.result as never };
  });
}, internalError);

export const discoverOpenCodeHandler = guarded(async (_input: Record<string, never>, context: PluginHandlerContext): Promise<OpenCodeDiscoverOutput> => {
  const prepared = await prepare(undefined, context);
  if ("refusal" in prepared) return refused(prepared.bound, prepared.refusal);
  const { runtime, scope, bound } = prepared;
  const outcome = await callApi(runtime, { action: "discover_opencode", surface: "connections" }, WRITE_TIMEOUT_MS);
  if (isFailure(outcome)) return refused(bound, outcome.failure);
  if (!outcome.ok) return refused(bound, convertError(outcome.error, scope));
  const result = { ...record(outcome.result) };
  displayField(result, "binary", scope);
  const parsed = openCodeDiscoveryResult.safeParse(result);
  return parsed.success
    ? { ok: true, binding: bound, result: parsed.data }
    : refused(bound, failure("invalid_response", "The Cortex configuration API returned an unexpected discovery result.").failure);
}, internalError);

export const refreshModelCatalogHandler = guarded(async (
  input: { workspaceId?: string }, context: PluginHandlerContext,
): Promise<CatalogRefreshOutput> => {
  const prepared = await prepare(input.workspaceId, context);
  if ("refusal" in prepared) return refused(prepared.bound, prepared.refusal);
  const { runtime, scope, bound } = prepared;
  return serializeWrite(async () => {
    const outcome = await callApi(runtime, {
      action: "refresh", surface: "route_catalog", ...(scope.projectRoot ? { project: scope.projectRoot } : {}),
    }, REFRESH_TIMEOUT_MS);
    if (isFailure(outcome)) return refused(bound, outcome.failure);
    if (!outcome.ok) return refused(bound, convertError(outcome.error, scope));
    const typed = typedSurface("route_catalog", outcome.result, scope);
    return isFailure(typed) ? refused(bound, typed.failure) : { ok: true, binding: bound, result: typed.result as never };
  });
}, internalError);

export const onboardOpenCodeHandler = guarded(async (
  input: { expected_revision: string; explicit_opt_in: true; name?: string }, context: PluginHandlerContext,
): Promise<OpenCodeOnboardOutput> => {
  const prepared = await prepare(undefined, context);
  if ("refusal" in prepared) return refused(prepared.bound, prepared.refusal);
  const { runtime, scope, bound } = prepared;
  return serializeWrite(async () => {
    const outcome = await callApi(runtime, {
      action: "onboard_opencode", surface: "connections", expected_revision: input.expected_revision,
      explicit_opt_in: input.explicit_opt_in === true, ...(input.name === undefined ? {} : { name: input.name }),
    }, WRITE_TIMEOUT_MS);
    if (isFailure(outcome)) return refused(bound, outcome.failure);
    if (!outcome.ok) return refused(bound, convertError(outcome.error, scope));
    const typed = typedSurface("connections", outcome.result, scope);
    return isFailure(typed) ? refused(bound, typed.failure) : { ok: true, binding: bound, result: typed.result as never };
  });
}, internalError);
