import { DomainError, entity, machineFor, nonempty, projectFor, requireFact } from "@cortex/workspace-domain";
import type { WorkspaceState, Task, Check } from "./types";

export { DomainError, entity, machineFor, nonempty, projectFor, requireFact };

export function validTime(now: number) { requireFact(Number.isFinite(now), "A finite mock clock is required"); }
export function nextId(state: WorkspaceState, kind: string) {
  return "mock-" + kind + "-" + String(state.nextId++).padStart(5, "0");
}
export function online(state: WorkspaceState, task: Task) {
  requireFact(machineFor(state, task).connection === "connected", "Machine offline; current outcome is unknown");
}
export function editable(task: Task) { requireFact(!task.archive, "Restore the task before changing it"); }
export function validateChecks(checks: Check[]) {
  for (const check of checks) {
    nonempty(check.name, "Check name"); nonempty(check.detail, "Check detail");
    requireFact(["passed", "failed", "not_run", "unverified"].includes(check.outcome), "Unknown check outcome");
  }
}
export function pauseSprint(state: WorkspaceState, task: Task, reason: string) {
  if (!task.sprintId) return;
  const sprint = entity(state.sprints, task.sprintId);
  sprint.queueState = "paused"; sprint.pauseReason = reason;
}
export function taskEvent(state: WorkspaceState, task: Task, text: string) {
  state.events.push({ id: nextId(state, "activity"), taskId: task.id, projectId: task.projectId, sequence: state.events.length + 1, text: "Mock: " + text, mock: true });
}
