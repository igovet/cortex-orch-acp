import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { loadServerModuleForTest } from "./test-bundle.ts";

// End-to-end tests of the configuration plugin RPCs: the client SDK's
// callPluginRpc, a host step that mirrors installed Paseo 0.10.0's plugin
// child (input parse -> handler(input, { paseo }) -> output parse), the real
// handlers, and the real Cortex configuration API from this checkout,
// against a disposable HOME/XDG and project.

const pluginRoot = resolve(".");
const repoRoot = resolve(pluginRoot, "../..");
const shared = await loadServerModuleForTest<typeof import("../shared/config.ts")>(resolve("shared/config.ts"));
const server = await loadServerModuleForTest<typeof import("./config.ts")>(resolve("server/config.ts"));
const { readConfig, updateConfig, discoverOpenCode, onboardOpenCode } = shared;

const WORKSPACE = "workspace-config-1";
const MISSING_STORE_WORKSPACE = "workspace-no-store";
const ZERO_REVISION = `sha256:${"0".repeat(64)}`;

type AnyContract = { name: string; input: any; output: any };
const handlers: Record<string, (input: any, context: any) => Promise<unknown>> = {
  [readConfig.name]: server.readConfigHandler,
  [updateConfig.name]: server.updateConfigHandler,
  [discoverOpenCode.name]: server.discoverOpenCodeHandler,
  [onboardOpenCode.name]: server.onboardOpenCodeHandler,
};
const contracts: Record<string, AnyContract> = Object.fromEntries(
  [readConfig, updateConfig, discoverOpenCode, onboardOpenCode].map((contract) => [contract.name, contract]),
);

/** The SDK's client step (`callPluginRpc` in @getpaseo/plugin dist/rpc.js,
 * identical in the installed 0.10.0 bundle; not exported from the package
 * root): parse input, invoke, parse output. */
async function callPluginRpc(contract: AnyContract, invoke: (method: string, input: unknown) => Promise<unknown>, input: unknown): Promise<any> {
  const parsedInput = await contract.input.parseAsync(input);
  const output = await invoke(contract.name, parsedInput);
  return contract.output.parseAsync(output);
}

/** Client -> host -> handler -> host -> client, as over the daemon socket. */
function call<C extends AnyContract>(contract: C, input: unknown, context: any): Promise<any> {
  return callPluginRpc(contract, async (method, parsedInput) => {
    const hostContract = contracts[method];
    const hostInput = await hostContract.input.parseAsync(JSON.parse(JSON.stringify(parsedInput)));
    const output = await handlers[method](hostInput, context);
    return JSON.parse(JSON.stringify(await hostContract.output.parseAsync(output)));
  }, input);
}

async function rejects(contract: AnyContract, input: unknown): Promise<void> {
  await assert.rejects(() => contract.input.parseAsync(input));
}

function context(directories: Record<string, string>, options: { throwOnRef?: boolean } = {}): any {
  return {
    paseo: {
      workspaces: {
        ref: (id: string) => {
          if (options.throwOnRef) throw new Error(`boom ${id} /home/private/detail`);
          return { refresh: async () => {}, directory: directories[id] ?? null };
        },
      },
    },
  };
}

const ENV_KEYS = ["HOME", "XDG_CONFIG_HOME", "XDG_STATE_HOME", "CORTEX_PASEO_SOURCE_ROOT", "CORTEX_ACP_COMMAND", "CORTEX_ROUTING", "CORTEX_PASEO_SHIM"];
const saved = new Map(ENV_KEYS.map((key) => [key, process.env[key]] as const));
function restoreEnv(): void {
  for (const [key, value] of saved) {
    if (value === undefined) delete process.env[key];
    else process.env[key] = value;
  }
}

function privateDir(path: string): string {
  mkdirSync(path, { recursive: true, mode: 0o700 });
  return path;
}

function initProjectStore(project: string): void {
  execFileSync("python3", ["-B", "-c", [
    "import sys",
    "sys.path.insert(0, sys.argv[1])",
    "from cortex_runtime.project_storage import canonical_project, project_store_directory",
    "from cortex_runtime.store import Store",
    "root = canonical_project(sys.argv[2])",
    "Store(project_store_directory(root), initialize=True, project_root=root)",
  ].join("\n"), join(repoRoot, "src"), project], { stdio: ["ignore", "ignore", "inherit"] });
}

function setupOwner(base: string): { home: string; configHome: string } {
  const home = privateDir(join(base, "home"));
  const configHome = privateDir(join(home, ".config"));
  privateDir(join(home, ".local", "state"));
  process.env.HOME = home;
  process.env.XDG_CONFIG_HOME = configHome;
  process.env.XDG_STATE_HOME = join(home, ".local", "state");
  delete process.env.CORTEX_ACP_COMMAND;
  delete process.env.CORTEX_ROUTING;
  delete process.env.CORTEX_PASEO_SHIM;
  return { home, configHome };
}

const base = mkdtempSync(join(tmpdir(), "cortex-config-rpc-"));
try {
  const { configHome } = setupOwner(base);
  process.env.CORTEX_PASEO_SOURCE_ROOT = repoRoot;
  const project = privateDir(join(base, "project"));
  initProjectStore(project);
  const bareProject = privateDir(join(base, "bare-project"));
  const ctx = context({ [WORKSPACE]: project, [MISSING_STORE_WORKSPACE]: bareProject });
  const noLeak = (value: unknown) => {
    const json = JSON.stringify(value);
    assert.ok(!json.includes(base), `response leaks a private absolute path: ${json.slice(0, 400)}`);
  };

  // Registration: the existing provider and read RPCs stay; the four
  // configuration RPCs are added. Paseo's settings document is not used.
  {
    const index = await loadServerModuleForTest<{ default: (server: unknown) => () => void }>(resolve("index.server.ts"));
    const names: string[] = [];
    const providers: string[] = [];
    let settings = 0;
    index.default({
      registerProvider: (provider: { id: string }) => providers.push(provider.id),
      handle: (contract: { name: string }) => names.push(contract.name),
      registerSettings: () => { settings += 1; },
    });
    assert.deepEqual(providers, ["cortex"]);
    const { readWorkerHistory } = await loadServerModuleForTest<typeof import("../shared/worker-history.ts")>(resolve("shared/worker-history.ts"));
    const { readPipeline } = await loadServerModuleForTest<typeof import("../shared/pipeline.ts")>(resolve("shared/pipeline.ts"));
    const { readWorkerResults } = await loadServerModuleForTest<typeof import("../shared/worker-results.ts")>(resolve("shared/worker-results.ts"));
    assert.deepEqual(names, [
      readWorkerHistory.name, readPipeline.name, readWorkerResults.name,
      "cortex.config.read", "cortex.config.update", "cortex.config.opencode.discover", "cortex.config.opencode.onboard",
    ]);
    assert.equal(settings, 0);
  }

  // The contracts refuse client-chosen paths, commands, binaries, unknown
  // fields and an implicit OpenCode opt-in before any handler runs.
  {
    await rejects(readConfig, { surface: "all", project: "/etc" });
    await rejects(readConfig, { surface: "defaults.json" });
    await rejects(updateConfig, { surface: "user_defaults", expected_revision: "absent", path: "/tmp/x.json" });
    await rejects(updateConfig, { surface: "routing", target: "env", expected_revision: "absent", rules: {} });
    await rejects(updateConfig, { surface: "routing", target: "user", expected_revision: "absent", rules: { unknown: 1 } });
    await rejects(updateConfig, { surface: "user_defaults", expected_revision: "latest" });
    await rejects(updateConfig, { surface: "project_ceiling", expected_revision: "absent", ceiling: null });
    await rejects(discoverOpenCode, { binary: "/bin/sh" });
    await rejects(onboardOpenCode, { expected_revision: "absent", explicit_opt_in: false });
    await rejects(onboardOpenCode, { expected_revision: "absent", explicit_opt_in: true, binary: "/bin/sh" });
  }

  // Owner-only read: every owner surface with its revision and effective
  // routing source; no project surface without a workspace; paths shown
  // only in display form.
  {
    const response = await call(readConfig, { surface: "all" }, ctx);
    assert.equal(response.ok, true, JSON.stringify(response));
    assert.deepEqual(response.binding, { apiVersion: 1, configHome: "~/.config/cortex", project: null });
    const result = response.result;
    assert.equal(result.user_defaults.ok, true);
    assert.equal(result.user_defaults.result.revision, "absent");
    assert.equal(result.user_defaults.result.path, "~/.config/cortex/defaults.json");
    assert.equal(result.routing.result.effective_source, "builtin");
    assert.deepEqual(result.routing.result.precedence, ["user"]);
    assert.equal(result.mcp_servers.result.revision, "absent");
    assert.equal(result.connections.result.revision, "absent");
    assert.ok(result.connections.result.engines.some((engine: any) => engine.engine === "opencode" && engine.onboarding === "explicit_opt_in"));
    assert.equal(result.project_ceiling, undefined);
    noLeak(response);
  }

  // Workspace-bound read: the project comes from the Paseo workspace, never
  // from the client; project surfaces appear with their own revisions.
  {
    const response = await call(readConfig, { workspaceId: WORKSPACE, surface: "all" }, ctx);
    assert.equal(response.ok, true, JSON.stringify(response));
    assert.equal(response.binding.project, "<project>");
    assert.equal(response.result.project_ceiling.ok, true);
    assert.equal(response.result.project_ceiling.result.project_root, "<project>");
    assert.equal(response.result.project_ceiling.result.revision, "absent");
    assert.deepEqual(response.result.routing.result.precedence, ["project", "user"]);
    assert.equal(response.result.routing.result.sources[0].path, "<project>/.cortex/routing.json");
    noLeak(response);

    const unknown = await call(readConfig, { workspaceId: "nope", surface: "routing" }, ctx);
    assert.equal(unknown.ok, false);
    assert.equal(unknown.error.code, "workspace_unavailable");
    const thrown = await call(readConfig, { workspaceId: WORKSPACE, surface: "routing" }, context({}, { throwOnRef: true }));
    assert.equal(thrown.ok, false);
    assert.equal(thrown.error.code, "workspace_unavailable");
    assert.ok(!JSON.stringify(thrown).includes("/home/private"));
    const needsWorkspace = await call(readConfig, { surface: "project_ceiling" }, ctx);
    assert.equal(needsWorkspace.error.code, "workspace_required");

    // A project without a Cortex store: a structured, path-scrubbed refusal.
    const missing = await call(readConfig, { workspaceId: MISSING_STORE_WORKSPACE, surface: "project_ceiling" }, ctx);
    assert.equal(missing.ok, false);
    assert.equal(missing.error.code, "project_store_missing");
    assert.match(missing.error.message, /<project>/);
    noLeak(missing);
  }

  // User defaults: write with the read revision, read back, and the file the
  // runtime reads holds the value.
  let revision: string;
  {
    const written = await call(updateConfig, { surface: "user_defaults", expected_revision: "absent", defaults: { language: "ru" } }, ctx);
    assert.equal(written.ok, true, JSON.stringify(written));
    assert.equal(written.result.changed, true);
    assert.match(written.result.revision, /^sha256:[0-9a-f]{64}$/);
    revision = written.result.revision;
    const read = await call(readConfig, { surface: "user_defaults" }, ctx);
    assert.equal(read.result.defaults.language, "ru");
    assert.equal(read.result.revision, revision);
    assert.equal(JSON.parse(readFileSync(join(configHome, "cortex", "defaults.json"), "utf8")).defaults.language, "ru");

    // A stale revision is refused with the revision to re-read; nothing is written.
    const before = readFileSync(join(configHome, "cortex", "defaults.json"), "utf8");
    const stale = await call(updateConfig, { surface: "user_defaults", expected_revision: "absent", defaults: { language: "en" } }, ctx);
    assert.equal(stale.ok, false);
    assert.equal(stale.error.code, "revision_conflict");
    assert.equal(stale.error.currentRevision, revision);
    assert.equal(readFileSync(join(configHome, "cortex", "defaults.json"), "utf8"), before);

    // Structured multi-field validation from the API itself.
    const invalid = await call(updateConfig, {
      surface: "user_defaults", expected_revision: revision,
      defaults: { route_id: "not-a-route" }, ceiling: { spend_limit: 5 },
    }, ctx);
    assert.equal(invalid.ok, false);
    assert.equal(invalid.error.code, "invalid_configuration");
    assert.deepEqual(invalid.error.errors.map((item: any) => item.field).sort(), ["ceiling.spend_limit", "defaults.route_id"]);
  }

  // Concurrency: two writers holding the same revision -- exactly one wins,
  // the other is told the winner's revision; reads alongside all succeed.
  {
    const [first, second, ...reads] = await Promise.all([
      call(updateConfig, { surface: "user_defaults", expected_revision: revision, defaults: { language: "en" } }, ctx),
      call(updateConfig, { surface: "user_defaults", expected_revision: revision, defaults: { language: null } }, ctx),
      ...Array.from({ length: 6 }, () => call(readConfig, { surface: "user_defaults" }, ctx)),
    ]);
    const outcomes = [first, second];
    const winners = outcomes.filter((outcome) => outcome.ok);
    const losers = outcomes.filter((outcome) => !outcome.ok);
    assert.equal(winners.length, 1, JSON.stringify(outcomes));
    assert.equal(losers.length, 1);
    assert.equal(losers[0].error.code, "revision_conflict");
    assert.equal(losers[0].error.currentRevision, winners[0].result.revision);
    for (const read of reads) assert.equal(read.ok, true);
    revision = winners[0].result.revision;
    const final = await call(readConfig, { surface: "user_defaults" }, ctx);
    assert.equal(final.result.revision, revision);
  }

  // Routing: the project file needs a workspace; writing it makes it the
  // effective source and shadows the user file; OpenCode routes must be exact.
  {
    const noWorkspace = await call(updateConfig, { surface: "routing", target: "project", expected_revision: "absent", rules: {} }, ctx);
    assert.equal(noWorkspace.error.code, "workspace_required");

    const projectWrite = await call(updateConfig, {
      surface: "routing", target: "project", workspaceId: WORKSPACE, expected_revision: "absent",
      rules: { deny: ["codex-acp/gpt-5.1"], auto_allow: ["opencode/anthropic/claude-sonnet-4-5"] },
    }, ctx);
    assert.equal(projectWrite.ok, true, JSON.stringify(projectWrite));
    assert.equal(projectWrite.result.effective_source, "project");
    assert.equal(projectWrite.result.shadowed_by, null);
    assert.ok(existsSync(join(project, ".cortex", "routing.json")));
    noLeak(projectWrite);

    const userWrite = await call(updateConfig, {
      surface: "routing", target: "user", workspaceId: WORKSPACE, expected_revision: "absent", rules: { providers: ["codex-acp"] },
    }, ctx);
    assert.equal(userWrite.ok, true, JSON.stringify(userWrite));
    assert.equal(userWrite.result.shadowed_by, "project");

    const family = await call(updateConfig, {
      surface: "routing", target: "user", expected_revision: userWrite.result.sources.find((s: any) => s.source === "user").revision,
      rules: { pin: { "*": "opencode/sonnet" } },
    }, ctx);
    assert.equal(family.ok, false);
    assert.equal(family.error.code, "invalid_configuration");
    assert.ok(family.error.errors.some((item: any) => item.code === "opencode_model_not_exact"));
  }

  // Project ceiling: bound to the workspace's project store and narrowing
  // the user ceiling.
  {
    const written = await call(updateConfig, {
      surface: "project_ceiling", workspaceId: WORKSPACE, expected_revision: "absent",
      ceiling: { allowed_routes: [], limits: { attempts: 3 } },
    }, ctx);
    assert.equal(written.ok, true, JSON.stringify(written));
    assert.deepEqual(written.result.effective_ceiling.allowed_routes, []);
    assert.equal(written.result.effective_ceiling.limits.attempts, 3);
    assert.match(written.result.revision, /^sha256:/);
    const read = await call(readConfig, { workspaceId: WORKSPACE, surface: "project_ceiling" }, ctx);
    assert.equal(read.result.revision, written.result.revision);
    noLeak(read);
  }

  // MCP servers: env values and credential-looking arguments never reach the
  // client; sending the redacted form back keeps the stored values; a changed
  // redacted part is refused and nothing is written.
  {
    const secrets = ["SUPERSECRET1234", "TOKENSECRET5678", "ENVSECRET9999"];
    const upsert = await call(updateConfig, {
      surface: "mcp_servers", expected_revision: "absent",
      operations: [{
        op: "upsert", name: "demo",
        server: {
          command: "demo-mcp", args: [`--api-key=${secrets[0]}`, "--token", secrets[1], "--verbose"],
          env: { API_TOKEN: secrets[2] }, allowed_tools: ["search"],
        },
      }],
    }, ctx);
    assert.equal(upsert.ok, true, JSON.stringify(upsert));
    const [demo] = upsert.result.servers;
    assert.deepEqual(demo.config.args, ["--api-key=[redacted]", "--token", "[redacted]", "--verbose"]);
    assert.deepEqual(demo.config.env, [{ name: "API_TOKEN", value_set: true }]);
    assert.equal(demo.redacted, true);
    for (const secret of secrets) assert.ok(!JSON.stringify(upsert).includes(secret));

    const read = await call(readConfig, { surface: "mcp_servers" }, ctx);
    for (const secret of secrets) assert.ok(!JSON.stringify(read).includes(secret));
    const shown = read.result.servers[0].config;

    const kept = await call(updateConfig, {
      surface: "mcp_servers", expected_revision: read.result.revision,
      operations: [{
        op: "upsert", name: "demo",
        server: { command: shown.command, args: shown.args, env: { API_TOKEN: { keep: true } }, allowed_tools: shown.allowed_tools, enabled: false },
      }],
    }, ctx);
    assert.equal(kept.ok, true, JSON.stringify(kept));
    assert.equal(kept.result.servers[0].enabled, false);
    const stored = JSON.parse(readFileSync(join(configHome, "cortex", "mcp-servers.json"), "utf8")).servers.demo;
    assert.deepEqual(stored.args, [`--api-key=${secrets[0]}`, "--token", secrets[1], "--verbose"]);
    assert.equal(stored.env.API_TOKEN, secrets[2]);
    assert.equal(stored.enabled, false);

    const before = readFileSync(join(configHome, "cortex", "mcp-servers.json"), "utf8");
    const tampered = await call(updateConfig, {
      surface: "mcp_servers", expected_revision: kept.result.revision,
      operations: [{ op: "upsert", name: "demo", server: { command: "demo-mcp", args: ["--api-key=[redacted]x", "--token", "[redacted]"], allowed_tools: ["search"] } }],
    }, ctx);
    assert.equal(tampered.ok, false);
    assert.equal(tampered.error.code, "invalid_configuration");
    assert.equal(tampered.error.errors[0].code, "redacted_value_changed");
    assert.equal(readFileSync(join(configHome, "cortex", "mcp-servers.json"), "utf8"), before);

    // A redacted keep against a stale revision is a conflict, not a guess.
    const stale = await call(updateConfig, {
      surface: "mcp_servers", expected_revision: read.result.revision,
      operations: [{ op: "upsert", name: "demo", server: { command: "demo-mcp", args: shown.args, allowed_tools: ["search"] } }],
    }, ctx);
    assert.equal(stale.ok, false);
    assert.equal(stale.error.code, "revision_conflict");
    assert.equal(stale.error.currentRevision, kept.result.revision);
  }

  // OpenCode: discovery takes no client path and writes nothing; onboarding
  // checks the revision before it looks for anything.
  {
    const before = await call(readConfig, { surface: "connections" }, ctx);
    const discovered = await call(discoverOpenCode, {}, ctx);
    assert.equal(discovered.ok, true, JSON.stringify(discovered));
    assert.equal(typeof discovered.result.found, "boolean");
    noLeak(discovered);
    const after = await call(readConfig, { surface: "connections" }, ctx);
    assert.equal(after.result.revision, before.result.revision);

    const onboard = await call(onboardOpenCode, { expected_revision: ZERO_REVISION, explicit_opt_in: true }, ctx);
    assert.equal(onboard.ok, false);
    assert.equal(onboard.error.code, "revision_conflict");
  }

  // Connections: an update against an unknown id is a structured refusal.
  {
    const connections = await call(readConfig, { surface: "connections" }, ctx);
    const refused = await call(updateConfig, {
      surface: "connections", expected_revision: connections.result.revision,
      connection_id: "cn_000000000000", operation: "rename", name: "x",
    }, ctx);
    assert.equal(refused.ok, false);
    assert.equal(refused.error.code, "unknown_connection");
  }
} finally {
  restoreEnv();
  rmSync(base, { recursive: true, force: true });
}

// Owner binding: the launch record's HOME/XDG overrides -- the environment
// the plugin starts Cortex with -- select the configuration home, not the
// daemon's own environment.
{
  const isolated = mkdtempSync(join(tmpdir(), "cortex-config-owner-"));
  try {
    setupOwner(join(isolated, "daemon"));
    delete process.env.CORTEX_PASEO_SOURCE_ROOT;
    const owner = privateDir(join(isolated, "owner"));
    privateDir(join(owner, ".config"));
    privateDir(join(owner, ".local", "state"));
    const record = privateDir(join(process.env.XDG_CONFIG_HOME!, "cortex"));
    writeFileSync(join(record, "paseo-plugin-launch.json"), JSON.stringify({
      argv: ["python3", "-B", join(repoRoot, "src", "cortex_acp.py")],
      env: { HOME: owner, XDG_CONFIG_HOME: join(owner, ".config"), XDG_STATE_HOME: join(owner, ".local", "state") },
      shim: join(repoRoot, "integrations", "paseo", "server", "acp-load-shim.mjs"),
    }));
    const response = await call(updateConfig, { surface: "user_defaults", expected_revision: "absent", defaults: { language: "en" } }, context({}));
    assert.equal(response.ok, true, JSON.stringify(response));
    assert.equal(response.binding.configHome, "~/.config/cortex");
    assert.ok(existsSync(join(owner, ".config", "cortex", "defaults.json")));
    assert.ok(!existsSync(join(record, "defaults.json")));
  } finally {
    restoreEnv();
    rmSync(isolated, { recursive: true, force: true });
  }
}

// No configured Cortex launch: a structured refusal, never a thrown error.
{
  const isolated = mkdtempSync(join(tmpdir(), "cortex-config-none-"));
  try {
    setupOwner(isolated);
    delete process.env.CORTEX_PASEO_SOURCE_ROOT;
    const response = await call(readConfig, { surface: "all" }, context({}));
    assert.equal(response.ok, false);
    assert.equal(response.binding, null);
    assert.equal(response.error.code, "config_api_unavailable");
    // A launch that is not a Python Cortex entry point is refused too.
    const foreign = server.configRuntime({ argv: ["/bin/sh", "-c", "echo"] });
    assert.equal("failure" in foreign && foreign.failure.code, "config_api_unavailable");
  } finally {
    restoreEnv();
    rmSync(isolated, { recursive: true, force: true });
  }
}

// Redaction and display helpers.
{
  const scope = { home: "/home/owner", projectRoot: "/home/owner/work/app" };
  assert.equal(server.displayPath("/home/owner/.config/cortex/routing.json", scope), "~/.config/cortex/routing.json");
  assert.equal(server.displayPath("/home/owner/work/app/.cortex/routing.json", scope), "<project>/.cortex/routing.json");
  assert.equal(server.displayPath("/tmp/elsewhere/x.json", scope), "[private path]");
  assert.equal(server.displayPath("/etc/cortex/routing.json", scope), "/etc/cortex/routing.json");
  assert.deepEqual(server.redactArgv(["--header", "Authorization: Bearer abc.def", "--password", "hunter2", "https://u:pw@host/x", "plain"]),
    ["--header", "Authorization: Bearer [redacted]", "--password", "[redacted]", "https://u:[redacted]@host/x", "plain"]);
  assert.equal(server.redactCredentials("--api-key=[redacted]"), "--api-key=[redacted]");
  assert.equal(server.scrubText("/home/owner/.config/cortex/mcp-servers.json: demo.command must be a non-empty string.", scope),
    "~/.config/cortex/mcp-servers.json: demo.command must be a non-empty string.");
}

console.log("config: ok");
