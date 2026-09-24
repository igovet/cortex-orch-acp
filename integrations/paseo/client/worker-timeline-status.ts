import type { WorkerHistory } from "../shared/worker-history.js";
import { workerPopoverStatus } from "./worker-popover-status.ts";

export type WorkerTimelineStatus = "running" | "completed" | "canceled" | "failed" | "working";

/** The only worker timeline state that should keep a visual activity indicator alive. */
export function isWorkerTimelineRunning(status: string | undefined): boolean {
  const value = (status ?? "").toLowerCase();
  return value === "running" || value === "working";
}

/**
 * The host timeline preserves only a coarse lifecycle status. Once the
 * pipeline has the durable assignment, use its latest attempt/result markers,
 * just as Pipeline and the worker header do. This retains cancellation and
 * completed-with-issues instead of treating both as a generic host failure.
 */
export function workerTimelineStatus(
  assignment: WorkerHistory["assignments"][number] | undefined,
  hostStatus: string,
): string {
  return assignment ? workerPopoverStatus(assignment) : hostStatus;
}
