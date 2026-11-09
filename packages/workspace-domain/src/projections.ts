import type { EngineUsageInput, EngineWeeklyUsage, WorkspaceState, Task } from "./types";
import { entity, machineFor } from "./core";

export function targetBranch(state: WorkspaceState, task: Task): string {
  const sprintTarget = task.sprintId ? entity(state.sprints, task.sprintId).targetBranch : null;
  return task.targetBranch ?? sprintTarget ?? entity(state.projects, task.projectId).defaultBranch;
}
export function boardPhase(state: WorkspaceState, taskId: string) {
  const task = entity(state.tasks, taskId);
  if (task.archive) return "archive";
  if (machineFor(state, task).connection !== "connected" || task.process === "unknown") return "unknown";
  // While Cortex works on a conflict nothing waits for the owner, so the task sits in Working.
  if (task.conflict === "conflict" || task.conflict === "failed") return "needs_you";
  return task.status;
}
export function weeklyUsage(rows: EngineUsageInput[]): EngineWeeklyUsage[] {
  return rows.flatMap(row => {
    if (!row.connected || (row.engine !== "codex" && row.engine !== "claude")) return [];
    const usedPercent = row.usedPercent !== null && Number.isFinite(row.usedPercent) && row.usedPercent >= 0 && row.usedPercent <= 100 ? row.usedPercent : null;
    return [{ ...row, engine: row.engine, usedPercent, window: "weekly" as const }];
  });
}
/** A short label for a description, for a place that needs one before a title exists. It is not a generated title. */
export function descriptionLabel(description: string) {
  const first = (description.trim().split(/\n|(?<=[.!?])\s/)[0] ?? "").replace(/\s+/g, " ").replace(/[.!?]+$/, "");
  const cut = first.slice(0, 60), space = cut.lastIndexOf(" ");
  const title = first.length > 60 ? (space > 30 ? cut.slice(0, space) : cut) + "…" : first;
  return title ? title[0].toUpperCase() + title.slice(1) : "Untitled task";
}
