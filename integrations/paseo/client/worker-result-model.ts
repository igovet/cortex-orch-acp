import type { StepItem } from "./pipeline-model.ts";

export function workerResultItems(checks: readonly { name: string; outcome: string; blocker?: string }[]): StepItem[] {
  return checks.map((check) => ({
    text: check.name,
    status: check.outcome === "passed" ? "done" : check.outcome === "failed" ? "failed" : "open",
    ...(check.blocker?.trim() && (check.outcome === "not_run" || check.outcome === "unverified") ? { blocker: check.blocker } : {}),
  }));
}
