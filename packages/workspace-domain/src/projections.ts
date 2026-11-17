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
const percent = (n: number | null) => n !== null && Number.isFinite(n) && n >= 0 && n <= 100 ? n : null;
/** Only connected Codex and Claude rows; a percentage outside 0 to 100 is unknown, never clamped. Claude keeps each weekly window by itself. */
export function weeklyUsage(rows: EngineUsageInput[]): EngineWeeklyUsage[] {
  return rows.flatMap(row => {
    if (!row.connected || (row.engine !== "codex" && row.engine !== "claude")) return [];
    const windows = row.windows?.filter(w => /^seven_day(_[a-z0-9]+)*$/.test(w.name)).map(w => ({ ...w, usedPercent: percent(w.usedPercent) }));
    return [{ ...row, engine: row.engine, usedPercent: percent(row.usedPercent), window: "weekly" as const, ...(windows ? { windows } : {}) }];
  });
}
/** A short label for a description, for a place that needs one before a title exists. It is not a generated title. */
export function descriptionLabel(description: string) {
  const first = (description.trim().split(/\n|(?<=[.!?])\s/)[0] ?? "").replace(/\s+/g, " ").replace(/[.!?]+$/, "");
  const cut = first.slice(0, 60), space = cut.lastIndexOf(" ");
  const title = first.length > 60 ? (space > 30 ? cut.slice(0, space) : cut) + "…" : first;
  return title ? title[0].toUpperCase() + title.slice(1) : "Untitled task";
}
