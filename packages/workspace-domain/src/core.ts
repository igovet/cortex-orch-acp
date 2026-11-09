import type { Task, WorkspaceState } from "./types";

export class DomainError extends Error {}
export function requireFact(condition: unknown, reason: string): asserts condition {
  if (!condition) throw new DomainError(reason);
}
export function nonempty(value: string, label: string) {
  requireFact(value.trim(), label + " is required");
  return value.trim();
}
export function entity<T extends { id: string }>(items: T[], id: string): T {
  const found = items.find(item => item.id === id);
  requireFact(found, "Unknown identity: " + id);
  return found;
}
export function projectFor(state: WorkspaceState, task: Task) { return entity(state.projects, task.projectId); }
export function machineFor(state: WorkspaceState, task: Task) { return entity(state.machines, projectFor(state, task).machineId); }
