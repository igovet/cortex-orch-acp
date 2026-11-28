import { existsSync } from "node:fs";
import { basename, dirname, isAbsolute, join } from "node:path";
import { cortexLaunchTarget } from "./provider.ts";
import { runProcess, STORE_READER_CWD, type ProcessRunResult } from "./process-runner.ts";

/**
 * One read-only store read, served by `cortex-paseo-tools` beside the Rust
 * `cortex-acp` entry this plugin launches (a launch argv of exactly one
 * absolute path named `cortex-acp`, from `CORTEX_ACP_RUST_BINARY`, a
 * launch record or the source tree's built binary). Cortex has no other
 * entry: any other launch shape, and a missing tool, is refused with an
 * explicit message and nothing is read.
 */
export type StoreReader = "project-root" | "task-id" | "worker-history" | "worker-results" | "running-commands" | "pipeline";

export const RUST_ACP_BINARY = "cortex-acp";
export const RUST_TOOLS_BINARY = "cortex-paseo-tools";

export const RUST_TOOLS_MISSING = "Cortex runs from the Rust cortex-acp entry, but the cortex-paseo-tools binary it reads "
  + "the workspace store with is missing beside it. Build or install cortex-paseo-tools next to cortex-acp; nothing was read.";

export const RUST_ENTRY_REQUIRED = "No Rust cortex-acp launch is configured for this Paseo daemon; the workspace store "
  + "cannot be read. Run the Cortex Paseo setup (cortex-paseo-tools bootstrap-paseo-git-install); nothing was read.";

export type StoreReaderRoute =
  | { kind: "rust"; binary: string }
  | { kind: "refused"; message: string };

type LaunchTarget = { argv: string[]; env?: Record<string, string> } | undefined;

/** True for the Rust `cortex-acp` launch shape. */
export function isRustLaunch(target: LaunchTarget): target is { argv: [string]; env?: Record<string, string> } {
  return target !== undefined && target.argv.length === 1 && isAbsolute(target.argv[0])
    && !target.argv[0].includes("\0") && basename(target.argv[0]) === RUST_ACP_BINARY;
}

export function storeReaderRoute(target: LaunchTarget = cortexLaunchTarget()): StoreReaderRoute {
  if (!isRustLaunch(target)) return { kind: "refused", message: RUST_ENTRY_REQUIRED };
  const binary = join(dirname(target.argv[0]), RUST_TOOLS_BINARY);
  return existsSync(binary) ? { kind: "rust", binary } : { kind: "refused", message: RUST_TOOLS_MISSING };
}

/** The refusal message when store reads cannot run for this launch, else null. */
export function storeReaderRefusal(target: LaunchTarget = cortexLaunchTarget()): string | null {
  const route = storeReaderRoute(target);
  return route.kind === "refused" ? route.message : null;
}

export type StoreReadResult = ProcessRunResult & { refusal?: string };

/** Run `reader` with the reader's argv. Status 0 with stdout, or nonzero. */
export async function runStoreReader(
  reader: StoreReader,
  args: readonly string[],
  options: { timeoutMs: number; maxBuffer?: number },
): Promise<StoreReadResult> {
  const launch = cortexLaunchTarget();
  const route = storeReaderRoute(launch);
  if (route.kind === "refused") return { status: 1, stdout: "", refusal: route.message };
  // The reader shares ACP's host-selected state root. No client fields or
  // unrelated inherited variables influence its environment.
  const env: NodeJS.ProcessEnv = {};
  for (const name of ["HOME", "XDG_STATE_HOME"] as const) {
    const value = launch?.env?.[name] ?? process.env[name];
    if (value !== undefined) env[name] = value;
  }
  return runProcess(route.binary, [reader, ...args], { ...options, cwd: STORE_READER_CWD, env });
}
