import { existsSync, readFileSync, statSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

/** What the production graph may never hold. The check reads source text, so it also catches a dynamic import. */
const FORBIDDEN_MODULE = /(^|\/)(sim|simulator|autoplay|mock|mocks|fixture|fixtures|scenarios|demo)(-[a-z]+)?\.(ts|tsx|js|mjs)$|(^|\/)[a-z-]*-fixture\.ts$|\/adapter\//;
const FORBIDDEN_WORD = /\b(createSimulator|createMockWorkspace|createMockControls|createDemoBackend|applyCommand|handleTask|handleGit|handleSession|handleSprint|handleArchive|handleConfig|handleMachine|generateMockTitle|mockConnect|loadFixtures|WORKSPACE_CASES|MOCK_MODELS|MOCK_EFFORTS|presetRouting|presetServers)\b/;
/** Text a production build must never show. */
const FORBIDDEN_TEXT = [/mock-machine-/, /mock-project-/, /Simulate the next step/, /Play the simulation/, /Demo — simulated data/, /Reset prototype/, /fixture data/i];

const PACKAGES = fileURLToPath(new URL("../../", import.meta.url));
const ALIASES: [RegExp, string][] = [
  [/^@cortex\/workspace-contract$/, `${PACKAGES}workspace-contract/index.js`],
  [/^@cortex\/workspace-domain$/, `${PACKAGES}workspace-domain/src/index.ts`],
  [/^@cortex\/workspace-domain\/(.*)$/, `${PACKAGES}workspace-domain/src/$1`],
  [/^@cortex\/workspace-ui$/, `${PACKAGES}workspace-ui/src/index.ts`],
  [/^@cortex\/workspace-ui\/(.*)$/, `${PACKAGES}workspace-ui/src/$1`],
];
const EXTENSIONS = ["", ".ts", ".tsx", ".js", ".mjs", "/index.ts", "/index.tsx", "/index.js"];
const IMPORT = /(?:^|[\s;}])(?:import|export)\s+(type\s+)?(?:[^"'`;]*?\sfrom\s+)?["']([^"']+)["']|\bimport\(\s*(?:\/\*.*?\*\/\s*)?["']([^"']+)["']\s*\)/g;

function resolveFile(from: string, specifier: string): string | null {
  let path = specifier;
  for (const [pattern, target] of ALIASES) if (pattern.test(path)) { path = path.replace(pattern, target); break; }
  if (!path.startsWith("/")) { if (!path.startsWith(".")) return null; path = resolve(dirname(from), path); }
  for (const extension of EXTENSIONS) {
    const candidate = path + extension;
    if (existsSync(candidate) && statSync(candidate).isFile()) return candidate;
  }
  throw new Error(`Cannot resolve ${specifier} from ${from}`);
}

export type Graph = { files: string[]; bare: string[] };
/** Every module reachable from an entry through runtime imports. A type-only import leaves no code behind, so it does not count. */
export function graphOf(entry: string): Graph {
  const files = new Set<string>(), bare = new Set<string>(), queue = [entry];
  while (queue.length) {
    const file = queue.pop()!;
    if (files.has(file)) continue;
    files.add(file);
    if (!/\.(ts|tsx|js|mjs)$/.test(file)) continue;
    const text = readFileSync(file, "utf8");
    for (const match of text.matchAll(IMPORT)) {
      if (match[1]) continue;
      const specifier = match[2] ?? match[3];
      const target = resolveFile(file, specifier);
      if (target) queue.push(target); else bare.add(specifier);
    }
  }
  return { files: [...files], bare: [...bare] };
}

export function sourceViolations(graph: Graph): string[] {
  const out: string[] = [];
  for (const file of graph.files) {
    if (/\/prototypes\//.test(file)) out.push(`prototype module in the graph: ${file}`);
    if (FORBIDDEN_MODULE.test(file)) out.push(`forbidden module in the graph: ${file}`);
    if (!/\.(ts|tsx|js|mjs)$/.test(file)) continue;
    const word = FORBIDDEN_WORD.exec(readFileSync(file, "utf8"));
    if (word) out.push(`${file}: forbidden name ${word[0]}`);
  }
  return out;
}

export function bundleViolations(chunks: { modules: string[]; code: string }[]): string[] {
  const out: string[] = [];
  for (const chunk of chunks) {
    for (const module of chunk.modules) {
      const path = module.split("?")[0];
      if (/\/prototypes\//.test(path) || FORBIDDEN_MODULE.test(path)) out.push(`forbidden module in the bundle: ${path}`);
    }
    for (const text of FORBIDDEN_TEXT) if (text.test(chunk.code)) out.push(`forbidden text in the bundle: ${text}`);
    const word = FORBIDDEN_WORD.exec(chunk.code);
    if (word) out.push(`forbidden name in the bundle: ${word[0]}`);
  }
  return out;
}
