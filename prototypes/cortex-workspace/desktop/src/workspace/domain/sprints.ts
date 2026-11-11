import type { PlanItem, Sprint, SprintEntry, WorkspaceCommand, WorkspaceState } from "./types";
import { dependencyReady, openPlanningQuestion, runBlocker, sprintTaskCheckout, validatePlan } from "@cortex/workspace-domain";
import { entity, nextId, nonempty, requireFact } from "./core";
import { generateMockTitle } from "./projections";
import { createTask, startTask } from "./tasks";

const say = (sprint: Sprint, kind: SprintEntry["kind"], text: string, now: number | undefined, options?: string[]) =>
  sprint.conversation.push({ kind, text, at: now ?? null, mock: true, ...(options?.length ? { options } : {}) });
function revise(sprint: Sprint, items: PlanItem[]) {
  if (sprint.plan) (sprint.history ??= []).push(sprint.plan);
  sprint.plan = { version: (sprint.plan?.version ?? 0) + 1, items: structuredClone(items) };
}
export function advanceSprint(state: WorkspaceState, sprint: Sprint, now?: number) {
  if (sprint.archivedAt || sprint.runMode !== "automatic" || sprint.queueState !== "running") return;
  const tasks = sprint.taskIds.map(id => entity(state.tasks, id));
  if (tasks.some(t => ["needs_you", "stopping", "paused"].includes(t.status) || t.closedAs)) {
    sprint.queueState = "paused"; sprint.pauseReason = "Task needs owner attention"; return;
  }
  if (tasks.some(t => t.status === "working")) return;
  if (tasks.some(t => t.status === "review")) {
    sprint.queueState = "paused"; sprint.pauseReason = "Review the result before continuing"; return;
  }
  const next = tasks.find(t => t.status === "draft" && t.dependencies.every(id => dependencyReady(entity(state.tasks, id))));
  if (next) startTask(state, next, now);
  else if (tasks.every(dependencyReady)) sprint.queueState = "complete";
  else { sprint.queueState = "paused"; sprint.pauseReason = "Dependencies are not ready"; }
}
export function handleSprint(state: WorkspaceState, command: WorkspaceCommand): boolean {
  if (!("sprintId" in command) || !command.sprintId || command.type === "create_task" || command.type === "set_target") return false;
  const sprint = entity(state.sprints, command.sprintId);
  const machine = entity(state.machines, entity(state.projects, sprint.projectId).machineId);
  requireFact(machine.connection === "connected", "Machine offline; sprint changes unavailable");
  requireFact(!sprint.archivedAt || command.type === "close_sprint", "Sprint is closed");
  const planningOnly = () => requireFact(!sprint.materializedVersion, "Plan already materialized");
  switch (command.type) {
    case "close_sprint":
      requireFact(command.confirmed, "Owner confirmation required");
      sprint.archivedAt ??= Date.now(); sprint.queueState = "paused"; sprint.pauseReason = "Sprint closed by you"; break;
    case "rename_sprint": sprint.title = nonempty(command.title, "Sprint title"); break;
    case "set_sprint_settings": sprint.settings = { model: "opus[1m]", effort: "high", ...sprint.settings, ...command.settings }; break;
    case "plan_note":
      planningOnly();
      say(sprint, "note", nonempty(command.text, "Note"), command.now); break;
    case "plan_question":
      planningOnly();
      say(sprint, "question", nonempty(command.question, "Question"), command.now, command.options);
      sprint.planning = "question"; break;
    case "answer_planning":
      requireFact(sprint.planning === "question", "No planning question open");
      say(sprint, "answer", nonempty(command.answer, "Answer"), command.now);
      sprint.planning = "planning"; break;
    case "plan_message":
      planningOnly();
      requireFact(sprint.planning !== "cancelled", "Resume planning first");
      say(sprint, "message", nonempty(command.text, "Message"), command.now);
      // A question stays open: asking for changes is not an answer to it.
      if (sprint.planning === "ready") sprint.planning = "planning";
      break;
    case "publish_plan":
      requireFact(sprint.planning !== "question" && !sprint.materializedVersion, "Answer the question first or plan is already materialized");
      validatePlan(command.items);
      revise(sprint, command.items);
      sprint.planning = "ready"; break;
    case "edit_plan":
      planningOnly();
      requireFact(sprint.plan, "There is no plan to edit yet");
      validatePlan(command.items);
      if (JSON.stringify(command.items) === JSON.stringify(sprint.plan.items)) return true;
      revise(sprint, command.items);
      say(sprint, "event", `You edited the plan, now Plan v${sprint.plan!.version}`, command.now); break;
    case "materialize_plan": {
      requireFact(sprint.plan?.version === command.version, "Plan version is stale");
      if (sprint.materializedVersion === command.version) return true;
      requireFact(sprint.planning === "ready", "Plan is not ready");
      const ids = new Map(sprint.plan.items.map(item => [item.key, nextId(state, "task")]));
      for (const item of sprint.plan.items) {
        const choice = sprintTaskCheckout(sprint, item), id = ids.get(item.key)!;
        // Fictional paths for the demo only; the real service owns actual worktree naming.
        const checkout = choice.kind === "main" ? choice : { ...choice, branch: choice.branch ?? `cw/${id}`, path: choice.path ?? `worktrees/${id}` };
        const task = createTask(state, { projectId: sprint.projectId, sprintId: sprint.id, description: item.description, checkout }, id);
        task.title = item.title ?? generateMockTitle(item.description); task.titleState = "generated";
        task.dependencies = item.dependsOn.map(key => ids.get(key)!);
        sprint.taskIds.push(task.id);
      }
      sprint.materializedVersion = command.version; sprint.planning = "created";
      say(sprint, "event", `You created ${sprint.plan.items.length} task${sprint.plan.items.length === 1 ? "" : "s"} from Plan v${command.version}`, command.now);
      break;
    }
    case "cancel_planning":
      planningOnly();
      sprint.planning = "cancelled"; break;
    case "resume_planning":
      requireFact(sprint.planning === "cancelled", "Planning was not cancelled");
      sprint.planning = openPlanningQuestion(sprint) ? "question" : sprint.plan ? "ready" : "planning"; break;
    case "run_sprint": {
      const blocker = runBlocker(state, sprint);
      requireFact(!blocker, blocker ?? "");
      sprint.queueState = "running"; sprint.pauseReason = null; advanceSprint(state, sprint, command.now); break;
    }
    case "pause_sprint": sprint.queueState = "paused"; sprint.pauseReason = "Queue paused by owner; active task is unchanged"; break;
    case "set_run_mode":
      sprint.runMode = command.runMode;
      // Switching never touches running work; a queue that was running just waits for an explicit run.
      if (sprint.queueState === "running") { sprint.queueState = "paused"; sprint.pauseReason = "Run mode changed; explicit run required"; }
      break;
    default: return false;
  }
  return true;
}
