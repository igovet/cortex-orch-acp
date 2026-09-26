import { build } from "esbuild";
import { mkdtempSync, rmSync } from "node:fs";
import { join } from "node:path";
import { pathToFileURL } from "node:url";

/**
 * Load one server module for direct `node --experimental-strip-types`
 * testing. These modules use `.js`-suffixed local imports (the NodeNext
 * convention `tsc --noEmit` accepts because it assumes a real build step
 * emits the matching `.js` file); there is no such build step here, so
 * Node's own loader cannot resolve them directly (confirmed in report
 * r_6df0ca1ceeb1). esbuild resolves local imports by the actual file on
 * disk regardless of the written extension, so bundling with real npm
 * packages left external (resolved normally by Node at import time)
 * produces a runnable ESM module backed by the exact unmodified source.
 */
export async function loadServerModuleForTest<T>(entryPoint: string): Promise<T> {
  // Bundled under the project's own node_modules so the externalized real
  // npm packages (@getpaseo/plugin, zod) still resolve at import time --
  // Node's resolution walks up from the importing file's own directory.
  const outDir = mkdtempSync(join(import.meta.dirname, "..", "node_modules", ".cortex-test-bundle-"));
  const outFile = join(outDir, "bundle.mjs");
  await build({
    entryPoints: [entryPoint],
    bundle: true,
    platform: "node",
    format: "esm",
    outfile: outFile,
    packages: "external",
    logLevel: "silent",
  });
  try {
    return (await import(pathToFileURL(outFile).href)) as T;
  } finally {
    // Safe once the ESM loader has read and cached the module's bytecode;
    // POSIX unlink of an open-but-unreferenced file leaves the running
    // process unaffected and just keeps node_modules clean between runs.
    rmSync(outDir, { recursive: true, force: true });
  }
}
