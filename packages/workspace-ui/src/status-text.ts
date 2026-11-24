/**
 * Plain-language text for every machine status, phase, wait/pause/hold/stop reason and refusal code the screens can show.
 * One table, one scrubber: a known code reads as a sentence, an unknown bare code reads as a generic sentence, and prose
 * keeps its words with any code inside it spelled out. Nothing here ever returns an underscore or a bare internal code.
 * Task titles, commands, paths and model text are not passed through this module.
 */

/** A bare snake_case code such as `review_cycle_limit`. */
const BARE_CODE = /^[a-z][a-z0-9]*(?:_[a-z0-9]+)+$/;
/** A snake_case token inside a sentence. */
const EMBEDDED_CODE = /\b[a-z][a-z0-9]*(?:_[a-z0-9]+)+\b/g;
/** What an unrecognised bare code reads as: the code itself is never shown. */
export const GENERIC_STATUS = "This step stopped for a reason the app cannot describe. Open the task for details";

/** Reason codes the runtime and service record for a stop, pause, hold or wait, each as a sentence. */
export const STATUS_TEXT: Record<string, string> = {
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
  native_session_lost: "The engine session was lost",
  coordinator_session_start_failed: "The coordinator session could not start",
  native_progress_limit: "The task reached its limit of steps",
  native_driver_progress_limit: "The task reached its limit of steps",
  coordinator_no_progress_limit: "The coordinator stopped making progress",
  review_incomplete: "The review is incomplete",
  review_blocked: "The independent review blocked this result",
  review_changes_without_findings: "The review requested changes without identifying findings",
  review_project_revision_changed: "The project changed while it was being reviewed",
  review_project_revision_unstable: "The project kept changing during review",
  review_cycle_limit: "The review reached its retry limit",
  review_no_progress: "Fixes and reviews stopped making progress",
  review_coverage_unfinished: "The review left parts unchecked",
  review_block_findings_required: "The review blocked the result without naming findings",
  assignment_blocker_required: "A failed step did not say what stopped it",
  missing_access: "Needs access or a permission the agent does not have",
  owner_decision: "Needs your decision",
  other_role_work: "Needs work from another role",
  tool_unavailable: "A required tool is unavailable",
  review_required: "A review is still required",
  review_resubmit_required: "The review result has to be submitted again",
  review_verdict_required: "The review gave no verdict",
  review_file_coverage_required: "The review did not cover every changed file",
  review_not_covered_required: "The review left some items unchecked",
  review_not_independent: "The review was not independent of the work",
  review_revision_unavailable: "The reviewed version is no longer available",
  review_predecessor_reports_required: "The review needs the earlier reports",
  review_already_bound: "This review already belongs to another result",
  review_in_progress: "A review is in progress",
  review_in_flight: "A review is in progress",
  automatic_acceptance_blocked: "Automatic acceptance needs your decision",
  automatic_acceptance_required: "The result still needs to be accepted",
  automatic_continuation_pending: "Preparing the next automatic step",
  automatic_continuation_paused: "Automatic continuation is paused",
  automatic_continuation: "Continuing automatically",
  automatic_evidence_missing: "Evidence the automatic step needs is missing",
  automatic_required_check_missing: "A check the automatic step needs has not been recorded",
  automatic_finish_unavailable: "The task cannot be finished automatically",
  automatic_queue_unavailable: "The automatic queue is unavailable",
  automatic_real_decision_pending: "A decision from you is pending",
  automatic_stop: "Automatic mode stopped",
  outcome_unknown: "The outcome could not be confirmed",
  task_content_unknown: "The task contents could not be confirmed",
  task_files_changed: "The task files changed after this result",
  workspace_drift: "The workspace changed after this result",
  stale_result: "This result is out of date",
  continuation_limit: "Automatic continuation reached its limit",
  continuation_cap: "Automatic continuation reached its limit",
  continuation_binding_lost: "The task lost track of what to continue",
  continuation_required: "The task has to continue before it can finish",
  answer_continuation_unconfirmed: "Whether your answer let the task continue could not be confirmed",
  external_blocker: "Something outside the task is blocking it",
  owner_blocker: "Another automatic attempt would not change this",
  settlement_blocked: "The task cannot be settled yet",
  queue_blocked: "The queue cannot continue yet",
  start_refused: "The task could not be started",
  owner_integration: "The result is waiting for you to finish it",
  owner_accepted: "You accepted this result; confirm how to finish it",
  owner_paused: "Paused by you",
  owner_pause: "Paused by you",
  owner_stopped: "Stopped by you",
  owner_resolved: "Resolved by you",
  authority_lost: "The task lost its permission to continue",
  authorization_required: "Your permission is needed",
  active_writer_conflict: "Another task is changing the same files",
  no_ready_nodes: "No step is ready to run",
  no_available_route: "No engine is available for this step",
  pinned_route_unavailable: "The engine chosen for this step is not available",
  user_requested_route_unavailable: "The engine you chose is not available",
  portable_coordinator_unavailable: "No engine is available to coordinate this task",
  native_route_authorization_unavailable: "The engine for this step is not authorised",
  native_route_executable_unavailable: "The engine program for this step is not installed",
  native_task_route_unavailable: "The engine for this task is not available",
  engine_authentication_required: "The engine needs you to sign in",
  engine_quota_exhausted: "The engine's usage limit has been reached",
  engine_unavailable: "The engine is not available",
  engine_launch_failed: "The engine could not start",
  engine_launch_retrying: "Starting the engine again",
  connection_unavailable: "The connection is not available",
  lost_worker_termination_unconfirmed: "A worker process was lost and its end could not be confirmed",
  lost_worker_action_outcome_unknown: "Whether an action of a lost worker finished is not known",
  native_answer_delivery_unconfirmed: "Whether your answer reached the task could not be confirmed",
  native_guidance_delivery_unknown: "Whether your message reached the worker is not known",
  verification_failed: "The final verification failed",
  verification_review_revision_changed: "The project changed after the review, so the result has to be reviewed and verified again",
  project_revision_unavailable: "The current state of the project could not be read",
  unsafe_storage: "The task's saved records are not stored safely, so they are not used",
  completion_unresolved_worker_check: "A worker's failed check was never resolved, so the work could not be confirmed as complete",
  completion_dropped_failed_check: "A failed check was left out of the final result, so the work could not be confirmed as complete",
  completion_worker_check_unverified: "A worker's check could not be verified, so the work could not be confirmed as complete",
  coordinator_prompt_failed: "The coordinator's engine failed while it was working",
  cancelled_worker_stopping: "Waiting for a cancelled worker to stop. The coordinator cancelled it to change the plan",
  check_preflight_unavailable: "A check could not be prepared",
  check_blocker_required: "A check marked not run gave no reason",
  not_run: "Not run",
  unverified: "Unverified",
  not_started: "Not started",
  waiting_for_files: "Waiting for another task to release files",
  automatic_continuation_unavailable: "Automatic continuation is unavailable",
  workspace_unobservable: "The workspace could not be read",
  task_paused: "Task paused",
  task_stopped: "Task stopped",
  task_stopped_for_question: "The task stopped to ask you a question",
  task_not_paused: "The task is not paused",
  task_not_resumable: "The task cannot be resumed",
  task_archived: "Restore the task first.",
  task_not_closed: "Only a closed task can be returned for rework.",
  stale_epoch: "The task owner changed. Refresh it before sending.",
  rework_pending: "A task returned for rework is not closed yet.",
  rework_target_unavailable: "The task’s target branch is unavailable.",
  rework_target_not_checked_out: "Check out the task’s target branch in the project folder first.",
  rework_target_changed: "The target branch changed. Refresh before sending.",
  native_execution_binding_invalid: "Could not confirm which execution to reopen.",
  task_not_stoppable: "The task cannot be stopped",
  user_input: "Waiting for your answer",
  revision_conflict: "Another change was made first; read the latest and try again",
  stale_version: "The page is out of date; read the latest and try again",
  partial_effect: "Only part of this finished",
  project_folder_missing: "The project folder is missing",
  worktree_git_required: "A separate worktree needs a Git repository. Initialize Git or choose Project folder.",
  worktree_head_unborn: "A separate worktree needs a commit. Make the first commit or choose Project folder.",
  worktree_dirty: "The worktree contains uncommitted, untracked, or ignored files.",
  worktree_cleanup_unconfirmed: "Refresh and inspect the worktree before attempting cleanup again.",
  unsafe_worktree_root: "Choose a safe worktree location outside registered project folders in machine settings.",
  contradictory_evidence: "The recorded evidence contradicts itself",
};

/** The phrase used when a code stands inside a sentence. */
const inSentence = (code: string) => {
  const text = STATUS_TEXT[code];
  return text ? text[0].toLowerCase() + text.slice(1) : code.replace(/_+/g, " ");
};

/** Whether a string is a bare internal code (snake_case, no spaces). */
export const isBareCode = (text: string) => BARE_CODE.test(text.trim());

/** Prose keeps its words. A snake_case token inside it reads as what it means (or, if unknown, as spaced words). */
export function scrubCodes(text: string): string {
  return text.replace(EMBEDDED_CODE, word => inSentence(word));
}

/** `code:detail`, such as `coordinator_prompt_failed:RuntimeError` or `cancelled_worker_stopping:qa-engineer`: a code with a
 *  machine detail (an error class, the roles it names), never with spaces. */
const DETAILED = /^([A-Za-z][A-Za-z0-9]*(?:_[A-Za-z0-9]+)+):(\S+)$/;
const names = (list: string[]) => list.length < 2 ? list.join("") : `${list.slice(0, -1).join(", ")} and ${list.at(-1)}`;
/** A detailed code in words: the roles a replan waits for are named; any other detail (an error class) is the code's own
 *  sentence, which never shows the detail. Null when it is not a detailed code the table knows. */
function detailed(text: string): string | null {
  const match = DETAILED.exec(text);
  if (!match) return null;
  const code = match[1].toLowerCase();
  if (code === "cancelled_worker_stopping") {
    const roles = match[2].split(",").map(r => r.trim()).filter(Boolean);
    return roles.length ? `Waiting for ${names(roles)} to stop. The coordinator cancelled ${roles.length === 1 ? "it" : "them"} to change the plan` : STATUS_TEXT[code];
  }
  return Object.hasOwn(STATUS_TEXT, code) ? STATUS_TEXT[code] : null;
}

/** A recorded reason for a person. A known code is its sentence; an unknown bare code is `GENERIC_STATUS` (or `fallback`
 *  when one is given); prose has any embedded code spelled out. The result never contains an underscore token. */
export function plainReason(value: string | null | undefined, fallback = ""): string {
  const text = (value ?? "").trim();
  if (!text) return fallback;
  if (Object.hasOwn(STATUS_TEXT, text)) return STATUS_TEXT[text];
  const coded = detailed(text);
  if (coded) return coded;
  if (BARE_CODE.test(text) || /^[a-z]+:[a-z0-9_]+$/.test(text) || DETAILED.test(text)) return fallback || GENERIC_STATUS;
  return scrubCodes(text);
}

/** The same text for a place inside the task, where "Open the task for details" points nowhere. */
export const inTask = (text: string) => text.replace(/\.?\s*Open the task for details\.?$/, "");

/** A wait or stop reason: an empty one is a lost turn, otherwise as `plainReason`. */
export function plainStop(reason: string | null | undefined): string {
  const code = (reason ?? "").trim();
  if (!code) return STATUS_TEXT.turn_interrupted;
  if (Object.hasOwn(STATUS_TEXT, code)) return STATUS_TEXT[code];
  const coded = detailed(code);
  if (coded) return coded;
  if (code.startsWith("completion_")) return "The work could not be confirmed as complete";
  if (code.startsWith("lost_worker_")) return "A worker process was lost";
  return plainReason(code);
}

/** A phase the runtime observed, as a short word or phrase. */
export function plainPhase(phase: string): string {
  switch (phase) {
    case "result": return "Runtime result";
    case "waiting": return "Waiting for you";
    case "recovery": return "Recovery";
    default: return Object.hasOwn(STATUS_TEXT, phase) ? STATUS_TEXT[phase] : isBareCode(phase) ? "Working" : phase[0].toUpperCase() + phase.slice(1);
  }
}
