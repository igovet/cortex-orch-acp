import { execFile } from "node:child_process";

/**
 * Asynchronous bounded process runner for the Rust store readers and the
 * settings tool. `spawnSync` would block the plugin's single Node event loop
 * for the whole subprocess runtime, serializing every other in-flight
 * `cortex-acp.invoke` call behind it; `execFile` runs the subprocess without
 * blocking the loop, so concurrent callers overlap.
 *
 * A small concurrency bound keeps a sudden burst (many worker cards or
 * panels refreshing at once) from forking unboundedly many processes; queued
 * callers still run as soon as a slot frees.
 */
const MAX_CONCURRENT_SPAWNS = 8;

let activeSpawns = 0;
const waiters: (() => void)[] = [];

function acquireSlot(): Promise<void> {
  if (activeSpawns < MAX_CONCURRENT_SPAWNS) {
    activeSpawns += 1;
    return Promise.resolve();
  }
  return new Promise((resolve) => waiters.push(resolve));
}

function releaseSlot(): void {
  activeSpawns -= 1;
  const next = waiters.shift();
  if (next) {
    activeSpawns += 1;
    next();
  }
}

export type ProcessRunResult = { status: number; stdout: string };

export type ProcessRunOptions = {
  timeoutMs: number;
  maxBuffer?: number;
  /** Written to the child's stdin, which is then closed. Keeps request
   * bodies (which may hold a new secret value) out of the argv. */
  input?: string;
  /** The child's whole environment; inherited when omitted. */
  env?: NodeJS.ProcessEnv;
  cwd?: string;
};

/** The store readers' fixed working directory: never a project or plugin
 * directory a planted module could be imported from. */
export const STORE_READER_CWD = "/";

/** Run one executable with optional stdin, environment and working
 * directory, bounded by the shared concurrency limit. A timeout or nonzero
 * exit is reported as status 1; a nonzero exit still returns whatever the
 * child wrote to stdout. */
export async function runProcess(command: string, args: readonly string[], options: ProcessRunOptions): Promise<ProcessRunResult> {
  await acquireSlot();
  try {
    const { stdout } = await execFileAsync(command, args, {
      encoding: "utf8",
      timeout: options.timeoutMs,
      maxBuffer: options.maxBuffer ?? 16 * 1024 * 1024,
      ...(options.env === undefined ? {} : { env: options.env }),
      ...(options.cwd === undefined ? {} : { cwd: options.cwd }),
    }, options.input);
    return { status: 0, stdout };
  } catch (error) {
    const stdout = isExecFileError(error) && typeof error.stdout === "string" ? error.stdout : "";
    return { status: 1, stdout };
  } finally {
    releaseSlot();
  }
}

type ExecOptions = { encoding: "utf8"; timeout: number; maxBuffer: number; env?: NodeJS.ProcessEnv; cwd?: string };

function execFileAsync(command: string, args: readonly string[], options: ExecOptions, input?: string): Promise<{ stdout: string }> {
  return new Promise((resolve, reject) => {
    const child = execFile(command, args as string[], options, (error, stdout) => {
      if (error) reject(Object.assign(error, { stdout }));
      else resolve({ stdout });
    });
    if (input !== undefined && child.stdin) {
      // A child that exits before reading surfaces through the callback.
      child.stdin.on("error", () => {});
      child.stdin.end(input);
    }
  });
}

function isExecFileError(value: unknown): value is { stdout?: unknown } {
  return typeof value === "object" && value !== null;
}
