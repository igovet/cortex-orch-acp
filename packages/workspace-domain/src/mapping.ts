import type { Check as WireCheck, Decision as WireDecision, Result as WireResult, Task as WireTask } from "@cortex/workspace-contract";
import { emptySession, isAutomaticContinuation } from "./invariants";
import type { ProjectSlice } from "./port";
import { descriptionLabel } from "./projections";
import type { AttachmentRef, Check, CheckOutcome, Connection, Decision, DecisionTarget, Project, Result, Task, TaskStatus } from "./types";

/**
 * Wire DTOs to view models. This reads what the runtime already decided; it never runs a transition and never promotes a
 * fact. A runtime result is not an acceptance, a failed or cancelled run is not Done, and anything missing or contradictory
 * stays unknown.
 */
const PHASE_OF_RUNTIME: Record<string, string> = {
  CREATED: "planning", PLANNING: "planning", PLAN_READY: "planning", EXECUTING: "execution", REVIEWING: "review", VERIFYING: "verification",
  RECONCILING: "finalizing", ANSWER_READY: "finalizing", WAITING_USER: "waiting", RECOVERING: "recovery", BLOCKED: "blocked", FAILED: "failed", CANCELLED: "stopped", COMPLETED: "result", PAUSED: "paused",
};
const STATUS_OF_PHASE: Record<string, TaskStatus> = {
  not_started: "draft", planning: "working", execution: "working", review: "working", verification: "working", finalizing: "working",
  paused:"paused", waiting: "needs_you", blocked: "needs_you", failed: "failed", stopped: "cancelled", result: "review",
};

/** A runtime state alone, for a place that has no phase. Without a connection nothing is claimed. */
export function statusOfRuntime(state: string | null, connection: Connection): TaskStatus | "unknown" {
  if (connection !== "connected" || !state) return "unknown";
  const phase = PHASE_OF_RUNTIME[state];
  return phase ? STATUS_OF_PHASE[phase] ?? "unknown" : "unknown";
}

export function mapCheckOutcome(outcome: WireCheck["outcome"]): CheckOutcome {
  if (outcome === "passed" || outcome === "failed" || outcome === "not_run") return outcome;
  return outcome === "not_applicable" ? "not_run" : "unverified";
}
/**
 * Every reason code the service and runtime record for a check, as [what it says on its own, the words that stand in for the
 * code inside a sentence]. Evidence gaps (`cortex_core::lifecycle::worker_checks`, and the codes retained stores still carry)
 * are facts about the cited command run; command failures come from the native command supervisor; the rest are check and
 * sandbox blockers. Only a code that names a missing tool says a tool is missing.
 */
const REASON_CODES: Record<string, readonly [string, string]> = {
  revision_mismatch: ["The worker ran this check before the files reached the state this result reports, so that run does not count for it.", "a run on an earlier version of the files"],
  trivial_command: ["The command cited for this check only shows or prints something; it does not test the claim, so it does not count as a check.", "a command that only shows something and does not test the claim"],
  tool_call_ref_missing: ["The worker named no recorded command run for this check.", "no cited command run"],
  tool_call_ref_unknown: ["The command run the worker cited was not recorded.", "an unrecorded command run"],
  foreign_attempt: ["The command run cited here belongs to another worker attempt, so it does not count for this one.", "a command run of another attempt"],
  ref_already_cited: ["The command run cited here is already cited by another check, so it does not count twice.", "a command run another check already cites"],
  not_a_command: ["What this check cites is not a command run.", "a citation that is not a command run"],
  engine_command_unverifiable: ["This check cites a command the engine ran itself; its exit code and output were not recorded, so it cannot confirm the check.", "an engine command whose result was not recorded"],
  receipt_missing: ["No receipt was recorded for the command run cited here.", "a missing command receipt"],
  ambiguous_ref: ["The command run cited here matches more than one recorded run.", "an ambiguous command run"],
  call_incomplete: ["The command run cited here did not finish recording its result.", "an unfinished command run"],
  result_missing: ["The command run cited here has no recorded result.", "a command run without a recorded result"],
  result_truncated: ["Only part of the cited command's output was kept, so its result is not confirmed.", "a command run whose output was cut short"],
  command_unrecorded: ["The command run cited here has no recorded command line.", "a command run without a recorded command line"],
  status_variable_unproven: ["The cited command reports success through a status it prints itself, which does not prove the check passed.", "a command whose printed status proves nothing"],
  later_run_unresolved: ["A later run of the same command did not finish, so this pass is not confirmed.", "a later run that did not finish"],
  later_run_failed: ["A later run of the same command failed.", "a later failed run"],
  nonzero_exit: ["The command run cited here did not exit successfully.", "a command that did not exit successfully"],
  zero_exit: ["This check is reported as failed, but the command it cites exited successfully.", "a failure claim whose command succeeded"],
  spawn_failed: ["The command could not be started.", "a command that could not start"],
  spawn_or_capture_failed: ["The command could not be started, or its output could not be captured.", "a command that could not start or be captured"],
  wait_failed: ["The command started, but how it ended could not be observed.", "a command whose end was not observed"],
  owner_stopped: ["The command was stopped by you before it finished.", "a command you stopped"],
  timeout: ["The command ran past its time limit and was stopped.", "a command that ran past its time limit"],
  interrupted_or_timeout: ["The command was interrupted or ran past its time limit.", "an interrupted command"],
  output_incomplete: ["The command's output was not recorded completely, so its result is not confirmed.", "incomplete command output"],
  authority_changed: ["The task's access changed while the command ran, so it was stopped.", "a change of access during the command"],
  deletion_not_confirmed: ["The command was not run because a deletion it needed was not confirmed.", "an unconfirmed deletion"],
  verification_review_revision_changed: ["The files changed after this check ran, so it no longer covers the version under review.", "files that changed after the check"],
  verification_executable_unavailable: ["The recorded check tool is not available on this machine. Install it to rerun, or use Accept with the acknowledgement checkbox.", "a check tool that is not installed here"],
  verification_command_unavailable: ["No executable command was recorded for this check. Request a new check, or use Accept with the acknowledgement checkbox.", "no recorded executable command"],
  bubblewrap_missing: ["The command sandbox (bubblewrap) is not installed on this machine, so the check could not run.", "a missing command sandbox"],
  bubblewrap_launch_failed: ["The command sandbox could not be started, so the check did not run.", "a sandbox that could not start"],
  namespace_or_mount_setup_failed: ["The command sandbox could not be set up, so the check did not run.", "a sandbox that could not be set up"],
  setup_timeout: ["Preparing the command sandbox took too long, so the check did not run.", "a sandbox setup that took too long"],
  check_preflight_unavailable: ["The check could not be prepared, so it did not run.", "a check that could not be prepared"],
  task_files_changed: ["The task files changed after this check, so it no longer covers them.", "task files that changed after the check"],
  workspace_drift: ["The workspace changed after this check, so it no longer covers it.", "a workspace that changed after the check"],
  stale_result: ["This check belongs to a result that is out of date.", "a result that is out of date"],
  write_grant_outside_project: ["The command asked to write outside the project, which is not allowed.", "a write outside the project"],
};
/** The plain words of one bare code, or undefined. `Object.hasOwn` keeps names like `constructor` from reading the prototype. */
const codeWords = (code: string) => Object.hasOwn(REASON_CODES, code) ? REASON_CODES[code] : undefined;
const evidenceGap = (reason: string): string | undefined => {
  const gap = /^Runtime kept this passed claim unverified: ([a-z_]+)\.?$/.exec(reason.trim())?.[1] ?? reason.trim();
  return codeWords(gap)?.[0];
};
const CODE = /\b[a-z][a-z0-9]*(?:_[a-z0-9]+)+\b/g;
/** Why a check is not a pass, in a person's words. A reason never claims a tool or service is missing unless its code says so. */
export const readableCheckReason = (reason: string) => {
  const text = reason.replace(/\br_[a-f0-9]{12}\b/g, "the recorded report");
  const bare = /^([a-z0-9_]+)(?::[A-Za-z0-9_]+)?$/.exec(text.trim())?.[1];
  return evidenceGap(text) ?? (/git_configuration_protected/.test(text)
    ? "Git settings are protected from agent changes. Choose the checkout in Workspace, or create it yourself with Git, then ask Cortex to retry the check."
    : /(?:command[_ -]run|receipt).*(?:complete|completeness)|(?:complete|completeness).*receipt/i.test(text)
    ? "The command's full result was not recorded. Ask Cortex to rerun the check and record its result."
    : bare !== undefined
    ? codeWords(bare)?.[0] ?? `The runtime did not confirm this check (recorded reason: ${bare.replace(/_+/g, " ")}). Ask Cortex to rerun it.`
    // A written reason keeps its words; a known code inside it reads as what it means, and anything else (a path, a name) stays as written.
    : text.replace(CODE, word => codeWords(word)?.[1] ?? word));
};
const mapCheck = (check: WireCheck): Check => ({ name: check.name, outcome: mapCheckOutcome(check.outcome), detail: typeof check.detail === "string" && check.detail ? readableCheckReason(check.detail) : check.evidenceRef ? "Recorded evidence available" : "", source: "runtime", kind: check.kind === "manual" || check.kind === "policy" ? check.kind : "automated", command: Array.isArray(check.command) ? check.command.filter((s): s is string => typeof s === "string") : undefined, cwd: typeof check.cwd === "string" ? check.cwd : undefined, rerunnable: check.rerunnable === true, output: typeof check.output === "string" ? check.output : undefined, exitCode: typeof check.exitCode === "number" ? check.exitCode : undefined, technicalReason: typeof check.technicalReason === "string" ? readableCheckReason(check.technicalReason) : undefined });

const wholeNumber = (decimal: string) => { const n = Number(decimal); return Number.isSafeInteger(n) ? n : 0; };
export function mapResult(result: WireResult): Result {
  // With no observed digest there is nothing to compare, so the result binds to its own identity and version.
  const digest = result.observedWorkspaceDigest ?? `result:${result.resultId}:${result.version}`;
  return { kind:result.kind === "answer" || result.kind === "work" ? result.kind : "unknown", acceptanceAvailable:typeof result.acceptanceAvailable === "boolean" ? result.acceptanceAvailable : undefined, sourceVersion:result.version, evidence:result.evidence, warnings:Array.isArray(result.warnings) ? result.warnings.filter((s): s is string => typeof s === "string") : [], version: wholeNumber(result.sequenceNumber ?? "1"), digest, checks: result.checks.map(mapCheck), summary: result.text };
}

export function mapDecision(decision: WireDecision): Decision {
  // The contract's known kinds do not list `permission`, so its decoder reads that kind as `unknown`; the runtime's
  // permission details stay with it and say what it is.
  const permission = decision.kind === "permission" || (decision.kind === "unknown" && typeof (decision as { permission?: unknown }).permission === "object" && (decision as { permission?: unknown }).permission !== null);
  // Only a pending decision that still allows an answer can be answered. Anything else, known or not, stays closed.
  const open = decision.status === "pending" && decision.allowedOperations.includes("decisions.answer");
  return {
    // A worker's question is answered in the task's Conversation; `askedBy` names the worker role that asked, a coordinator's has none.
    id: decision.decisionId, kind: permission ? "permission" : "question", from: typeof decision.askedBy === "string" ? decision.askedBy : "", workerId: null, batchId: decision.batchId, at: typeof decision.createdAt === "string" ? lastConfirmed(decision.createdAt) : null, text: decision.prompt,
    // The runtime's stop question says what each answer does and which one it recommends.
    options: decision.options.map(option => ({ label: option.label, optionId:option.optionId, ...(typeof option.detail === "string" && option.detail ? { sub: option.detail } : {}), ...(option.recommended === true ? { recommended: true as const } : {}) })), answer: decision.status === "answered" ? { text: typeof decision.answerText === "string" ? decision.answerText : "Answered", at: typeof decision.answeredAt === "string" ? lastConfirmed(decision.answeredAt) : null, ...answerFiles(decision.answerAttachments) } : null, withdrawn: !open && decision.status !== "answered" && decision.status !== "pending", suspended:decision.status === "pending" && !open,
    ...(decision.optionsOnly === true ? { optionsOnly: true as const } : {}),
    ...(typeof decision.headline === "string" && decision.headline ? { headline: decision.headline } : {}),
    // A batch member's short subject label, when the asker gave one.
    ...(typeof decision.topic === "string" && decision.topic ? { topic: decision.topic } : {}),
    ...(Array.isArray(decision.targets) ? { targets: decision.targets.flatMap(decisionTarget) } : {}),
    ...(typeof decision.replacedBy === "string" && decision.replacedBy ? { replacedBy: decision.replacedBy } : {}),
    ...(permission ? { permission: mapPermission((decision as { permission?: unknown }).permission) } : {}),
  };
}
/** The files the owner attached to an answer: only complete saved references are kept; anything else is dropped, never guessed. */
function answerFiles(raw: unknown): { attachments?: AttachmentRef[] } {
  if (!Array.isArray(raw)) return {};
  const files = raw.filter((f): f is AttachmentRef => !!f && typeof f === "object" && typeof f.name === "string" && typeof f.mimeType === "string"
    && typeof f.size === "number" && typeof f.sha256 === "string" && /^[0-9a-f]{64}$/.test(f.sha256));
  return files.length ? { attachments: files.map(({ name, mimeType, size, sha256 }) => ({ name, mimeType, size, sha256 })) } : {};
}
/** One target the runtime named on its stop question, kept only when it is complete; anything else is dropped, never guessed. */
function decisionTarget(raw: unknown): DecisionTarget[] {
  if (typeof raw !== "object" || raw === null) return [];
  const t = raw as Record<string, unknown>, text = (v: unknown) => typeof v === "string" && v ? v : null;
  if (t.kind === "task" && text(t.taskId)) return [{ kind: "task", taskId: text(t.taskId)!, title: text(t.title) ?? "" }];
  if (t.kind === "report" && text(t.reportId)) return [{ kind: "report", reportId: text(t.reportId)!, role: text(t.role) ?? "" }];
  if (t.kind === "rule" && text(t.path)) return [{ kind: "rule", path: text(t.path)! }];
  return [];
}

/** A phase is believed only when the runtime state, if reported, agrees with it. */
function phaseOf(task: WireTask): string {
  if (!(task.phase in STATUS_OF_PHASE)) return "unknown";
  if (task.runtimeState !== null && PHASE_OF_RUNTIME[task.runtimeState] !== task.phase) return "unknown";
  return task.phase;
}
const lastConfirmed = (at: string | null) => { const ms = at === null ? NaN : Date.parse(at); return Number.isNaN(ms) ? null : ms; };

export function mapTask(task: WireTask, context: { projectId: string }, result?: WireResult): Task {
  // A cancelled turn, or a recovery whose reason the runtime recorded, waits in RECOVERING for the next
  // owner message, Stop or Close. This is not generic recovery or proof that every engine process has stopped;
  // a recovery without a stated reason stays unconfirmed.
  const continuation = isAutomaticContinuation(task.automaticContinuation) ? task.automaticContinuation : undefined;
  const consistent = !task.warnings.includes("contradictory_evidence");
  // A replan waiting for a worker it cancelled to stop names that worker in `waitReason`; it is still the unconfirmed
  // `process_exit_pending` hold the runtime settles by itself, never a recorded recovery that waits for the owner.
  const replanStop = task.warnings.includes("cancelled_worker_stopping");
  const pending = consistent && !!continuation && task.phase === "recovery" && task.runtimeState === "RECOVERING"
    && task.executionState === "recovering" && task.waitReason === "automatic_continuation_pending";
  const paused = consistent && !!continuation?.pause && task.phase === "paused" && task.executionState === "paused"
    && task.runtimeState === "RECOVERING";
  const interrupted = !pending && !replanStop && task.executionId !== null && task.phase === "recovery" && task.runtimeState === "RECOVERING"
    && task.executionState === "recovering" && typeof task.waitReason === "string" && task.waitReason.trim() !== ""
    && !task.warnings.includes("contradictory_evidence");
  const phase = paused ? "paused" : interrupted || pending ? "recovery" : phaseOf(task), mapped = result ? mapResult(result) : null;
  const known = interrupted || pending || paused || (phase !== "unknown" && task.phase !== "recovery");
  const ownerResolved = phase === "stopped" && task.runtimeState === "CANCELLED" && task.warnings.includes("owner_resolved_without_certification") && !task.warnings.includes("contradictory_evidence");
  const closedUnconfirmed = !consistent && task.executionId !== null && task.warnings.includes("owner_closed_unconfirmed");
  const status = closedUnconfirmed ? "closed_unverified" : ownerResolved ? "done" : interrupted ? "needs_you" : STATUS_OF_PHASE[phase] ?? (task.executionId ? "working" : "draft");
  return {
    id: task.taskId, projectId: context.projectId, sprintId: null, description: task.description, sourceVersion: task.version,
    title: task.title ?? descriptionLabel(task.description), titleState: task.title === null ? "pending" : "generated",
    checkout: { kind: "main", branch: "" }, targetBranch: null, dependencies: [], status, resumeStatus: "working",
    ...(continuation ? { automaticContinuation: continuation } : {}),
    observedPhase:phase, coordinatorRoute: task.coordinatorRoute ?? null, warnings:task.warnings, issue: task.waitReason, executionId: task.executionId, result: mapped, workspaceDigest: mapped?.digest ?? null, acceptance: null,
    conflict: "none", merge: null, resolvedBy: null, integration: null, closedAs: closedUnconfirmed ? "unconfirmed" : ownerResolved ? "as_is" : null, notVerifiedByCortex: ownerResolved || closedUnconfirmed, doneAt: ownerResolved || closedUnconfirmed ? lastConfirmed(task.lastConfirmedAt) : null,
    archive: null, work: "unmerged",
    process: closedUnconfirmed || !known ? "unknown" : status === "working" || status === "needs_you" ? "live" : "stopped",
    unexpectedChanges: false, purgeBlocker: null, settings: { model: "", effort: "", mode: "guarded", language: "auto" }, lastConfirmedAt: lastConfirmed(task.lastConfirmedAt),
  };
}

export function mapProjectSlice(project: Project, tasks: WireTask[], decisions: WireDecision[] = [], results: WireResult[] = []): ProjectSlice {
  const mapped = tasks.map(task => {
    const value = mapTask(task, { projectId: project.id }, results.find(r => r.taskId === task.taskId));
    // A worker can ask while other workers keep running. This is an owner-facing
    // wait, not a claim that the whole runtime entered WAITING_USER.
    const pending = decisions.find(d => task.executionId !== null && d.executionId === task.executionId && d.status === "pending" && d.allowedOperations.includes("decisions.answer"));
    return pending && value.process === "live" ? { ...value, status:"needs_you" as const, issue:pending.prompt } : value;
  });
  const sessions = tasks.flatMap(task => {
    const own = decisions.filter(d => task.executionId !== null && d.executionId === task.executionId);
    if (!own.length) return [];
    const session = emptySession(task.taskId);
    session.decisions = own.map(mapDecision);
    session.timeline = own.map(d => ({ id: `decision:${d.decisionId}`, kind: "decision" as const, decisionId: d.decisionId }));
    return [session];
  });
  return { project, tasks: mapped, sessions };
}
/** A permission decision's details, each field kept only when it has the expected shape. */
function mapPermission(raw: unknown): NonNullable<Decision["permission"]> {
  const p = typeof raw === "object" && raw !== null ? raw as Record<string, unknown> : {};
  const text = (v: unknown) => typeof v === "string" ? v : "";
  return { operation: text(p.operation), paths: Array.isArray(p.paths) ? p.paths.filter((s): s is string => typeof s === "string") : [],
    reason: text(p.reason), reasonText: text(p.reasonText), access: text(p.access) };
}
