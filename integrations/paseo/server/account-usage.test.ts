import assert from "node:assert/strict";
import { mkdtempSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { resolve } from "node:path";
import test from "node:test";
import { nativeAccountUsage } from "./provider.ts";

test("reads the installed Cortex account-usage path without an engine call", async () => {
  const isolated = mkdtempSync(resolve(tmpdir(), "cortex-account-usage-"));
  const saved = new Map(
    ["HOME", "XDG_CONFIG_HOME", "XDG_STATE_HOME", "CORTEX_ACP_COMMAND", "CORTEX_PASEO_SOURCE_ROOT"]
      .map((key) => [key, process.env[key]] as const),
  );
  try {
    process.env.HOME = isolated;
    process.env.XDG_CONFIG_HOME = resolve(isolated, "config");
    process.env.XDG_STATE_HOME = resolve(isolated, "state");
    process.env.CORTEX_PASEO_SOURCE_ROOT = resolve(".");
    process.env.CORTEX_ACP_COMMAND = JSON.stringify([
      "python3", "-B", resolve("src/cortex_acp.py"),
    ]);
    assert.deepEqual(await nativeAccountUsage(), []);
  } finally {
    for (const [key, value] of saved) {
      if (value === undefined) delete process.env[key];
      else process.env[key] = value;
    }
    rmSync(isolated, { recursive: true, force: true });
  }
});

test("isolated XDG profile uses this source tree before a stale home launch record", async () => {
  const isolated = mkdtempSync(resolve(tmpdir(), "cortex-account-usage-priority-"));
  const saved = new Map(
    ["HOME", "XDG_CONFIG_HOME", "XDG_STATE_HOME", "CORTEX_ACP_COMMAND", "CORTEX_PASEO_SOURCE_ROOT"]
      .map((key) => [key, process.env[key]] as const),
  );
  try {
    const regularConfig = resolve(isolated, ".config", "cortex");
    mkdirSync(regularConfig, { recursive: true });
    writeFileSync(resolve(regularConfig, "paseo-plugin-launch.json"), JSON.stringify({
      argv: ["python3", "-B", "-c", "raise SystemExit(27)"],
      shim: resolve("integrations/paseo/server/acp-load-shim.mjs"),
    }));
    process.env.HOME = isolated;
    process.env.XDG_CONFIG_HOME = resolve(isolated, "private-config");
    process.env.XDG_STATE_HOME = resolve(isolated, "private-state");
    process.env.CORTEX_PASEO_SOURCE_ROOT = resolve(".");
    delete process.env.CORTEX_ACP_COMMAND;
    assert.deepEqual(await nativeAccountUsage(), []);
  } finally {
    for (const [key, value] of saved) {
      if (value === undefined) delete process.env[key];
      else process.env[key] = value;
    }
    rmSync(isolated, { recursive: true, force: true });
  }
});

test("production launch record survives a different daemon XDG config root", async () => {
  const isolated = mkdtempSync(resolve(tmpdir(), "cortex-account-usage-home-"));
  const saved = new Map(
    ["HOME", "XDG_CONFIG_HOME", "XDG_STATE_HOME", "CORTEX_ACP_COMMAND", "CORTEX_PASEO_SOURCE_ROOT"]
      .map((key) => [key, process.env[key]] as const),
  );
  try {
    const config = resolve(isolated, ".config", "cortex");
    mkdirSync(config, { recursive: true });
    writeFileSync(resolve(config, "paseo-plugin-launch.json"), JSON.stringify({
      argv: ["python3", "-B", resolve("src/cortex_acp.py")],
      shim: resolve("integrations/paseo/server/acp-load-shim.mjs"),
    }));
    process.env.HOME = isolated;
    process.env.XDG_CONFIG_HOME = resolve(isolated, "different-config");
    process.env.XDG_STATE_HOME = resolve(isolated, "state");
    delete process.env.CORTEX_ACP_COMMAND;
    delete process.env.CORTEX_PASEO_SOURCE_ROOT;
    assert.deepEqual(await nativeAccountUsage(), []);
  } finally {
    for (const [key, value] of saved) {
      if (value === undefined) delete process.env[key];
      else process.env[key] = value;
    }
    rmSync(isolated, { recursive: true, force: true });
  }
});
