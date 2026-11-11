import type { WorkspaceCommand, WorkspaceState } from "./types";
import { editable, entity, nextId, nonempty, online, requireFact } from "./core";
import { handleGit } from "./git";
import { handleSession } from "./session";
import { createTask, handleTask } from "./tasks";
import { advanceSprint, handleSprint } from "./sprints";
import { handleArchive } from "./archive";
import { handleMachine, newMachine } from "./machines";

export function applyCommand(state: WorkspaceState, command: WorkspaceCommand): string | undefined {
  if (handleArchive(state, command) || handleTask(state, command) || handleSprint(state, command) || handleMachine(state, command)) {
    if (command.type === "accept_result" || command.type === "publish_result" || command.type === "finish_check") {
      const task = entity(state.tasks, command.taskId);
      if (task.sprintId) advanceSprint(state, entity(state.sprints, task.sprintId), "now" in command ? command.now : undefined);
    }
    return command.type === "pair_machine" ? command.id : undefined;
  }
  const session = handleSession(state, command) ?? handleGit(state, command);
  if (session) return session.id;
  switch (command.type) {
    case "add_project":
      entity(state.machines, command.machineId);
      requireFact(!state.projects.some(p => p.id === command.id), "Duplicate project identity");
      state.projects.push({
        id: nonempty(command.id, "Project id"), machineId: command.machineId,
        name: nonempty(command.name, "Project name"), repository: nonempty(command.repository, "Repository"),
        openBranch: nonempty(command.openBranch, "Open branch"), defaultBranch: nonempty(command.defaultBranch, "Git default branch"), mock: true,
      }); return command.id;
    case "add_remote":
      requireFact(!state.machines.some(m => m.id === command.id), "Duplicate machine identity");
      state.machines.push(newMachine({ id: nonempty(command.id, "Machine id"), kind: "remote", name: nonempty(command.name, "Machine name"), connection: "disconnected" })); return command.id;
    case "create_task": {
      const project = entity(state.projects, command.projectId);
      requireFact(entity(state.machines, project.machineId).connection === "connected", "Machine offline");
      const task = createTask(state, command);
      if (task.sprintId) {
        const sprint = entity(state.sprints, task.sprintId); sprint.taskIds.push(task.id);
        if (sprint.queueState === "complete") { sprint.queueState = "paused"; sprint.pauseReason = "New task added. Continue the queue when ready."; }
      }
      return task.id;
    }
    case "create_sprint": {
      const project = entity(state.projects, command.projectId);
      requireFact(entity(state.machines, project.machineId).connection === "connected", "Machine offline");
      const id = nextId(state, "sprint");
      state.sprints.push({
        id, projectId: project.id, description: nonempty(command.description, "Description"), runMode: command.runMode, checkout: command.checkout,
        targetBranch: null, planning: "planning", conversation: [], plan: null, history: [], materializedVersion: null,
        taskIds: [], queueState: "idle", pauseReason: null, mock: true,
      }); return id;
    }
    case "set_target": {
      const branch = command.branch === null ? null : nonempty(command.branch, "Target branch");
      if (command.taskId) { const task = entity(state.tasks, command.taskId); editable(task); online(state, task); task.targetBranch = branch; }
      else {
        const sprint = entity(state.sprints, command.sprintId!);
        requireFact(entity(state.machines, entity(state.projects, sprint.projectId).machineId).connection === "connected", "Machine offline");
        sprint.targetBranch = branch;
      }
      return undefined;
    }
    default: throw new Error("Unknown mock command: " + command.type);
  }
}
