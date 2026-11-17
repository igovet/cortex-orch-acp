// @vitest-environment node
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import ts from "typescript";
import { expect, it } from "vitest";

const at = (path: string) => fileURLToPath(new URL(path, import.meta.url));
const config = at("../tsconfig.json");

it("the entire TypeScript program, including type-only imports and tests, excludes Paseo", () => {
  const loaded = ts.readConfigFile(config, ts.sys.readFile);
  expect(loaded.error).toBeUndefined();
  const parsed = ts.parseJsonConfigFileContent(loaded.config, ts.sys, at("../"));
  expect(parsed.errors).toEqual([]);
  const program = ts.createProgram(parsed.fileNames, parsed.options);
  const files = program.getSourceFiles().map(file => file.fileName.replaceAll("\\", "/"));
  expect(files.some(file => file.includes("/workspace-ui/src/tool-names.ts"))).toBe(true);
  expect(files.filter(file => file.includes("/integrations/paseo/"))).toEqual([]);
});

it("the self-contained Paseo distribution retains exactly the same pure presentation implementation", () => {
  // Paseo is installed as a standalone directory; keep its vendored pure files
  // in lockstep without importing or compiling the plugin from this package.
  for (const name of ["tool-presentation", "gateway-presentation", "shell-command", "timeline-visibility"]) {
    expect(readFileSync(at(`../../workspace-domain/src/${name}.ts`), "utf8"))
      .toBe(readFileSync(at(`../../../integrations/paseo/shared/${name}.ts`), "utf8"));
  }
  const groups = readFileSync(at("../../../integrations/paseo/client/worker-panel-groups.ts"), "utf8")
    .replaceAll("../shared/", "./")
    .replace("./worker-panel-events.ts", "./worker-history.ts");
  expect(readFileSync(at("../../workspace-domain/src/worker-panel-groups.ts"), "utf8")).toBe(groups);
});
