import type { StepItem } from "./pipeline-model.ts";

type WorkerCheck = {
  name: string; outcome: string; blocker?: string;
  closedBy?: string; claimedOutcome?: string; evidenceGap?: string;
};

export function workerResultItems(checks: readonly WorkerCheck[]): StepItem[] {
  return checks.map((check) => ({
    text: check.name,
    // A failed check an exact later rerun closed stays visible as resolved
    // history, never as a current open failure or a downgraded warning.
    status: check.outcome === "failed" && check.closedBy ? "resolved"
      : check.outcome === "passed" ? "done" : check.outcome === "failed" ? "failed" : "open",
    ...(check.blocker?.trim() && (check.outcome === "not_run" || check.outcome === "unverified") ? { blocker: check.blocker } : {}),
    // claimedOutcome/evidenceGap: the runtime kept a "passed" claim without
    // sufficient evidence (or an older result predates the strict rule).
    ...(check.claimedOutcome || check.evidenceGap ? { weakEvidence: true } : {}),
  }));
}
