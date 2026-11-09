import type { Task, WorkspaceCommand, WorkspaceState } from "./types";
import { DAY, inMachine, previewClear, previewPurge, purgeBlocker } from "@cortex/workspace-domain";
import { entity, machineFor, online, requireFact, validTime } from "./core";

export function requestArchive(state: WorkspaceState, task: Task, now: number) {
  validTime(now); online(state, task);
  requireFact(task.status === "done" && task.process === "stopped", "Close task and confirm stopped processes before archive");
  if (task.archive) return;
  if (task.work === "unmerged") { task.archiveQuestion = true; return; }
  task.archive = { at: now, previousStatus: "done", sizeMb: task.workFacts?.sizeMb ?? null }; task.archiveQuestion = false;
}
function purge(state: WorkspaceState, task: Task, now: number) {
  state.tasks = state.tasks.filter(t => t.id !== task.id);
  state.workers = state.workers.filter(w => w.taskId !== task.id);
  state.messages = state.messages.filter(m => m.taskId !== task.id);
  state.events = state.events.filter(e => e.taskId !== task.id);
  state.sessions = state.sessions.filter(s => s.taskId !== task.id);
  state.reviews = state.reviews.filter(r => r.taskId !== task.id);
  state.repos.forEach(r => r.checkouts.forEach(c => { if (c.taskId === task.id) c.taskId = null; }));
  state.sprints.forEach(s => s.taskIds = s.taskIds.filter(id => id !== task.id));
  state.tasks.forEach(t => t.dependencies = t.dependencies.filter(id => id !== task.id));
  state.purged.push({ taskId: task.id, at: now, mock: true });
}
function archiveDue(state: WorkspaceState, now: number, machineId?: string) {
  let asked = 0;
  for (const task of state.tasks) {
    const machine = machineFor(state, task);
    if (inMachine(state, task, machineId) && !task.archive && task.status === "done" && task.doneAt !== null && now >= task.doneAt + machine.autoArchiveDays * DAY &&
      (task.archiveAskAfter === null || now >= task.archiveAskAfter) && machine.connection === "connected" && task.process === "stopped") {
      requestArchive(state, task, now);
      if (task.archiveQuestion) asked++;
    }
  }
  return asked;
}
function purgeDue(state: WorkspaceState, now: number, machineId?: string) {
  const out = { removed: 0, skipped: 0, freedMb: 0 };
  for (const task of [...state.tasks]) {
    if (!task.archive || !inMachine(state, task, machineId) || now < task.archive.at + machineFor(state, task).retentionDays * DAY) continue;
    task.purgeBlocker = purgeBlocker(state, task);
    if (task.purgeBlocker) { out.skipped++; continue; }
    out.removed++; out.freedMb += task.archive.sizeMb ?? 0;
    purge(state, task, now);
  }
  return out;
}
export function handleArchive(state: WorkspaceState, command: WorkspaceCommand): boolean {
  switch (command.type) {
    case "set_retention":
      requireFact(Number.isInteger(command.days) && command.days > 0, "Retention must be a positive integer in days");
      entity(state.machines, command.machineId).retentionDays = command.days; return true;
    case "archive_due":
      validTime(command.now);
      archiveDue(state, command.now);
      return true;
    case "purge_due":
      validTime(command.now);
      purgeDue(state, command.now);
      return true;
    case "run_cleanup": {
      validTime(command.now);
      const machine = entity(state.machines, command.machineId);
      requireFact(machine.connection === "connected", `${machine.name} is offline. It cleans up at its next run`);
      const asked = archiveDue(state, command.now, machine.id);
      machine.cleanup = { at: command.now, asked, ...purgeDue(state, command.now, machine.id) };
      return true;
    }
    case "archive_task": requestArchive(state, entity(state.tasks, command.taskId), command.now); return true;
    case "answer_archive": {
      validTime(command.now);
      const task = entity(state.tasks, command.taskId);
      online(state, task); requireFact(task.archiveQuestion, "No unmerged-work question is open");
      task.archiveQuestion = false;
      if (!command.discard) { task.archiveAskAfter = command.now + machineFor(state, task).autoArchiveDays * DAY; return true; }
      task.work = "discard_confirmed"; requestArchive(state, task, command.now); return true;
    }
    case "restore_task": {
      const task = entity(state.tasks, command.taskId);
      online(state, task); requireFact(task.archive, "Task is not archived");
      task.status = task.archive.previousStatus; task.archive = null; task.purgeBlocker = null;
      if (task.work === "discard_confirmed") task.work = "unmerged";
      // The Done clock starts again, or the next daily run would send it straight back.
      if (command.now !== undefined) task.doneAt = command.now;
      task.archiveAskAfter = null; return true;
    }
    case "purge_task": {
      validTime(command.now);
      const task = entity(state.tasks, command.taskId);
      requireFact(command.confirmed, "Owner confirmation required for manual purge");
      requireFact(!purgeBlocker(state, task), purgeBlocker(state, task) ?? "Purge blocked");
      purge(state, task, command.now); return true;
    }
    case "purge_many": {
      validTime(command.now);
      requireFact(command.confirmed, "Owner confirmation required for manual purge");
      const { deletable } = previewPurge(state, command.taskIds);
      requireFact(deletable.length > 0, "Nothing here can be deleted");
      for (const id of deletable) purge(state, entity(state.tasks, id), command.now);
      return true;
    }
    case "clear_purge_block": {
      const task = entity(state.tasks, command.taskId);
      online(state, task); requireFact(task.archive, "Task is not archived");
      requireFact(command.confirmed, "Owner confirmation required");
      requireFact(task.process === "stopped", "A live or unknown process cannot be waved through");
      task.unexpectedChanges = false; task.purgeBlocker = null; return true;
    }
    case "clear_old": {
      validTime(command.now);
      requireFact(command.confirmed, "Owner confirmation required");
      const { remove } = previewClear(state, command.machineId, command.cutoff, command.force);
      requireFact(remove.length > 0, "No old task can be cleared");
      for (const id of remove) purge(state, entity(state.tasks, id), command.now);
      return true;
    }
    case "set_safety": {
      const task = entity(state.tasks, command.taskId);
      // Explicit simulation, not a claim about a real OS process or filesystem.
      task.process = command.process; task.unexpectedChanges = command.unexpectedChanges; return true;
    }
    default: return false;
  }
}
