import { readableCheckReason, ACCESS_MODES, effortLabel, emptySession, engineLabel, isOpen, LANGUAGES, machineDefaults } from "@cortex/workspace-domain";
import type { Check, Decision, EngineKey, Machine, Message, SessionOptions, TaskSession, TaskSettings, Worker, WorkerRoute, WorkerStatus } from "@cortex/workspace-domain";
import { automaticAcceptanceHold, automaticStaleResultHold, staleResult, clock, interruptedTurn, ownerHold, plainChoice, readyLabel, readyStep, replanStopping, sameBranch, type TaskView, type WorkspaceIndex } from "./selectors";
import { inTask } from "./status-text";

/** Read models for the task session. Every count on the screen comes from these lists; nothing here changes state. */
export type Face = { label: string; tone: string; icon: string; spin?: boolean };
const FACES: Record<WorkerStatus, Face> = {
  working: { label: "Working", tone: "st-run", icon: "loader", spin: true },
  waiting_you: { label: "Waiting for you", tone: "st-warn", icon: "question" },
  pending: { label: "Waiting", tone: "st-idle", icon: "hourglass" },
  not_started: { label: "Not started", tone: "st-idle", icon: "circle" },
  paused: { label: "Stopped", tone: "st-idle", icon: "pause" },
  unknown: { label: "Not confirmed", tone: "st-warn", icon: "help" },
  completed: { label: "Finished", tone: "st-ok", icon: "check-circle" },
  failed: { label: "Failed", tone: "st-bad", icon: "x-circle" },
  cancelled: { label: "Cancelled", tone: "st-idle", icon: "x-circle" },
};
export const workerFace = (status: WorkerStatus) => FACES[status];
/** A failed worker that named its dead end reads as that dead end: it found problems another role has to fix, or it could
 *  not run its work on this machine. Neither is a crash. */
const FAILED_AS: Record<string, Face> = {
  other_role_work: { label: "Found problems", tone: "st-warn", icon: "alert" },
  tool_unavailable: { label: "Could not run here", tone: "st-warn", icon: "alert" },
};
/** A finished worker whose check failed never reads as a clean finish. A cancelled worker whose session is still being
 *  stopped reads as stopping; an attempt a newer plan replaced says so rather than reading as a current failure. */
export function workerState(w: Worker): Face {
  if (w.status === "cancelled" && w.stopping) return { label: "Stopping…", tone: "st-warn", icon: "loader", spin: true };
  if (w.status === "failed" && w.replacedIn !== undefined) return { label: `Replaced in plan v${w.replacedIn}`, tone: "st-idle", icon: "refresh" };
  if (w.status === "failed" && w.blockerKind && Object.hasOwn(FAILED_AS, w.blockerKind)) return FAILED_AS[w.blockerKind];
  const failed = w.status === "completed" ? w.result?.checks.filter(c => c.outcome === "failed").length ?? 0 : 0;
  return failed ? { label: `${failed} check${failed === 1 ? "" : "s"} failed`, tone: "st-bad", icon: "x-circle" } : workerFace(w.status);
}
/**
 * The finished workers as the plan ran them: each step's newest attempt, with its earlier attempts (retries of the same
 * step) under it. A worker without a recorded step is its own group. The order of the list is kept.
 */
export function attemptsByNode(workers: Worker[]): { latest: Worker; earlier: Worker[] }[] {
  const groups: { key: string; latest: Worker; earlier: Worker[] }[] = [];
  for (const w of workers) {
    const key = w.node ? `node:${w.node}` : `worker:${w.id}`, group = groups.find(g => g.key === key);
    if (group) group.earlier.push(w); else groups.push({ key, latest: w, earlier: [] });
  }
  return groups.map(({ latest, earlier }) => ({ latest, earlier }));
}
const ACTIVE: WorkerStatus[] = ["working", "waiting_you", "paused", "unknown"];
const FINISHED: WorkerStatus[] = ["completed", "failed", "cancelled"];
export const isFinished = (w: Worker) => FINISHED.includes(w.status);
/** How many workers are at work, finished with a result, cancelled and failed. A cancelled or failed worker never counts as finished. */
export function workerCounts(workers: Worker[]) {
  const count = (status: WorkerStatus) => workers.filter(w => w.status === status).length;
  return { active: workers.filter(w => ACTIVE.includes(w.status)).length, done: count("completed"), cancelled: count("cancelled"), failed: count("failed") };
}
/**
 * Where an owner message to the coordinator is, from the runtime's facts: taken from the queue by the service, recorded
 * and waiting for the coordinator's next step, or delivered (the engine answered the prompt that carried it).
 */
export function messageDelivery(m: Pick<Message, "delivery" | "deliveredAt" | "failure">, paused: boolean): string {
  switch (m.delivery) {
    case "sending": return "Taken from the queue for delivery";
    case "delivered": return m.deliveredAt !== null ? `Delivered to the coordinator ${clock(m.deliveredAt)}` : "Delivered to the coordinator";
    case "failed": return m.failure ?? "Not delivered";
    default: return paused ? "Received, kept for when you resume" : "Received, waiting for the coordinator's next step";
  }
}
/** The one outcome line for a settled set of workers, shown in the task header and the right-hand panel. */
export function workersOutcome(workers: Worker[], active: number): string {
  const c = workerCounts(workers);
  return `${active} active, ${c.done} finished${c.cancelled ? `, ${c.cancelled} cancelled` : ""}${c.failed ? `, ${c.failed} failed` : ""}`;
}

export type ComposerMode = "live" | "paused" | "stopping" | "draft" | "review" | "closed" | "ended" | "archived" | "offline" | "unconfirmed" | "automatic";
export type SessionModel = {
  view: TaskView; session: TaskSession; workers: Worker[]; active: Worker[]; finished: Worker[]; running: Worker[];
  messages: Message[]; queue: Message[]; open: Decision[]; mode: ComposerMode; planDone: number; planTotal: number;
};
function composerMode(v: TaskView): ComposerMode {
  if (v.column === "archive") return "archived";
  if (v.machine.connection !== "connected") return "offline";
  if (v.unknown && v.task.status !== "stopping") return "unconfirmed";
  if (v.ownerDecision) return "live";
  if (v.automaticFact && (v.task.status !== "working" || v.task.observedPhase === "recovery")) return "automatic";
  switch (v.task.status) {
    case "working": case "needs_you": return "live";
    case "paused": return "paused";
    case "stopping": return "stopping";
    case "draft": return "draft";
    case "done": case "closed_unverified": return "closed";
    // A failed run with known effects takes a message: it reopens the task for the agent (Request changes).
    case "failed": return v.task.reviewActions?.unblockAction === "request_changes" ? "review" : "ended";
    case "cancelled": return "ended";
    default: return "review";
  }
}
export function sessionModel(index: WorkspaceIndex, id: string): SessionModel | null {
  const view = index.byId.get(id);
  if (!view) return null;
  const session = index.state.sessions.find(s => s.taskId === id) ?? emptySession(id);
  const workers = view.workers, messages = index.state.messages.filter(m => m.taskId === id);
  const nodes = session.plan?.nodes ?? [], byId = new Map(workers.map(w => [w.id, w]));
  return {
    view, session, workers, messages, mode: composerMode(view),
    active: workers.filter(w => ACTIVE.includes(w.status) && !w.stale), finished: workers.filter(isFinished), running: workers.filter(w => w.status === "working"),
    queue: messages.filter(m => m.delivery === "queued"), open: view.task.closedAs === "unconfirmed" ? [] : session.decisions.filter(isOpen),
    planDone: nodes.filter(n => n.workerId && byId.get(n.workerId)?.status === "completed").length, planTotal: nodes.length,
  };
}
const last = <T,>(list: T[], test: (item: T) => boolean) => { for (let i = list.length - 1; i >= 0; i--) if (test(list[i])) return list[i]; return null; };
/** When the owner last stopped this task, from the timeline line the stop wrote. */
export const stoppedAt = (m: SessionModel) => {
  const line = last(m.session.timeline, e => e.kind === "line" && e.icon === "pause");
  return line?.kind === "line" ? line.at : null;
};

/** Whether work is going on right now: the task runs or a worker still does. A task that waits for an answer with nothing
 *  running is waiting for the owner, and says so wherever its state shows, never "Working". */
export const workingNow = (m: SessionModel) => m.running.length > 0 || m.view.task.status === "working";
/** The Conversation's live line, which speaks for the coordinator alone: while workers run it waits for them, and a worker's own
 *  step (its command, edit or read) is read on that worker's page, never here. */
export const coordinatorLine = (m: SessionModel) => m.running.length === 0 ? "Working…" : m.running.length === 1 ? `Waiting for ${m.running[0].role}…` : `Waiting for ${m.running.length} workers…`;

/** The review projection is optional for older services; without it the existing worker line remains unchanged. */
export function reviewLine(w: Worker): string | null {
  const r = w.review;
  if (!r) return null;
  const verdict = r.verdict === "APPROVE" ? "Review approved"
    : r.verdict === "REQUEST_CHANGES" ? "Review requested changes"
    : r.verdict === "BLOCK" ? "Review blocked this result"
    : r.verdict === "INCOMPLETE" ? "Review incomplete"
    : null;
  if (r.rerun?.followUpWorkerId === w.id) {
    const item = r.notCovered?.[0]?.item;
    return item ? `Follow-up review: checking ${item}` : "Follow-up review: covering the previous review gap";
  }
  if (r.rerun?.previousWorkerId === w.id) return `${verdict ?? "Review"}. Follow-up review started`;
  return verdict;
}

export type Chip = { icon: string; tone: string; spin?: boolean; text: string; wide?: string; next?: string };
/** The current state and the next step, for the title band. Inside the task, a reason never says to open the task. */
export function stateChip(m: SessionModel): Chip {
  const chip = chipOf(m);
  return { ...chip, text: inTask(chip.text) };
}
function chipOf(m: SessionModel): Chip {
  const { task } = m.view;
  if (task.status === "closed_unverified") return { icon: "alert", tone: "st-idle", text: m.view.fact.text };
  // A replan waiting for a worker it cancelled: the plan is changing, nothing is working and nothing waits for the owner.
  if (m.view.unknown && replanStopping(task)) return { icon: "loader", tone: "st-warn", spin: true, text: "Changing the plan" };
  if (m.view.unknown && task.status !== "stopping") return { icon: "alert", tone: "st-idle", text: m.mode === "unconfirmed" ? "Not confirmed" : m.view.fact.text };
  if (m.view.ownerDecision) return { icon: m.view.fact.icon ?? "question", tone: "st-warn", text: "Needs you" };
  if (m.view.ownerIntegration) return { icon: "merge", tone: "st-warn", text: "Needs you", next: "Confirm integration" };
  if (m.view.automaticFact) return { ...m.view.automaticFact, icon: m.view.automaticFact.icon ?? "hourglass" };
  if (task.observedPhase === "recovery") {
    // A recovery whose stop or process exit is not confirmed does not wait for the owner: it waits for that evidence.
    const run = resumeState(m.view);
    if (run?.kind === "evidence") return { icon: "hourglass", tone: "st-warn", text: run.text };
    return { icon: "pause", tone: "st-warn", text: interruptedTurn(task.issue) ? "Interrupted" : "Stopped", next: "Waiting for you" };
  }
  if (task.conflict === "resolving") return { icon: "loader", tone: "st-run", spin: true, text: "Resolving" };
  if (task.conflict !== "none") return { icon: "alert", tone: "st-bad", text: "Conflict" };
  switch (task.status) {
    case "draft": return { icon: "circle", tone: "st-idle", text: "Not started" };
    case "paused": { const at = stoppedAt(m); return { icon: "pause", tone: "st-idle", text: "Paused", wide: at === null ? " by you" : ` by you, ${clock(at)}` }; }
    case "stopping": return { icon: "loader", tone: "st-warn", spin: true, text: "Stopping" };
    case "review": return { icon: "eye", tone: "st-idle", text: "Review", next: readyStep(task, m.view.target, m.view.project.hasGit) };
    case "ready_to_merge": return { icon: "check", tone: "st-idle", text: readyLabel(task, m.view.target) };
    case "done": return { icon: m.view.fact.icon ?? "check-circle", tone: "st-idle", text: m.view.fact.text };
    case "failed": return { icon: "x-circle", tone: "st-bad", text: "Run failed" };
    case "cancelled": return { icon: "x-circle", tone: "st-idle", text: "Run cancelled" };
    default: return workingNow(m) ? { icon: "loader", tone: "st-run", spin: true, text: "Working", next: m.view.sprint?.runMode === "automatic" ? undefined : "Review" } : { icon: "question", tone: "st-warn", text: "Needs you" };
  }
}

export type Phase = { label: string; state: "done" | "now" | "todo" };
/** Where a runtime phase sits on the bar. A phase with no place, such as a wait, says nothing about how far the task got. */
const PHASE_STEP: Record<string, number> = { not_started: -1, planning: 0, execution: 1, review: 2, verification: 3, finalizing: 4 };
export function phases(m: SessionModel): Phase[] {
  const { status, result, acceptance, closedAs, conflict, work, observedPhase } = m.view.task;
  if (closedAs === "unconfirmed") return [];
  if (closedAs === "external") return [{ label: "Done outside the app", state: "done" }];
  if (result && !closedAs && (conflict !== "none" || ["review", "ready_to_merge", "done"].includes(status))) {
    const finished = m.view.task.integration?.automatic ? "Done" : sameBranch(m.view.task, m.view.target) ? "Finished" : "Merged";
    const held = acceptance ? `${acceptance.automatic ? "Automatically accepted" : "Accepted"} v${acceptance.version}` : `Result v${result.version}`;
    const end: Phase[] = status === "review" ? [{ label: `Result v${result.version}`, state: "now" }]
      : (work === "merged" || (status === "done" && m.view.task.integration?.automatic)) ? [{ label: held, state: "done" }, { label: m.view.task.integration?.method === "commit" && m.view.task.integration.committed !== false ? "Committed" : finished, state: "done" }]
      : [{ label: conflict === "resolving" ? "Resolving conflict" : held, state: "now" }, { label: finished, state: "todo" }];
    return [...["Planned", "Executed", "Reviewed", "Verification ran"].map((label): Phase => ({ label, state: "done" })), ...end];
  }
  const count = m.planTotal ? ` ${m.planDone}/${m.planTotal}` : "";
  if (observedPhase) {
    const at = PHASE_STEP[observedPhase];
    if (at === undefined) return [];
    const state = (step: number): Phase["state"] => step < at ? "done" : step === at ? "now" : "todo";
    return [at > 0 ? "Planned" : "Planning", `Executing${count}`, "Independent review", "Verification", "Completion"].map((label, step) => ({ label, state: state(step) }));
  }
  const started = status !== "draft", late = ["review", "ready_to_merge", "done"].includes(status);
  const executing = status === "paused" ? `Paused at Executing${count}` : status === "stopping" ? `Stopping at Executing${count}` : `Executing${count}`;
  return [
    { label: "Planned", state: started ? "done" : "now" },
    { label: executing, state: late ? "done" : started ? "now" : "todo" },
    { label: "Independent review", state: late ? "done" : "todo" },
    { label: "Verification", state: late ? "done" : "todo" },
    { label: "Completion", state: status === "done" ? "done" : late ? "now" : "todo" },
  ];
}

export const tokens = (n: number) => n >= 1000 ? `${Math.round(n / 1000)}k` : String(n);
export const contextOf = (c: { used: number; window: number } | null) => c === null ? null : { percent: Math.round(c.used / c.window * 100), text: `${tokens(c.used)} of ${tokens(c.window)}` };
export const contextLine = (c: { used: number; window: number } | null) => { const v = contextOf(c); return v ? `${v.percent}%, ${v.text}` : null; };
/** A recorded route in the session's words. A route that names its engine reads "Engine · Model", the model named as that
 *  engine's catalogue on the machine names it, so two workers on different engines never read alike. */
export const routeLabel = (route: WorkerRoute | null, options: SessionOptions, machines: readonly Pick<Machine, "config">[] = []) => {
  if (!route) return null;
  const engine = route.engine as EngineKey | undefined;
  const catalogued = engine ? machines.map(m => m.config?.catalogue?.[engine]?.models.find(x => x.id === route.model)?.label).find(Boolean) : undefined;
  const model = route.modelLabel || catalogued || options.models.find(m => m.id === route.model)?.label || plainChoice(route.model), effort = options.efforts.find(e => e.id === route.effort)?.label ?? effortLabel(route.effort);
  return [engine ? `${engineLabel(engine)} · ${model}` : model, effort].filter(Boolean).join(", ") || null;
};
/** A worker's own model as its engine session was configured, or why none is shown yet. Never the coordinator's route. */
export const workerModel = (w: Pick<Worker, "route" | "status">, label: (route: WorkerRoute | null) => string | null) =>
  label(w.route) ?? (w.status === "pending" || w.status === "not_started" ? "Chosen when it starts" : "Not confirmed");
export const modeLabel = (id: string) => ACCESS_MODES.find(m => m.id === id)?.label ?? id;
export const languageLabel = (id: string) => LANGUAGES.find(l => l.id === id)?.label ?? id;
/** A machine with no way to resume ends the run on Stop, and the button says so. */
export const stopLabel = (resumable: boolean) => resumable ? "Stop" : "End run";
/** A task with no route of its own runs on what its machine sets for every task. */
export const usesMachineRoute = (m: SessionModel) => !m.view.task.settings.model;
export const taskSettings = (m: SessionModel): TaskSettings => usesMachineRoute(m) ? machineDefaults(m.view.machine).settings : m.view.task.settings;
export const elapsedOf = (w: Pick<Worker, "elapsedMs" | "runningSince">, now: number) =>
  w.elapsedMs === null && w.runningSince === null ? null : (w.elapsedMs ?? 0) + (w.runningSince === null ? 0 : Math.max(0, now - w.runningSince));
export function duration(ms: number | null) {
  if (ms === null || !Number.isFinite(ms)) return "";
  const total = Math.max(0, Math.floor(ms / 1000)), hours = Math.floor(total / 3600), minutes = Math.floor(total / 60) % 60, seconds = total % 60;
  return hours ? `${hours}h ${minutes}m ${seconds}s` : minutes ? `${minutes}m ${seconds}s` : `${seconds}s`;
}
/** What the runtime's own verification came to. Only the task's result holds it, so nothing here is read from a worker's claim. */
export function verificationFace(m: SessionModel): Face & { checks: Check[] } {
  const checks = (m.view.task.result?.checks ?? []).filter(c => c.source === "runtime");
  if (m.view.task.observedPhase === "verification") return { label: "Running", tone: "st-run", icon: "loader", spin: true, checks };
  const failed = checks.filter(c => c.outcome === "failed").length;
  if (failed) return { label: `${failed} failed`, tone: "st-bad", icon: "x-circle", checks };
  if (checks.length && checks.every(c => c.outcome === "passed")) return { label: `${checks.length} passed`, tone: "st-ok", icon: "check-circle", checks };
  if (checks.length) return { label: "Not all confirmed", tone: "st-warn", icon: "help", checks };
  return { label: "Not run", tone: "st-idle", icon: "circle", checks };
}
/** Why a check is not a pass, in a person's words. A stated reason wins, and a bare evidence reference is never shown. */
export function checkWhy(check: Check) {
  const said = check.technicalReason?.trim() || (check.source === "worker" ? check.detail.trim() : "");
  if (said) return readableCheckReason(said);
  if (check.outcome === "not_run") return "This check did not run, so it proves nothing about the result.";
  if (check.outcome === "unverified") return "It ran, but its result could not be confirmed. Treat it as unproven.";
  return "This check failed. See the result for what it found.";
}
/** The task's result, or while there is none the checks of this version's workers: the one list the Inspector counts. A worker of an
 *  earlier version (before Request changes or a replan) reported on files that are gone, so its checks are not counted as current. */
export const checksOf = (m: SessionModel): Check[] => m.view.task.result?.checks ?? m.workers.filter(w => !w.superseded).reverse().flatMap(w => w.result?.checks ?? []);
export const editedFiles = (w: Worker) => {
  const items = w.activity.filter(a => a.kind === "edit");
  return { files: new Set(items.map(a => a.code)).size, add: items.reduce((n, a) => n + (a.diff?.add ?? 0), 0), del: items.reduce((n, a) => n + (a.diff?.del ?? 0), 0) };
};

/** The coordinator's model as the runtime recorded it, or nothing. The demo has no runtime and shows its own settings. */
export const recordedCoordinatorRoute = (m: SessionModel, demo: boolean): { model: string; effort: string } | null => {
  const recorded = m.view.task.coordinatorRoute ?? null;
  if (recorded || !demo) return recorded;
  const s = taskSettings(m);
  return s.model ? { model: s.model, effort: s.effort } : null;
};
const count = (n: number, one: string, many = `${one}s`) => `${n} ${n === 1 ? one : many}`;
const names = (list: string[]) => list.length < 2 ? list.join("") : `${list.slice(0, -1).join(", ")} and ${list.at(-1)}`;
/** A result that passed review and verification and waits in Ready to merge: closing it keeps it checked, never Not verified. */
export const checkedResult = (m: SessionModel) => {
  const t = m.view.task;
  return t.status === "ready_to_merge" && !!t.result && !staleResult(t) && !automaticStaleResultHold(m.view) && (!!t.acceptance || t.result.automaticReady === true);
};
/** What closing does, in numbers, before the owner agrees to it. */
export function closeConsequences(m: SessionModel) {
  if (unconfirmedState(m)?.kind === "records") return [
    { icon: "alert", text: "The records still disagree. Closing records your decision without trying to stop or resume the run, and repeats nothing." },
    { icon: "eye", text: "The task will be closed without verification. The conflicting records stay available in Diagnostics." },
  ];
  // A question left open by a stop is not a running worker: it is counted as a question below.
  const live = m.workers.filter(w => w.status === "working" || (w.status === "waiting_you" && m.mode === "live"));
  const pending = m.messages.filter(x => x.workerId && ["sending", "recorded"].includes(x.delivery)).length;
  const lines: { icon: string; text: string }[] = [];
  if (automaticAcceptanceHold(m.view)) {
    lines.push({ icon: "alert", text: m.view.fact.text });
    for (const check of checksOf(m).filter(c => c.kind !== "manual" && c.outcome !== "passed")) {
      lines.push({ icon: "eye", text: check.name + " · " + check.outcome.replace("_", " ") });
    }

  }
  if (unconfirmedState(m)?.closable) lines.push({ icon: "alert", text: "A process of this task is not confirmed as stopped. Closing stops what the service still runs for it and repeats nothing it did." });
  if (live.length) lines.push({ icon: "users", text: `${count(live.length, "worker")} will be stopped: ${names(live.map(w => w.role))}` });
  else if (m.workers.length) lines.push({ icon: "users", text: "All workers are already stopped" });
  if (m.open.length) lines.push({ icon: "question", text: `${count(m.open.length, "open question")} will be withdrawn` });
  if (m.queue.length || pending) lines.push({ icon: "queue", text: `${[m.queue.length ? count(m.queue.length, "queued message") : "", pending ? count(pending, "pending worker message") : ""].filter(Boolean).join(" and ")} will not be applied` });
  const t = m.view.task;
  if (checkedResult(m)) lines.push({ icon: "check-circle", text: `Result v${t.result!.version}${t.acceptance ? t.acceptance.automatic ? ", accepted automatically," : ", accepted by you," : ""} stays on record with its checks` });
  else lines.push({ icon: "eye", text: "The result will be marked Not verified" });
  const c = m.session.changes, where = m.view.project.hasGit === false ? "project folder" : m.view.task.checkout.kind === "worktree" ? "worktree" : "project folder";
  const files = c ? `${count(c.files, "file")}, +${c.add} −${c.del}` : "";
  lines.push({ icon: "file", text: automaticAcceptanceHold(m.view)
    ? `Closing creates no commit. Pending changes stay in the project folder used by the next task${files ? ` (${files})` : ""}.`
    : `Changes stay in the ${where}${files ? `: ${files}` : ""}` });
  return lines;
}
/** The rule that decides Stop, Close and the composer, in one place. */
/**
 * Why a task's state is not confirmed, as the runtime states it (a projection warning), and who acts next:
 * - `stopping`: a session of the task is still being stopped; the runtime decides by itself once its stop is observed.
 * - `exit_unconfirmed`: the runtime recorded that a process did not confirm its exit. Nothing in the running service
 *   observes it again (the next service start does), so the owner decides now: inspect, or Close as is, which replays nothing.
 * - `records`: the execution cannot be trusted; only a confirmed close of the Workspace task is allowed,
 *   and only with an explicit contradiction from the service.
 */
export type UnconfirmedState = { kind: "stopping" | "exit_unconfirmed" | "records"; closable: boolean };
export function unconfirmedState(m: SessionModel): UnconfirmedState | null {
  if (m.mode !== "unconfirmed") return null;
  const warnings = m.view.task.warnings ?? [];
  if (warnings.includes("process_exit_pending")) return { kind: "stopping", closable: true };
  if (warnings.includes("process_exit_unconfirmed")) return { kind: "exit_unconfirmed", closable: true };
  return { kind: "records", closable: !!m.view.task.executionId && !!m.view.task.sourceVersion && warnings.includes("contradictory_evidence") };
}
export const closeBlocked = (m: SessionModel) =>
  m.mode === "stopping" ? "Available once every worker is confirmed as stopped"
    : m.mode === "offline" || (m.mode === "unconfirmed" && !unconfirmedState(m)?.closable) ? "Unavailable until this task's state is confirmed"
    : m.mode === "archived" ? "Restore the task first"
    : m.mode === "closed" ? "This task is already closed" : m.view.task.integration?.method === "squash" ? "This task is already merged"
    : m.view.task.conflict !== "none" ? "A merge is in progress. Abort it first" : null;

/**
 * Which existing run control a stopped, held or ended task offers, read from its projection alone. The card, the task menu,
 * the palette, the title band, the strip and the composer all ask this one rule, so a state never offers Resume in one place
 * and Continue, or nothing, in another:
 * - `paused`: Stop paused it (or the runtime held it paused). Resume continues it.
 * - `held`: the runtime recorded an interruption and waits for the owner. Continue (the runtime's retry) or a message.
 * - `automatic`: the runtime or the sprint's Automatic run goes on by itself from the recorded state; no button is needed.
 * - `evidence`: a stop or a process exit is not confirmed yet; nothing can resume or continue until it is.
 * - `final`: a cancelled or failed run, or a closed task. It is never resumed.
 * - `unknown`: the state is not confirmed or the machine is offline. Nothing is offered.
 * `action` is the one command that applies, or null with `why` saying exactly why none does. Null for a task that simply runs,
 * has not started or waits on a review, a question or a merge: those have their own actions.
 */
export type ResumeKind = "paused" | "held" | "automatic" | "evidence" | "final" | "unknown";
export type ResumeState = { kind: ResumeKind; action: "continue_task" | "retry_task" | null; text: string; why: string };
export const NOT_CONFIRMED = "Unavailable until this task's state is confirmed";
export function resumeState(v: TaskView): ResumeState | null {
  const t = v.task, mode = composerMode(v), warnings = t.warnings ?? [];
  if (mode === "archived") return null;
  if (mode === "offline") return { kind: "unknown", action: null, text: "Machine disconnected", why: NOT_CONFIRMED };
  if (mode === "stopping") return { kind: "evidence", action: null, text: "Stopping", why: "Available once every worker is confirmed as stopped" };
  if (warnings.includes("process_exit_unconfirmed")) return { kind: "evidence", action: null, text: "Waiting for exit evidence",
    why: "A process of this task did not confirm that it stopped. Nothing can resume or continue while that is unknown; the service checks it again the next time it starts." };
  if (warnings.includes("process_exit_pending") || replanStopping(t)) return { kind: "evidence", action: null, text: "Waiting for processes to stop",
    why: "The service is confirming that this task's processes stopped and then continues by itself. Nothing can resume or continue until then." };
  if (mode === "unconfirmed") return { kind: "unknown", action: null, text: "Not confirmed", why: NOT_CONFIRMED };
  if (t.status === "done" || t.status === "closed_unverified") return { kind: "final", action: null, text: "Closed", why: "This task is closed. A closed task is never resumed." };
  if (t.status === "cancelled") return { kind: "final", action: null, text: "Run cancelled",
    why: "This run was cancelled. A cancelled run is final and cannot be resumed; Close task moves it to Done and keeps what was done." };
  if (t.status === "failed") return { kind: "final", action: null, text: "Run failed",
    why: mode === "review" ? "This run failed and cannot be resumed. Send a message to reopen it for a new plan."
      : ownerHold(v)?.actions.length ? "This run failed and cannot be resumed. What you can do is on the card above the message box."
      : "This run failed and cannot be resumed. Close task moves it to Done and keeps what was done." };
  if (t.status === "paused") return { kind: "paused", action: "continue_task", text: t.automaticContinuation?.pause ? v.automaticFact?.text ?? "Paused" : "Paused by you", why: "" };
  if (mode === "automatic") return { kind: "automatic", action: null, text: t.observedPhase === "recovery" && t.status === "working" ? "Recovering automatically" : v.automaticFact?.text ?? "Automatic",
    why: t.observedPhase === "recovery" && t.status === "working"
      ? "The service continues this task from its recorded state by itself. Nothing is needed from you, and nothing is replayed."
      : "Automatic decides the next step from the recorded state. Anything it needs from you is on the card above the message box." };
  if (t.observedPhase === "recovery" && t.status === "needs_you" && t.process === "live")
    return t.conflict === "none" ? { kind: "held", action: "retry_task", text: interruptedTurn(t.issue) ? "Interrupted" : "Stopped", why: "" }
      : { kind: "held", action: null, text: "Conflict", why: "A merge is in progress. Resolve or abort it first" };
  return null;
}
