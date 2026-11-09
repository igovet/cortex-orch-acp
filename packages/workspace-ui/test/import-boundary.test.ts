// @vitest-environment node
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { build, type Rollup } from "vite";
import { bundleViolations, graphOf, sourceViolations } from "./boundary";

const at = (path: string) => fileURLToPath(new URL(path, import.meta.url));
const ENTRY = at("../production/main.tsx");

describe("import boundary", () => {
  it("the production graph holds no simulator, autoplay, mock mutation or fixture", () => {
    const graph = graphOf(ENTRY);
    expect(graph.files.length).toBeGreaterThan(30);
    expect(graph.files.some(f => f.endsWith("workspace-domain/src/mapping.ts"))).toBe(true);
    expect(graph.files.some(f => f.endsWith("workspace-ui/src/port/real.ts"))).toBe(true);
    expect(sourceViolations(graph)).toEqual([]);
  });

  it("the shared packages stay free of them as a whole, not just along one entry", () => {
    expect(sourceViolations(graphOf(at("../src/index.ts")))).toEqual([]);
    expect(sourceViolations(graphOf(at("../../workspace-domain/src/index.ts")))).toEqual([]);
  });

  it("the domain package holds no component and no store", () => {
    const graph = graphOf(at("../../workspace-domain/src/index.ts"));
    expect(graph.files.every(f => f.includes("/packages/workspace-domain/") || f.includes("/packages/workspace-contract/"))).toBe(true);
    expect(graph.bare.filter(name => name.startsWith("solid-js"))).toEqual([]);
  });

  it("fails for an import that crosses into the prototype", () => {
    const dir = mkdtempSync(join(tmpdir(), "cw-boundary-"));
    try {
      const prototype = join(dir, "prototypes/cortex-workspace/desktop/src/workspace/ui");
      mkdirSync(prototype, { recursive: true });
      writeFileSync(join(prototype, "sim.ts"), "export const createSimulator = () => 1;\n");
      const entry = join(dir, "entry.ts");
      writeFileSync(entry, `import { createSimulator } from "./prototypes/cortex-workspace/desktop/src/workspace/ui/sim";\ncreateSimulator();\n`);
      const found = sourceViolations(graphOf(entry));
      expect(found.some(v => v.includes("prototype module"))).toBe(true);
      expect(found.some(v => v.includes("createSimulator"))).toBe(true);
    } finally { rmSync(dir, { recursive: true, force: true }); }
  });

  it("fails for a forbidden module or word inside a package, even with no prototype path", () => {
    const dir = mkdtempSync(join(tmpdir(), "cw-boundary-"));
    try {
      const entry = join(dir, "entry.ts");
      writeFileSync(join(dir, "fixtures.ts"), "export const fixture = 1;\n");
      writeFileSync(join(dir, "autoplay.ts"), "export const a = 1;\n");
      writeFileSync(entry, `import { fixture } from "./fixtures";\nimport "./autoplay";\nconst x = await import("./lazy");\nconsole.log(fixture, x, loadFixtures());\n`);
      writeFileSync(join(dir, "lazy.ts"), "export const applyCommand = 1;\n");
      const found = sourceViolations(graphOf(entry)).join("\n");
      expect(found).toMatch(/fixtures\.ts/);
      expect(found).toMatch(/autoplay\.ts/);
      expect(found).toMatch(/lazy\.ts.*applyCommand/);
      expect(found).toMatch(/loadFixtures/);
    } finally { rmSync(dir, { recursive: true, force: true }); }
  });

  it("the built production bundle really contains none of it", async () => {
    const result = await build({ configFile: at("../vite.config.ts"), logLevel: "silent", build: { write: false, minify: false } }) as Rollup.RollupOutput | Rollup.RollupOutput[];
    const outputs = (Array.isArray(result) ? result : [result]).flatMap(r => r.output);
    const chunks = outputs.flatMap(o => o.type === "chunk" ? [{ modules: Object.keys(o.modules), code: o.code }] : []);
    expect(chunks.length).toBeGreaterThan(0);
    expect(chunks.flatMap(c => c.modules).some(m => m.includes("/packages/workspace-ui/src/port/real.ts"))).toBe(true);
    expect(bundleViolations(chunks)).toEqual([]);
  }, 60000);

  it("the bundle check itself fails on a leak", () => {
    expect(bundleViolations([{ modules: ["/x/prototypes/cortex-workspace/desktop/src/workspace/ui/sim.ts"], code: "" }])).not.toEqual([]);
    expect(bundleViolations([{ modules: ["/x/a.ts"], code: "const id = 'mock-machine-00000';" }])).not.toEqual([]);
    expect(bundleViolations([{ modules: ["/x/a.ts"], code: "label('Demo — simulated data, no real actions')" }])).not.toEqual([]);
  });
});
