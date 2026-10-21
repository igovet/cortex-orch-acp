import type { WorkerHistory } from "../shared/worker-history.ts";

type Attempt = WorkerHistory["assignments"][number]["attempts"][number];
type Check = NonNullable<Attempt["checks"]>[number];

export type AttemptResultInput = Pick<Attempt, "status" | "summary" | "summaryTruncated" | "checks">;

export type AttemptCheckView = {
  name: string;
  label: string;
  tone: "success" | "danger" | "warning" | "muted";
  icon: string;
  weakEvidence: boolean;
  details: string[];
  accessibilityLabel: string;
};

export type AttemptResultView = {
  summary: string | null;
  summaryTruncated: boolean;
  /** `null`: the result carries no check data at all ("Check details
   * unavailable"); `[]`: a known empty list ("No checks reported"). */
  checks: AttemptCheckView[] | null;
  openCount: number;
};

/** Attempt statuses that mean the worker submitted a result. */
const RESULT_STATUSES = new Set(["completed", "completed_with_issues", "reported", "succeeded", "success"]);

function present(check: Check): AttemptCheckView {
  const outcome = check.outcome.toLowerCase();
  const resolved = outcome === "failed" && Boolean(check.closedBy);
  const presentation = resolved ? { label: "Resolved", tone: "muted" as const, icon: "History" }
    : outcome === "passed" ? { label: "Passed", tone: "success" as const, icon: "CircleCheck" }
    : outcome === "failed" ? { label: "Failed", tone: "danger" as const, icon: "CircleX" }
    : outcome === "not_run" ? { label: "Not run", tone: "warning" as const, icon: "Circle" }
    : outcome === "unverified" ? { label: "Unverified", tone: "warning" as const, icon: "Circle" }
    : { label: check.outcome || "Unknown", tone: "muted" as const, icon: "Circle" };
  const details: string[] = [];
  const blocker = check.blocker?.trim();
  const gap = check.evidenceGap?.trim();
  if (blocker) details.push(blocker);
  if (gap && gap !== blocker) details.push(`Evidence gap: ${gap}`);
  if (check.claimedOutcome && check.claimedOutcome !== check.outcome) details.push(`Claimed ${check.claimedOutcome}; kept ${presentation.label.toLowerCase()} by the runtime.`);
  if (resolved) details.push("Closed by a later identical rerun.");
  const weakEvidence = Boolean(check.claimedOutcome || check.evidenceGap);
  return {
    name: check.name,
    ...presentation,
    weakEvidence,
    details,
    accessibilityLabel: [check.name, presentation.label, weakEvidence ? "weak evidence" : null, ...details].filter(Boolean).join(". "),
  };
}

/**
 * The selected attempt's Result/Checks view, or `null` when it has none to
 * show: an active attempt, or one that ended without submitting a result
 * (its failure reason is shown separately). Bound to this exact attempt's
 * own retained summary and checks -- never another attempt's result.
 */
export function attemptResultView(attempt: AttemptResultInput | null | undefined): AttemptResultView | null {
  if (!attempt) return null;
  const hasResult = attempt.summary !== undefined || attempt.checks !== undefined
    || RESULT_STATUSES.has(attempt.status.toLowerCase());
  if (!hasResult) return null;
  const checks = attempt.checks ? attempt.checks.map(present) : null;
  return {
    summary: attempt.summary?.trim() ? attempt.summary : null,
    summaryTruncated: attempt.summaryTruncated === true,
    checks,
    openCount: attempt.checks ? attempt.checks.filter((check) => check.outcome !== "passed" && !check.closedBy).length : 0,
  };
}
