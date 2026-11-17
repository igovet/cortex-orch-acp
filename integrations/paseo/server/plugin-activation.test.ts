import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
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

/** A checkout stand-in: the shim file plus built stand-in Rust binaries (the
 * tools stand-in records a `setup` run in HOME). Nothing real runs. */
function fakeCheckout(root: string): string {
  const server = join(root, "integrations", "paseo", "server");
  mkdirSync(server, { recursive: true });
  writeFileSync(join(server, "acp-load-shim.mjs"), "// fake shim");
  const bin = join(root, "target", "debug");
  mkdirSync(bin, { recursive: true });
  writeFileSync(join(bin, "cortex-acp"), "#!/bin/sh\nexit 0\n", { mode: 0o700 });
  writeFileSync(join(bin, "cortex-paseo-tools"), "#!/bin/sh\n[ \"$1\" = setup ] && : > \"$HOME/automatic-ran\"\nexit 0\n", { mode: 0o700 });
  return root;
}

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
    ["HOME", "XDG_CONFIG_HOME", "CORTEX_PASEO_SOURCE_ROOT", "CORTEX_ACP_COMMAND", "CORTEX_ACP_ENTRY"]
      .map((key) => [key, process.env[key]] as const),
  );
  try {
    process.env.HOME = isolated;
    process.env.XDG_CONFIG_HOME = join(isolated, "config");
    // Loader check only: stand-in binaries keep activation from running the
    // checkout's real native first-run setup.
    process.env.CORTEX_PASEO_SOURCE_ROOT = fakeCheckout(join(isolated, "checkout"));
    delete process.env.CORTEX_ACP_COMMAND;
    await evaluateLikePaseo();
  } finally {
    for (const [key, value] of saved) {
      if (value === undefined) delete process.env[key];
      else process.env[key] = value;
    }
    rmSync(isolated, { recursive: true, force: true, maxRetries: 10, retryDelay: 50 });
  }
});

const LAUNCH_ENV = ["HOME", "XDG_CONFIG_HOME", "CORTEX_PASEO_SOURCE_ROOT", "CORTEX_ACP_COMMAND", "CORTEX_PASEO_SHIM", "CORTEX_ACP_RUST_BINARY", "CORTEX_ACP_RUNTIME_ROOT", "CORTEX_ACP_ENTRY"];
const shim = join(repoRoot, "integrations", "paseo", "server", "acp-load-shim.mjs");

test("a source root launches its built Rust entry; there is no Python entry", async () => {
  const isolated = mkdtempSync(join(tmpdir(), "cortex-plugin-default-"));
  const saved = new Map(LAUNCH_ENV.map((key) => [key, process.env[key]] as const));
  try {
    for (const key of LAUNCH_ENV) delete process.env[key];
    process.env.HOME = isolated;
    process.env.XDG_CONFIG_HOME = join(isolated, "config");
    const root = join(isolated, "checkout");
    mkdirSync(join(root, "integrations", "paseo", "server"), { recursive: true });
    writeFileSync(join(root, "integrations", "paseo", "server", "acp-load-shim.mjs"), "// fake shim");
    process.env.CORTEX_PASEO_SOURCE_ROOT = root;
    const provider = await loadServerModuleForTest<typeof import("./provider.ts")>(join(pluginRoot, "server", "provider.ts"));

    // Unbuilt default: refuse; there is no other entry to start.
    assert.equal(provider.cortexLaunchTarget(), undefined);
    const binary = join(root, "target", "debug", "cortex-acp");
    mkdirSync(dirname(binary), { recursive: true });
    writeFileSync(binary, "#!/bin/sh\nexit 0\n", { mode: 0o700 });
    assert.deepEqual(provider.cortexLaunchTarget()?.argv, [binary]);
    assert.deepEqual(provider.cortexLaunchTarget()?.env, { CORTEX_ACP_RUNTIME_ROOT: root });
    // The removed Python opt-out has no effect: the Rust entry is still the only one.
    for (const value of ["rust", "python", "other"]) {
      process.env.CORTEX_ACP_ENTRY = value;
      assert.deepEqual(provider.cortexLaunchTarget()?.argv, [binary], value);
    }
    process.env.CORTEX_ACP_RUST_BINARY = binary;
    assert.deepEqual(provider.cortexLaunchTarget()?.argv, [binary]);
  } finally {
    for (const [key, value] of saved) {
      if (value === undefined) delete process.env[key]; else process.env[key] = value;
    }
    rmSync(isolated, { recursive: true, force: true, maxRetries: 10, retryDelay: 50 });
  }
});

test("Rust ingress is explicit, source-isolated, and answers model efforts", async () => {
  const isolated = mkdtempSync(join(tmpdir(), "cortex-plugin-rust-"));
  const saved = new Map(LAUNCH_ENV.map((key) => [key, process.env[key]] as const));
  try {
    for (const key of LAUNCH_ENV) delete process.env[key];
    process.env.HOME = isolated;
    process.env.XDG_CONFIG_HOME = join(isolated, "config");
    process.env.CORTEX_PASEO_SOURCE_ROOT = repoRoot;
    const provider = await loadServerModuleForTest<typeof import("./provider.ts")>(join(pluginRoot, "server", "provider.ts"));
    // Fake executable, not a paid engine; it only accepts the efforts mode.
    const binary = join(isolated, "cortex-acp");
    writeFileSync(binary, [
      "#!/bin/sh",
      '[ "$CORTEX_MODEL_EFFORTS" = "1" ] || exit 7',
      '[ -n "$CORTEX_ACP_RUNTIME_ROOT" ] || exit 8',
      'printf \'{"fixture-model":["low","high"]}\\n\'',
    ].join("\n"), { mode: 0o700 });
    process.env.CORTEX_ACP_RUST_BINARY = binary;
    process.env.CORTEX_ACP_COMMAND = "/must/not/override-rust";
    const target = provider.cortexLaunchTarget();
    assert.deepEqual(target?.argv, [binary]);
    assert.deepEqual(target?.env, { CORTEX_ACP_RUNTIME_ROOT: repoRoot });
    const command = ["env", `CORTEX_ACP_TARGET=${JSON.stringify(target)}`];
    assert.deepEqual(await provider.modelEfforts(command), { "fixture-model": ["low", "high"] });
    const config = await loadServerModuleForTest<typeof import("./config.ts")>(join(pluginRoot, "server", "config.ts"));
    assert.equal((config.configRuntime(target) as { failure: { code: string } }).failure.code, "config_api_unavailable");

    // Invalid opt-in refuses: never silently return the source target.
    process.env.CORTEX_ACP_RUST_BINARY = "relative/cortex-acp";
    assert.equal(provider.cortexLaunchTarget(), undefined);
    process.env.CORTEX_ACP_RUST_BINARY = join(isolated, "missing");
    assert.equal(provider.cortexLaunchTarget(), undefined);

    // A durable opt-in record works without source-root or override variables.
    delete process.env.CORTEX_PASEO_SOURCE_ROOT;
    delete process.env.CORTEX_ACP_RUST_BINARY;
    delete process.env.CORTEX_ACP_COMMAND;
    const directory = join(process.env.XDG_CONFIG_HOME, "cortex");
    mkdirSync(directory, { recursive: true });
    writeFileSync(join(directory, "paseo-plugin-launch.json"), JSON.stringify({
      argv: [binary], shim, env: {
        HOME: isolated, XDG_CONFIG_HOME: process.env.XDG_CONFIG_HOME,
        CORTEX_ACP_RUNTIME_ROOT: repoRoot,
      },
    }));
    assert.deepEqual(provider.cortexLaunchTarget()?.argv, [binary]);
    assert.equal(provider.cortexLaunchTarget()?.env?.CORTEX_ACP_RUNTIME_ROOT, repoRoot);
  } finally {
    for (const [key, value] of saved) {
      if (value === undefined) delete process.env[key]; else process.env[key] = value;
    }
    rmSync(isolated, { recursive: true, force: true, maxRetries: 10, retryDelay: 50 });
  }
});

test("real shim accepts the provider's Rust target and rejects invalid target environments", async () => {
  const isolated = mkdtempSync(join(tmpdir(), "cortex-plugin-rust-shim-"));
  const saved = new Map(LAUNCH_ENV.map((key) => [key, process.env[key]] as const));
  try {
    for (const key of LAUNCH_ENV) delete process.env[key];
    process.env.HOME = isolated;
    process.env.XDG_CONFIG_HOME = join(isolated, "config");
    process.env.CORTEX_PASEO_SOURCE_ROOT = repoRoot;
    const binary = join(isolated, "cortex-acp");
    // A no-paid protocol fixture: echoes only the public runtime root, not host env.
    writeFileSync(binary, [
      `#!${process.execPath}`,
      'let input = "";',
      'process.stdin.setEncoding("utf8");',
      'process.stdin.on("data", chunk => { input += chunk; });',
      'process.stdin.on("end", () => {',
      '  const frame = JSON.parse(input);',
      '  process.stdout.write(JSON.stringify({ jsonrpc: "2.0", id: frame.id,',
      '    result: { runtimeRoot: process.env.CORTEX_ACP_RUNTIME_ROOT } }) + "\\n");',
      '});',
    ].join("\n"), { mode: 0o700 });
    process.env.CORTEX_ACP_RUST_BINARY = binary;
    const provider = await loadServerModuleForTest<typeof import("./provider.ts")>(join(pluginRoot, "server", "provider.ts"));
    const runShim = (target: unknown) => spawnSync(process.execPath, [shim], {
      input: JSON.stringify({ jsonrpc: "2.0", id: "fixture", method: "initialize", params: {} }) + "\n",
      env: { HOME: isolated, XDG_CONFIG_HOME: join(isolated, "config"),
        XDG_STATE_HOME: join(isolated, "state"), CORTEX_ACP_TARGET: JSON.stringify(target) },
      encoding: "utf8", timeout: 5000,
    });
    const checkTarget = () => {
      const target = provider.cortexLaunchTarget();
      assert.deepEqual(target?.argv, [binary]);
      const result = runShim(target);
      assert.equal(result.error, undefined);
      assert.equal(result.status, 0, result.stderr);
      assert.equal(result.stderr, "");
      assert.deepEqual(JSON.parse(result.stdout), {
        jsonrpc: "2.0", id: "fixture", result: { runtimeRoot: repoRoot },
      });
      return target!;
    };
    const target = checkTarget();
    // The explicit root (without source-tree selection) goes through the same shim.
    delete process.env.CORTEX_PASEO_SOURCE_ROOT;
    process.env.CORTEX_PASEO_SHIM = shim;
    process.env.CORTEX_ACP_RUNTIME_ROOT = repoRoot;
    checkTarget();
    process.env.CORTEX_ACP_RUNTIME_ROOT = "relative/root";
    assert.equal(provider.cortexLaunchTarget(), undefined);

    for (const env of [
      { CORTEX_ACP_RUNTIME_ROOT: "relative/root" },
      { CORTEX_ACP_RUNTIME_ROOT: 42 },
      { CORTEX_ACP_RUNTIME_ROOT: "" },
      { ...target.env, UNAPPROVED_TARGET_ENV: "/absolute" },
    ]) {
      const rejected = runShim({ ...target, env });
      assert.equal(rejected.error, undefined);
      assert.equal(rejected.status, 1);
      assert.match(rejected.stderr, /CORTEX_ACP_TARGET is invalid/);
      assert.equal(rejected.stdout, "");
    }
  } finally {
    for (const [key, value] of saved) {
      if (value === undefined) delete process.env[key]; else process.env[key] = value;
    }
    rmSync(isolated, { recursive: true, force: true, maxRetries: 10, retryDelay: 50 });
  }
});

function writeRecord(configHome: string, entry: string, env: Record<string, string>): void {
  const directory = join(configHome, "cortex");
  mkdirSync(directory, { recursive: true, mode: 0o700 });
  writeFileSync(join(directory, "paseo-plugin-launch.json"), JSON.stringify({ argv: [entry], env, shim }));
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
    writeRecord(ownerConfig, "/owner/bin/cortex-acp", ownerEnv);
    process.env.HOME = home;

    // The conventional profile keeps using HOME/.config.
    assert.deepEqual(provider.cortexLaunchTarget()?.argv, ["/owner/bin/cortex-acp"]);
    process.env.XDG_CONFIG_HOME = ownerConfig;
    assert.deepEqual(provider.cortexLaunchTarget()?.env, ownerEnv);

    // An isolated profile without its own record refuses the owner's record.
    process.env.XDG_CONFIG_HOME = devConfig;
    assert.equal(provider.cortexLaunchTarget(), undefined);

    // With its own record it launches exactly that runtime and storage.
    writeRecord(devConfig, "/dev/bin/cortex-acp", devEnv);
    const target = provider.cortexLaunchTarget();
    assert.deepEqual(target?.argv, ["/dev/bin/cortex-acp"]);
    assert.deepEqual(target?.env, devEnv);

    // A record an older setup wrote under HOME for this same profile still works.
    rmSync(join(devConfig, "cortex"), { recursive: true, force: true });
    writeRecord(ownerConfig, "/legacy/bin/cortex-acp", devEnv);
    assert.deepEqual(provider.cortexLaunchTarget()?.env, devEnv);

    // A Python-argv record (an older setup) is never launched.
    writeFileSync(join(ownerConfig, "cortex", "paseo-plugin-launch.json"), JSON.stringify({
      argv: ["python3", "-B", "/legacy/src/cortex_acp.py"], env: devEnv, shim }));
    assert.equal(provider.cortexLaunchTarget(), undefined);
  } finally {
    for (const [key, value] of saved) {
      if (value === undefined) delete process.env[key];
      else process.env[key] = value;
    }
    rmSync(isolated, { recursive: true, force: true, maxRetries: 10, retryDelay: 50 });
  }
});

test("automatic setup runs the native first-run pass of the configured launch", async () => {
  const isolated = mkdtempSync(join(tmpdir(), "cortex-auto-start-"));
  const saved = new Map(LAUNCH_ENV.map((key) => [key, process.env[key]] as const));
  try {
    for (const key of LAUNCH_ENV) delete process.env[key];
    process.env.HOME = isolated;
    process.env.XDG_CONFIG_HOME = join(isolated, "config");
    process.env.CORTEX_PASEO_SOURCE_ROOT = fakeCheckout(join(isolated, "checkout"));
    const config = await loadServerModuleForTest<typeof import("./config.ts")>(join(pluginRoot, "server", "config.ts"));
    await config.startAutomaticSetup();
    assert.equal(existsSync(join(isolated, "automatic-ran")), true);

    // No Rust launch, no setup: nothing is started.
    rmSync(join(isolated, "automatic-ran"));
    delete process.env.CORTEX_PASEO_SOURCE_ROOT;
    process.env.CORTEX_ACP_COMMAND = JSON.stringify(["/bin/sh", "-c", "exit 0"]);
    await config.startAutomaticSetup();
    assert.equal(existsSync(join(isolated, "automatic-ran")), false);
  } finally {
    for (const [key, value] of saved) {
      if (value === undefined) delete process.env[key]; else process.env[key] = value;
    }
    rmSync(isolated, { recursive: true, force: true, maxRetries: 10, retryDelay: 50 });
  }
});

test("an explicit source root wins over an XDG launch record", async () => {
  const isolated = mkdtempSync(join(tmpdir(), "cortex-plugin-git-"));
  const saved = new Map(
    ["HOME", "XDG_CONFIG_HOME", "CORTEX_PASEO_SOURCE_ROOT", "CORTEX_ACP_COMMAND", "CORTEX_ACP_ENTRY"]
      .map((key) => [key, process.env[key]] as const),
  );
  try {
    const configHome = join(isolated, "config");
    const cortexConfig = join(configHome, "cortex");
    mkdirSync(cortexConfig, { recursive: true });
    writeFileSync(join(cortexConfig, "paseo-plugin-launch.json"), JSON.stringify({
      argv: ["/missing/cortex-acp"],
      shim: "/missing/acp-load-shim.mjs",
    }));
    process.env.HOME = isolated;
    process.env.XDG_CONFIG_HOME = configHome;
    // Loader check only, without the checkout's real native first-run setup.
    process.env.CORTEX_PASEO_SOURCE_ROOT = fakeCheckout(join(isolated, "checkout"));
    delete process.env.CORTEX_ACP_COMMAND;
    await evaluateLikePaseo();
  } finally {
    for (const [key, value] of saved) {
      if (value === undefined) delete process.env[key];
      else process.env[key] = value;
    }
    rmSync(isolated, { recursive: true, force: true, maxRetries: 10, retryDelay: 50 });
  }
});
