import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { expect, it } from "vitest";
import { KNOWN_CORTEX_TOOL_NAMES, presentToolSemantics } from "../src/tool-presentation.ts";
import { nativeRow } from "../src/worker-panel-groups.ts";

/** Every tool the runtime advertises, read from the live schema catalogue rather than a copied list. */
const advertised: string[] = [...new Set(Object.values((JSON.parse(readFileSync(fileURLToPath(new URL("../../../crates/cortex-engine/schema/tools.json", import.meta.url)), "utf8")) as {
  tools: Record<string, { name: string }>;
}).tools).map(tool => tool.name))];
/** The only advertised tools that run a command. */
const COMMANDS = new Set(["command_run", "outside_command"]);

it("names every advertised Cortex tool, whatever shape or kind the engine reports it with", () => {
  expect(advertised.length).toBeGreaterThan(30);
  for (const name of advertised) {
    expect(KNOWN_CORTEX_TOOL_NAMES).toContain(name);
    // Codex reports every MCP call with the kind `execute`; Claude with `other`.
    for (const [title, toolKind] of [[`mcp.cortex.${name}`, "execute"], [`mcp__cortex__${name}`, "other"], [`mcp__cortex__${name}`, "execute"], [`cortex.${name}`, "execute"]]) {
      const row = presentToolSemantics({ name: title, title, toolKind, input: {} });
      expect(row.action, title).not.toBe("");
      expect(row.label, title).not.toMatch(/mcp|cortex|_/i);
      if (!COMMANDS.has(name)) {
        expect(row.kind, `${title} (${toolKind})`).not.toBe("shell");
        expect(row.label, `${title} (${toolKind})`).not.toMatch(/^Shell\b/);
      }
    }
  }
});

it("shows a Codex coordinator's route listing as itself, not as an empty Shell row", () => {
  // The recorded call: `mcp.cortex.list_route_options` with kind `execute` and no arguments.
  const row = nativeRow({ id: 1, type: "tool_call", text: "", createdAt: "", title: "mcp.cortex.list_route_options", tool_kind: "execute", raw_input: {} });
  expect(row.label).toBe("List model routes");
  expect(presentToolSemantics({ title: "mcp.cortex.list_route_options", toolKind: "execute", input: {} }).kind).toBe("read");
});

it("keeps a command's own command line on its row", () => {
  const row = presentToolSemantics({ name: "mcp.cortex.outside_command", toolKind: "execute", rawInput: { argv: ["git", "status"] } });
  expect(row.kind).toBe("shell");
  expect(row.label).toBe("Shell outside sandbox: git status");
  expect(presentToolSemantics({ name: "mcp__cortex__check_preflight", rawInput: { argv: ["node", "--version"] } }).label).toBe("Check tool availability: node --version");
});
