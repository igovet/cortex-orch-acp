/**
 * The only terminal-result fact a coordinator worker card carries: how many
 * of the worker's checks are still open issues. The summary, check names,
 * blockers and evidence gaps stay in the worker tab (durable attempt
 * history), never in the coordinator timeline item or its replay.
 *
 * Server side (`presentTimeline`) this replaces the runtime's raw result on
 * the persisted tool_call item; client side (`worker-tool-call-row`) it also
 * reduces an older persisted item that still carries the full
 * `cortexWorkerResult` projection, so a replayed session drops it too.
 */

export const WORKER_CARD_OUTCOME_KEY = "cortexWorkerOutcome";
export const LEGACY_WORKER_RESULT_KEY = "cortexWorkerResult";

export type WorkerCardOutcome = { openIssues: number; status?: "completed" | "failed" };

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** Open issues of a runtime `{summary, checks}` result: checks that did not
 * pass and that no exact later rerun closed. `undefined` for anything that
 * is not that result shape -- never guessed. */
export function openIssuesOfResult(value: unknown): number | undefined {
  if (!isRecord(value)) return undefined;
  if (value.summary !== undefined && value.summary !== null && typeof value.summary !== "string") return undefined;
  if (!Array.isArray(value.checks)) return undefined;
  let open = 0;
  for (const check of value.checks) {
    if (!isRecord(check) || typeof check.name !== "string" || typeof check.outcome !== "string") return undefined;
    const closed = typeof check.closedBy === "string" && check.closedBy.trim() !== ""
      || typeof check.closed_by === "string" && check.closed_by.trim() !== "";
    if (check.outcome !== "passed" && !closed) open += 1;
  }
  return open;
}

function parseOutcome(value: unknown): WorkerCardOutcome | undefined {
  if (!isRecord(value)) return undefined;
  const open = value.openIssues;
  if (typeof open !== "number" || !Number.isInteger(open) || open < 0) return undefined;
  return { openIssues: open, ...(value.status === "completed" || value.status === "failed" ? { status: value.status } : {}) };
}

/** The card outcome from a worker tool_call item's metadata, accepting the
 * current compact key or a legacy embedded result. */
export function workerCardOutcome(metadata: unknown, fallbackResult?: unknown): WorkerCardOutcome | undefined {
  const source = isRecord(metadata) ? metadata : {};
  const output = isRecord(fallbackResult) ? fallbackResult : {};
  const current = parseOutcome(source[WORKER_CARD_OUTCOME_KEY]) ?? parseOutcome(output[WORKER_CARD_OUTCOME_KEY]);
  if (current) return current;
  const legacy = openIssuesOfResult(source[LEGACY_WORKER_RESULT_KEY] ?? fallbackResult);
  return legacy === undefined ? undefined : { openIssues: legacy };
}
