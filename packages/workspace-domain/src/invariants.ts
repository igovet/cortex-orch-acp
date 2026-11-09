import type { Check, PlanItem, ProjectRepo, RepoCheckout, Sprint, SprintEntry, Task, TaskSession, WorkspaceState } from "./types";
import { DomainError, entity, machineFor, nonempty, projectFor, requireFact } from "./core";

/**
 * Pure presentation invariants: what a screen may offer and how it words a blocker. They read a projection and change nothing.
 * The machine decides. A rule here never authorises anything, it only keeps a control honest about what it can do.
 */
export const repoOf = (state: WorkspaceState, projectId: string) => state.repos.find(r => r.projectId === projectId) ?? null;
export function checkoutOf(state: WorkspaceState, id: string): { repo: ProjectRepo; checkout: RepoCheckout } {
  for (const repo of state.repos) {
    const checkout = repo.checkouts.find(c => c.id === id);
    if (checkout) return { repo, checkout };
  }
  throw new DomainError("Unknown checkout: " + id);
}
/** The record of the checkout a task works in, when there is one. */
export const taskCheckout = (state: WorkspaceState, task: Task) =>
  repoOf(state, task.projectId)?.checkouts.find(c => c.taskId === task.id || (c.kind === "main" && task.checkout.kind === "main")) ?? null;
/** What deleting this worktree would lose. Merged commits are safe in the target branch. */
export function worktreeWork(state: WorkspaceState, checkout: RepoCheckout) {
  const task = checkout.taskId ? state.tasks.find(t => t.id === checkout.taskId) ?? null : null;
  return { task, files: checkout.files.length, commits: task?.work === "merged" ? 0 : checkout.ahead };
}
export const DAY = 86400000;
export function purgeBlocker(state: WorkspaceState, task: Task): string | null {
  if (!task.archive) return "Task is not archived";
  if (machineFor(state, task).connection !== "connected") return "Machine offline";
  if (task.work === "unmerged") return "Unmerged work needs explicit discard confirmation";
  if (task.unexpectedChanges) return "Unexpected workspace changes";
  if (task.process !== "stopped") return "Live or unknown process";
  if (state.tasks.some(t => t.dependencies.includes(task.id) && t.status !== "done" && !t.archive)) return "An active dependent task still needs this result";
  return null;
}
/** What a delete would do, before anything is deleted. Blocked tasks are named and skipped, never removed. */
export function previewPurge(state: WorkspaceState, ids: string[]) {
  const tasks = state.tasks.filter(t => ids.includes(t.id));
  const deletable = tasks.filter(t => !purgeBlocker(state, t));
  const sizes = deletable.map(t => t.archive?.sizeMb ?? null);
  return {
    deletable: deletable.map(t => t.id), skipped: tasks.filter(t => purgeBlocker(state, t)).map(t => ({ id: t.id, title: t.title, reason: purgeBlocker(state, t)! })),
    sizeMb: sizes.reduce<number>((sum, s) => sum + (s ?? 0), 0), sizeKnown: sizes.every(s => s !== null),
  };
}
export const inMachine = (state: WorkspaceState, task: Task, machineId?: string) => !machineId || projectFor(state, task).machineId === machineId;
/** Old tasks outside the Archive. Unmerged work is never cleared here, and a live or unknown process needs `force`. */
export function previewClear(state: WorkspaceState, machineId: string, cutoff: number, force: boolean) {
  const old = state.tasks.filter(t => inMachine(state, t, machineId) && !t.archive && t.status === "done" && t.doneAt !== null && t.doneAt <= cutoff);
  const merged = old.filter(t => t.work !== "unmerged"), risky = merged.filter(t => t.process !== "stopped");
  return { remove: merged.filter(t => force || t.process === "stopped").map(t => t.id), guarded: force ? 0 : risky.length, kept: old.length - merged.length };
}
export function validatePlan(items: PlanItem[]) {
  requireFact(items.length > 0, "Plan must contain tasks");
  const byKey = new Map(items.map(item => [nonempty(item.key, "Plan key"), item]));
  requireFact(byKey.size === items.length, "Duplicate plan key");
  items.forEach(item => {
    nonempty(item.description, "Description");
    if (item.title !== undefined) nonempty(item.title, "Task title");
    item.dependsOn.forEach(key => requireFact(key !== item.key && byKey.has(key), "Self or missing dependency: " + item.key + " → " + key));
  });
  const visiting = new Set<string>(), visited = new Set<string>();
  function visit(key: string, path: string[]) {
    requireFact(!visiting.has(key), "Dependency cycle: " + [...path, key].join(" → "));
    if (visited.has(key)) return;
    visiting.add(key);
    byKey.get(key)!.dependsOn.forEach(dependency => visit(dependency, [...path, key]));
    visiting.delete(key); visited.add(key);
  }
  items.forEach(item => visit(item.key, []));
}
/** Why `key` cannot wait for `dependency`, in the words the plan table shows. Null means the edit is safe. */
export function dependencyProblem(items: PlanItem[], key: string, dependency: string): string | null {
  const no = (k: string) => items.findIndex(i => i.key === k) + 1;
  if (key === dependency) return `Not applied: task ${no(key)} cannot wait for itself.`;
  const byKey = new Map(items.map(i => [i.key, i]));
  if (!byKey.has(dependency)) return `Not applied: there is no task ${dependency} in this plan.`;
  const seen = new Set<string>();
  const reaches = (k: string): boolean => {
    if (k === key) return true;
    if (seen.has(k)) return false;
    seen.add(k);
    return (byKey.get(k)?.dependsOn ?? []).some(reaches);
  };
  return reaches(dependency) ? `Not applied: ${no(key)} after ${no(dependency)} makes a cycle, because ${no(dependency)} already waits for ${no(key)}.` : null;
}
/** Dependency order, with plan order breaking ties: the order an automatic run would use, one task at a time. */
export function runOrder(items: PlanItem[]): string[] {
  const done = new Set<string>(), out: string[] = [];
  while (out.length < items.length) {
    const next = items.find(i => !done.has(i.key) && i.dependsOn.every(d => done.has(d)));
    if (!next) break;
    done.add(next.key); out.push(next.key);
  }
  return out;
}
export const openPlanningQuestion = (sprint: Sprint): SprintEntry | null => {
  const last = sprint.conversation.filter(e => e.kind === "question" || e.kind === "answer").at(-1);
  return last?.kind === "question" ? last : null;
};
/** Why the queue cannot run right now, and the task that holds it when one does. Null means Run sprint would succeed. */
export function runBlockerOf(state: WorkspaceState, sprint: Sprint): { reason: string; taskId: string | null } | null {
  if (sprint.planning !== "created") return { reason: "Create the plan tasks first", taskId: null };
  if (sprint.runMode !== "automatic") return { reason: "Select Automatic to run the queue", taskId: null };
  for (const id of sprint.taskIds) {
    const task = entity(state.tasks, id);
    if (task.status === "done" && (task.closedAs === "external" || task.closedAs === "as_is")) continue;
    if (task.closedAs) return { reason: `${task.title} was closed, so the queue cannot go on`, taskId: id };
    if (task.status === "needs_you") return { reason: `${task.title} needs you first`, taskId: id };
    if (task.status === "review") return { reason: task.reviewActions?.reason ? `${task.title}: ${task.reviewActions.reason}` : `Review ${task.title} first`, taskId: id };
    if (task.status === "stopping" || task.status === "paused") return { reason: `${task.title} is stopped. Resume it first`, taskId: id };
  }
  return null;
}
export const runBlocker = (state: WorkspaceState, sprint: Sprint): string | null => runBlockerOf(state, sprint)?.reason ?? null;
export const checksPassed = (checks: Check[]) => checks.length > 0 && checks.every(c => c.outcome === "passed" && !c.running);
export function checkAcceptanceBlocker(checks: Check[]): string | null {
  if (!checks.length) return "No checks reported. Run the required checks";
  if (checks.some(c => c.running)) return "Checks are running; wait for the recorded outcome";
  if (checks.some(c => c.kind !== "manual" && c.outcome !== "passed" && !c.technicalReason?.trim())) return "Run the unpassed checks first. Only a recorded technical blocker permits acceptance";
  return null;
}
export function dependencyReady(task: Task) {
  if (task.closedAs === "cancel") return false;
  return (task.status === "done" && (task.closedAs === "external" || task.closedAs === "as_is")) || task.work === "merged" || (task.status === "ready_to_merge" && task.result !== null &&
    (task.acceptance?.digest === task.workspaceDigest || (task.result.automaticReady === true && checksPassed(task.result.checks))) && task.result.digest === task.workspaceDigest && task.conflict === "none");
}
export const emptySession = (taskId: string, createdAt: number | null = null): TaskSession => ({
  taskId, createdAt, base: null, plan: null, timeline: [], decisions: [], context: null, tokens: null, planUsage: null,
  coordinator: { note: "", elapsedMs: null, usage: null }, verification: [], changes: null,
});
export const isOpen = (d: { answer: unknown; withdrawn: boolean }) => !d.answer && !d.withdrawn;
