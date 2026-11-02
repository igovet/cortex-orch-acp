import assert from "node:assert/strict";
import { existsSync, mkdtempSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import test from "node:test";
import { build } from "esbuild";
import { loadServerModuleForTest } from "./test-bundle.ts";

// Activation tests must never download engines or use native accounts.
process.env.CORTEX_ENGINE_UPDATE = "off";

const pluginRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const repoRoot = resolve(pluginRoot, "../..");
const pluginRequire = createRequire(pathToFileURL(join(pluginRoot, "index.server.ts")));

async function evaluateLikePaseo() {
  const result = await build({
    entryPoints: [join(pluginRoot, "index.server.ts")],
    bundle: true,
    format: "cjs",
    platform: "node",
    target: "node20",
    packages: "external",
    write: false,
  });
  assert.deepEqual(result.warnings, []);
  const module = { exports: {} as { default?: (server: unknown) => () => void } };
  // Paseo 0.9.1 compiles a CJS bundle and evaluates it in its plugin child.
  // This context has no import.meta.url, unlike direct Node TS imports.
  new Function("require", "module", "exports", result.outputFiles[0].text)(
    pluginRequire, module, module.exports,
  );
  const providers: Array<{ id: string }> = [];
  assert.equal(typeof module.exports.default, "function");
  const cleanup = module.exports.default!({
    registerProvider(provider: { id: string }) { providers.push(provider); },
    handle() {},
  });
  assert.equal(typeof cleanup, "function");
  assert.deepEqual(providers.map((provider) => provider.id), ["cortex"]);
}

test("Paseo CJS loader activates a directory plugin with explicit source root", async () => {
  const isolated = mkdtempSync(join(tmpdir(), "cortex-plugin-directory-"));
  const saved = new Map(
    ["HOME", "XDG_CONFIG_HOME", "CORTEX_PASEO_SOURCE_ROOT", "CORTEX_ACP_COMMAND"]
      .map((key) => [key, process.env[key]] as const),
  );
  try {
    process.env.HOME = isolated;
    process.env.XDG_CONFIG_HOME = join(isolated, "config");
    process.env.CORTEX_PASEO_SOURCE_ROOT = repoRoot;
    delete process.env.CORTEX_ACP_COMMAND;
    await evaluateLikePaseo();
  } finally {
    for (const [key, value] of saved) {
      if (value === undefined) delete process.env[key];
      else process.env[key] = value;
    }
    rmSync(isolated, { recursive: true, force: true });
  }
});

const LAUNCH_ENV = ["HOME", "XDG_CONFIG_HOME", "CORTEX_PASEO_SOURCE_ROOT", "CORTEX_ACP_COMMAND", "CORTEX_PASEO_SHIM"];
const shim = join(repoRoot, "integrations", "paseo", "server", "acp-load-shim.mjs");

function writeRecord(configHome: string, entry: string, env: Record<string, string>): void {
  const directory = join(configHome, "cortex");
  mkdirSync(directory, { recursive: true, mode: 0o700 });
  writeFileSync(join(directory, "paseo-plugin-launch.json"), JSON.stringify({ argv: ["python3", "-B", entry], env, shim }));
}

// Setup writes the launch record into the config root of the environment it
// describes; the provider must select the record of its own profile and never
// launch another Paseo instance's runtime from the shared HOME/.config record.
test("each Paseo profile launches only its own launch record", async () => {
  const isolated = mkdtempSync(join(tmpdir(), "cortex-plugin-profiles-"));
  const saved = new Map(LAUNCH_ENV.map((key) => [key, process.env[key]] as const));
  try {
    const provider = await loadServerModuleForTest<typeof import("./provider.ts")>(join(pluginRoot, "server", "provider.ts"));
    for (const key of LAUNCH_ENV) delete process.env[key];
    const home = join(isolated, "home");
    const ownerConfig = join(home, ".config");
    const devConfig = join(home, ".paseo-dev", "cortex-xdg", "config");
    const ownerEnv = { HOME: home, XDG_CONFIG_HOME: ownerConfig, XDG_STATE_HOME: join(home, ".local", "state") };
    const devEnv = { HOME: home, XDG_CONFIG_HOME: devConfig, XDG_STATE_HOME: join(home, ".paseo-dev", "cortex-xdg", "state") };
    writeRecord(ownerConfig, "/owner/src/cortex_acp.py", ownerEnv);
    process.env.HOME = home;

    // The conventional profile keeps using HOME/.config.
    assert.deepEqual(provider.cortexLaunchTarget()?.argv, ["python3", "-B", "/owner/src/cortex_acp.py"]);
    process.env.XDG_CONFIG_HOME = ownerConfig;
    assert.deepEqual(provider.cortexLaunchTarget()?.env, ownerEnv);

    // An isolated profile without its own record refuses the owner's record.
    process.env.XDG_CONFIG_HOME = devConfig;
    assert.equal(provider.cortexLaunchTarget(), undefined);

    // With its own record it launches exactly that runtime and storage.
    writeRecord(devConfig, "/dev/src/cortex_acp.py", devEnv);
    const target = provider.cortexLaunchTarget();
    assert.deepEqual(target?.argv, ["python3", "-B", "/dev/src/cortex_acp.py"]);
    assert.deepEqual(target?.env, devEnv);

    // A record an older setup wrote under HOME for this same profile still works.
    rmSync(join(devConfig, "cortex"), { recursive: true, force: true });
    writeRecord(ownerConfig, "/legacy/src/cortex_acp.py", devEnv);
    assert.deepEqual(provider.cortexLaunchTarget()?.env, devEnv);
  } finally {
    for (const [key, value] of saved) {
      if (value === undefined) delete process.env[key];
      else process.env[key] = value;
    }
    rmSync(isolated, { recursive: true, force: true });
  }
});

test("automatic setup uses the configured owner runtime", async () => {
  const isolated = mkdtempSync(join(tmpdir(), "cortex-auto-start-"));
  const saved = new Map(LAUNCH_ENV.map((key) => [key, process.env[key]] as const));
  try {
    const src = join(isolated, "src");
    mkdirSync(join(src, "cortex_runtime", "configuration"), { recursive: true });
    mkdirSync(join(src, "cortex_runtime", "connections"), { recursive: true });
    writeFileSync(join(src, "cortex_acp.py"), "# fake entry");
    const shimDir = join(isolated, "integrations", "paseo", "server");
    mkdirSync(shimDir, { recursive: true });
    writeFileSync(join(shimDir, "acp-load-shim.mjs"), "// fake shim");
    writeFileSync(join(src, "cortex_runtime", "configuration", "management.py"), "# supported API");
    writeFileSync(join(src, "cortex_runtime", "connections", "automatic.py"), [
      "import os, pathlib",
      "def run():",
      "    pathlib.Path(os.environ['HOME'], 'automatic-ran').touch()",
    ].join("\n"));
    process.env.HOME = isolated;
    process.env.XDG_CONFIG_HOME = join(isolated, "config");
    process.env.CORTEX_PASEO_SOURCE_ROOT = isolated;
    delete process.env.CORTEX_ACP_COMMAND;
    const config = await loadServerModuleForTest<typeof import("./config.ts")>(join(pluginRoot, "server", "config.ts"));
    await config.startAutomaticSetup();
    assert.equal(existsSync(join(isolated, "automatic-ran")), true);
  } finally {
    for (const [key, value] of saved) {
      if (value === undefined) delete process.env[key]; else process.env[key] = value;
    }
    rmSync(isolated, { recursive: true, force: true });
  }
});

test("an explicit source root wins over an XDG launch record", async () => {
  const isolated = mkdtempSync(join(tmpdir(), "cortex-plugin-git-"));
  const saved = new Map(
    ["HOME", "XDG_CONFIG_HOME", "CORTEX_PASEO_SOURCE_ROOT", "CORTEX_ACP_COMMAND"]
      .map((key) => [key, process.env[key]] as const),
  );
  try {
    const configHome = join(isolated, "config");
    const cortexConfig = join(configHome, "cortex");
    mkdirSync(cortexConfig, { recursive: true });
    writeFileSync(join(cortexConfig, "paseo-plugin-launch.json"), JSON.stringify({
      argv: ["/missing/cortex-acp.py"],
      shim: "/missing/acp-load-shim.mjs",
    }));
    process.env.HOME = isolated;
    process.env.XDG_CONFIG_HOME = configHome;
    process.env.CORTEX_PASEO_SOURCE_ROOT = repoRoot;
    delete process.env.CORTEX_ACP_COMMAND;
    await evaluateLikePaseo();
  } finally {
    for (const [key, value] of saved) {
      if (value === undefined) delete process.env[key];
      else process.env[key] = value;
    }
    rmSync(isolated, { recursive: true, force: true });
  }
});
