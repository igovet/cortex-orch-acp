import { execFile } from "node:child_process";

/**
 * Async replacement for the store readers' former `spawnSync("python3", ...)`.
 * `spawnSync` blocks the plugin's single Node event loop for the whole
 * subprocess runtime, serializing every other in-flight `cortex-acp.invoke`
 * call behind it (measured in report r_6df0ca1ceeb1: ~52ms/call at every
 * concurrency level from 1 to 40, i.e. zero overlap). `execFile` runs the
 * subprocess without blocking the loop, so concurrent callers overlap.
 *
 * A small concurrency bound keeps a sudden burst (many worker cards or
 * panels refreshing at once) from forking unboundedly many python3
 * processes; it does not reintroduce serialization since the bound is well
 * above realistic simultaneous callers and queued callers still run as soon
 * as a slot frees, not after every other queued call completes.
 */
const MAX_CONCURRENT_PYTHON_SPAWNS = 8;

let activeSpawns = 0;
const waiters: (() => void)[] = [];

function acquireSlot(): Promise<void> {
  if (activeSpawns < MAX_CONCURRENT_PYTHON_SPAWNS) {
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

export type PythonRunResult = { status: number; stdout: string };

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

/** Run `python3 -B -c <script> ...args` asynchronously, bounded by the
 * shared concurrency limit above. Mirrors the `spawnSync` result shape
 * (`status`/`stdout`) the store readers already check, so callers are
 * unchanged beyond adding `await`. A timeout or nonzero exit is reported as
 * a nonzero `status`, matching `spawnSync`'s own timeout behavior (`status`
 * is `null`, which the existing `!== 0` checks already treat as failure). */
export async function runPython(args: readonly string[], options: { timeoutMs: number; maxBuffer?: number }): Promise<PythonRunResult> {
  return runProcess("python3", args, options);
}

/** `runPython` for an explicit interpreter, with optional stdin, environment
 * and working directory; shares the same concurrency bound. A nonzero exit
 * still returns whatever the child wrote to stdout. */
export async function runProcess(command: string, args: readonly string[], options: ProcessRunOptions): Promise<PythonRunResult> {
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
