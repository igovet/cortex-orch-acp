import type { Sprint, TaskOverlay } from "@cortex/workspace-domain";

/** A sprint as the service saves it. An unfinished deletion keeps the sprint, archived, with what it set out to delete. */
export type SavedSprint = Sprint & { planningTaskId?: string; deleting?: unknown; deleteTaskIds?: unknown; deletedTaskIds?: unknown };
const ids = (v: unknown): string[] | null => Array.isArray(v) && v.every(x => typeof x === "string") ? v : null;

/**
 * A member the service has deleted is no longer the sprint's: the runtime holds no task for it and never will again. The
 * service marks it `purged` and, during a sprint deletion, lists it as deleted, while the sprint's own `taskIds` still name it.
 */
export function deletedMember(tasks: Record<string, TaskOverlay>, sprint: SavedSprint, taskId: string) {
  return tasks?.[taskId]?.purged === true || (ids(sprint.deletedTaskIds)?.includes(taskId) ?? false);
}

/** The members the runtime's board must hold for this sprint to be shown. */
export function liveMembers(tasks: Record<string, TaskOverlay>, sprint: SavedSprint) {
  return (ids(sprint.taskIds) ?? []).filter(id => !deletedMember(tasks, sprint, id));
}

/** Members the board does not hold, named precisely for a read's diagnostics. */
export function unreadMembers(tasks: Record<string, TaskOverlay>, sprints: readonly SavedSprint[], held: (taskId: string) => boolean): string | null {
  for (const sprint of sprints) {
    const missing = liveMembers(tasks, sprint).filter(id => !held(id));
    if (missing.length) return `Sprint ${sprint.id} names ${missing.length} task${missing.length === 1 ? "" : "s"} the board does not hold: ${missing.slice(0, 3).join(", ")}${missing.length > 3 ? ", …" : ""}`;
  }
  return null;
}

/**
 * The sprint the screens show: only the members that still exist, and an unfinished deletion said as one, with the tasks it
 * still has to delete. The service's bookkeeping fields stay with the service.
 */
export function presentSprint(tasks: Record<string, TaskOverlay>, sprint: SavedSprint): Sprint & { planningTaskId?: string } {
  // Not a sprint record this can read: it reaches the screens as it came, as before.
  if (!ids(sprint.taskIds)) return sprint;
  const { deleting, deleteTaskIds, deletedTaskIds, ...rest } = sprint;
  const taskIds = liveMembers(tasks, sprint);
  if (deleting !== true) return deleting === undefined && deleteTaskIds === undefined && deletedTaskIds === undefined && taskIds.length === sprint.taskIds.length ? sprint : { ...rest, taskIds };
  const members = [...new Set(ids(deleteTaskIds) ?? sprint.taskIds)];
  return { ...rest, taskIds, deletion: { total: members.length, left: members.filter(id => !deletedMember(tasks, sprint, id)) } };
}
