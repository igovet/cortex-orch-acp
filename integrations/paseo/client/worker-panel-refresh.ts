import type { WorkerHistory } from "../shared/worker-history.js";

type WorkerAssignment = WorkerHistory["assignments"][number];
type WorkerAttempt = WorkerAssignment["attempts"][number];
type WorkerEvent = WorkerAttempt["events"][number];

// The panel is a read-only view over the project-local store. ACP task events
// are durable SQLite rows, so a short bounded poll gives the
// selected panel near-real-time updates without requiring an unsupported
// Paseo subscription API.  The request itself carries the last event id and
// therefore does not re-read the transcript on every tick.
export const WORKER_HISTORY_POLL_MS = 750;

/**
 * Background polling must leave the current transcript mounted. The loading
 * affordance is only useful before the first response for a panel context;
 * showing it for every cursor request changes the list height and makes the
 * workspace jump while workers are streaming.
 */
export function shouldShowWorkerHistoryLoader(history: WorkerHistory | null): boolean {
  return history === null;
}

const TERMINAL_STATUSES = new Set([
  "reported", "completed", "succeeded", "success", "failed", "cancelled", "canceled", "error",
]);

/** Continue refreshing until at least one assignment is visible and all of its
 * latest attempts have reached a durable terminal status. Unbound and empty
 * histories remain live because delegation may bind/populate them shortly. */
export function shouldPollWorkerHistory(history: WorkerHistory | null): boolean {
  if (!history || !history.available || history.assignments.length === 0 || history.nextCursor != null) return true;
  return history.assignments.some((assignment: WorkerHistory["assignments"][number]) => {
    const attempt = assignment.attempts[assignment.attempts.length - 1];
    return !attempt || !TERMINAL_STATUSES.has(attempt.status);
  });
}

/** Merge a cursor response into the currently displayed history.
 *
 * Cursor responses contain only events after the supplied id, while attempt
 * status is repeated even when there are no new events. Preserve assignment
 * and attempt identity, append only unseen durable event ids, and update the
 * latest metadata so status transitions become visible immediately.
 */
export function mergeWorkerHistory(current: WorkerHistory, update: WorkerHistory): WorkerHistory {
  const byAssignment = new Map(current.assignments.map((assignment) => [assignment.id, assignment]));
  for (const incomingAssignment of update.assignments as WorkerAssignment[]) {
    const priorAssignment = byAssignment.get(incomingAssignment.id);
    if (!priorAssignment) {
      byAssignment.set(incomingAssignment.id, incomingAssignment);
      continue;
    }
    const byAttempt = new Map(priorAssignment.attempts.map((attempt) => [attempt.id, attempt]));
    for (const incomingAttempt of incomingAssignment.attempts as WorkerAttempt[]) {
      const priorAttempt = byAttempt.get(incomingAttempt.id);
      if (!priorAttempt) {
        byAttempt.set(incomingAttempt.id, incomingAttempt);
        continue;
      }
      const seen = new Set(priorAttempt.events.map((event: WorkerEvent) => event.id));
      byAttempt.set(incomingAttempt.id, {
        ...priorAttempt,
        ...incomingAttempt,
        // A cursor response can omit the earlier terminal result while the
        // graph row still says running. Do not regress a durable terminal
        // attempt (a retry is represented by a new attempt id).
        status: TERMINAL_STATUSES.has(priorAttempt.status) && !TERMINAL_STATUSES.has(incomingAttempt.status)
          ? priorAttempt.status
          : incomingAttempt.status,
        events: [...priorAttempt.events, ...incomingAttempt.events.filter((event: WorkerEvent) => !seen.has(event.id))]
          .sort((a: WorkerEvent, b: WorkerEvent) => a.id - b.id),
      });
    }
    const mergedAttempts = [...byAttempt.values()].sort((a, b) => a.attemptNo - b.attemptNo);
    const mergedLatest = mergedAttempts[mergedAttempts.length - 1];
    byAssignment.set(incomingAssignment.id, {
      ...priorAssignment,
      ...incomingAssignment,
      status: mergedLatest?.status ?? incomingAssignment.status,
      attempts: mergedAttempts,
    });
  }
  return {
    ...current,
    ...update,
    assignments: [...byAssignment.values()],
  };
}

export function historyCursor(history: WorkerHistory | null, attemptId?: string | null): number {
  if (attemptId) {
    const attempt = history?.assignments.flatMap(a => a.attempts).find(a => a.id === attemptId);
    return Math.max(0, ...(attempt?.events.map(e => e.id) ?? []));
  }
  return history?.assignments.reduce(
    (maxAssignment: number, assignment: WorkerAssignment) => Math.max(maxAssignment, ...assignment.attempts.flatMap((attempt: WorkerAttempt) => attempt.events.map((event: WorkerEvent) => event.id))),
    0,
  ) ?? 0;
}
