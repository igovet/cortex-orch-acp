import type { Check as WireCheck, Decision as WireDecision, Result as WireResult, Task as WireTask } from "@cortex/workspace-contract";
import { emptySession } from "./invariants";
import type { ProjectSlice } from "./port";
import { descriptionLabel } from "./projections";
import type { Check, CheckOutcome, Connection, Decision, Project, Result, Task, TaskStatus } from "./types";

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
export const readableCheckReason = (reason: string) => /git_configuration_protected/.test(reason)
  ? "Git settings are protected from agent changes. Choose the checkout in Workspace, or create it yourself with Git, then ask Cortex to retry the check."
  : /(?:command[_ -]run|receipt).*(?:complete|completeness)|(?:complete|completeness).*receipt/i.test(reason)
  ? "The command's full result was not recorded. Ask Cortex to rerun the check and record its result."
  : reason === "verification_executable_unavailable"
  ? "The recorded check tool is not available on this machine. Install it to rerun, or use Accept with the acknowledgement checkbox."
  : reason === "verification_command_unavailable"
  ? "No executable command was recorded for this check. Request a new check, or use Accept with the acknowledgement checkbox."
  : /^[a-z0-9_]+(?::[A-Za-z0-9_]+)?$/.test(reason)
  ? "This check could not start because a required tool or service is unavailable. Restore it and rerun, or use Accept with acknowledgement."
  : /\b[a-z][a-z0-9]*(?:_[a-z0-9]+)+\b/.test(reason.replace(/\br_[a-f0-9]{12}\b/g, "the recorded report"))
  ? "This check could not be confirmed because a required tool or service was unavailable. Ask Cortex to rerun it and explain any remaining blocker."
  : reason.replace(/\br_[a-f0-9]{12}\b/g, "the recorded report");
const mapCheck = (check: WireCheck): Check => ({ name: check.name, outcome: mapCheckOutcome(check.outcome), detail: typeof check.detail === "string" && check.detail ? readableCheckReason(check.detail) : check.evidenceRef ? "Recorded evidence available" : "", source: "runtime", kind: check.kind === "manual" || check.kind === "policy" ? check.kind : "automated", command: Array.isArray(check.command) ? check.command.filter((s): s is string => typeof s === "string") : undefined, cwd: typeof check.cwd === "string" ? check.cwd : undefined, rerunnable: check.rerunnable === true, output: typeof check.output === "string" ? check.output : undefined, exitCode: typeof check.exitCode === "number" ? check.exitCode : undefined, technicalReason: typeof check.technicalReason === "string" ? readableCheckReason(check.technicalReason) : undefined });

const wholeNumber = (decimal: string) => { const n = Number(decimal); return Number.isSafeInteger(n) ? n : 0; };
export function mapResult(result: WireResult): Result {
  // With no observed digest there is nothing to compare, so the result binds to its own identity and version.
  const digest = result.observedWorkspaceDigest ?? `result:${result.resultId}:${result.version}`;
  return { kind:result.kind === "answer" || result.kind === "work" ? result.kind : "unknown", acceptanceAvailable:typeof result.acceptanceAvailable === "boolean" ? result.acceptanceAvailable : undefined, sourceVersion:result.version, evidence:result.evidence, warnings:Array.isArray(result.warnings) ? result.warnings.filter((s): s is string => typeof s === "string") : [], version: wholeNumber(result.sequenceNumber ?? "1"), digest, checks: result.checks.map(mapCheck), summary: result.text };
}

export function mapDecision(decision: WireDecision): Decision {
  // Only a pending decision that still allows an answer can be answered. Anything else, known or not, stays closed.
  const open = decision.status === "pending" && decision.allowedOperations.includes("decisions.answer");
  return {
    id: decision.decisionId, kind: "question", from: "", workerId: null, batchId: decision.batchId, at: typeof decision.createdAt === "string" ? lastConfirmed(decision.createdAt) : null, text: decision.prompt,
    options: decision.options.map(option => ({ label: option.label, optionId:option.optionId })), answer: decision.status === "answered" ? { text: typeof decision.answerText === "string" ? decision.answerText : "Answered", at: typeof decision.answeredAt === "string" ? lastConfirmed(decision.answeredAt) : null } : null, withdrawn: !open && decision.status !== "answered" && decision.status !== "pending", suspended:decision.status === "pending" && !open,
  };
}

/** A phase is believed only when the runtime state, if reported, agrees with it. */
function phaseOf(task: WireTask): string {
  if (!(task.phase in STATUS_OF_PHASE)) return "unknown";
  if (task.runtimeState !== null && PHASE_OF_RUNTIME[task.runtimeState] !== task.phase) return "unknown";
  return task.phase;
}
const lastConfirmed = (at: string | null) => { const ms = at === null ? NaN : Date.parse(at); return Number.isNaN(ms) ? null : ms; };

export function mapTask(task: WireTask, context: { projectId: string }, result?: WireResult): Task {
  const phase = phaseOf(task), mapped = result ? mapResult(result) : null;
  const known = phase !== "unknown" && task.phase !== "recovery";
  const status = STATUS_OF_PHASE[phase] ?? (task.executionId ? "working" : "draft");
  return {
    id: task.taskId, projectId: context.projectId, sprintId: null, description: task.description,
    title: task.title ?? descriptionLabel(task.description), titleState: task.title === null ? "pending" : "generated",
    checkout: { kind: "main", branch: "" }, targetBranch: null, dependencies: [], status, resumeStatus: "working",
    observedPhase:phase, coordinatorRoute: task.coordinatorRoute ?? null, warnings:task.warnings, issue: task.waitReason, executionId: task.executionId, result: mapped, workspaceDigest: mapped?.digest ?? null, acceptance: null,
    conflict: "none", merge: null, resolvedBy: null, integration: null, closedAs: null, notVerifiedByCortex: false, doneAt: null,
    archive: null, archiveQuestion: false, archiveAskAfter: null, work: "unmerged",
    process: !known ? "unknown" : status === "working" || status === "needs_you" ? "live" : "stopped",
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
