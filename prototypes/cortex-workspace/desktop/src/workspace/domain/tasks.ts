import type { NewTask, Task, WorkspaceState, WorkspaceCommand } from "./types";
import { editable, entity, nextId, nonempty, online, pauseSprint, requireFact, taskEvent, validateChecks, validTime } from "./core";
import { afterMerge, stopResolution } from "./git";
import { checkAcceptanceBlocker, checksPassed, dependencyReady, targetBranch } from "@cortex/workspace-domain";
import { generateMockTitle } from "./projections";
import { addEntry, addLine, cutCommands, DEFAULT_SETTINGS, isOpen, newWorker, pushActivity, sessionFor, startClock, stopClock, validateSettings } from "./timeline";

export function createTask(state: WorkspaceState, input: NewTask, id = nextId(state, "task")): Task {
  const project = entity(state.projects, input.projectId);
  requireFact(!state.tasks.some(t => t.id === id), "Duplicate task identity");
  const description = nonempty(input.description, "Description");
  if (input.sprintId) requireFact(entity(state.sprints, input.sprintId).projectId === project.id, "Sprint belongs to another project");
  const checkout = input.checkout.kind === "main"
    ? { kind: "main" as const, branch: project.openBranch }
    : { ...input.checkout, branch: nonempty(input.checkout.branch, "Worktree branch"), path: nonempty(input.checkout.path, "Worktree path") };
  if (checkout.kind === "worktree") {
    requireFact(!checkout.path.startsWith("/") && !checkout.path.split(/[\\/]/).includes(".."), "Mock worktree paths must be project-relative");
    requireFact(!state.tasks.some(t => t.projectId === project.id && t.checkout.kind === "worktree" && t.checkout.path === checkout.path), "Worktree already bound to a task");
  }
  const task: Task = {
    id, projectId: project.id, sprintId: input.sprintId ?? null, description,
    title: "Generating title…", titleState: "pending", checkout, targetBranch: null, dependencies: [],
    status: "draft", resumeStatus: "working", issue: null, executionId: null,
    result: null, workspaceDigest: null, acceptance: null, conflict: "none", merge: null, resolvedBy: null, integration: null,
    closedAs: null, notVerifiedByCortex: false, doneAt: null, archive: null,
    archiveQuestion: false, archiveAskAfter: null, work: "unmerged", process: "stopped",
    unexpectedChanges: false, purgeBlocker: null, settings: validateSettings(input.settings ?? DEFAULT_SETTINGS), mock: true,
  };
  state.tasks.push(task);
  return task;
}
function readyAfterChecks(state: WorkspaceState, task: Task) {
  if (task.status !== "review" || task.conflict !== "none" || task.closedAs || !task.result || task.workspaceDigest !== task.result.digest) return;
  if (task.sprintId && entity(state.sprints, task.sprintId).runMode === "automatic" && checksPassed(task.result.checks)) {
    task.status = "ready_to_merge"; task.result.automaticReady = true;
  }
}
export function startTask(state: WorkspaceState, task: Task, now?: number) {
  editable(task); online(state, task);
  requireFact(task.status === "draft", "Only a draft can start");
  requireFact(task.dependencies.every(id => dependencyReady(entity(state.tasks, id))), "A dependency is not accepted and ready");
  if (task.sprintId) {
    const sprint = entity(state.sprints, task.sprintId);
    requireFact(!sprint.taskIds.some(id => id !== task.id && ["working", "needs_you", "stopping", "paused"].includes(entity(state.tasks, id).status)), "Sprint already has an active task");
  }
  // Concurrent writers to a shared checkout are not qualified: fail closed.
  requireFact(task.checkout.kind !== "main" || !state.tasks.some(t => t.id !== task.id && t.projectId === task.projectId && t.checkout.kind === "main" && t.process !== "stopped"), "Another task may be writing to the shared checkout");
  task.status = "working"; task.process = "live"; task.executionId = nextId(state, "execution");
  const worker = newWorker({
    id: nextId(state, "worker"), taskId: task.id, executionId: task.executionId, role: "Frontend Dev", status: "working",
    goal: generateMockTitle(task.description), now: "Mock worker started; no engine call",
    route: { model: task.settings.model, effort: task.settings.effort }, attempt: { n: 1, of: 1 }, elapsedMs: 0,
  });
  startClock(worker, now);
  pushActivity(worker, "Mock worker started; no engine call", { at: now ?? null });
  state.workers.push(worker);
  const session = sessionFor(state, task);
  session.createdAt ??= now ?? null;
  addEntry(state, task, { kind: "cortex", text: "Mock session. A real run would plan here and start workers; this prototype calls no engine." });
  addEntry(state, task, { kind: "workers", title: "Started 1 worker", workerIds: [worker.id] });
}
/** Continuing keeps every open question and queued message. Only what the stop paused starts again. */
export function continueTask(state: WorkspaceState, task: Task, now?: number) {
  requireFact(task.status === "paused" && task.process === "stopped", "Stop is not confirmed or task is not paused");
  task.status = task.resumeStatus; task.process = "live";
  state.workers.filter(w => w.taskId === task.id && w.status === "paused").forEach(w => { w.status = "working"; w.now = "Resumed"; startClock(w, now); });
  addLine(state, task, "play", "You resumed the task", now, true);
}
export function handleTask(state: WorkspaceState, command: WorkspaceCommand): boolean {
  if (!("taskId" in command) || !command.taskId) return false;
  const handled = ["rename_task", "generate_title", "start_task", "stop_task", "confirm_stop", "continue_task", "close_task", "task_issue", "clear_issue", "publish_result", "rerun_check", "finish_check", "accept_result", "workspace_changed", "integrate"];
  if (!handled.includes(command.type)) return false;
  const task = entity(state.tasks, command.taskId);
  editable(task); online(state, task);
  switch (command.type) {
    case "rename_task": task.title = nonempty(command.title, "Title"); task.titleState = "user"; break;
    case "generate_title":
      if (task.titleState !== "user") {
        task.titleState = command.fail ? "failed" : "generated";
        task.title = command.fail ? "Title unavailable · retry or rename" : generateMockTitle(task.description);
      }
      break;
    case "start_task": startTask(state, task, command.now); break;
    case "stop_task": {
      requireFact(["working", "needs_you"].includes(task.status), "Task is not running");
      // Stopping Cortex on a conflict hands the conflict back, it does not pause the task.
      if (task.conflict === "resolving") { stopResolution(state, task, command.now); break; }
      const live = state.workers.filter(w => w.taskId === task.id && w.status === "working");
      task.resumeStatus = task.status === "needs_you" ? "needs_you" : "working";
      task.status = command.confirmed ? "paused" : "stopping"; task.process = command.confirmed ? "stopped" : "unknown";
      live.forEach(w => {
        w.status = command.confirmed ? "paused" : "unknown"; stopClock(w, command.now);
        if (command.confirmed) { cutCommands(w, "Task stopped by you"); w.now = "Stopped by you, the action was cut off"; }
        else w.now = "Stop requested, no confirmation yet";
      });
      addLine(state, task, "pause", live.length ? `You stopped the task, ${live.length} worker action${live.length === 1 ? " was" : "s were"} interrupted` : "You stopped the task", command.now, true);
      pauseSprint(state, task, "Task stopped by owner"); break;
    }
    case "confirm_stop":
      requireFact(task.status === "stopping", "No unconfirmed stop");
      task.status = "paused"; task.process = "stopped";
      state.workers.filter(w => w.taskId === task.id && w.status === "unknown").forEach(w => { w.status = "paused"; cutCommands(w, "Task stopped by you"); w.now = "Stopped by you, the action was cut off"; });
      addLine(state, task, "check", "Every worker is confirmed as stopped", command.now); break;
    case "continue_task": continueTask(state, task, command.now); break;
    case "close_task": {
      validTime(command.now);
      requireFact(command.confirmed, "Owner confirmation required");
      requireFact(task.status !== "stopping" && task.process !== "unknown", "Engine stop is not confirmed");
      requireFact(!task.integration || task.integration.method !== "squash", "Task is already merged");
      requireFact(task.status !== "done", "Task is already closed");
      requireFact(task.conflict === "none", "A merge is in progress. Abort it first");
      task.status = "done"; task.closedAs = command.choice; task.notVerifiedByCortex = true;
      task.doneAt = command.now; task.process = "stopped"; task.acceptance = null; task.issue = null;
      state.workers.filter(w => w.taskId === task.id && ["working", "paused", "waiting_you", "pending", "not_started"].includes(w.status)).forEach(w => {
        w.status = "cancelled"; stopClock(w, command.now); cutCommands(w, "Task closed by you");
      });
      state.messages.filter(m => m.taskId === task.id && ["queued", "sending", "recorded"].includes(m.delivery) && (m.delivery !== "recorded" || m.workerId)).forEach(m => m.delivery = "withdrawn");
      sessionFor(state, task).decisions.filter(isOpen).forEach(d => { d.withdrawn = true; });
      addLine(state, task, command.choice === "as_is" ? "x" : "x-circle", command.choice === "as_is" ? "You closed the task as is, not verified" : "You cancelled the task", command.now, true);
      pauseSprint(state, task, "Task closed without verified completion"); break;
    }
    case "task_issue":
      requireFact(["working", "review", "ready_to_merge"].includes(task.status), "Task cannot wait for a decision now");
      task.issue = nonempty(command.reason, "Issue"); task.status = "needs_you";
      pauseSprint(state, task, task.issue); break;
    case "clear_issue":
      requireFact(task.status === "needs_you" && task.conflict === "none", "Resolve the conflict first or no issue exists");
      task.issue = null; task.status = task.result ? "review" : "working"; break;
    case "publish_result":
      requireFact(["working", "review", "needs_you"].includes(task.status) || task.closedAs === "as_is", "Task cannot publish a result now");
      requireFact(task.conflict === "none" && !sessionFor(state, task).decisions.some(isOpen), "Resolve the conflict or answer open questions first");
      validateChecks(command.checks);
      task.workspaceDigest = nonempty(command.digest, "Digest");
      task.result = { version: (task.result?.version ?? 0) + 1, digest: command.digest, checks: structuredClone(command.checks), summary: command.summary ?? "Mock result; no command run", mock: true };
      task.acceptance = null; task.status = "review"; task.process = "stopped"; task.issue = null;
      for (const check of task.result.checks) {
        if (check.outcome === "not_run" && !check.technicalReason?.trim()) {
          check.running = true;
          addLine(state, task, "play", `Automatically running skipped check: ${check.name} (mock)`, undefined);
        }
      }
      readyAfterChecks(state, task);
      state.workers.filter(w => w.taskId === task.id && ["working", "paused"].includes(w.status)).forEach(w => {
        w.status = command.checks.some(c => c.outcome === "failed") ? "failed" : "completed";
        w.result = { summary: task.result!.summary, checks: structuredClone(command.checks), mock: true };
      });
      if (!command.checks.length || command.checks.some(c => c.outcome !== "passed")) pauseSprint(state, task, "Result has failed, not run or unverified checks");
      break;
    case "rerun_check":
    case "finish_check": {
      requireFact((task.status === "review" || task.status === "ready_to_merge") && !task.integration && task.conflict === "none", "Check rerun unavailable now");
      requireFact(task.result && task.result.digest === command.digest && task.workspaceDigest === command.digest, "Stale or absent result");
      const check = task.result.checks[command.checkIndex];
      requireFact(Number.isInteger(command.checkIndex) && check, "Check not found");
      task.acceptance = null; task.status = "review";
      if (command.type === "rerun_check") {
        requireFact(check.outcome !== "passed" && !check.running, "Check already passed or running");
        check.running = true;
        addLine(state, task, "play", `You requested a rerun of ${check.name} (mock)`, undefined);
        pauseSprint(state, task, "Check rerun in progress");
      } else {
        requireFact(check.running, "No check run pending");
        validateChecks([{ ...check, outcome: command.outcome, detail: command.detail }]);
        check.outcome = command.outcome; check.detail = nonempty(command.detail, "Check detail"); check.running = false;
        check.technicalReason = command.outcome !== "passed" && command.technicalReason ? nonempty(command.technicalReason, "Technical reason") : undefined;
        addLine(state, task, "check", `Mock check outcome: ${check.name}: ${check.outcome}. ${check.detail}`, undefined);
        readyAfterChecks(state, task);
        if (!checksPassed(task.result.checks)) pauseSprint(state, task, "Result has failed, not run or unverified checks");
      }
      break;
    }
    case "accept_result":
      // A resolved merge comes back in Ready to merge with no acceptance yet: the owner still has to accept that version.
      requireFact((task.status === "review" || (task.status === "ready_to_merge" && !task.acceptance)) && task.conflict === "none", "Result not available for acceptance");
      requireFact(task.result && task.result.digest === command.digest && task.workspaceDigest === command.digest, "Stale or absent result");
      {
        const count = (outcome: "failed" | "not_run" | "unverified") => task.result!.checks.filter(c => c.outcome === outcome).length;
        const open = { failed: count("failed"), not_run: count("not_run"), unverified: count("unverified") };
        const blocker = checkAcceptanceBlocker(task.result.checks);
        requireFact(!blocker, blocker ?? "");
        // Never implicit: the owner has to say so, and the acceptance keeps what was still open.
        requireFact(command.openChecks || (task.result.checks.length > 0 && task.result.checks.every(c => c.outcome === "passed")), "Checks have not all passed; no implicit override");
        task.acceptance = { version: task.result.version, digest: command.digest, at: command.now ?? null, open: open.failed + open.not_run + open.unverified ? open : null };
      }
      task.status = "ready_to_merge";
      addLine(state, task, "check", `You accepted v${task.result.version}`, command.now);
      break;
    case "workspace_changed":
      task.workspaceDigest = nonempty(command.digest, "Workspace digest");
      if (task.acceptance?.digest !== task.workspaceDigest) {
        task.acceptance = null;
        if (task.status === "ready_to_merge") task.status = "review";
      }
      break;
    case "integrate":
      validTime(command.now);
      requireFact(command.confirmed, "Owner confirmation required");
      requireFact(task.status === "ready_to_merge" && task.acceptance && task.result, "Task must pass Ready to merge");
      requireFact(task.conflict === "none" && task.process === "stopped", "Conflict or active process blocks integration");
      requireFact(task.acceptance.digest === command.digest && task.workspaceDigest === command.digest && task.result.digest === command.digest, "Acceptance is stale");
      requireFact(targetBranch(state, task) === command.targetBranch, "Target changed; review and confirm the current branch");
      requireFact(command.method === "squash" || task.integration?.method !== "pr", "A pull request is already open for this version");
      task.integration = { method: command.method, targetBranch: command.targetBranch, digest: command.digest, mock: true, message: command.message, title: command.title, draft: command.draft };
      if (command.method === "squash") {
        task.status = "done"; task.work = "merged"; task.doneAt = command.now;
        addLine(state, task, "merge", `You merged into ${command.targetBranch}`, command.now, true);
        afterMerge(state, task, command.message, command.deleteWorktree, command.now);
      } else addLine(state, task, "merge", `You created a pull request into ${command.targetBranch}. It is not merged`, command.now, true);
      break;
    default: return false;
  }
  taskEvent(state, task, command.type);
  return true;
}
