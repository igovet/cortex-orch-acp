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
const viewKey = (agentId: string) => `cortex-acp:worker-view:${agentId}`;

export function selectWorker(agentId: string, assignmentRef: string): void {
  write(handoffKey(agentId), assignmentRef);
  // A chat card opens a different worker from the top, not the old view.
  write(viewKey(agentId), null);
  for (const listener of listeners) listener();
}

export function selectedWorker(agentId: string): string | null {
  return read(handoffKey(agentId));
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
