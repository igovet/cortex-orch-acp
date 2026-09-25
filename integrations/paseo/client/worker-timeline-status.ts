export type WorkerTimelineStatus = "running" | "completed" | "canceled" | "failed" | "working";

/** The only worker timeline state that should keep a visual activity indicator alive. */
export function isWorkerTimelineRunning(status: string | undefined): boolean {
  const value = (status ?? "").toLowerCase();
  return value === "running" || value === "working";
}
