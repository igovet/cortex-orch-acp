export type WorkerSection = "active" | "completed";

export function workerSection(status: string | undefined, eventCount: number): WorkerSection {
  const value = (status ?? "").toLowerCase();
  // A host-disconnect pause is not running; it belongs with the other
  // terminal-for-now cards, not the live "active" section.
  if (["reported", "completed", "completed_with_issues", "succeeded", "success", "failed", "cancelled", "canceled", "error", "interrupted"].includes(value)) return "completed";
  if (["running", "provisioning", "ready", "in_progress", "in-progress", "started"].includes(value)) return "active";
  // The durable assignment is enough to show its worker immediately. The
  // first engine event may arrive later while the worker is provisioning.
  return "active";
}

export function workerStatus(status: string | undefined, eventCount: number): string {
  if ((status ?? "").toLowerCase() === "completed_with_issues") return "completed with issues";
  const section = workerSection(status, eventCount);
  if (section === "completed") return (status ?? "completed").toLowerCase() === "reported" ? "completed" : (status ?? "error").toLowerCase();
  if (section === "active") return "running";
  return "pending";
}
