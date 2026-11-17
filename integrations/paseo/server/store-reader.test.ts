import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { chmodSync, copyFileSync, existsSync, readFileSync, mkdirSync, mkdtempSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { loadServerModuleForTest } from "./test-bundle.ts";
import { execSqlite, sqlValues } from "./test-fixture-db.ts";

// The Rust launch serves every store read with cortex-paseo-tools; these
// reads run with PATH emptied, so no other program can be started.
// Build the tool first: cargo build --locked --offline -p cortex-paseo-tools

const pluginRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const repoRoot = resolve(pluginRoot, "../..");
const toolsBinary = process.env.CORTEX_PASEO_TOOLS_TEST_BINARY?.trim() || join(repoRoot, "target", "debug", "cortex-paseo-tools");
assert.ok(existsSync(toolsBinary), `cortex-paseo-tools is not built at ${toolsBinary}; run cargo build -p cortex-paseo-tools`);

const SESSION_ID = "acps_0123456789ab";
const WORKSPACE_ID = "workspace-1";
const AGENT_ID = "agent-1";
const TASK_ID = "t_rust00000001";
const COMMAND_REF = "gateway:0123456789abcdef0123456789abcdef";
const NOW = "2026-11-15T00:00:00+00:00";

/** A real format-12 store: cortex-core's own schema, header and access lock
 * (fixture setup only). */
function makeStore(root: string, store = ".cortex"): void {
  const directory = join(root, store);
  mkdirSync(directory, { mode: 0o700 });
  writeFileSync(join(directory, ".access.lock"), "", { mode: 0o600 });
  const rows = `
    insert into tasks (id, project_root, created_at) values ${sqlValues([TASK_ID, root, NOW])};
    insert into acp_host_sessions (session_id, project_root, task_id, client_info, client_capabilities, mcp_servers, created_at)
      values ${sqlValues([SESSION_ID, root, TASK_ID, "{}", "{}", "[]", NOW])};
    insert into work_assignments (assignment_id, task_id, node_key, requirement_bindings_json, status, idempotency_key, created_at)
      values ${sqlValues(["wa_1", TASK_ID, "n1", "[]", "running", "ref-1", NOW])};
    insert into work_assignment_attempts (attempt_id, task_id, assignment_id, attempt_no, requirement_bindings_json, status, idempotency_key, created_at)
      values ${sqlValues(["att_1", TASK_ID, "wa_1", 1, "[]", "running", "idem-att_1", NOW])};
    insert into task_events (task_id, event_type, assignment_ref, dedup_key, payload_json, origin, created_at)
      values ${sqlValues([TASK_ID, "delegation_requested", "ref-1", "k1", JSON.stringify({ title: "Fix the bug", role: "worker-fullstack-dev", brief: "Fix it", acceptance: "Tests pass", work_attempt_ref: "att_1" }), "test", NOW])};
    insert into task_events (task_id, event_type, assignment_ref, dedup_key, payload_json, origin, created_at)
      values ${sqlValues([TASK_ID, "worker_result", "xa_exec1", "k2", JSON.stringify({ summary: "Done; token=abc", checks: [{ name: "unit", outcome: "passed" }] }), "test", NOW])};
    insert into delegation_provisioning (idempotency_key, task_id, attempt_id, role_id, step, status, channel_id, owner_epoch, created_at, updated_at)
      values ${sqlValues(["ref-1", TASK_ID, "xa_exec1", "worker", "ready", "ready", "ch_1", 1, NOW, NOW])};
    insert into runtime_tasks (task_id, state, revision, goal, created_at, updated_at) values ${sqlValues([TASK_ID, "EXECUTING", 1, "goal", NOW, NOW])};
    insert into runtime_nodes (task_id, revision, node_id, role, objective, dependencies_json, scope_json, stage, status, assignment_ref)
      values ${sqlValues([TASK_ID, 1, "n1", "worker-fullstack-dev", "Fix the bug", "[]", "[]", "execution", "running", "ref-1"])};
    insert into task_owners (task_id, owner_ref, epoch, acquired_at) values ${sqlValues([TASK_ID, "o", 1, NOW])};
    insert into trusted_channels (channel_id, secret_hash, task_id, principal_thread_id, role, attempt_id, owner_epoch, issued_at)
      values ${sqlValues(["ch_1", "h", TASK_ID, "p", "worker", "xa_exec1", 1, NOW])};
    insert into runtime_events (task_id, type, actor, payload_json, created_at)
      values ${sqlValues([TASK_ID, "COMMAND_PROCESS_STARTED", "ch_1", JSON.stringify({ tool_call_ref: COMMAND_REF, argv: ["npm", "test"] }), NOW])};
  `;
  const dbPath = join(directory, "cortex.sqlite3");
  const schema = readFileSync(join(repoRoot, "crates", "cortex-core", "src", "store", "schema.sql"), "utf8");
  execSqlite(dbPath, schema + "\npragma application_id=1129467218;\npragma user_version=12;\n" + rows);
  chmodSync(dbPath, 0o600);
}

function mockContext(directory: string): any {
  const runtimeInfo = { sessionId: `plugin:${JSON.stringify({ version: 1, data: { sessionId: SESSION_ID } })}` };
  return {
    paseo: {
      workspaces: { ref: (id: string) => ({ refresh: async () => {}, directory: id === WORKSPACE_ID ? directory : null }) },
      agents: { ref: (id: string) => ({ refresh: async () => {}, current: () => id === AGENT_ID ? { workspaceId: WORKSPACE_ID, runtimeInfo } : null }) },
    },
  };
}

const LAUNCH_ENV = ["HOME", "XDG_CONFIG_HOME", "XDG_STATE_HOME", "CORTEX_ROUTING", "PATH", "CORTEX_PASEO_SOURCE_ROOT", "CORTEX_ACP_COMMAND", "CORTEX_PASEO_SHIM", "CORTEX_ACP_RUST_BINARY", "CORTEX_ACP_RUNTIME_ROOT", "CORTEX_WORKSPACE_CHANNEL"];
const saved = new Map(LAUNCH_ENV.map((key) => [key, process.env[key]] as const));
const isolated = realpathSync(mkdtempSync(join(tmpdir(), "cortex-store-reader-")));
try {
  const project = join(isolated, "project");
  mkdirSync(project);
  makeStore(project);
  const bin = join(isolated, "bin");
  mkdirSync(bin);
  // Only the file name and location select the Rust route; nothing runs it.
  writeFileSync(join(bin, "cortex-acp"), "#!/bin/sh\nexit 9\n", { mode: 0o700 });
  copyFileSync(toolsBinary, join(bin, "cortex-paseo-tools"));
  for (const key of LAUNCH_ENV) delete process.env[key];
  process.env.HOME = isolated;
  process.env.XDG_CONFIG_HOME = join(isolated, "config");
  process.env.XDG_STATE_HOME = join(isolated, "state");
  process.env.CORTEX_PASEO_SOURCE_ROOT = repoRoot;
  process.env.CORTEX_ACP_RUST_BINARY = join(bin, "cortex-acp");
  const host = JSON.parse(execFileSync(join(bin, "cortex-paseo-tools"), ["config", "--host"], { encoding: "utf8" }));
  assert.ok(["dev", "release"].includes(host.channel));
  const dev = host.channel === "dev";
  const ownerConfig = join(isolated, "config", ...(dev ? ["cortex-workspace-dev"] : []), "cortex");
  const otherConfig = join(isolated, "config", ...(dev ? [] : ["cortex-workspace-dev"]), "cortex");
  assert.equal(host.config_home, ownerConfig);
  assert.equal(host.store_name, dev ? ".cortex-dev" : ".cortex");
  if (dev) makeStore(project, ".cortex-dev");
  mkdirSync(otherConfig, { recursive: true, mode: 0o700 });
  writeFileSync(join(otherConfig, "defaults.json"), "other channel must not be read or changed", { mode: 0o600 });
  // Legacy store readers use release; native Settings still follows its build.
  process.env.CORTEX_WORKSPACE_CHANNEL = "release";
  // No program can be found on PATH from here on.
  process.env.PATH = "";

  const routes = await loadServerModuleForTest<typeof import("./store-reader.ts")>(join(pluginRoot, "server", "store-reader.ts"));
  assert.deepEqual(routes.storeReaderRoute(), { kind: "rust", binary: join(bin, "cortex-paseo-tools") });
  // Only the exact Rust launch shape reads; every other shape is refused (there is no other entry).
  for (const argv of [["python3", "-B", "/x/src/cortex_acp.py"], ["/opt/cortex/cortex-acp", "--flag"], ["cortex-acp"], ["/opt/cortex/other"]]) {
    assert.deepEqual(routes.storeReaderRoute({ argv }), { kind: "refused", message: routes.RUST_ENTRY_REQUIRED }, JSON.stringify(argv));
  }

  const history = await loadServerModuleForTest<typeof import("./worker-history.ts")>(join(pluginRoot, "server", "worker-history.ts"));
  const context = mockContext(project);
  const resolved = await history.resolveAgentTask({ workspaceId: WORKSPACE_ID, agentId: AGENT_ID }, context);
  assert.deepEqual(resolved, { projectRoot: project, taskId: TASK_ID });
  const workers = await history.getWorkerHistory({ workspaceId: WORKSPACE_ID, agentId: AGENT_ID }, context);
  assert.equal(workers.available, true, JSON.stringify(workers));
  assert.equal(workers.assignments.length, 1);
  assert.equal(workers.assignments[0].title, "Fix the bug");
  assert.equal(workers.assignments[0].role, "fullstack-dev");
  // The runtime node still reads "running" and outranks the worker_result.
  assert.equal(workers.assignments[0].attempts[0].status, "running");
  assert.equal(workers.assignments[0].attempts[0].summary, "Done; [redacted]");

  const results = await loadServerModuleForTest<typeof import("./worker-results.ts")>(join(pluginRoot, "server", "worker-results.ts"));
  const read = await results.getWorkerResults({ workspaceId: WORKSPACE_ID, agentId: AGENT_ID, assignmentRefs: ["ref-1", "ref-9"] }, context);
  assert.deepEqual(read, { available: true, results: { "ref-1": { summary: "Done; [redacted]", checks: [{ name: "unit", outcome: "passed" }] }, "ref-9": null } });

  const stop = await loadServerModuleForTest<typeof import("./command-stop.ts")>(join(pluginRoot, "server", "command-stop.ts"));
  const running = await stop.getRunningCommands({ workspaceId: WORKSPACE_ID, agentId: AGENT_ID }, context);
  assert.equal(running.available, true, JSON.stringify(running));
  assert.deepEqual(running.commands.map((command) => [command.toolCallRef, command.assignmentRef, command.argvSummary]), [[COMMAND_REF, "ref-1", "npm test"]]);

  const pipeline = await loadServerModuleForTest<typeof import("./pipeline.ts")>(join(pluginRoot, "server", "pipeline.ts"));
  const plan = await pipeline.getPipeline({ workspaceId: WORKSPACE_ID, agentId: AGENT_ID }, context);
  assert.equal(plan.available, true, JSON.stringify(plan));
  assert.equal(plan.plan?.nodes[0]?.id, "n1");
  assert.equal(plan.workers.length, 1);

  // Settings: cortex-paseo-tools answers the settings API.
  const config = await loadServerModuleForTest<typeof import("./config.ts")>(join(pluginRoot, "server", "config.ts"));
  const runtime = config.configRuntime({ argv: [join(bin, "cortex-acp")] });
  assert.ok(!("failure" in runtime) && runtime.interpreter === join(bin, "cortex-paseo-tools"), JSON.stringify(runtime));
  assert.deepEqual(Object.keys(runtime.env).sort(), ["CORTEX_WORKSPACE_CHANNEL", "HOME", "PATH", "XDG_CONFIG_HOME", "XDG_STATE_HOME"]);
  const defaults = await config.readConfigHandler({ surface: "user_defaults" }, context);
  assert.equal(defaults.ok, true, JSON.stringify(defaults));
  assert.equal(defaults.binding?.configHome, dev ? "~/config/cortex-workspace-dev/cortex" : "~/config/cortex");
  assert.equal(defaults.ok && (defaults.result as { exists: boolean }).exists, false);
  const all = await config.readConfigHandler({ surface: "all", workspaceId: WORKSPACE_ID }, context);
  assert.equal(all.ok, true, JSON.stringify(all));
  const surfaces = (all.ok ? all.result : {}) as Record<string, { ok: boolean; error?: { code: string } }>;
  for (const surface of ["user_defaults", "routing", "routing_profiles", "mcp_servers", "connections", "project_ceiling"]) {
    assert.equal(surfaces[surface]?.ok, true, surface);
  }
  // Defaults changes persist natively; a stale revision never overwrites them.
  const write = await config.updateConfigHandler({ surface: "user_defaults", expected_revision: "absent", defaults: { language: "en" } }, context);
  assert.ok(write.ok, JSON.stringify(write));
  const readBack = await config.readConfigHandler({ surface: "user_defaults" }, context);
  assert.ok(readBack.ok && "defaults" in readBack.result && readBack.result.defaults.language === "en", JSON.stringify(readBack));
  const stale = await config.updateConfigHandler({ surface: "user_defaults", expected_revision: "absent", defaults: { language: "ru" } }, context);
  assert.ok(!stale.ok && stale.error.code === "revision_conflict", JSON.stringify(stale));
  const unchanged = await config.readConfigHandler({ surface: "user_defaults" }, context);
  assert.ok(unchanged.ok && "defaults" in unchanged.result && unchanged.result.defaults.language === "en", JSON.stringify(unchanged));
  assert.equal(existsSync(join(isolated, "state", "defaults.json")), false);
  assert.equal(existsSync(join(ownerConfig, "defaults.json")), true);

  // Every other change class, end to end through the real handlers, still
  //: write, read back, and a stale revision never overwrites.
  const text = (value: unknown) => JSON.stringify(value);
  const update = (input: unknown) => config.updateConfigHandler(input as never, context);
  const routingWrite = await update({ surface: "routing", target: "user", expected_revision: "absent",
    rules: { model_policy_version: 2, enabled: { "codex-acp": ["fixture-model"] } } });
  assert.ok(routingWrite.ok, text(routingWrite));
  const routingRead = await config.readConfigHandler({ surface: "routing" }, context);
  assert.ok(routingRead.ok && text(routingRead.result).includes("fixture-model"), text(routingRead));
  const routingStale = await update({ surface: "routing", target: "user", expected_revision: "absent", rules: { model_policy_version: 2 } });
  assert.ok(!routingStale.ok && routingStale.error.code === "revision_conflict", text(routingStale));
  const projectRouting = await update({ surface: "routing", target: "project", workspaceId: WORKSPACE_ID, expected_revision: "absent",
    rules: { model_policy_version: 2, deny: ["opencode/*"] } });
  assert.ok(projectRouting.ok, text(projectRouting));
  assert.equal(existsSync(join(project, host.store_name, "routing.json")), true);

  const mcpWrite = await update({ surface: "mcp_servers", expected_revision: "absent",
    operations: [{ op: "upsert", name: "web", server: { command: "/usr/bin/web", allowed_tools: ["fetch"], env: { TOKEN: "fixture-secret" } } }] });
  assert.ok(mcpWrite.ok && !text(mcpWrite).includes("fixture-secret"), text(mcpWrite));
  const mcpRead = await config.readConfigHandler({ surface: "mcp_servers" }, context);
  assert.ok(mcpRead.ok && text(mcpRead.result).includes("\"web\""), text(mcpRead));
  const mcpStale = await update({ surface: "mcp_servers", expected_revision: "absent", operations: [{ op: "remove", name: "web" }] });
  assert.ok(!mcpStale.ok && mcpStale.error.code === "revision_conflict", text(mcpStale));

  const ceilingWrite = await update({ surface: "project_ceiling", workspaceId: WORKSPACE_ID, expected_revision: "absent",
    ceiling: { allowed_routes: ["route-b", "route-a"] } });
  assert.ok(ceilingWrite.ok, text(ceilingWrite));
  const ceilingRead = await config.readConfigHandler({ surface: "project_ceiling", workspaceId: WORKSPACE_ID } as never, context);
  assert.ok(ceilingRead.ok && text(ceilingRead.result).includes("[\"route-a\",\"route-b\"]"), text(ceilingRead));
  const ceilingStale = await update({ surface: "project_ceiling", workspaceId: WORKSPACE_ID, expected_revision: "absent", ceiling: null });
  assert.ok(!ceilingStale.ok && ceilingStale.error.code === "revision_conflict", text(ceilingStale));

  const profiles = await config.readConfigHandler({ surface: "routing_profiles" }, context);
  assert.ok(profiles.ok, text(profiles));
  const profileView = profiles.result as { revision: string; current_revisions: Record<string, string> };
  const created = await update({ surface: "routing_profiles", action: "create", expected_revision: profileView.revision,
    expected_revisions: profileView.current_revisions, name: "work", description: "Fixture" });
  assert.ok(created.ok && text(created).includes("\"work\""), text(created));
  const createdView = created.result as { revision: string };
  const deactivated = await update({ surface: "routing_profiles", action: "deactivate", expected_revision: createdView.revision });
  assert.ok(deactivated.ok, text(deactivated));
  const profileStale = await update({ surface: "routing_profiles", action: "delete", expected_revision: profileView.revision, name: "work" });
  assert.ok(!profileStale.ok && profileStale.error.code === "revision_conflict", text(profileStale));

  // A registered connection: rename it, then a stale rename refuses.
  const registryPath = join(ownerConfig, "connections.json");
  writeFileSync(registryPath, text({ schema_version: 1, connections: { cn_000000000001: { id: "cn_000000000001", name: "Fixture",
    engine: "codex-acp", provider: "Fixture", auth_mode: "native_account", key_kind: null, endpoint: null, launch_command: null,
    credential_ref: { kind: "engine_native_dir", path: join(isolated, "native"), owned: false }, approved_fingerprint: "untrusted",
    state: "configured", qualified: true, created_at: NOW, updated_at: NOW } } }), { mode: 0o600 });
  const connections = await config.readConfigHandler({ surface: "connections" }, context);
  assert.ok(connections.ok, text(connections));
  const connectionsRevision = (connections.result as { revision: string }).revision;
  const renamed = await update({ surface: "connections", expected_revision: connectionsRevision, connection_id: "cn_000000000001",
    operation: "rename", name: "Renamed" });
  assert.ok(renamed.ok && text(renamed).includes("Renamed"), text(renamed));
  const renameStale = await update({ surface: "connections", expected_revision: connectionsRevision, connection_id: "cn_000000000001",
    operation: "rename", name: "Again" });
  assert.ok(!renameStale.ok && renameStale.error.code === "revision_conflict", text(renameStale));

  // Catalog refresh (no launchable engine), discovery, onboarding and the preset.
  const refreshed = await config.refreshModelCatalogHandler({}, context);
  assert.ok(refreshed.ok && text(refreshed).includes("engine_unreachable"), text(refreshed));
  const discovered = await config.discoverConnectionsHandler({}, context);
  assert.ok(discovered.ok && text(discovered).includes("not_downloaded"), text(discovered));
  const onboard = await config.onboardConnectionHandler({ engine: "claude-agent-acp", expected_revision: "absent", explicit_opt_in: true } as never, context);
  assert.ok(!onboard.ok && onboard.error.code === "revision_conflict", text(onboard));
  const opencode = await config.discoverOpenCodeHandler({}, context);
  assert.ok(opencode.ok && text(opencode).includes("\"found\":false"), text(opencode));
  const applied = await config.applyDefaultPresetHandler({} as never, context);
  assert.ok(applied.ok && text(applied).includes("\"current\""), text(applied));
  const preset = await config.readDefaultPresetHandler({} as never, context);
  assert.ok(preset.ok, text(preset));
  const reset = await config.resetDefaultPresetHandler({ expected_revision: (preset.result as { revision: string }).revision, confirm: true } as never, context);
  assert.ok(reset.ok && text(reset).includes("cortex-backup-"), text(reset));

  // Activation setup runs the native first-run pass (no engine is installed
  // here, so it registers nothing).
  // Its owner root is cortex-acp's own (a dev-channel build uses its channel
  // subdirectory, exactly as that cortex-acp does).
  await config.startAutomaticSetup();
  assert.ok(existsSync(join(ownerConfig, "first-run-codex-acp.json")));
  assert.equal(readFileSync(join(otherConfig, "defaults.json"), "utf8"), "other channel must not be read or changed");

  // A present but incompatible tool cannot supply a guessed release binding.
  writeFileSync(join(bin, "cortex-paseo-tools"), "#!/bin/sh\nexit 2\n");
  const unresolvedHost = await config.readConfigHandler({ surface: "user_defaults" }, context);
  assert.ok(!unresolvedHost.ok && unresolvedHost.error.code === "config_api_unavailable");
  assert.equal(unresolvedHost.binding, null);

  // Without the tool beside cortex-acp every read says so; nothing falls back.
  rmSync(join(bin, "cortex-paseo-tools"));
  assert.equal(routes.storeReaderRoute().kind, "refused");
  const fresh = await loadServerModuleForTest<typeof import("./worker-history.ts")>(join(pluginRoot, "server", "worker-history.ts"));
  assert.deepEqual(await fresh.resolveAgentTask({ workspaceId: WORKSPACE_ID, agentId: AGENT_ID }, context), { message: routes.RUST_TOOLS_MISSING });
  const missing = await stop.readRunningCommandsInStore(project, TASK_ID);
  assert.deepEqual(missing, { available: false, commands: [], message: routes.RUST_TOOLS_MISSING });
  assert.equal(await fresh.resolveProjectRoot(project), null);
  const settingsRefusal = config.configRuntime({ argv: [join(bin, "cortex-acp")] });
  assert.ok("failure" in settingsRefusal && settingsRefusal.failure.message === config.RUST_SETTINGS_TOOLS_MISSING);
  const refusedRead = await config.readConfigHandler({ surface: "user_defaults" }, context);
  assert.ok(!refusedRead.ok && refusedRead.error.code === "config_api_unavailable");
} finally {
  for (const [key, value] of saved) {
    if (value === undefined) delete process.env[key]; else process.env[key] = value;
  }
  rmSync(isolated, { recursive: true, force: true });
}
console.log("store-reader: Rust launch reads stores and settings; missing tool refuses explicitly");
