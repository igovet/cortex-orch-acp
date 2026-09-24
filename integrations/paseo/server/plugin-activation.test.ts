import assert from "node:assert/strict";
import { mkdtempSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import test from "node:test";
import { build } from "esbuild";

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
