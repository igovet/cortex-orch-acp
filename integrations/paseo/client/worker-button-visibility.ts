import type { WorkerHistory } from "../shared/worker-history.js";

/**
 * The Workers affordance belongs to the current ACP agent, not to a project
 * or to the global work graph.  An unavailable/ambiguous history must stay
 * hidden rather than exposing a panel populated from another session.
 */
export function hasCurrentSessionWorkers(history: Pick<WorkerHistory, "available" | "assignments"> | null | undefined): boolean {
  return history?.available === true && history.assignments.length > 0;
}
