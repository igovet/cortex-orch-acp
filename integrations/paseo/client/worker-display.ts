import type { WorkerHistory } from "../shared/worker-history.js";

type WorkerAssignment = WorkerHistory["assignments"][number];

/**
 * The coordinator timeline calls a worker by its resolved role (for example
 * `explorer`). Assignment refs are durable lookup keys and must never become
 * user-facing worker names.
 */
export function workerDisplayName(assignment: Pick<WorkerAssignment, "role" | "title" | "nodeKey">): string {
  const role = assignment.role?.trim();
  if (role) return role.replace(/^worker-/i, "");

  const title = assignment.title?.trim();
  // Older history rows occasionally used the internal delegate key as the
  // title. Prefer the graph node key in that case, which is the same role
  // identity used by the coordinator card when no resolved role is present.
  if (title && !/^delegate:[0-9a-f-]+$/i.test(title)) return title;
  const nodeKey = assignment.nodeKey.trim().replace(/^worker-/i, "");
  return nodeKey || "worker";
}
