import { readableCheckReason, ACCESS_MODES, effortLabel, emptySession, isOpen, LANGUAGES, machineDefaults } from "@cortex/workspace-domain";
import type { Check, Decision, Message, SessionOptions, TaskSession, TaskSettings, Worker, WorkerStatus } from "@cortex/workspace-domain";
import { clock, plainChoice, readyLabel, sameBranch, type TaskView, type WorkspaceIndex } from "./selectors";

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
/** A finished worker whose check failed never reads as a clean finish. */
export function workerState(w: Worker): Face {
  const failed = w.status === "completed" ? w.result?.checks.filter(c => c.outcome === "failed").length ?? 0 : 0;
  return failed ? { label: `${failed} check${failed === 1 ? "" : "s"} failed`, tone: "st-bad", icon: "x-circle" } : workerFace(w.status);
}
const ACTIVE: WorkerStatus[] = ["working", "waiting_you", "paused", "unknown"];
const FINISHED: WorkerStatus[] = ["completed", "failed", "cancelled"];
export const isFinished = (w: Worker) => FINISHED.includes(w.status);

export type ComposerMode = "live" | "paused" | "stopping" | "draft" | "review" | "closed" | "ended" | "archived" | "offline" | "unconfirmed";
export type SessionModel = {
  view: TaskView; session: TaskSession; workers: Worker[]; active: Worker[]; finished: Worker[]; running: Worker[];
  messages: Message[]; queue: Message[]; open: Decision[]; mode: ComposerMode; planDone: number; planTotal: number;
};
function composerMode(v: TaskView): ComposerMode {
  if (v.column === "archive") return "archived";
  if (v.machine.connection !== "connected") return "offline";
  if (v.unknown && v.task.status !== "stopping") return "unconfirmed";
  switch (v.task.status) {
    case "working": case "needs_you": return "live";
    case "paused": return "paused";
    case "stopping": return "stopping";
    case "draft": return "draft";
    case "done": return "closed";
    case "failed": case "cancelled": return "ended";
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
    active: workers.filter(w => ACTIVE.includes(w.status)), finished: workers.filter(isFinished), running: workers.filter(w => w.status === "working"),
    queue: messages.filter(m => m.delivery === "queued"), open: session.decisions.filter(isOpen),
    planDone: nodes.filter(n => n.workerId && byId.get(n.workerId)?.status === "completed").length, planTotal: nodes.length,
  };
}
const last = <T,>(list: T[], test: (item: T) => boolean) => { for (let i = list.length - 1; i >= 0; i--) if (test(list[i])) return list[i]; return null; };
/** When the owner last stopped this task, from the timeline line the stop wrote. */
export const stoppedAt = (m: SessionModel) => {
  const line = last(m.session.timeline, e => e.kind === "line" && e.icon === "pause");
  return line?.kind === "line" ? line.at : null;
};

export type Chip = { icon: string; tone: string; spin?: boolean; text: string; wide?: string; next?: string };
/** The current state and the next step, for the title band. */
export function stateChip(m: SessionModel): Chip {
  const { task } = m.view;
  if (m.view.unknown && task.status !== "stopping") return { icon: "alert", tone: "st-idle", text: m.mode === "unconfirmed" ? "Not confirmed" : m.view.fact.text };
  if (task.conflict === "resolving") return { icon: "loader", tone: "st-run", spin: true, text: "Resolving" };
  if (task.conflict !== "none") return { icon: "alert", tone: "st-bad", text: "Conflict" };
  switch (task.status) {
    case "draft": return { icon: "circle", tone: "st-idle", text: "Not started" };
    case "paused": { const at = stoppedAt(m); return { icon: "pause", tone: "st-idle", text: "Paused", wide: at === null ? " by you" : ` by you, ${clock(at)}` }; }
    case "stopping": return { icon: "loader", tone: "st-warn", spin: true, text: "Stopping" };
    case "review": return { icon: "eye", tone: "st-idle", text: "Review", next: m.view.project.hasGit === false || sameBranch(task, m.view.target) ? "Ready to finish" : "Ready to merge" };
    case "ready_to_merge": return { icon: "check", tone: "st-idle", text: readyLabel(task, m.view.target) };
    case "done": return { icon: m.view.fact.icon ?? "check-circle", tone: "st-idle", text: m.view.fact.text };
    case "failed": return { icon: "x-circle", tone: "st-bad", text: "Run failed" };
    case "cancelled": return { icon: "x-circle", tone: "st-idle", text: "Run cancelled" };
    default: return m.running.length || task.status === "working" ? { icon: "loader", tone: "st-run", spin: true, text: "Working", next: "Review" } : { icon: "question", tone: "st-warn", text: "Needs you" };
  }
}

export type Phase = { label: string; state: "done" | "now" | "todo" };
/** Where a runtime phase sits on the bar. A phase with no place, such as a wait, says nothing about how far the task got. */
const PHASE_STEP: Record<string, number> = { not_started: -1, planning: 0, execution: 1, review: 2, verification: 3, finalizing: 4 };
export function phases(m: SessionModel): Phase[] {
  const { status, result, acceptance, closedAs, conflict, work, observedPhase } = m.view.task;
  if (closedAs === "external") return [{ label: "Done outside the app", state: "done" }];
  if (result && !closedAs && (conflict !== "none" || ["review", "ready_to_merge", "done"].includes(status))) {
    const finished = sameBranch(m.view.task, m.view.target) ? "Finished" : "Merged";
    const held = acceptance ? `Accepted v${acceptance.version}` : `Result v${result.version}`;
    const end: Phase[] = status === "review" ? [{ label: `Result v${result.version}`, state: "now" }]
      : work === "merged" ? [{ label: held, state: "done" }, { label: m.view.task.integration?.method === "commit" ? "Committed" : finished, state: "done" }]
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
export const routeLabel = (route: { model: string; effort: string } | null, options: SessionOptions) => {
  if (!route) return null;
  const model = options.models.find(m => m.id === route.model)?.label ?? plainChoice(route.model), effort = options.efforts.find(e => e.id === route.effort)?.label ?? effortLabel(route.effort);
  return [model, effort].filter(Boolean).join(", ") || null;
};
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
/** All worker checks plus the task's result, the one list the Inspector counts. */
export const checksOf = (m: SessionModel): Check[] => m.view.task.result?.checks ?? m.workers.flatMap(w => w.result?.checks ?? []);
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
  return t.status === "ready_to_merge" && !!t.result && (!!t.acceptance || t.result.automaticReady === true);
};
/** What closing does, in numbers, before the owner agrees to it. */
export function closeConsequences(m: SessionModel) {
  // A question left open by a stop is not a running worker: it is counted as a question below.
  const live = m.workers.filter(w => w.status === "working" || (w.status === "waiting_you" && m.mode === "live"));
  const pending = m.messages.filter(x => x.workerId && ["sending", "recorded"].includes(x.delivery)).length;
  const lines: { icon: string; text: string }[] = [];
  if (live.length) lines.push({ icon: "users", text: `${count(live.length, "worker")} will be stopped: ${names(live.map(w => w.role))}` });
  else if (m.workers.length) lines.push({ icon: "users", text: "All workers are already stopped" });
  if (m.open.length) lines.push({ icon: "question", text: `${count(m.open.length, "open question")} will be withdrawn` });
  if (m.queue.length || pending) lines.push({ icon: "queue", text: `${[m.queue.length ? count(m.queue.length, "queued message") : "", pending ? count(pending, "pending worker message") : ""].filter(Boolean).join(" and ")} will not be applied` });
  const t = m.view.task;
  if (checkedResult(m)) lines.push({ icon: "check-circle", text: `Result v${t.result!.version}${t.acceptance ? ", accepted by you," : ""} stays on record with its checks` });
  else lines.push({ icon: "eye", text: "Review and verification did not finish, so the result is marked not verified" });
  const c = m.session.changes, where = m.view.project.hasGit === false ? "project folder" : m.view.task.checkout.kind === "worktree" ? "worktree" : "project folder";
  lines.push({ icon: "file", text: c ? `Changes stay in the ${where}: ${count(c.files, "file")}, +${c.add} −${c.del}` : `Changes stay in the ${where}` });
  return lines;
}
/** The rule that decides Stop, Close and the composer, in one place. */
export const closeBlocked = (m: SessionModel) =>
  m.mode === "stopping" ? "Available once every worker is confirmed as stopped"
    : m.mode === "offline" || m.mode === "unconfirmed" ? "Unavailable until this task's state is confirmed"
    : m.mode === "archived" ? "Restore the task first"
    : m.mode === "closed" ? "This task is already closed" : m.view.task.integration?.method === "squash" ? "This task is already merged"
    : m.view.task.conflict !== "none" ? "A merge is in progress. Abort it first" : null;
