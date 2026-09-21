type Listener = () => void;

const selected = new Map<string, string>();
const listeners = new Set<Listener>();

export function selectWorker(agentId: string, assignmentRef: string): void {
  selected.set(agentId, assignmentRef);
  for (const listener of listeners) listener();
}

export function selectedWorker(agentId: string): string | null {
  return selected.get(agentId) ?? null;
}

export function subscribeWorkerSelection(listener: Listener): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}
