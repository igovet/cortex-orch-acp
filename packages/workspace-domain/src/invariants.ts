import type { AutomaticContinuation, AutomaticQueue, Check, HoldActor, LiveSprintGraph, LiveSprintMember, OwnerHoldAction, PlanItem, ProjectRepo, RepoCheckout, Sprint, SprintEntry, SprintOrderPending, SprintOrderWorking, SprintRevision, SprintRevisionChange, SprintRunMode, Task, TaskSession, WorkspaceState } from "./types";
import { DomainError, entity, machineFor, nonempty, projectFor, requireFact } from "./core";

/**
 * Pure presentation invariants: what a screen may offer and how it words a blocker. They read a projection and change nothing.
 * The machine decides. A rule here never authorises anything, it only keeps a control honest about what it can do.
 */
/** Editing the saved request is distinct from messaging or changing a live session. */
export const taskEditable = (task: Task) => task.executionId === null && task.status === "draft" && task.process === "stopped" && !task.archive && !task.closedAs;
/** Close preflight for a sprint's members on the read side. The service rechecks the current records and running effects. */
export function sprintCloseBlockers(state: WorkspaceState, sprint: Sprint) {
  const tasks = new Map(state.tasks.filter(task => task.projectId === sprint.projectId).map(task => [task.id, task]));
  const ids = new Set([...sprint.taskIds, ...[...tasks.values()].filter(task => task.sprintId === sprint.id).map(task => task.id)]);
  return [...ids].flatMap(id => {
    const task = tasks.get(id);
    const reason = !task ? "task details not loaded; refresh the sprint"
      : task.sprintId && task.sprintId !== sprint.id ? "membership changed; refresh the sprint"
      : task.closedAs === "unconfirmed" || task.warnings?.some(w => w === "contradictory_evidence" || w === "owner_closed_unconfirmed") ? "task records disagree"
      : task.reworkEvent && task.status !== "done" ? "returned for rework and not closed"
      : task.status !== "done" && (task.status !== "draft" || task.executionId !== null) ? "started but not done" : null;
    return reason ? [{ id, title: task?.title ?? id, reason }] : [];
  });
}
/** The title is presentation, not lifecycle: any task still kept can be renamed, started, finished or closed. Only a cleared task cannot. */
export const taskRenamable = (task: Task) => !task.archive;
export const repoOf = (state: WorkspaceState, projectId: string) => state.repos.find(r => r.projectId === projectId) ?? null;
export function checkoutOf(state: WorkspaceState, id: string): { repo: ProjectRepo; checkout: RepoCheckout } {
  for (const repo of state.repos) {
    const checkout = repo.checkouts.find(c => c.id === id);
    if (checkout) return { repo, checkout };
  }
  throw new DomainError("Unknown checkout: " + id);
}
/** The record of the checkout a task works in, when there is one. */
export const taskCheckout = (state: WorkspaceState, task: Task) => {
  const binding = task.checkout;
  const checkouts = repoOf(state, task.projectId)?.checkouts;
  if (binding.kind === "main") return checkouts?.find(c => c.kind === "main") ?? null;
  if (binding.unavailable) return null;
  return checkouts?.find(c => c.kind === "worktree" && (binding.id ? c.id === binding.id : c.taskId === task.id)) ?? null;
};
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
/** A sprint-wide owner choice wins over plan hints. Older sprints keep their per-item choices. */
export const sprintTaskCheckout = (sprint: Pick<Sprint, "checkout">, item?: Pick<PlanItem, "checkout">): NonNullable<PlanItem["checkout"]> =>
  sprint.checkout ?? item?.checkout ?? { kind: "main" };

export function validatePlan(items: PlanItem[]) {
  requireFact(items.length > 0, "Plan must contain tasks");
  const byKey = new Map(items.map(item => [nonempty(item.key, "Plan key"), item]));
  requireFact(byKey.size === items.length, "Duplicate plan key");
  items.forEach(item => {
    nonempty(item.description, "Description");
    if (item.title !== undefined) nonempty(item.title, "Task title");
    if (item.complexity !== undefined) requireFact(["low", "normal", "high"].includes(item.complexity), "Complexity must be low, normal or high: " + item.key);
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
export const SPRINT_RUN_MODES = ["manual", "hybrid", "automatic"] as const satisfies readonly SprintRunMode[];
export const isSprintRunMode = (mode: unknown): mode is SprintRunMode => SPRINT_RUN_MODES.some(value => value === mode);
export const queueDrivenMode = (mode: SprintRunMode): boolean => mode === "hybrid" || mode === "automatic";

const record = (value: unknown): value is Record<string, unknown> =>
  value !== null && typeof value === "object" && !Array.isArray(value);
const recordedText = (value: unknown): value is string => typeof value === "string" && value.length <= 32000;
export function isAutomaticContinuation(value: unknown): value is AutomaticContinuation {
  if (!record(value) || value.policy !== "automatic_continuation" || typeof value.continuationCount !== "number"
    || !Number.isSafeInteger(value.continuationCount) || value.continuationCount < 0
    || !(value.lastContinuationReason === null || recordedText(value.lastContinuationReason))) return false;
  const pause = value.pause;
  return pause === null || (record(pause) && recordedText(pause.eventId) && recordedText(pause.reason)
    && (pause.kind === "external_blocker" || pause.kind === "continuation_cap" || pause.kind === "outcome_unknown" || pause.kind === "owner_blocker"
      || pause.kind === "obstacle" || pause.kind === "infrastructure"));
}
const HOLD_ACTORS: readonly HoldActor[] = ["runtime", "agent", "owner"];
export const OWNER_HOLD_ACTIONS: readonly OwnerHoldAction[] = ["allow_finish", "accept_result", "allow_repair", "allow_retries", "confirm_and_continue", "resume_task", "retry",
  "confirm_integration", "resolve_conflict", "change_run_mode", "ask_agent", "close_as_is"];
/** A hold's decision, when the service recorded one: one actor; an owner decision always offers an action and says what it asks. */
function holdDecision(value: Record<string, unknown>) {
  if (value.nextActor === undefined && value.ownerActions === undefined && value.asks === undefined) return true;
  const actions = value.ownerActions, asks = value.asks;
  return HOLD_ACTORS.includes(value.nextActor as HoldActor) && Array.isArray(actions) && Array.isArray(asks)
    && actions.every(a => OWNER_HOLD_ACTIONS.includes(a as OwnerHoldAction)) && asks.every(recordedText)
    && (value.nextActor === "owner") === (value.needsOwner === true)
    && (value.nextActor !== "owner" || (actions.length > 0 && asks.length > 0))
    && (value.allowedAt === undefined || typeof value.allowedAt === "number");
}
export function isAutomaticQueue(value: unknown): value is AutomaticQueue {
  return record(value) && recordedText(value.code) && recordedText(value.reason)
    && (value.taskId === null || recordedText(value.taskId))
    && (value.kind === "owner_integration" ? value.needsOwner === true && !!value.taskId
      : value.kind === "continuation" ? value.needsOwner === false
        || (value.needsOwner === true && !!value.taskId && ["continuation_cap", "outcome_unknown", "owner_blocker"].includes(value.code))
      : (value.kind === "start_refused" || value.kind === "settlement_blocked") && typeof value.needsOwner === "boolean")
    && holdDecision(value);
}
/** The hold of a queue-driven sprint (Hybrid or Automatic). Both modes keep system holds in the same record. */
export const automaticQueueOf = (sprint: Sprint | null | undefined) =>
  (sprint?.runMode === "automatic" || sprint?.runMode === "hybrid") && isAutomaticQueue(sprint.automaticQueue) ? sprint.automaticQueue : null;

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
  if (!queueDrivenMode(sprint.runMode)) return { reason: "Select Hybrid or Automatic to run the queue", taskId: null };
  const decided = automaticQueueOf(sprint);
  // A hold decided by the service: Continue queue is offered exactly when it offers Try again.
  if (decided?.nextActor) return decided.ownerActions?.includes("retry") || decided.nextActor !== "owner" && decided.taskId === null
    ? null : { reason: decided.reason, taskId: decided.taskId };
  if (sprint.runMode === "automatic") {
    const hold = decided;
    if (!hold || hold.kind === "start_refused") return null;
    // With no task link, Continue rechecks the queue. For other recoverable
    // finish refusals it retries through the same durable integration guard.
    if (hold.kind === "settlement_blocked" && hold.needsOwner) {
      if (hold.code === "dependencies_blocked" || (hold.code === "queue_blocked" && !hold.taskId)) return null;
      const taskOnly = ["queue_blocked", "automatic_acceptance_blocked", "outcome_unknown", "integration_outcome_unknown", "integration_failed"];
      if (hold.taskId && !taskOnly.includes(hold.code)) return null;
    }
    return { reason: hold.reason, taskId: hold.taskId };
  }
  for (const id of sprint.taskIds) {
    const task = entity(state.tasks, id);
    if (task.status === "done" && (task.closedAs === "external" || task.closedAs === "as_is")) continue;
    // The service skips a cancelled or archived member, as Automatic does; only its dependents wait, and the service
    // holds them with a decision that names the task. Continue queue must not be refused for the skipped member itself.
    if (task.closedAs === "cancel" || task.closedAs === "unconfirmed" || task.archive) continue;
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
export function dependencyReady(task: Task, mode?: SprintRunMode) {
  if (task.closedAs === "cancel" || task.closedAs === "unconfirmed") return false;
  if (mode === "automatic") return task.status === "done";
  return (task.status === "done" && (task.closedAs === "external" || task.closedAs === "as_is")) || task.work === "merged" || (task.status === "ready_to_merge" && task.result !== null &&
    (task.acceptance?.digest === task.workspaceDigest || (task.result.automaticReady === true && checksPassed(task.result.checks))) && task.result.digest === task.workspaceDigest && task.conflict === "none");
}
export const emptySession = (taskId: string, createdAt: number | null = null): TaskSession => ({
  taskId, createdAt, base: null, plan: null, timeline: [], decisions: [], context: null, tokens: null, planUsage: null,
  coordinator: { note: "", elapsedMs: null, usage: null }, verification: [], changes: null,
});
export const isOpen = (d: { answer: unknown; withdrawn: boolean }) => !d.answer && !d.withdrawn;

const LIVE_ID = /^[A-Za-z0-9_:.-]{1,128}$/;
const liveIds = (v: unknown, max: number): v is string[] => Array.isArray(v) && v.length <= max && v.every(x => typeof x === "string" && LIVE_ID.test(x));
/**
 * A live sprint graph exactly as contract version 1 shapes it, or null. Any other shape (an older service, a partial reply)
 * is not a graph this app can edit: the caller shows the sprint read-only and never fills a gap with a guess.
 */
export function parseLiveSprintGraph(value: unknown): LiveSprintGraph | null {
  if (!record(value) || value.contractVersion !== 1 || typeof value.sprintId !== "string" || !LIVE_ID.test(value.sprintId)
    || typeof value.graphRevision !== "string" || !/^[0-9a-f]{64}$/.test(value.graphRevision) || typeof value.complete !== "boolean"
    || !Array.isArray(value.members) || value.members.length > 1000 || !liveIds(value.priority, 1000) || !record(value.order)) return null;
  const members: LiveSprintMember[] = [];
  for (const m of value.members) {
    if (!record(m) || typeof m.taskId !== "string" || !LIVE_ID.test(m.taskId) || !["confirmed", "missing", "unreadable", "purged"].includes(m.readStatus as string)
      || !(m.dependencies === null || liveIds(m.dependencies, 10_000)) || typeof m.editable !== "boolean" || typeof m.canBePredecessor !== "boolean"
      || !(m.approvedPlanKey === null || m.approvedPlanKey === undefined || typeof m.approvedPlanKey === "string")) return null;
    members.push({ taskId: m.taskId, readStatus: m.readStatus as LiveSprintMember["readStatus"], dependencies: m.dependencies === null ? null : [...m.dependencies as string[]],
      editable: m.editable, canBePredecessor: m.canBePredecessor, approvedPlanKey: typeof m.approvedPlanKey === "string" ? m.approvedPlanKey : null });
  }
  if (new Set(members.map(m => m.taskId)).size !== members.length) return null;
  const o = value.order;
  if (!isSprintRunMode(o.mode) || !["idle", "running", "paused", "complete"].includes(o.queueState as string) || !Array.isArray(o.working) || !Array.isArray(o.notStarted)) return null;
  const working: SprintOrderWorking[] = [], notStarted: SprintOrderPending[] = [];
  for (const w of o.working) {
    if (!record(w) || typeof w.taskId !== "string" || !LIVE_ID.test(w.taskId) || !(w.startOrder === null || (typeof w.startOrder === "string" && /^(0|[1-9][0-9]*)$/.test(w.startOrder)))) return null;
    working.push({ taskId: w.taskId, startOrder: w.startOrder });
  }
  for (const p of o.notStarted) {
    if (!record(p) || typeof p.taskId !== "string" || !LIVE_ID.test(p.taskId) || !(p.rank === null || (typeof p.rank === "number" && Number.isSafeInteger(p.rank) && p.rank >= 1))
      || !(p.reason === null || (typeof p.reason === "string" && p.reason.length <= 64)) || typeof p.next !== "boolean" || typeof p.conditional !== "boolean") return null;
    notStarted.push({ taskId: p.taskId, rank: p.rank, reason: p.reason, next: p.next, conditional: p.conditional });
  }
  // At most one current candidate, and only a ranked one.
  if (notStarted.filter(p => p.next).length > 1 || notStarted.some(p => p.next && p.rank === null)) return null;
  return { contractVersion: 1, sprintId: value.sprintId, graphRevision: value.graphRevision, complete: value.complete, members, priority: [...value.priority],
    order: { mode: o.mode, queueState: o.queueState as LiveSprintGraph["order"]["queueState"], working, notStarted },
    ...(Array.isArray(value.revisions) ? { revisions: parseRevisions(value.revisions) } : {}) };
}
const REVISION_STATUS = ["review", "changes_requested", "approved", "awaiting_owner", "applied", "rejected", "apply_refused", "superseded"];
const text = (v: unknown, max: number): string | null => typeof v === "string" ? v.slice(0, max) : null;
const ids = (v: unknown): string[] => Array.isArray(v) ? v.filter((x): x is string => typeof x === "string" && LIVE_ID.test(x)) : [];
/** Revisions the service named; a malformed one is left out rather than guessed. */
function parseRevisions(rows: unknown[]): SprintRevision[] {
  const out: SprintRevision[] = [];
  for (const r of rows.slice(-20)) {
    if (!record(r) || typeof r.revisionId !== "string" || !REVISION_STATUS.includes(r.status as string)) continue;
    const changes: SprintRevisionChange[] = (Array.isArray(r.changes) ? r.changes : []).filter(record).filter(c => ["update", "add", "propose_cancel"].includes(c.op as string)).map(c => ({
      op: c.op as SprintRevisionChange["op"], ...(typeof c.corrects === "string" && c.corrects ? { corrects: c.corrects } : {}), memberId: text(c.member_id, 128), key: text(c.key, 100), title: text(c.title, 200),
      description: text(c.description, 8000), dependsOn: Array.isArray(c.dependsOn) ? c.dependsOn.filter((d): d is string => typeof d === "string") : null, reason: text(c.reason, 2000),
    }));
    out.push({ revisionId: r.revisionId, taskId: typeof r.taskId === "string" ? r.taskId : null, status: r.status as SprintRevision["status"], applied: r.applied === true,
      reason: text(r.reason, 2000) ?? "", ...(Array.isArray(r.startedAffected) ? { startedAffected: ids(r.startedAffected) } : {}), changes, held: ids(r.held), updated: ids(r.updated), added: ids(r.added) });
  }
  return out;
}
/** Output readiness is a separate service decision from completion of the producer task. */
export function deliverableWaitText(reason: string | null | undefined): string | null {
  if (!reason) return null;
  if (reason === "invalid_deliverable_contract") return "The required output contract is invalid";
  if (!reason.startsWith("deliverable_")) return null;
  return ({
    deliverable_missing: "Waiting for the required output",
    deliverable_not_declared: "The required output was not declared",
    deliverable_unreviewed: "The required output is awaiting review",
    deliverable_unverified: "The required output is awaiting verification",
    deliverable_producer_unsettled: "Waiting for the task producing the required output",
    deliverable_ambiguous: "The required output has more than one match",
    deliverable_stale_evidence: "The evidence for the required output is out of date",
    deliverable_invalid_evidence: "The evidence for the required output is invalid",
  } as Record<string, string>)[reason] ?? "The required output could not be confirmed";
}

/** Why a not-started member has no place in the order, in words. Unknown codes stay generic rather than guessed. */
export function orderReasonText(reason: string | null): string | null {
  if (reason === null) return null;
  return deliverableWaitText(reason) ?? ({
    task_missing: "Task details not loaded", task_state_unknown: "Task state unknown", sprint_closed: "Sprint closed",
    queue_paused: "Queue paused", queue_held: "Queue held", queue_complete: "Queue complete", queue_blocked: "Queue blocked",
    dependencies_blocked: "Waits for a prerequisite", native_worktree_unavailable: "Worktree unavailable",
    sprint_revision_pending: "Waits for a sprint revision",
  } as Record<string, string>)[reason] ?? "Not in the order";
}
