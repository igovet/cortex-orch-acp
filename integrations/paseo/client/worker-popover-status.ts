import type { WorkerHistory } from "../shared/worker-history.js";

type Assignment = WorkerHistory["assignments"][number];

/** Prefer terminal signals retained on the exact latest attempt over a lagging graph row. */
export function workerPopoverStatus(assignment: Assignment): string {
  const attempt = assignment.attempts[assignment.attempts.length - 1];
  const status = (attempt?.status ?? assignment.status ?? "").toLowerCase();
  // The server normalizes this from the latest durable terminal marker,
  // independently of the bounded event page. Prefer it over older events
  // retained in that page.
  if (status === "completed_with_issues") return status;
  if (["reported", "completed", "succeeded", "success"].includes(status)) return "completed";
  if (["failed", "error"].includes(status)) return status;
  if (["cancelled", "canceled"].includes(status)) return status;
  const eventTypes = new Set((attempt?.events ?? []).map((event) => event.type));
  if (eventTypes.has("worker_result")) return "completed";
  if (eventTypes.has("assignment_failed")) return "failed";
  if (eventTypes.has("assignment_cancelled")) return "cancelled";
  if (["running", "provisioning", "ready", "in_progress", "in-progress", "started"].includes(status)) return "running";
  return (attempt?.events.length ?? 0) > 0 ? "running" : "pending";
}

/** Running first; terminal attempts use their durable completion date, newest first.
 * This date is independent of the event page, which may contain only startup.
 */
export function orderPopoverWorkers(assignments: readonly Assignment[]): Assignment[] {
  const active = (a: Assignment) => ["running", "pending"].includes(workerPopoverStatus(a));
  const completedAt = (a: Assignment) => {
    const latest = a.attempts[a.attempts.length - 1];
    const value = latest?.completedAt ? Date.parse(latest.completedAt) : NaN;
    return Number.isFinite(value) ? value : -Infinity;
  };
  return [...assignments].sort((a, b) => {
    const aActive = active(a), bActive = active(b);
    if (aActive !== bActive) return aActive ? -1 : 1;
    if (aActive) return 0;
    const aDate = completedAt(a), bDate = completedAt(b);
    return aDate === bDate ? 0 : aDate > bDate ? -1 : 1;
  });
}
