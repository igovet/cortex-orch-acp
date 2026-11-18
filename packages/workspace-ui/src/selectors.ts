import { boardPhase, dependencyReady, descriptionLabel, isOpen, plainModel, targetBranch } from "@cortex/workspace-domain";
import type { Check, CheckOutcome, EngineWeeklyUsage, Machine, Project, Sprint, Task, TaskStatus, Worker, WorkspaceState } from "@cortex/workspace-domain";
import type { Scope } from "./router";
import { share } from "./stable";

/**
 * Read models for the screens. Every counter and empty state comes from these lists, so a badge and the list it
 * counts can never disagree. Rules stay in the domain; nothing here changes state or invents a fact the domain lacks.
 */
export const COLUMNS = [
  { id: "needs", label: "Needs you", icon: "question", tone: "st-warn" },
  { id: "todo", label: "Not started", icon: "circle", tone: "" },
  { id: "work", label: "Working", icon: "loader", tone: "st-run" },
  { id: "review", label: "Review", icon: "eye", tone: "st-warn" },
  { id: "merge", label: "Ready to merge", icon: "merge", tone: "st-ok" },
  { id: "done", label: "Done", icon: "check-circle", tone: "" },
] as const;
export type ColumnId = typeof COLUMNS[number]["id"];
const STATUS_COLUMN: Record<TaskStatus, ColumnId> = {
  draft: "todo", working: "work", stopping: "work",
  needs_you: "needs", review: "review", ready_to_merge: "merge", done: "done",
  // A run that ended on its own is the owner's to look at. It is never Done. A paused task runs nothing and waits for the
  // owner's Resume or Close, so it is never counted as Working either.
  failed: "needs", cancelled: "needs", paused: "needs",
};
const STATUS_WORD: Record<TaskStatus, string> = {
  draft: "Not started", working: "Working", stopping: "Stopping", paused: "Paused",
  needs_you: "Needs you", review: "Review", ready_to_merge: "Ready to merge", done: "Done", failed: "Run failed", cancelled: "Run cancelled",
};

export const sameBranch = (task: Pick<Task, "checkout">, target: string) => !!task.checkout.branch && task.checkout.branch === target;
/** A project-folder task whose target branch is not known yet (a record written by an earlier version names none) is not
 *  called mergeable: nothing says it works on another branch than the one it would finish on. */
const targetUnknown = (task: Pick<Task, "checkout">, target: string) => task.checkout.kind === "main" && !target;
/** Where an accepted result waits: a task on its target branch, or in a folder without Git, is finished in place, never merged. */
export const readyStep = (task: Pick<Task, "checkout">, target: string, hasGit: boolean | undefined) => hasGit === false || sameBranch(task, target) || targetUnknown(task, target) ? "Ready to finish" : "Ready to merge";
/** A task the owner closed as is. Only a result that finished review and verification (accepted, or ready on its own) reads as checked. */
export const closedAsIs = (task: Task) => task.notVerifiedByCortex || (!task.acceptance && task.result?.automaticReady !== true) ? "Closed as is, not verified"
  : task.acceptance ? `Closed as is, accepted v${task.acceptance.version}` : "Closed as is, verified";
/** An answer that changed no files has nothing to commit; it is closed, never reviewed again or committed. */
export const nothingToCommit = (task: Task) => task.integrationFacts?.noChanges === true && !task.integrationFacts.commitAvailable;
export const readyLabel = (task: Task, target: string) => task.integrationFacts?.alreadyInTarget && task.integrationFacts.targetBranch === target ? `Already in ${target}` : nothingToCommit(task) ? "Nothing to commit" : sameBranch(task, target) ? task.integrationFacts?.commitAvailable ? `Commit to ${target}` : "Review task" : targetUnknown(task, target) ? "Ready to finish" : "Ready to merge";

/** Recovery and stop reasons the runtime records as a code, in words. A stop that is not a lost turn or an unknown outcome
 *  (a result whose completion could not be confirmed, a step that could not continue) is a stop, not an interruption. */
const STOP_REASONS: Record<string, string> = {
  turn_interrupted: "The turn was interrupted",
  native_session_outcome_unknown: "The app stopped while this task ran, so how its engine session ended is not known",
  native_control_outcome_unknown: "Whether your last stop, pause or message took effect is not known",
  native_shutdown_unconfirmed: "The engine's exit could not be confirmed",
  native_answer_delivery_outcome_unknown: "Whether your answer reached the task is not known",
  native_outside_outcome_unknown: "Whether an approved command or write outside the sandbox finished is not known",
  native_verification_outcome_unknown: "Whether the final check finished is not known",
  legacy_effect_outcome_unknown: "Whether a command started by an earlier version of the app finished is not known",
  native_recovery_failed: "The app could not restore this task after a restart",
  native_continuation_unavailable: "The task could not continue with its next step",
  native_session_start_failed: "A worker session could not start",
  coordinator_session_start_failed: "The coordinator session could not start",
  native_progress_limit: "The task reached its limit of steps",
  native_driver_progress_limit: "The task reached its limit of steps",
  coordinator_no_progress_limit: "The coordinator stopped making progress",
};
/** A waiting reason in words: a known code reads as what it means, an unknown bare code as an internal stop with its code
 *  spelled out, and a written reason (a question, the runtime's own sentence) stays as written. */
export function stopReason(reason: string | null | undefined): string {
  const code = (reason ?? "").trim();
  if (!code) return "The turn was interrupted";
  if (Object.hasOwn(STOP_REASONS, code)) return STOP_REASONS[code];
  if (code.startsWith("completion_")) return "The work could not be confirmed as complete";
  if (code.startsWith("lost_worker_")) return "A worker process was lost";
  return /^[a-z][a-z0-9]*(?:_[a-z0-9]+)+$/.test(code) ? `Stopped for an internal reason (${code.replace(/_/g, " ")})` : code;
}
/** Why the runtime refused a worker's result submission, in words. An unknown code is named as recorded. */
const REFUSALS: Record<string, string> = {
  open_failed_check: "a check it cited failed and was still open",
  omitted_failed_check: "it left out a failed check an earlier submission had cited",
  check_blocker_required: "a check it marked not run or unverified gave no reason",
  invalid_native_result: "the result was incomplete",
  native_inline_report_required: "its report was missing",
  native_summary_required: "its summary was empty",
  invalid_publication: "its report title was not valid",
  invalid_check_retraction: "it withdrew a failed check without a valid correction",
  invalid_check_supersession: "it replaced a failed check without a valid correction",
  result_report_not_owned: "it named a report it did not write",
  request_key_reused: "it reused a request key",
  invalid_arguments: "its submission did not match the expected form",
  channel_identity_changed: "its session was no longer the current one",
  worker_required: "it was not allowed to submit a result",
};
export const refusalReason = (code: string) => Object.hasOwn(REFUSALS, code) ? REFUSALS[code] : `the runtime refused it (${code.replace(/_/g, " ")})`;
/** Whether a recovery hold is a lost turn or an outcome nobody could observe, rather than a stop the runtime decided. */
export const interruptedTurn = (reason: string | null | undefined) => { const code = (reason ?? "").trim();
  return !code || code === "turn_interrupted" || code === "native_recovery_failed" || /(?:_outcome_unknown|_unconfirmed)$/.test(code); };

export type Fact = { text: string; tone: string; icon?: string; spin?: boolean };
export type TaskView = {
  id: string; task: Task; project: Project; machine: Machine; sprint: Sprint | null;
  column: ColumnId | "archive"; unknown: boolean; fact: Fact; at: number | null;
  target: string; workers: Worker[]; running: number;
};
export type ProjectSummary = {
  project: Project; machine: Machine; offline: boolean; views: TaskView[];
  counts: Record<ColumnId, number>; archived: number; conflicts: number; worktrees: number;
  sprints: Sprint[]; at: number | null; signal: { text: string; tone: string } | null;
};
export type WorkspaceIndex = {
  now: number; state: WorkspaceState; views: TaskView[]; byId: Map<string, TaskView>;
  summaries: Map<string, ProjectSummary>; archive: TaskView[];
};

export const sprintTitle = (sprint: Sprint) => sprint.title ?? descriptionLabel(sprint.description);
/** The icon for a one-line row: the fact's own icon, otherwise its column's. */
export const rowIcon = (view: TaskView): string => view.fact.icon ?? (view.column === "archive" ? "archive" : COLUMNS.find(c => c.id === view.column)!.icon);
export const checkCounts = (checks: Check[]) => {
  const out: Record<CheckOutcome, number> = { passed: 0, failed: 0, not_run: 0, unverified: 0 };
  for (const check of checks) if (check.kind !== "manual") out[check.outcome]++;
  return out;
};

function statusFact(task: Task, ctx: { target: string; running: number; title: (id: string) => string; sprint: Sprint | null; ready: (id: string) => boolean; next: boolean }): Fact {
  // The runtime's own notice while the coordinator's engine has not opened: a retry, or giving up.
  if (task.warnings?.includes("engine_launch_failed")) return { text: task.issue ?? "The engine could not start", tone: "st-bad", icon: "alert" };
  if (task.warnings?.includes("engine_launch_retrying")) return { text: task.issue ?? "Starting the engine again", tone: "st-warn", icon: "hourglass" };
  // A step queued behind another task's files waits for that task and starts on its own once they are released.
  if (task.warnings?.includes("waiting_for_files") && task.status === "working") return { text: task.issue ?? "Waiting for another task to release files", tone: "st-warn", icon: "hourglass" };
  if (task.observedPhase === "recovery" && task.process === "live") return { text: interruptedTurn(task.issue) ? "Turn interrupted; waiting for your message" : "Stopped; waiting for your message", tone: "st-warn", icon: "pause" };
  if (task.observedPhase && task.observedPhase !== "not_started") return {text:task.observedPhase === "result" ? "Runtime result" : task.observedPhase === "waiting" ? "Waiting for you" : task.observedPhase[0].toUpperCase()+task.observedPhase.slice(1),tone:"st-idle"};
  if (task.conflict === "conflict") return { text: `Conflict with ${ctx.target}`, tone: "st-bad", icon: "alert" };
  if (task.conflict === "resolving") return { text: "Resolving the conflict", tone: "st-run", icon: "loader", spin: true };
  if (task.conflict === "failed") return { text: "Automatic resolution failed", tone: "st-bad", icon: "alert" };
  switch (task.status) {
    case "draft": {
      const waiting = task.dependencies.find(id => !ctx.ready(id));
      if (waiting) return { text: `Waits for ${ctx.title(waiting)}`, tone: "st-idle", icon: "hourglass" };
      if (ctx.sprint) return { text: `${ctx.next ? "Next" : "Queued"} in ${sprintTitle(ctx.sprint)}`, tone: "st-idle", icon: "layers" };
      return { text: "Draft, ready to start", tone: "st-idle" };
    }
    case "working": return { text: ctx.running ? `Executing, ${ctx.running} worker${ctx.running === 1 ? "" : "s"}` : "Executing", tone: "st-run", icon: "loader", spin: true };
    case "failed": return { text: "Run failed", tone: "st-bad", icon: "x-circle" };
    case "cancelled": return { text: "Run cancelled", tone: "st-idle", icon: "x-circle" };
    case "paused": return { text: "Paused by you", tone: "st-idle", icon: "pause" };
    case "stopping": return { text: "Stopping", tone: "st-warn", icon: "hourglass" };
    case "needs_you": return { text: task.issue ? `Asks: ${task.issue.replace(/^question:\s*/i, "") || task.issue}` : "Waiting for you", tone: "st-warn" };
    case "review": {
      if (task.result && task.workspaceDigest !== task.result.digest) return { text: "Stale result, workspace changed", tone: "st-warn", icon: "alert" };
      const checks = task.result?.checks ?? [];
      return { text: checks.length ? `Checks ${checkCounts(checks).passed}/${checks.length}` : "No checks reported", tone: "st-idle" };
    }
    case "ready_to_merge":
      if (sameBranch(task, ctx.target)) return { text: readyLabel(task, ctx.target), tone: "st-idle", icon: "check" };
      if (task.integrationFacts?.alreadyInTarget) return { text: `Already in ${task.integrationFacts.targetBranch}`, tone: "st-idle", icon: "check" };
      if (!task.acceptance) return { text: `${task.resolvedBy === "you" ? "Resolved by you" : "Resolved automatically"}, review v${task.result?.version ?? 1}`, tone: "st-warn", icon: "eye" };
      if (task.integration?.method === "pr") return { text: "Pull request open, not merged", tone: "st-idle", icon: "merge" };
      return { text: `${task.acceptance.automatic ? "Automatically accepted" : "Accepted"} v${task.acceptance.version}`, tone: "st-ok" };
    case "done":
      if (task.integration?.method === "commit") return { text: `Committed to ${task.integration.targetBranch}`, tone: "st-idle", icon: "check" };
      if (task.closedAs === "external") return { text: "Done outside the app", tone: "st-idle", icon: "circle" };
      if (task.closedAs === "as_is") return { text: closedAsIs(task), tone: "st-idle", icon: "circle" };
      if (task.closedAs === "cancel") return { text: "Cancelled", tone: "st-idle", icon: "x-circle" };
      return { text: task.work === "merged" ? "Merged" : "Done", tone: "st-merge", icon: "merge" };
  }
}

/** A route the engines stopped offering has no label left. Its id is still not one. */
export const plainChoice = (id: string) => /^xr_[0-9a-f]{6,}$/.test(id) ? "Model no longer offered" : plainModel(id);
/** The version people talk about. The build hash stays on the diagnostics page. */
export const plainVersion = (version: string) => version.split("+")[0];

/** A sprint the lists show: an open one, or one whose deletion has not finished. That one is archived, but vanishing would
 *  hide the tasks it still has to delete and the retry. */
export const onView = (sprint: Sprint) => !sprint.archivedAt || !!sprint.deletion;

/** An unfinished deletion in words: how many tasks are left, the one by name, and what it waits for. */
export function deletionState(index: WorkspaceIndex, sprint: Sprint) {
  const left = sprint.deletion?.left ?? [];
  if (!left.length) return "Deleting, no tasks left, waiting to finish";
  const named = left.length === 1 ? index.state.tasks.find(t => t.id === left[0])?.title : undefined;
  return left.length === 1 ? `Deleting, 1 task left${named ? `: ${named}` : ""}, waiting for its stop`
    : `Deleting, ${left.length} tasks left, waiting for them to stop`;
}

export function buildIndex(state: WorkspaceState, now: number): WorkspaceIndex {
  const projects = new Map(state.projects.map(p => [p.id, p]));
  const machines = new Map(state.machines.map(m => [m.id, m]));
  const sprints = new Map(state.sprints.filter(onView).map(s => [s.id, s]));
  const tasks = new Map(state.tasks.map(t => [t.id, t]));
  const last = new Map<string, string>();
  for (const event of state.events) if (event.occurredAt) last.set(event.taskId, event.occurredAt);
  const workers = new Map<string, Worker[]>();
  for (const worker of state.workers) workers.set(worker.taskId, [...(workers.get(worker.taskId) ?? []), worker]);
  const ready = (id: string) => { const t = tasks.get(id); return !!t && dependencyReady(t); };
  const title = (id: string) => tasks.get(id)?.title ?? "another task";
  // A sprint comes from the project's plan and its tasks from the project's runtime, so a sprint can name tasks that are
  // not loaded yet (the runtime is starting, stopped or refused). The index is read by every screen: it must not throw.
  const nextInSprint = new Set(state.sprints.filter(s => !s.archivedAt).flatMap(s => {
    const next = s.taskIds.map(id => tasks.get(id)).find(t => t?.status === "draft" && t.dependencies.every(ready));
    return next ? [next.id] : [];
  }));

  const views = state.tasks.map((task): TaskView => {
    const project = projects.get(task.projectId)!, machine = machines.get(project.machineId)!;
    // The sprint editor's ordered membership is authoritative, including older task projections without a reverse link.
    const sprint = state.sprints.find(s => onView(s) && s.projectId === task.projectId && s.taskIds.includes(task.id))
      ?? (task.sprintId ? sprints.get(task.sprintId) ?? null : null);
    // The task's own state and its workers come from separate reads. When the workers' read could not be refreshed, a worker
    // last seen running under a task that has since ended is not confirmed: it never shows as working or counts as running.
    const ended = ["review", "ready_to_merge", "done", "failed", "cancelled", "paused"].includes(task.status);
    const phase = boardPhase(state, task.id), own = (workers.get(task.id) ?? []).map((w): Worker => ended && (w.status === "working" || w.status === "waiting_you") ? { ...w, status: "unknown", now: null, runningSince: null, stale: true } : w).reverse();
    // The machine lists workers oldest first; every screen shows the newest one on top, so the order is turned once, here.
    const running = own.filter(w => w.status === "working").length, target = targetBranch(state, task);
    const unknown = phase === "unknown";
    const column = phase === "archive" ? "archive" : phase === "unknown" ? STATUS_COLUMN[task.status] : STATUS_COLUMN[phase];
    const fact: Fact = !unknown ? statusFact(task, { target, running, title, sprint, ready, next: nextInSprint.has(task.id) })
      : machine.connection !== "connected" ? { text: `Last known: ${STATUS_WORD[task.status]}`, tone: "st-idle", icon: "help" }
      : task.status === "stopping" ? { text: "Stop not confirmed", tone: "st-warn", icon: "hourglass" }
      : { text: "Process state unknown", tone: "st-idle", icon: "help" };
    const seen = last.get(task.id);
    return { id: task.id, task, project, machine, sprint, column, unknown, fact, at: task.doneAt ?? (seen ? Date.parse(seen) : task.lastConfirmedAt ?? null), target, workers: own, running };
  });

  const summaries = new Map<string, ProjectSummary>(state.projects.map(project => {
    const machine = machines.get(project.machineId)!, offline = machine.connection !== "connected";
    const own = views.filter(v => v.project.id === project.id), active = own.filter(v => v.column !== "archive");
    const counts = Object.fromEntries(COLUMNS.map(c => [c.id, active.filter(v => v.column === c.id).length])) as Record<ColumnId, number>;
    const conflicts = active.filter(v => v.task.conflict !== "none").length;
    const signal = offline ? { text: "Offline", tone: "muted" }
      : conflicts ? { text: conflicts === 1 ? "Conflict" : `Conflicts ${conflicts}`, tone: "st-bad" }
      : counts.needs ? { text: `Needs you ${counts.needs}`, tone: "st-warn" }
      : counts.review ? { text: `In review ${counts.review}`, tone: "st-warn" }
      : counts.merge ? { text: `Ready to finish ${counts.merge}`, tone: "st-warn" } : null;
    const worktrees = active.filter(v => v.task.checkout.kind === "worktree" && ["needs", "work", "review", "merge"].includes(v.column)).length;
    const at = own.reduce<number | null>((max, v) => v.at !== null && (max === null || v.at > max) ? v.at : max, null);
    return [project.id, {
      project, machine, offline, views: active, counts, archived: own.length - active.length, conflicts, worktrees,
      sprints: state.sprints.filter(s => onView(s) && s.projectId === project.id), at, signal,
    }];
  }));
  return { now, state, views, byId: new Map(views.map(v => [v.id, v])), summaries, archive: views.filter(v => v.column === "archive") };
}

/**
 * The index rebuilt for a new state, holding every task view and project summary that equals the last one as the last object.
 * Every screen hangs off the index: a view rebuilt as a new object for an unrelated change would redraw each row built from
 * it and drop whatever the owner had opened there. Only what really changed is new.
 */
export function stableIndex(prev: WorkspaceIndex | undefined, next: WorkspaceIndex): WorkspaceIndex {
  if (!prev) return next;
  const views = share(prev.views, next.views);
  const byId = new Map(views.map(v => [v.id, v]));
  const own = (list: TaskView[]) => list.map(v => byId.get(v.id) ?? v);
  const summaries = new Map([...next.summaries].map(([id, s]) => [id, share(prev.summaries.get(id) as ProjectSummary, { ...s, views: own(s.views) })] as const));
  return { ...next, views, byId, summaries, archive: share(prev.archive, own(next.archive)) };
}

/** Where a card's primary action lands: the screen that decision is made on, not just the task. */
export const decisionRoute = (view: TaskView) => {
  const base = { name: "task" as const, machine: view.machine.id, task: view.id };
  if (view.task.conflict === "conflict" || view.task.conflict === "failed") return { ...base, tab: "changes" as const, sub: "conflict" as const };
  return view.column === "review" || view.column === "merge" ? { ...base, tab: "changes" as const } : base;
};
export const projectsOn = (index: WorkspaceIndex, machineId: string) => [...index.summaries.values()].filter(s => s.machine.id === machineId);

/**
 * The board's columns add up to the projects table. `attention` is Home's one Needs you list: every task waiting on the owner,
 * each once, in the order its decision matters (a question, permission or conflict, then a result to review, then accepted
 * work to finish), and the title band counts the same list.
 */
export function homeView(index: WorkspaceIndex, machineId: string) {
  const projects = projectsOn(index, machineId), views = projects.flatMap(p => p.views);
  const of = (column: ColumnId) => views.filter(v => v.column === column);
  const needs = of("needs"), review = of("review"), merge = of("merge");
  return { projects, needs, working: of("work"), review, merge, attention: [...needs.filter(v => !v.unknown), ...review.filter(v => !v.unknown), ...merge.filter(v => !v.unknown)] };
}

/** The owner is what this task is waiting for, so it must read differently from the work that runs by itself. */
export const awaitsOwner = (view: TaskView) => !view.unknown && (view.column === "needs" || view.column === "review" || view.column === "merge");
/** The one word on a sidebar task row. A stopping task sits in the Working column but never reads as working; a paused
 *  one waits in Needs you and says it is paused. */
export function sideWord(view: TaskView): string {
  if (view.task.status === "paused") return "Paused";
  if (view.column === "needs") return "Needs you";
  if (view.column === "work") return view.task.status === "stopping" ? "Stopping" : "Working";
  if (view.column === "review") return "Review";
  return view.project.hasGit === false || sameBranch(view.task, view.target) ? "Finish" : "Merge";
}

export type Decision = { key: string; kind: string; icon: string; tone: string; ask: string; where: string; view: TaskView | null; project: Project; sprintId: string | null; at: number | null };
/** Inbox covers every machine, so a remote one names itself. */
const placed = (machine: Machine, text: string) => machine.kind === "local" ? text : `${machine.name} / ${text}`;
export function decisionOf(view: TaskView): Decision {
  const { task } = view, issue = task.issue ?? "";
  const base = { key: `task:${view.id}`, where: placed(view.machine, `${view.project.name} / ${task.title}`), view, project: view.project, sprintId: null, at: view.at };
  if (task.conflict !== "none") return { ...base, kind: "Conflict", icon: "alert", tone: "st-bad", ask: `Merge conflict with ${view.target}` };
  // The domain keeps the waiting reason as text, so the row label follows it rather than a separate kind.
  if (/permission/i.test(issue)) return { ...base, kind: "Permission", icon: "shield", tone: "st-warn", ask: issue };
  if (/question/i.test(issue)) return { ...base, kind: "Question", icon: "question", tone: "st-warn", ask: issue.replace(/^question:\s*/i, "") || issue };
  // A paused, failed or cancelled run asks nothing: it reads as what happened, never as "Waiting for you".
  if (task.status === "paused") return { ...base, kind: "Paused", icon: "pause", tone: "st-idle", ask: "Paused by you. Resume it, or close it" };
  if (task.status === "failed" || task.status === "cancelled") return { ...base, kind: task.status === "failed" ? "Failed" : "Cancelled", icon: "x-circle", tone: task.status === "failed" ? "st-bad" : "st-idle", ask: view.fact.text };
  return { ...base, kind: "Decision", icon: "pause", tone: "st-bad", ask: issue ? stopReason(issue) : "Waiting for you" };
}
/** A task's one line in Needs you: the decision it waits on, its result to review, or its accepted work to finish. */
export function attentionOf(view: TaskView): Decision {
  if (view.column === "needs") return decisionOf(view);
  const { task } = view;
  const base = { key: `task:${view.id}`, where: placed(view.machine, `${view.project.name} / ${task.title}`), view, project: view.project, sprintId: null, at: view.at };
  if (view.column === "review") {
    if (task.result && task.workspaceDigest !== task.result.digest) return { ...base, kind: "Review", icon: "alert", tone: "st-warn", ask: "The result is stale: the workspace changed after it" };
    const c = checkCounts(task.result?.checks ?? []);
    const parts = [c.passed && `${c.passed} passed`, c.failed && `${c.failed} failed`, c.not_run && `${c.not_run} not run`, c.unverified && `${c.unverified} unverified`].filter(Boolean);
    return { ...base, kind: "Review", icon: "eye", tone: "st-warn", ask: `Result v${task.result?.version ?? 1} is ready${parts.length ? `: ${parts.join(", ")}` : ", no checks reported"}` };
  }
  const into = sameBranch(task, view.target) || !task.checkout.branch ? "" : `: ${task.checkout.branch} into ${view.target}`;
  return { ...base, kind: "Finish", icon: "merge", tone: "st-ok", ask: `${view.fact.text}${into}` };
}
/**
 * A board card's or list row's one concise fact: the decision a waiting task asks for (the question itself, the result's check
 * counts, what is accepted and where it goes), "Working" without a worker count for running work, otherwise the status fact.
 * A runtime notice (an engine that would not start, an observed phase) keeps its own words.
 */
export function cardFact(view: TaskView): Fact {
  const notice = !!view.task.warnings?.length || (!!view.task.observedPhase && view.task.observedPhase !== "not_started");
  if (view.unknown || notice) return view.fact;
  // A run that failed or was cancelled waits on the owner too, but its own outcome is the fact to read.
  const waiting = view.column === "needs" && (view.task.status === "needs_you" || view.task.conflict !== "none");
  if (waiting || view.column === "review" || view.column === "merge") {
    const d = attentionOf(view);
    return { text: d.ask, tone: d.tone, icon: d.icon };
  }
  if (view.task.status === "working" && view.task.conflict === "none") return { ...view.fact, text: "Working" };
  return view.fact;
}

/**
 * Inbox is Home's Needs you list for every connected machine, in the same order (attentionOf): what waits on an answer, a
 * permission or a conflict, with each sprint planning question beside them, then results to review, then accepted work to
 * finish. Each task appears once; each row opens where that is decided.
 */
export function inboxItems(index: WorkspaceIndex): Decision[] {
  const connected = (m: Machine) => m.connection === "connected";
  const live = index.views.filter(v => !v.unknown && connected(v.machine));
  const of = (column: ColumnId) => live.filter(v => v.column === column).map(attentionOf);
  const planning = index.state.sprints.filter(s => !s.archivedAt).flatMap((sprint): Decision[] => {
    const summary = index.summaries.get(sprint.projectId)!;
    if (sprint.planning !== "question" || !connected(summary.machine)) return [];
    const open = index.state.sessions.find(s => s.taskId === sprint.planningTaskId)?.decisions.filter(d => d.kind === "question" && isOpen(d)) ?? [];
    const first = open[0]?.text ?? sprint.conversation.filter(m => m.kind === "question").at(-1)?.text ?? "Planning question";
    const ask = open.length > 1 ? `${open.length} questions: ${first}` : first;
    return [{ key: `sprint:${sprint.id}`, kind: "Planning", icon: "layers", tone: "st-warn", ask, where: placed(summary.machine, `${summary.project.name} / ${sprintTitle(sprint)}`), view: null, project: summary.project, sprintId: sprint.id, at: open[0]?.at ?? null }];
  });
  return [...of("needs"), ...planning, ...of("review"), ...of("merge")];
}

export const inScope = (summary: ProjectSummary, scope: Scope) =>
  summary.views.filter(v => scope.kind === "all" || (scope.kind === "standalone" ? !v.sprint : v.sprint?.id === scope.id));
export function columnsOf(views: TaskView[]) {
  const out = Object.fromEntries(COLUMNS.map(c => [c.id, [] as TaskView[]])) as Record<ColumnId, TaskView[]>;
  for (const view of views) if (view.column !== "archive") out[view.column].push(view);
  return out;
}

export type GroupedRow = { kind: "task"; view: TaskView } | { kind: "sprint"; sprint: Sprint; count: number; column?: ColumnId };
/** One column or list group of mixed work: standalone tasks, then each sprint under its own name. */
export function bySprint(views: TaskView[]): GroupedRow[] {
  const sprints = new Map<string, { sprint: Sprint; views: TaskView[] }>();
  for (const view of views) if (view.sprint) (sprints.get(view.sprint.id) ?? sprints.set(view.sprint.id, { sprint: view.sprint, views: [] }).get(view.sprint.id)!).views.push(view);
  return [
    ...views.filter(v => !v.sprint).map((view): GroupedRow => ({ kind: "task", view })),
    ...[...sprints.values()].flatMap((g): GroupedRow[] => [{ kind: "sprint", sprint: g.sprint, count: g.views.length }, ...g.views.map((view): GroupedRow => ({ kind: "task", view }))]),
  ];
}

export function sprintView(index: WorkspaceIndex, sprint: Sprint) {
  const views = sprint.taskIds.flatMap(id => { const v = index.byId.get(id); return v ? [v] : []; });
  const drafts = views.filter(v => v.task.status === "draft");
  const ready = (v: TaskView) => v.task.dependencies.every(id => { const d = index.byId.get(id); return !!d && dependencyReady(d.task); });
  const started = views.length - drafts.length;
  const planning = { planning: "Planning, no tasks yet", question: "Planning, question open", ready: "Plan ready", cancelled: "Planning cancelled", created: "" }[sprint.planning];
  const queue = { idle: "queue idle", running: "queue running", paused: "queue paused", complete: "queue complete" }[sprint.queueState];
  return {
    sprint, title: sprintTitle(sprint), views, started, missing: sprint.taskIds.length - views.length, usable: sprint.planning === "created",
    now: views.find(v => v.task.status === "working") ?? null, next: drafts.find(ready) ?? null, blocked: drafts.filter(v => !ready(v)).length,
    state: sprint.deletion ? deletionState(index, sprint) : planning || `${sprint.runMode === "automatic" ? "Automatic" : "Manual"}, ${queue}, ${started} of ${sprint.taskIds.length} started${views.length < sprint.taskIds.length && index.state.projects.find(p => p.id === sprint.projectId)?.issue ? ", task details unavailable" : ""}`,
  };
}

/** The sprint's landing tab: its plan until tasks exist, its board after. */
export function sprintRoute(machine: string, sprint: Sprint) {
  // An unfinished deletion lands where it is said and retried: the sprint's summary.
  if (sprint.deletion) return { name: "sprint" as const, machine, project: sprint.projectId, sprint: sprint.id, tab: "summary" as const };
  return sprint.planning === "created"
    ? { name: "project" as const, machine, project: sprint.projectId, view: "board" as const, scope: { kind: "sprint" as const, id: sprint.id } }
    : { name: "sprint" as const, machine, project: sprint.projectId, sprint: sprint.id, tab: "planning" as const };
}
export type SideRow = { kind: "task"; view: TaskView } | { kind: "sprint"; sprint: Sprint; started: number; total: number };
/** Up to five live tasks and sprints, the most urgent first. A sprint whose queue is complete stays, as Done, until it is
 *  closed into the Archive: its tasks may still wait for the owner to finish them, and nothing closes by itself. */
export function sideRows(index: WorkspaceIndex, summary: ProjectSummary): SideRow[] {
  const of = (column: ColumnId): SideRow[] => summary.views.filter(v => v.column === column).map(view => ({ kind: "task", view }));
  const open = summary.sprints.filter(s => s.planning !== "cancelled");
  const sprints = [...open.filter(s => s.queueState !== "complete"), ...open.filter(s => s.queueState === "complete")].map((sprint): SideRow => {
    const view = sprintView(index, sprint);
    return { kind: "sprint", sprint, started: view.started, total: sprint.taskIds.length };
  });
  return [...of("needs"), ...sprints, ...of("work"), ...of("review"), ...of("merge")].slice(0, 5);
}

const ENGINE_LABEL = { claude: "Claude", codex: "Codex", opencode: "OpenCode" } as const;
/** One weekly window as shown: `label` is short (sidebar, inspector), `name` says which window in full. */
export type LimitWindow = { key: string; label: string; name: string; usedPercent: number | null; resetAt: string | null; observedAt: string | null };
export type EngineLimit = Omit<EngineWeeklyUsage, "engine" | "windows"> & { engine: keyof typeof ENGINE_LABEL; label: string; windows: LimitWindow[] };
type LimitSource = Pick<EngineWeeklyUsage, "usedPercent" | "resetAt" | "observedAt" | "windows">;
const titled = (words: string) => words.split("_").map(w => w && w[0].toUpperCase() + w.slice(1)).join(" ");
/** Claude always shows its general weekly window and its Fable window, each "Not reported" until reported,
 * then any other reported model window. Other engines have one weekly window. */
function limitWindows(engine: EngineLimit["engine"], row: LimitSource): LimitWindow[] {
  if (engine !== "claude") return [{ key: "weekly", label: "This week", name: "This week", usedPercent: row.usedPercent, resetAt: row.resetAt, observedAt: row.observedAt }];
  const reported = row.windows ?? [];
  const shown = (key: string, label: string, scope: string): LimitWindow => {
    const w = reported.find(x => x.name === key);
    return { key, label, name: `This week, ${scope}`, usedPercent: w?.usedPercent ?? null, resetAt: w?.resetAt ?? null, observedAt: w?.observedAt ?? null };
  };
  const others = reported.filter(w => w.name.startsWith("seven_day_") && w.name !== "seven_day_fable" && w.usedPercent !== null)
    .map(w => { const label = titled(w.name.slice("seven_day_".length)); return shown(w.name, label, label); });
  return [shown("seven_day", "All models", "all models"), shown("seven_day_fable", "Fable", "Fable"), ...others];
}
/** Every connected engine with a usage source is represented, even when no observation exists yet.
 * OpenCode has no weekly usage source: it shows a row only when usage is actually reported, never "Not reported". */
export function engineLimits(state: WorkspaceState, machine: Machine): { rows: EngineLimit[]; note: string | null } {
  if (machine.kind !== "local") return { rows: [], note: `Engine limits are not available for ${machine.name}.` };
  const rows: EngineLimit[] = (["claude", "codex", "opencode"] as const).flatMap<EngineLimit>(engine => {
    const row = state.engineUsage.find(u => u.engine === engine && u.connected);
    const connected = machine.connections.some(c => c.engine === engine && c.state === "connected");
    if (row) return [{ ...row, engine, label: ENGINE_LABEL[engine], windows: limitWindows(engine, row) }];
    if (!connected || engine === "opencode") return [];
    const empty = { connected: true, usedPercent: null, resetAt: null, observedAt: null, window: "weekly" as const };
    return [{ ...empty, engine, label: ENGINE_LABEL[engine], windows: limitWindows(engine, empty) }];
  });
  return { rows, note: rows.length ? null : "No engine is connected on this machine." };
}
/** A snapshot older than this is marked stale. Limits are read again every few minutes while the app is open. */
export const LIMIT_STALE_MS = 15 * 60_000;
/** When a value was observed, and whether that is too long ago to rely on. `null` when no time was reported. */
export function asOf(observedAt: string | null, now: number): { text: string; stale: boolean } | null {
  const at = observedAt === null ? NaN : Date.parse(observedAt);
  if (!Number.isFinite(at)) return null;
  const stale = now - at > LIMIT_STALE_MS;
  return { text: `${stale ? "Stale, as of" : "As of"} ${now - at > 86_400_000 ? dayClock(observedAt!) : clock(observedAt)}`, stale };
}
/** The newest observation among an engine's windows. */
export const newestObserved = (row: EngineLimit) => row.windows.map(w => w.observedAt).filter((t): t is string => t !== null).sort().at(-1) ?? null;
/** Who a window belongs to, in words: "Claude all models", "Claude Fable", "Codex". */
export const windowTitle = (row: EngineLimit, w: LimitWindow) => row.engine === "claude" ? `${row.label} ${w.key === "seven_day" ? "all models" : w.label}` : row.label;
/** "Claude all models 67%, Claude Fable 2%, Codex 33%": every window, an unknown one said as such. */
export const limitsText = (rows: EngineLimit[]) => rows.flatMap(r => r.windows.map(w => `${windowTitle(r, w)} ${w.usedPercent === null ? "not reported" : `${w.usedPercent}%`}`)).join(", ");

const two = (n: number) => String(n).padStart(2, "0");
/** Times are shown on this computer's own clock, in its time zone. */
export const clock = (at: number | string | null) => {
  if (at === null) return "unknown";
  const d = new Date(at);
  return `${two(d.getHours())}:${two(d.getMinutes())}`;
};
export const dayClock = (at: string) => `${["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"][new Date(at).getDay()]} ${clock(at)}`;
export function age(at: number | null, now: number) {
  if (at === null) return "";
  const seconds = Math.max(0, (now - at) / 1000);
  if (seconds < 60) return "now";
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m`;
  if (seconds < 86400) return `${Math.floor(seconds / 3600)}h`;
  return `${Math.floor(seconds / 86400)}d`;
}
