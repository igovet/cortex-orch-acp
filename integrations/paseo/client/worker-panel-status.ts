export type WorkerSection = "active" | "completed" | null;

export function workerSection(status: string | undefined, eventCount: number): WorkerSection {
  const value = (status ?? "").toLowerCase();
  if (["reported", "completed", "succeeded", "success", "failed", "cancelled", "canceled", "error"].includes(value)) return "completed";
  if (["running", "provisioning", "ready", "in_progress", "in-progress", "started"].includes(value)) return "active";
  // A planned/pending assignment with no durable activity has not started.
  if (eventCount > 0) return "active";
  return null;
}

export function workerStatus(status: string | undefined, eventCount: number): string {
  const section = workerSection(status, eventCount);
  if (section === "completed") return (status ?? "completed").toLowerCase() === "reported" ? "completed" : (status ?? "error").toLowerCase();
  if (section === "active") return "running";
  return "pending";
}
