import type { WorkerHistory } from "../shared/worker-history.js";

type Assignment = WorkerHistory["assignments"][number];

/** Prefer terminal signals retained on the exact latest attempt over a lagging graph row. */
export function workerPopoverStatus(assignment: Assignment): string {
  const attempt = assignment.attempts[assignment.attempts.length - 1];
  const status = (attempt?.status ?? assignment.status ?? "").toLowerCase();
  // The server normalizes this from the latest durable terminal marker,
  // independently of the bounded event page. Prefer it over older events
  // retained in that page.
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
