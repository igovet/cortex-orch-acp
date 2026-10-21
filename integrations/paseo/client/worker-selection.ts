import type { WorkerHistory } from "../shared/worker-history.js";

type Listener = () => void;

const listeners = new Set<Listener>();

/** Everything the Workers panel shows. Opening a file tab navigates Paseo's
 * router, which remounts the panel and can re-evaluate this module, so the
 * last view is kept in sessionStorage, not module memory. */
export type WorkerPanelView = {
  selected: string | null;
  history: WorkerHistory | null;
  historyContext: string | null;
  nextCursor: number | null;
  nextAssignmentCursor: string | null;
  expandedEvent: number | null;
  expandedGroups: number[];
  /** Scroll offsets by scroll area; mutated in place while scrolling. */
  scroll: Record<string, number>;
};

// Native clients have no sessionStorage; module memory is the fallback.
const memory = new Map<string, string>();
function read(key: string): string | null {
  // Memory first: it keeps a full view that sessionStorage had to trim.
  try { return memory.get(key) ?? globalThis.sessionStorage?.getItem(key) ?? null; } catch { return memory.get(key) ?? null; }
}
/** False when sessionStorage refused the value (quota); memory still has it. */
function write(key: string, value: string | null): boolean {
  if (value === null) memory.delete(key); else memory.set(key, value);
  try {
    if (value === null) globalThis.sessionStorage?.removeItem(key); else globalThis.sessionStorage?.setItem(key, value);
    return true;
  } catch { return false; }
}
const handoffKey = (agentId: string) => `cortex-acp:worker-handoff:${agentId}`;
const handoffDetailKey = (agentId: string) => `cortex-acp:worker-handoff-detail:${agentId}`;
const viewKey = (agentId: string) => `cortex-acp:worker-view:${agentId}`;

/** What the opening card already knew about its worker: the host's own
 * bounded, server-redacted failure reason. The worker tab shows it only
 * for a worker whose durable history has no attempt to explain itself
 * (e.g. one that could not be started); the card shows only a category. */
export type WorkerHandoffDetail = { failureReason?: string };

export function selectWorker(agentId: string, assignmentRef: string, detail?: WorkerHandoffDetail): void {
  write(handoffKey(agentId), assignmentRef);
  const failureReason = detail?.failureReason?.trim();
  write(handoffDetailKey(agentId), failureReason ? JSON.stringify({ assignmentRef, failureReason }) : null);
  // A chat card opens a different worker from the top, not the old view.
  write(viewKey(agentId), null);
  for (const listener of listeners) listener();
}

export function selectedWorker(agentId: string): string | null {
  return read(handoffKey(agentId));
}

/** The handed-off detail for exactly this assignment, never another one's. */
export function selectedWorkerDetail(agentId: string, assignmentRef: string): WorkerHandoffDetail | null {
  const raw = read(handoffDetailKey(agentId));
  if (!raw) return null;
  try {
    const value = JSON.parse(raw) as { assignmentRef?: unknown; failureReason?: unknown };
    return value.assignmentRef === assignmentRef && typeof value.failureReason === "string" && value.failureReason.trim()
      ? { failureReason: value.failureReason } : null;
  } catch { return null; }
}

export function subscribeWorkerSelection(listener: Listener): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

export function saveWorkerView(agentId: string, view: WorkerPanelView): void {
  if (write(viewKey(agentId), JSON.stringify(view))) return;
  // ponytail: a history too large for sessionStorage is refetched after a
  // module re-evaluation; everything else of the view still survives.
  write(viewKey(agentId), JSON.stringify({ ...view, history: null, historyContext: null, nextCursor: null }));
  memory.set(viewKey(agentId), JSON.stringify(view));
}

export function savedWorkerView(agentId: string): WorkerPanelView | null {
  const raw = read(viewKey(agentId));
  if (!raw) return null;
  try { return JSON.parse(raw) as WorkerPanelView; } catch { return null; }
}
