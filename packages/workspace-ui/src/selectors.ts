import { automaticQueueOf, boardPhase, dependencyReady, descriptionLabel, isOpen, orderReasonText, plainModel, targetBranch } from "@cortex/workspace-domain";
import type { Decision as TaskDecision, Check, CheckOutcome, EngineWeeklyUsage, HoldActor, Machine, OwnerHoldAction, Project, PlanningFailure, Sprint, SprintOrder, SprintOrderPending, SprintRunMode, Task, TaskStatus, Worker, WorkspaceState } from "@cortex/workspace-domain";
import type { Scope } from "./router";
import { share } from "./stable";
import { plainPhase, plainReason, plainStop } from "./status-text";

export const sprintModeLabel: Record<SprintRunMode, string> = { manual: "Manual", hybrid: "Hybrid", automatic: "Automatic" };

export const sprintModeDescription: Record<SprintRunMode, string> = {
  manual: "You start each ready task. Starting one never starts its neighbours.",
  hybrid: "The queue runs one task at a time in dependency order. You confirm each merge.",
  automatic: "Tasks go to Done on their own in the current branch. You answer work questions and permission requests, and review blocked results.",
};

export const staleResult = (task: Task) => !!task.result &&
  // A check receipt is not the final result's workspace identity.
  !(task.reviewActions?.pending === true && task.reviewActions.unblockAction === "wait") &&
  (task.workspaceDigest !== task.result.digest || task.result.warnings?.includes("workspace_drift") === true);

type ReviewHoldView = Pick<TaskView, "id" | "sprint" | "task" | "unknown">;

function resultHold(view: ReviewHoldView) {
  const hold = automaticQueueOf(view.sprint);
  if (!view.unknown && hold?.needsOwner && hold.taskId === view.id && hold.kind === "settlement_blocked" &&
      (view.task.status === "review" || view.task.status === "ready_to_merge")) return hold;
}

export function automaticStaleResultHold(view: ReviewHoldView) {
  const hold = resultHold(view);
  return !!hold && (["stale_result", "workspace_drift", "task_content_unknown", "task_files_changed"].includes(hold.code)
    || hold.code === "automatic_acceptance_blocked" && !!view.task.acceptance?.automatic && staleResult(view.task));
}

/** Surface the queue's recorded review decision on the task it belongs to. */
export function automaticAcceptanceHold(view: ReviewHoldView) {
  const hold = automaticQueueOf(view.sprint);
  if (!view.unknown && view.task.status === "ready_to_merge" && hold?.needsOwner && hold.taskId === view.id &&
      hold.kind === "owner_integration" && hold.code === "owner_accepted") return true;
  return (!view.task.acceptance && resultHold(view)?.code === "automatic_acceptance_blocked") || automaticStaleResultHold(view);
}

/** The service's decision about a held task: who acts next, what the owner is asked and the actions offered. It comes from
 *  the queue hold of a Hybrid or Automatic sprint, or, outside a queue hold, from the task's own facts (a failed run).
 *  The pinned conversation card renders from this alone; nothing here re-derives the decision from other flags. */
export type OwnerHold = {
  source: "queue" | "task"; mode: SprintRunMode | null; sprintId: string | null; code: string;
  nextActor: HoldActor; reason: string; asks: string[]; actions: OwnerHoldAction[]; allowed: boolean;
  /** The service's identity of this exact hold; an action sent from an older card is refused instead of applied to a newer hold. */
  holdId?: string;
};
export function ownerHold(view: ReviewHoldView): OwnerHold | null {
  if (view.unknown || view.task.status === "done" || view.task.status === "closed_unverified" || view.task.archive) return null;
  const sprint = view.sprint, hold = automaticQueueOf(sprint);
  if (sprint && hold?.nextActor && hold.taskId === view.id && sprint.queueState === "paused")
    return { source: "queue", mode: sprint.runMode, sprintId: sprint.id, code: hold.code, nextActor: hold.nextActor, reason: hold.reason,
      asks: hold.asks ?? [], actions: hold.ownerActions ?? [], allowed: typeof hold.allowedAt === "number", ...(hold.holdId ? { holdId: hold.holdId } : {}) };
  // A queue that runs decides itself first: a failed member goes back to its agent before anyone is asked.
  if (sprint && sprint.runMode !== "manual" && sprint.queueState === "running") return null;
  const facts = view.task.reviewActions;
  if (view.task.status === "failed" && facts?.nextActor && facts.ownerActions?.length)
    return { source: "task", mode: sprint?.runMode ?? null, sprintId: sprint?.id ?? null, code: "task_failed", nextActor: facts.nextActor,
      reason: facts.reason ?? "The last run of this task ended as failed.", asks: facts.asks ?? [], actions: facts.ownerActions, allowed: false };
  return null;
}

/** One authorization applies to a displayed service result on its own local branch. */
export function automaticFinishOffer(view: TaskView) {
  const { sprint, task, target } = view;
  if (!sprint || sprint.queueState !== "paused" || !automaticAcceptanceHold(view) || automaticStaleResultHold(view) ||
      !task.result?.sourceVersion || staleResult(task) || task.acceptance?.automatic || task.conflict !== "none" ||
      task.integration || task.checkout.kind !== "main" || !sameBranch(task, target) || view.project.hasGit === false ||
      (sprint.targetBranch && sprint.targetBranch !== target) || task.reviewActions?.pending || !task.reviewActions?.accept) return null;
  return { sprintId: sprint.id, targetBranch: target, sourceVersion: task.result.sourceVersion, digest: task.result.digest };
}

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
  { id: "closed_unverified", label: "Closed, not verified", icon: "alert", tone: "" },
] as const;
export type ColumnId = typeof COLUMNS[number]["id"];
const STATUS_COLUMN: Record<TaskStatus, ColumnId> = {
  draft: "todo", working: "work", stopping: "work",
  needs_you: "needs", review: "review", ready_to_merge: "merge", done: "done", closed_unverified: "closed_unverified",
  // A run that ended on its own is the owner's to look at. It is never Done. A paused task runs nothing and waits for the
  // owner's Resume or Close, so it is never counted as Working either.
  failed: "needs", cancelled: "needs", paused: "needs",
};
const STATUS_WORD: Record<TaskStatus, string> = {
  draft: "Not started", working: "Working", stopping: "Stopping", paused: "Paused",
  needs_you: "Needs you", review: "Review", ready_to_merge: "Ready to merge", done: "Done", closed_unverified: "Closed, not verified", failed: "Run failed", cancelled: "Run cancelled",
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
// The code-to-sentence table lives in ./status-text.

/**
 * Formats a machine status/reason for a person. This is intentionally only
 * used with fields whose contract is a status/reason; task titles, commands,
 * paths, reports and model text must remain untouched.
 */
export const humanStatus = plainReason;
/** A waiting reason in words: a known code reads as what it means, an unknown bare code as a generic sentence (never the
 *  code), and a written reason (a question, the runtime's own sentence) stays as written. */
export const stopReason = plainStop;
/** Why the runtime refused a worker's result submission, in words. An unknown code reads generically, never as the code. */
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
  review_file_coverage_required: "it had not read every changed file in full",
  review_project_revision_changed: "the files changed while it reviewed",
  review_project_revision_unstable: "the files kept changing while it reviewed",
  command_not_settled: "a command it cited had not finished",
  review_predecessor_reports_required: "it had not read the earlier workers' reports",
  failed_check_repair_required: "it reported a failed check without repairing it",
  review_coverage_unfinished: "it left parts unchecked that it could still check",
  assignment_blocker_required: "it reported a failure without saying what stopped it",
  report_sections_missing: "its report lacked required sections",
  draft_guidance_remaining: "its report still held the template's guidance",
  review_resubmit_required: "it resubmitted a refused review unchanged",
  review_block_findings_required: "it blocked the result without naming the problems",
  review_not_covered_required: "its verdict did not match what it left unchecked",
};
/** An item a refusal named, without the runtime's note on it: `README.md (current complete read is missing or stale)` is `README.md`. */
const missingItem = (item: string) => item.replace(/\s*\([^()]*\)\s*$/, "").trim();
const listed = (items: string[]) => items.length < 2 ? items.join("") : `${items.slice(0, -1).join(", ")} and ${items.at(-1)}`;
/** Why the runtime sent a worker's result back, in words. The items the refusal named make it concrete where they apply. */
export const refusalReason = (code: string, missing: readonly string[] = []) => {
  const items = missing.map(missingItem).filter(Boolean);
  if (items.length) switch (code) {
    case "review_file_coverage_required": return `it had not read ${listed(items)} in full`;
    case "review_coverage_unfinished": return `it left ${listed(items)} unchecked although it could still check ${items.length === 1 ? "it" : "them"}`;
    case "report_sections_missing": return `its report lacked ${items.length === 1 ? "the section" : "the sections"} ${listed(items)}`;
  }
  return Object.hasOwn(REFUSALS, code) ? REFUSALS[code] : "the runtime refused it";
};
/**
 * The worker card's line about its refused result submissions. A worker that was later accepted reads neutrally (it is
 * history and asks nothing); one still without an accepted result reads as the problem it is, with the earlier reasons.
 */
export function refusalLine(r: NonNullable<Worker["resultRefusals"]>, accepted: boolean): { text: string; tone: string } {
  const times = r.count === 1 ? "once" : `${r.count} times`, latest = refusalReason(r.code, r.missing ?? []);
  if (accepted) return { text: `Sent back ${times} before it was accepted (most recently: ${latest}). Nothing is needed from you.`, tone: "muted" };
  const earlier = (r.codes ?? []).filter(c => c !== r.code).map(c => refusalReason(c));
  return { text: `Its result was refused ${times}: ${latest}.${earlier.length ? ` Earlier: ${earlier.join("; ")}.` : ""}`, tone: "st-bad" };
}
const lowerFirst = (text: string) => text ? text[0].toLowerCase() + text.slice(1) : text;
/** What the automatic attempts counted against the current budget went to, in words, or null without that record. */
export function attemptsSpentOn(task: Pick<Task, "automaticContinuation">): string | null {
  const spent = task.automaticContinuation?.spentOn;
  if (!Array.isArray(spent)) return null;
  const parts = spent.filter(s => !!s && typeof s.reason === "string" && Number.isSafeInteger(s.count) && s.count > 0)
    .map(s => `${lowerFirst(plainStop(s.reason)).replace(/\.$/, "")} (${s.count === 1 ? "once" : `${s.count} times`})`);
  return parts.length ? `The attempts went to: ${parts.join("; ")}.` : null;
}
/** Items a reviewer names by their machine name. */
const DEAD_END_ITEM: Record<string, string> = { change_set: "the full list of changed files", "change set": "the full list of changed files" };
/**
 * A review that cannot be completed: the reviewer reported the same item unreachable with its tools in two or more reviews,
 * and the newest still names it. Said to the owner while the task is still before verification, with what they can do.
 * Null otherwise.
 */
export function reviewDeadEnd(task: Pick<Task, "automaticContinuation" | "status" | "observedPhase" | "archive">): string | null {
  const d = task.automaticContinuation?.reviewDeadEnd;
  if (!d || typeof d.item !== "string" || !d.item.trim() || !(typeof d.count === "number" && d.count >= 2)) return null;
  if (task.status === "done" || task.status === "closed_unverified" || task.archive || ["verification", "finalizing", "result"].includes(task.observedPhase ?? "")) return null;
  const raw = d.item.trim(), named = DEAD_END_ITEM[raw.toLowerCase()];
  const file = !named && /^[^\s]+\.[A-Za-z0-9]+$|\//.test(raw) && !/\s/.test(raw);
  const item = named ?? (file ? raw : `“${raw}”`);
  return `The reviewer cannot read ${item} with its tools, so this review cannot be completed. ${file ? "Close as is, or change the file so it can be read." : "Close as is, or change the task so the reviewer can reach it."}`;
}
/** While a replan waits for a worker it cancelled to stop: that wait in words, naming the worker. Null otherwise. */
export const replanStopping = (task: Pick<Task, "warnings" | "issue">) =>
  task.warnings?.includes("cancelled_worker_stopping") ? plainStop(task.issue?.startsWith("cancelled_worker_stopping") ? task.issue : "cancelled_worker_stopping") : null;
/** Whether a recovery hold is a lost turn or an outcome nobody could observe, rather than a stop the runtime decided. */
export const interruptedTurn = (reason: string | null | undefined) => { const code = (reason ?? "").trim();
  return !code || code === "turn_interrupted" || code === "native_recovery_failed" || /(?:_outcome_unknown|_unconfirmed)$/.test(code); };

export type Fact = { text: string; tone: string; icon?: string; spin?: boolean };
export type TaskView = {
  id: string; task: Task; project: Project; machine: Machine; sprint: Sprint | null;
  column: ColumnId | "archive"; unknown: boolean; fact: Fact; at: number | null;
  target: string; workers: Worker[]; running: number;
  filesWait?: { text: string; link?: { taskId: string; title: string; before: string; after: string } };
  automaticFact?: Fact; ownerDecision?: TaskDecision; ownerIntegration?: NonNullable<Sprint["automaticQueue"]>;
};
export type ProjectSummary = {
  project: Project; machine: Machine; offline: boolean; views: TaskView[];
  counts: Record<ColumnId, number>; archived: number; conflicts: number; worktrees: number;
  sprints: Sprint[]; at: number | null; signal: { text: string; tone: string } | null;
};
/** `views` are the tasks the lists show; `planning` the tasks that plan a sprint, read only inside that sprint. `byId` holds both. */
export type WorkspaceIndex = {
  now: number; state: WorkspaceState; views: TaskView[]; planning: TaskView[]; byId: Map<string, TaskView>;
  summaries: Map<string, ProjectSummary>; archive: TaskView[];
  /** The service's execution order of each sprint whose live graph is current and matches the sprint's run mode. */
  orders?: ReadonlyMap<string, SprintOrder>;
};
const NO_ORDERS: ReadonlyMap<string, SprintOrder> = new Map();
const ordinal = (n: number) => `${n}${n % 100 >= 11 && n % 100 <= 13 ? "th" : (["th", "st", "nd", "rd"][n % 10] ?? "th")}`;
/** A sprint's order, only while it still describes this sprint: an order read under another run mode or queue state is stale. */
export const orderOf = (index: Pick<WorkspaceIndex, "orders">, sprint: Sprint | null | undefined): SprintOrder | null => {
  const order = sprint ? index.orders?.get(sprint.id) : undefined;
  return order && sprint && order.mode === sprint.runMode && order.queueState === sprint.queueState ? order : null;
};

/**
 * The tasks that plan a sprint. Each is an ordinary runtime task, but it belongs to its sprint: it has no row, card, count or
 * page of its own, and its conversation and workers read on the sprint's page. Every list is built without these, here.
 */
export const planningTaskIds = (state: Pick<WorkspaceState, "sprints" | "tasks">) => new Set([
  ...state.sprints.flatMap(s => s.planningTaskId ? [s.planningTaskId] : []),
  ...state.tasks.filter(t => !!t.planningSprintId || t.retiredPlanner === true).map(t => t.id),
]);
/** The sprint a planning task plans, or null for any other task. */
export const planningSprintOf = (state: Pick<WorkspaceState, "sprints">, taskId: string) => state.sprints.find(s => s.planningTaskId === taskId) ?? null;
/** The linked planner, when its task row is available. A classified draft alone is not the active planner. */
export const planningTaskOf = (state: Pick<WorkspaceState, "tasks">, sprint: Pick<Sprint, "planningTaskId">) =>
  sprint.planningTaskId && state.tasks.some(t => t.id === sprint.planningTaskId) ? sprint.planningTaskId : null;
/** The open questions and permissions of a sprint's planning task: they wait on the owner as the sprint's, never as a task's. */
export const planningWaits = (state: Pick<WorkspaceState, "tasks" | "sessions">, sprint: Sprint) => {
  const id = sprint.planning === "created" || sprint.planning === "cancelled" ? null : planningTaskOf(state, sprint);
  return id ? state.sessions.find(s => s.taskId === id)?.decisions.filter(d => isOpen(d) && !d.suspended) ?? [] : [];
};
/** The sprint's planning waits on the owner: its question, or an open question or permission of its planning task. */
export const planningNeedsOwner = (state: Pick<WorkspaceState, "tasks" | "sessions">, sprint: Sprint) => sprint.planning === "question" || planningWaits(state, sprint).length > 0;

const planningStep: Record<PlanningFailure["step"], string> = {
  request: "While preparing the planning request",
  create: "While creating the planning task",
  start: "While starting the planning task",
  continue: "While continuing planning",
  run: "While running the planning task",
  restart: "During a service restart",
  internal: "While checking or advancing planning",
};
export function planningRequestInvalid(sprint: Sprint) {
  const failure = sprint.planningFailure;
  return !!failure && typeof failure === "object" && failure.code === "planning_request_invalid";
}
/** Older saves and nonterminal diagnostics do not carry a classified kind or recommendation. */
export function planningChangeRequired(sprint: Sprint) {
  const failure = sprint.planningFailure;
  return planningRequestInvalid(sprint) || (!!failure && typeof failure === "object" && "recommendedAction" in failure && failure.recommendedAction === "change_request");
}
export function classifiedPlanningFailure(sprint: Sprint) {
  const failure = sprint.planningFailure;
  const kind = failure && typeof failure === "object" && "kind" in failure ? failure.kind : null;
  return kind === "owner_stopped" || kind === "limit" || kind === "unknown" ? kind : null;
}
export const stoppedPlanningText = "You stopped planning.";
export const savedPlanningText = "Your saved request, attachments and answers are kept, along with any previously approved proposal.";
export function stoppedPlanningComposerText(sprint: Sprint) {
  return planningChangeRequired(sprint) ? "Change the request to try again."
    : sprint.planningFailure ? "Use the planning actions above."
    : "Planning is paused. Resume to continue.";
}
export function planningFailureDetails(sprint: Sprint) {
  const failure = sprint.planningFailure;
  if (!failure) return null;
  return typeof failure === "string"
    ? { text: humanStatus(failure), step: null, at: null, kind: null }
    : { text: failure.text, step: planningStep[failure.step], at: failure.at,
        kind: classifiedPlanningFailure(sprint) };
}
export const activePlanningWarning = (sprint: Sprint) =>
  sprint.planning === "created" || sprint.planning === "cancelled" ? undefined : sprint.planningWarning;

export const sprintTitle = (sprint: Sprint) => sprint.title ?? descriptionLabel(sprint.description);
/** The icon for a one-line row: the fact's own icon, otherwise its column's. */
export const rowIcon = (view: TaskView): string => view.fact.icon ?? (view.column === "archive" ? "archive" : COLUMNS.find(c => c.id === view.column)!.icon);
export const checkCounts = (checks: Check[]) => {
  const out: Record<CheckOutcome, number> = { passed: 0, failed: 0, not_run: 0, unverified: 0 };
  for (const check of checks) if (check.kind !== "manual") out[check.outcome]++;
  return out;
};

function automaticStatusFact(task: Task, sprint: Sprint | null): Fact | undefined {
  if (task.status === "done" || task.status === "closed_unverified" || task.status === "stopping") return;
  const hold = automaticQueueOf(sprint);
  if (hold?.taskId === task.id && hold.kind !== "owner_integration")
    return { text: hold.kind === "continuation" ? stopReason(hold.reason) : humanStatus(hold.reason), tone: "st-warn", icon: hold.kind === "continuation" ? "pause" : "alert" };
  const continuation = task.automaticContinuation;
  if (continuation?.pause && task.status === "paused")
    return { text: stopReason(continuation.pause.reason), tone: "st-warn", icon: "pause" };
  if (continuation && task.status === "working" && continuation.lastContinuationReason)
    return { text: `Automatic continuation: ${humanStatus(continuation.lastContinuationReason)}`, tone: "st-run", icon: "loader", spin: true };
  if (continuation && task.issue === "automatic_continuation_pending")
    return { text: "Preparing automatic continuation", tone: "st-idle", icon: "hourglass" };
  if (sprint?.runMode !== "automatic") return;
  if (task.conflict !== "none") return { text: "Integration conflict", tone: "st-bad", icon: "alert" };
  switch (task.status) {
    case "review": return { text: "Result recorded; awaiting automatic acceptance", tone: "st-idle", icon: "hourglass" };
    case "ready_to_merge": return { text: task.acceptance ? "Accepted; awaiting automatic finish" : "Awaiting automatic finish", tone: "st-idle", icon: "hourglass" };
    case "needs_you": return { text: humanStatus(task.issue, "Run waiting"), tone: "st-idle", icon: "pause" };
    case "paused": return { text: humanStatus(task.issue, "Paused"), tone: "st-idle", icon: "pause" };
    case "failed": case "cancelled": return { text: humanStatus(task.issue, STATUS_WORD[task.status]), tone: "st-warn", icon: "x-circle" };
  }
}

function statusFact(task: Task, filesWait: TaskView["filesWait"], ctx: { target: string; running: number; title: (id: string) => string; sprint: Sprint | null; ready: (id: string) => boolean; pending?: SprintOrderPending | null }): Fact {
  if (task.status === "closed_unverified") return { text: "Closed by you · Not verified", tone: "st-idle", icon: "alert" };
  const handoff = task.merge?.handoff?.state;
  if (task.conflict !== "none" && (task.merge?.gitState === "unknown" || handoff === "unknown" || handoff === "dispatch_unknown"))
    return { text: "Merge outcome unconfirmed", tone: "st-warn", icon: "alert" };
  if (handoff === "active" && task.conflict === "resolving") return { text: "App handoff recorded", tone: "st-run", icon: "loader" };
  const fresh = task.merge?.freshChecks?.state;
  if (fresh === "not_requested" || fresh === "request_unknown") return {
    text: fresh === "not_requested" ? "Fresh checks not requested" : "Fresh-check request unconfirmed", tone: "st-warn", icon: "alert",
  };
  // The runtime's own notice while the coordinator's engine has not opened: a retry, or giving up.
  if (task.warnings?.includes("engine_launch_failed")) return { text: humanStatus(task.issue, "The engine could not start"), tone: "st-bad", icon: "alert" };
  if (task.warnings?.includes("engine_launch_retrying")) return { text: humanStatus(task.issue, "Starting the engine again"), tone: "st-warn", icon: "hourglass" };
  // A step queued behind another task's files waits for that task and starts on its own once they are released.
  if (filesWait && task.status === "working") return { text: filesWait.text, tone: "st-warn", icon: "hourglass" };
  if (task.observedPhase === "recovery" && task.process === "live") return { text: interruptedTurn(task.issue) ? "Turn interrupted; waiting for your message" : "Stopped; waiting for your message", tone: "st-warn", icon: "pause" };
  if (task.observedPhase && task.observedPhase !== "not_started") return {text:plainPhase(task.observedPhase),tone:"st-idle"};
  if (task.conflict === "conflict") return { text: `Conflict with ${ctx.target}`, tone: "st-bad", icon: "alert" };
  if (task.conflict === "resolving") return { text: "Resolving the conflict", tone: "st-run", icon: "loader", spin: true };
  if (task.conflict === "failed") return { text: "Automatic resolution failed", tone: "st-bad", icon: "alert" };
  switch (task.status) {
    case "draft": {
      const waiting = task.dependencies.find(id => !ctx.ready(id));
      if (waiting) return { text: `Waits for ${ctx.title(waiting)}`, tone: "st-idle", icon: "hourglass" };
      // With the service's order, its words: Next only for its current candidate, a place for a ranked task, a reason otherwise.
      const p = ctx.pending;
      if (ctx.sprint && p) return { text: p.next ? `Next in ${sprintTitle(ctx.sprint)}`
        : p.rank !== null ? `${ctx.sprint.runMode === "manual" ? "Suggested" : "Queued"} ${ordinal(p.rank)} in ${sprintTitle(ctx.sprint)}`
        : `${orderReasonText(p.reason) ?? "Not in the order"} · ${sprintTitle(ctx.sprint)}`, tone: "st-idle", icon: "layers" };
      // No current order from the service (loading, failed, unsupported or stale): a ready draft is only queued, never Next.
      if (ctx.sprint) return { text: `Queued in ${sprintTitle(ctx.sprint)}`, tone: "st-idle", icon: "layers" };
      return { text: "Draft, ready to start", tone: "st-idle" };
    }
    case "working": return { text: ctx.running ? `Executing, ${ctx.running} worker${ctx.running === 1 ? "" : "s"}` : "Executing", tone: "st-run", icon: "loader", spin: true };
    case "failed": return { text: "Run failed", tone: "st-bad", icon: "x-circle" };
    case "cancelled": return { text: "Run cancelled", tone: "st-idle", icon: "x-circle" };
    case "paused": return { text: "Paused by you", tone: "st-idle", icon: "pause" };
    case "stopping": return { text: "Stopping", tone: "st-warn", icon: "hourglass" };
    case "needs_you": return { text: task.issue ? `Asks: ${humanStatus(task.issue.replace(/^question:\s*/i, "")) || task.issue}` : "Waiting for you", tone: "st-warn" };
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
      if (task.integration?.automatic) return { text: task.integration.committed === true ? `Done · Committed to ${task.integration.targetBranch}`
        : task.integration.committed === false ? `Done · Already in ${task.integration.targetBranch}` : "Done", tone: "st-idle", icon: "check" };
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

export function buildIndex(state: WorkspaceState, now: number, orders: ReadonlyMap<string, SprintOrder> = NO_ORDERS): WorkspaceIndex {
  const projects = new Map(state.projects.map(p => [p.id, p]));
  const machines = new Map(state.machines.map(m => [m.id, m]));
  const sprints = new Map(state.sprints.filter(onView).map(s => [s.id, s]));
  const tasks = new Map(state.tasks.map(t => [t.id, t]));
  const last = new Map<string, string>();
  for (const event of state.events) if (event.occurredAt) last.set(event.taskId, event.occurredAt);
  const workers = new Map<string, Worker[]>();
  for (const worker of state.workers) workers.set(worker.taskId, [...(workers.get(worker.taskId) ?? []), worker]);
  const ready = (id: string, mode?: SprintRunMode) => { const t = tasks.get(id); return !!t && dependencyReady(t, mode); };
  const sessions = new Map(state.sessions.map(session => [session.taskId, session]));
  const title = (id: string) => tasks.get(id)?.title ?? "another task";
  // A sprint comes from the project's plan and its tasks from the project's runtime, so a sprint can name tasks that are
  // not loaded yet (the runtime is starting, stopped or refused). The index is read by every screen: it must not throw.
  // A sprint has a Next only when the service's current order names one; without that order no ready draft is called Next.
  const pendingOf = new Map<string, SprintOrderPending>();
  for (const s of state.sprints) {
    if (s.archivedAt) continue;
    const order = orderOf({ orders }, s);
    if (order) for (const p of order.notStarted) pendingOf.set(p.taskId, p);
  }

  const views = state.tasks.map((task): TaskView => {
    const project = projects.get(task.projectId)!, machine = machines.get(project.machineId)!;
    // The sprint editor's ordered membership is authoritative, including older task projections without a reverse link.
    const sprint = state.sprints.find(s => onView(s) && s.projectId === task.projectId && s.taskIds.includes(task.id))
      ?? (task.sprintId ? sprints.get(task.sprintId) ?? null : null);
    // The task's own state and its workers come from separate reads. When the workers' read could not be refreshed, a worker
    // last seen running under a task that has since ended is not confirmed: it never shows as working or counts as running.
    const ended = ["review", "ready_to_merge", "done", "closed_unverified", "failed", "cancelled", "paused"].includes(task.status);
    const phase = boardPhase(state, task.id), own = (workers.get(task.id) ?? []).map((w): Worker => ended && (w.status === "working" || w.status === "waiting_you") ? { ...w, status: "unknown", now: null, runningSince: null, stale: true } : w).reverse();
    // The machine lists workers oldest first; every screen shows the newest one on top, so the order is turned once, here.
    const running = own.filter(w => w.status === "working").length, target = targetBranch(state, task);
    const unknown = phase === "unknown";
    let filesWait: TaskView["filesWait"];
    if (!unknown && phase !== "archive" && task.process === "live"
      && (task.status === "working" || task.status === "needs_you") && task.warnings?.includes("waiting_for_files")) {
      const context = task.filesWait?.holder;
      const holder = context && tasks.get(context.holderTaskId);
      if (context && holder && holder.id !== task.id && holder.projectId === task.projectId) {
        const before = "Waiting for the task “", after = `” to release ${context.releaseTarget}`;
        filesWait = { text: before + holder.title + after,
          ...(!holder.planningSprintId ? { link: { taskId: holder.id, title: holder.title, before, after } } : {}) };
      } else {
        filesWait = { text: humanStatus(task.filesWait ? task.filesWait.message : task.issue, "Waiting for another task to release files") };
      }
    }
    const queue = automaticQueueOf(sprint);
    const ownerIntegration = !unknown && phase !== "archive" && task.status === "ready_to_merge" && queue?.kind === "owner_integration" && queue.taskId === task.id ? queue : undefined;
    // Only a real pending question or permission is actionable, not a recovery's text or coarse native "waiting" phase.
    const ownerDecision = sprint?.runMode === "automatic" && !ended && !unknown && phase !== "archive" && task.status !== "stopping"
      ? sessions.get(task.id)?.decisions.find(d => isOpen(d) && !d.suspended && !d.unavailable) : undefined;
    const automaticFact = !unknown && !ownerIntegration && !ownerDecision ? automaticStatusFact(task, sprint) : undefined;
    const acceptanceHold = automaticAcceptanceHold({ id: task.id, sprint, task, unknown });
    const column = phase === "archive" || phase === "closed_unverified" ? phase : ownerIntegration ? "merge" : ownerDecision || acceptanceHold ? "needs"
      : automaticFact || (unknown && sprint?.runMode === "automatic" && task.status !== "done" && task.status !== "draft") ? "work"
      : STATUS_COLUMN[phase === "unknown" ? task.status : phase];
    const fact: Fact = !unknown ? ownerDecision ? { text: ownerDecision.text, tone: "st-warn", icon: ownerDecision.kind === "permission" ? "shield" : "question" }
      : ownerIntegration ? { text: humanStatus(ownerIntegration.reason), tone: "st-warn", icon: "merge" }
      : automaticFact ?? statusFact(task, filesWait, { target, running, title, sprint, ready: id => ready(id, sprint?.runMode), pending: sprint ? pendingOf.get(task.id) ?? null : null })
      : machine.connection !== "connected" ? { text: `Last known: ${STATUS_WORD[task.status]}`, tone: "st-idle", icon: "help" }
      : task.status === "stopping" ? { text: "Stop not confirmed", tone: "st-warn", icon: "hourglass" }
      : replanStopping(task) ? { text: replanStopping(task)!.replace(/\..*$/, ""), tone: "st-warn", icon: "hourglass" }
      : { text: "Process state unknown", tone: "st-idle", icon: "help" };
    const seen = last.get(task.id);
    return { id: task.id, task, project, machine, sprint, column, unknown, fact, filesWait, automaticFact, ownerDecision, ownerIntegration, at: task.doneAt ?? (seen ? Date.parse(seen) : task.lastConfirmedAt ?? null), target, workers: own, running };
  });

  // A sprint's planning task reads only on its sprint: no list, count, badge or search below sees it.
  const hidden = planningTaskIds(state);
  const listed = hidden.size ? views.filter(v => !hidden.has(v.id)) : views, planning = hidden.size ? views.filter(v => hidden.has(v.id)) : [];
  const summaries = new Map<string, ProjectSummary>(state.projects.map(project => {
    const machine = machines.get(project.machineId)!, offline = machine.connection !== "connected";
    const own = listed.filter(v => v.project.id === project.id), active = own.filter(v => v.column !== "archive");
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
  return { now, state, views: listed, planning, byId: new Map(views.map(v => [v.id, v])), summaries, archive: listed.filter(v => v.column === "archive"), orders };
}

/**
 * The index rebuilt for a new state, holding every task view and project summary that equals the last one as the last object.
 * Every screen hangs off the index: a view rebuilt as a new object for an unrelated change would redraw each row built from
 * it and drop whatever the owner had opened there. Only what really changed is new.
 */
export function stableIndex(prev: WorkspaceIndex | undefined, next: WorkspaceIndex): WorkspaceIndex {
  if (!prev) return next;
  const views = share(prev.views, next.views), planning = share(prev.planning ?? [], next.planning);
  const byId = new Map([...views, ...planning].map(v => [v.id, v]));
  const own = (list: TaskView[]) => list.map(v => byId.get(v.id) ?? v);
  const summaries = new Map([...next.summaries].map(([id, s]) => [id, share(prev.summaries.get(id) as ProjectSummary, { ...s, views: own(s.views) })] as const));
  return { ...next, views, planning, byId, summaries, archive: share(prev.archive, own(next.archive)) };
}

/** Where a card's primary action lands: the screen that decision is made on, not just the task. */
export const decisionRoute = (view: TaskView) => {
  const base = { name: "task" as const, machine: view.machine.id, task: view.id };
  if (view.task.conflict === "conflict" || view.task.conflict === "failed") return { ...base, tab: "changes" as const, sub: "conflict" as const };
  if (automaticAcceptanceHold(view)) return base;
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
  if (automaticQueueOf(view.sprint)?.taskId === view.id && view.sprint?.automaticQueue?.needsOwner) return "Needs you";
  if (view.automaticFact) {
    if (view.task.status === "review") return "Checking";
    if (view.task.status === "ready_to_merge") return "Finishing";
    return STATUS_WORD[view.task.status];
  }
  if (view.task.status === "paused") return "Paused";
  if (view.column === "needs") return "Needs you";
  // A finished task is listed only while its finish is new to the owner.
  if (view.column === "done") return "Done";
  if (view.column === "closed_unverified") return "Closed";
  // A task in recovery is not working: a replan waiting for a cancelled worker says so, any other recovery is unconfirmed.
  if (view.column === "work") return view.task.status === "stopping" ? "Stopping" : replanStopping(view.task) ? "Changing plan"
    : view.unknown && view.task.warnings?.some(w => w.startsWith("process_exit_")) ? "Not confirmed" : "Working";
  if (view.column === "review") return "Review";
  return view.project.hasGit === false || sameBranch(view.task, view.target) ? "Finish" : "Merge";
}

export type Decision = { key: string; kind: string; icon: string; tone: string; ask: string; where: string; view: TaskView | null; project: Project; sprintId: string | null; at: number | null };
/** Inbox covers every machine, so a remote one names itself. */
const placed = (machine: Machine, text: string) => machine.kind === "local" ? text : `${machine.name} / ${text}`;
export function decisionOf(view: TaskView): Decision {
  const { task } = view, issue = task.issue ?? "";
  const base = { key: `task:${view.id}`, where: placed(view.machine, `${view.project.name} / ${task.title}`), view, project: view.project, sprintId: null, at: view.at };
  if (view.ownerDecision) return { ...base, kind: view.ownerDecision.kind === "permission" ? "Permission" : "Question",
    icon: view.fact.icon ?? "question", tone: "st-warn", ask: view.ownerDecision.text };
  if (automaticAcceptanceHold(view)) return { ...base, kind: "Verification", icon: "alert", tone: "st-warn", ask: view.fact.text };
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
  if (view.ownerIntegration) return { ...base, kind: "Finish", icon: "merge", tone: "st-warn", ask: humanStatus(view.ownerIntegration.reason) };
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
  // A current file wait takes precedence over the historical automatic continuation spinner on the card.
  if (view.column === "work" && view.filesWait && view.automaticFact?.spin)
    return { text: view.filesWait.text, tone: "st-warn", icon: "hourglass" };
  const notice = !!view.task.warnings?.length || (!!view.task.observedPhase && view.task.observedPhase !== "not_started");
  if (view.unknown || notice || view.automaticFact || view.ownerDecision || view.ownerIntegration) return view.fact;
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
    const summary = index.summaries.get(sprint.projectId);
    if (!summary || !connected(summary.machine)) return [];
    const row = { key: `sprint:${sprint.id}`, where: placed(summary.machine, `${summary.project.name} / ${sprintTitle(sprint)}`), view: null, project: summary.project, sprintId: sprint.id };
    const failure = planningFailureDetails(sprint);
    if (failure && failure.kind !== "owner_stopped") return [{ ...row, kind: "Planning failed", icon: "alert", tone: "st-bad", ask: failure.text, at: failure.at }];
    if (!planningNeedsOwner(index.state, sprint)) {
      const warning = activePlanningWarning(sprint);
      return warning ? [{ ...row, kind: "Planning problem", icon: "alert", tone: "st-warn", ask: warning.text, at: warning.at }] : [];
    }
    // The planning task's own questions and permissions are the sprint's one row: the task has none of its own.
    const waits = planningWaits(index.state, sprint);
    const open = waits.length ? waits : index.state.sessions.find(s => s.taskId === sprint.planningTaskId)?.decisions.filter(d => d.kind === "question" && isOpen(d)) ?? [];
    const first = open[0]?.text ?? sprint.conversation.filter(m => m.kind === "question").at(-1)?.text ?? "Planning question";
    const ask = open.length > 1 ? `${open.length} ${open.every(d => d.kind === "question") ? "questions" : "decisions"}: ${first}` : first;
    return [{ ...row, kind: "Planning", icon: "layers", tone: "st-warn", ask, at: open[0]?.at ?? null }];
  });
  return [...of("needs"), ...planning, ...of("review"), ...of("merge")];
}

export const inScope = (summary: ProjectSummary, scope: Scope) =>
  summary.views.filter(v => scope.kind === "all" || (scope.kind === "standalone" ? !v.sprint : v.sprint?.id === scope.id));
/**
 * Each column, and with `index` each sprint's own tasks in it in execution order: Working in confirmed start order, Not
 * started by the service's rank, the unranked after them. A sprint's tasks keep the places they held among other work, so
 * standalone tasks and other sprints never move, and a sprint with no current order keeps the order it arrived in.
 */
export function columnsOf(views: TaskView[], index?: Pick<WorkspaceIndex, "orders">) {
  const out = Object.fromEntries(COLUMNS.map(c => [c.id, [] as TaskView[]])) as Record<ColumnId, TaskView[]>;
  for (const view of views) if (view.column !== "archive") out[view.column].push(view);
  if (index?.orders?.size) for (const id of Object.keys(out) as ColumnId[]) out[id] = inExecutionOrder(out[id], index);
  return out;
}
/** `views` with each sprint's members sorted among the places they hold (see `columnsOf`). Ties keep their order. */
export function inExecutionOrder(views: TaskView[], index: Pick<WorkspaceIndex, "orders">): TaskView[] {
  const groups = new Map<string, { at: number[]; rank: Map<string, number> }>();
  views.forEach((v, i) => {
    const order = orderOf(index, v.sprint);
    if (!order || !v.sprint) return;
    let g = groups.get(v.sprint.id);
    if (!g) groups.set(v.sprint.id, g = { at: [], rank: new Map([...order.working.map(w => w.taskId), ...order.notStarted.map(p => p.taskId)].map((id, n) => [id, n])) });
    g.at.push(i);
  });
  if (!groups.size) return views;
  const out = [...views];
  for (const g of groups.values()) {
    const members = g.at.map(i => views[i]), rank = (v: TaskView) => g.rank.get(v.id) ?? Infinity;
    const sorted = members.map((v, n) => ({ v, n })).sort((a, b) => rank(a.v) - rank(b.v) || a.n - b.n).map(x => x.v);
    g.at.forEach((i, n) => { out[i] = sorted[n]; });
  }
  return out.every((v, i) => v === views[i]) ? views : out;
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
  const ready = (v: TaskView) => v.task.dependencies.every(id => { const d = index.byId.get(id); return !!d && dependencyReady(d.task, sprint.runMode); });
  const started = views.length - drafts.length;
  const planning = { planning: "Planning, no tasks yet", question: "Planning, question open", ready: "Plan ready", cancelled: "Planning cancelled", created: "" }[sprint.planning];
  const queue = { idle: "queue idle", running: "queue running", paused: "queue paused", complete: "queue complete" }[sprint.queueState];
  // The service's order says what runs now (first in start order) and what is next; Manual has no Next, only a suggestion.
  const order = orderOf(index, sprint), byId = (id: string) => views.find(v => v.id === id);
  const now = order ? order.working.map(w => byId(w.taskId)).find(v => v?.task.status === "working") ?? null : views.find(v => v.task.status === "working") ?? null;
  // Next is only the service's own candidate. A Manual sprint has a suggestion instead: the first task in the service's ranked
  // order that can start now, or before that order is current the first ready draft, so the Start control never depends on the
  // queue's Next. The ranking simulates earlier work settling, so a ranked task whose prerequisite still runs is passed over.
  const next = order ? byId(order.notStarted.find(p => p.next)?.taskId ?? "") ?? null : null;
  const startable = (v: TaskView | undefined): v is TaskView => !!v && v.task.status === "draft" && ready(v);
  const suggested = sprint.runMode !== "manual" ? null
    : order ? order.notStarted.filter(p => p.rank !== null).map(p => byId(p.taskId)).find(startable) ?? null : drafts.find(ready) ?? null;
  return {
    sprint, title: sprintTitle(sprint), views, started, missing: sprint.taskIds.length - views.length, usable: sprint.planning === "created",
    now, next, suggested, ordered: !!order, blocked: drafts.filter(v => !ready(v)).length,
    state: sprint.deletion ? deletionState(index, sprint) : planning || `${sprintModeLabel[sprint.runMode]}, ${queue}, ${started} of ${sprint.taskIds.length} started${views.length < sprint.taskIds.length && index.state.projects.find(p => p.id === sprint.projectId)?.issue ? ", task details unavailable" : ""}`,
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
/** Where an address of a sprint's planning task opens: its sprint's Workers tab for a worker, its Conversation otherwise. */
export function planningPlace(machine: string, sprint: Sprint, route: { tab?: string; worker?: string }) {
  return route.tab === "workers"
    ? { name: "sprint" as const, machine, project: sprint.projectId, sprint: sprint.id, tab: "workers" as const, ...(route.worker ? { worker: route.worker } : {}) }
    : { name: "sprint" as const, machine, project: sprint.projectId, sprint: sprint.id, tab: "planning" as const };
}
type SideTask = { kind: "task"; view: TaskView; nested: boolean };
type SideSprint = { kind: "sprint"; sprint: Sprint; started: number; total: number };
export type SideRow = SideTask | SideSprint;
const sideColumns = COLUMNS.filter(c => c.id === "work" || c.id === "review" || c.id === "merge");
const sideTask = (view: TaskView, nested = false): SideTask => ({ kind: "task", view, nested });
const sideSprint = (index: WorkspaceIndex, sprint: Sprint): SideSprint =>
  ({ kind: "sprint", sprint, started: sprintView(index, sprint).started, total: sprint.taskIds.length });

function sprintChildren(views: TaskView[], sprintIds: ReadonlySet<string>) {
  const children = new Map<string, SideTask[]>();
  for (const view of views) {
    const id = view.sprint?.id;
    if (!id || !sprintIds.has(id)) continue;
    const row = sideTask(view, true), siblings = children.get(id);
    if (siblings) siblings.push(row);
    else children.set(id, [row]);
  }
  return children;
}

/** Finished tasks and sprints the sidebar keeps listed although they no longer ask anything: a finish the owner has not opened yet. */
export type SideKeep = { task: (view: TaskView) => boolean; sprint: (sprint: Sprint) => boolean };
const finishedColumn = (view: TaskView) => view.column === "done" || view.column === "closed_unverified";

/** At most five visible rows, counting sprint parents and their children alike. Completed sprints
 *  remain until archived. Tasks with no visible sprint group keep the normal column ordering.
 *  A finished task that `keep` holds follows the tasks that need the owner. */
export function sideRows(index: WorkspaceIndex, summary: ProjectSummary, keep?: SideKeep): SideRow[] {
  const open = summary.sprints;
  const sprints = [...open.filter(s => s.queueState !== "complete"), ...open.filter(s => s.queueState === "complete")];
  const ids = new Set(sprints.map(s => s.id));
  const hasParent = (view: TaskView) => !!view.sprint && ids.has(view.sprint.id);
  const needs = summary.views.filter(v => v.column === "needs");
  const work = sideColumns.flatMap(c => summary.views.filter(v => v.column === c.id));
  const finished = keep ? summary.views.filter(v => finishedColumn(v) && keep.task(v)) : [];
  const children = sprintChildren([...needs, ...work, ...finished], ids);
  return [
    ...needs.filter(v => !hasParent(v)).map(v => sideTask(v)),
    ...finished.filter(v => !hasParent(v)).map(v => sideTask(v)),
    ...sprints.flatMap(s => [sideSprint(index, s), ...(children.get(s.id) ?? [])]),
    ...work.filter(v => !hasParent(v)).map(v => sideTask(v)),
  ].slice(0, 5);
}

export type TopWorkGroup = { id: string; label: string; rows: (SideRow & { projectName: string })[] };

/** Uncapped machine-wide status groups. Running and paused sprints have one tree each;
 *  their children are not repeated in the standalone status groups. With `keep`, a Finished group
 *  holds the finished tasks and sprints it keeps, each sprint with its kept tasks under it. */
export function topWorkRows(index: WorkspaceIndex, machine: string, keep?: SideKeep): TopWorkGroup[] {
  const projects = projectsOn(index, machine);
  const sprints = projects.flatMap(({ project, sprints }) => sprints
    .filter(s => s.queueState === "running" || s.queueState === "paused")
    .map(sprint => ({ sprint, projectName: project.name })));
  const ids = new Set(sprints.map(({ sprint }) => sprint.id));
  const views = projects.flatMap(p => p.views);
  const groups = sideColumns.map(c => ({ ...c, views: views.filter(v => v.column === c.id) }));
  const finished = keep ? views.filter(v => finishedColumn(v) && keep.task(v)) : [];
  const done = keep ? projects.flatMap(({ project, sprints }) => sprints.filter(s => !ids.has(s.id) && keep.sprint(s)).map(sprint => ({ sprint, projectName: project.name }))) : [];
  // Every sprint with a tree here, in progress or finished, holds its tasks: none of them is listed again on its own.
  const parents = new Set([...ids, ...done.map(({ sprint }) => sprint.id)]);
  const children = sprintChildren([...groups.flatMap(c => c.views), ...finished], parents);
  const result: TopWorkGroup[] = groups.map(({ id, label, views }) => ({
    id, label, rows: views.filter(v => !v.sprint || !parents.has(v.sprint.id)).map(v => ({ ...sideTask(v), projectName: v.project.name })),
  }));
  const tree = (list: { sprint: Sprint; projectName: string }[]) => list.flatMap(({ sprint, projectName }) => [
    { ...sideSprint(index, sprint), projectName },
    ...(children.get(sprint.id) ?? []).map(row => ({ ...row, projectName })),
  ]);
  result.push({ id: "sprints", label: "Sprints in progress", rows: tree(sprints) });
  result.push({ id: "finished", label: "Finished", rows: [
    ...finished.filter(v => !v.sprint || !parents.has(v.sprint.id)).map(v => ({ ...sideTask(v), projectName: v.project.name })),
    ...tree(done),
  ] });
  return result.filter(group => group.rows.length > 0);
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
