import { describe, expect, it } from "vitest";
import { createMockWorkspace } from "../adapter/mock";
import { boardPhase, generateMockTitle, statusOfRuntime, weeklyUsage } from "./index";
import type { WorkspaceCommand, WorkspacePort } from "./index";

const DAY = 86400000, NOW = Date.UTC(2030, 0, 1);
function send(port: WorkspacePort, command: WorkspaceCommand) {
  const receipt = port.dispatch(command);
  expect(receipt.mock).toBe(true);
  expect(receipt.ok, receipt.message).toBe(true);
  return receipt;
}
function setup() {
  const port = createMockWorkspace();
  send(port, { type: "add_project", id: "project", machineId: "mock-machine-00000", name: "Synthetic", repository: "fictional/project", openBranch: "feature/open", defaultBranch: "trunk" });
  return port;
}
function task(port: WorkspacePort, extra = {}) {
  return send(port, { type: "create_task", projectId: "project", description: "Fix the synthetic race. Keep history.", checkout: { kind: "main" }, ...extra }).entityId!;
}
function ready(port: WorkspacePort, id: string) {
  send(port, { type: "start_task", taskId: id });
  send(port, { type: "publish_result", taskId: id, digest: "mock-v1", checks: [{ name: "Mock check", outcome: "passed", detail: "Simulation, no command run" }] });
  send(port, { type: "accept_result", taskId: id, digest: "mock-v1" });
}

describe("CW-D01 / CW-DM02: machine and adapter boundary", () => {
  it("boots one local process, never a machine per project; snapshots cannot mutate the adapter", () => {
    const p = setup();
    send(p, { type: "add_project", id: "other", machineId: "mock-machine-00000", name: "Other", repository: "fictional/other", openBranch: "topic", defaultBranch: "develop" });
    expect(p.snapshot().machines).toHaveLength(1);
    expect(p.snapshot().machines[0].name).toBe("This machine");
    expect(new Set(p.snapshot().projects.map(x => x.machineId)).size).toBe(1);
    const copy = p.snapshot(); copy.projects[0].name = "Changed";
    expect(p.snapshot().projects[0].name).toBe("Synthetic");
    const id = task(p);
    send(p, { type: "add_remote", id: "remote", name: "Synthetic remote" });
    expect(p.snapshot().machines).toHaveLength(2);
    expect(p.snapshot().machines[1].connection).toBe("disconnected");
    send(p, { type: "set_connection", machineId: "mock-machine-00000", connection: "disconnected" });
    expect(boardPhase(p.snapshot(), id)).toBe("unknown");
    expect(p.dispatch({ type: "start_task", taskId: id }).ok).toBe(false);
  });
  it("rejects invalid references atomically and preserves the snapshot", () => {
    const p = setup(), before = p.snapshot();
    expect(p.dispatch({ type: "create_task", projectId: "missing", description: "Test", checkout: { kind: "main" } }).ok).toBe(false);
    expect(p.snapshot()).toEqual(before);
  });
  it.each(["COMPLETED", "CANCELLED", "FAILED", "RECOVERING", "WAITING_USER", "EXECUTING", "FUTURE_STATE"])("maps only explicit runtime facts: %s", state => {
    const expected = ({ COMPLETED: "review", CANCELLED: "cancelled", FAILED: "failed", WAITING_USER: "needs_you", EXECUTING: "working" } as Record<string, string>)[state] ?? "unknown";
    expect(statusOfRuntime(state, "connected")).toBe(expected);
    expect(statusOfRuntime(state, "disconnected")).toBe("unknown");
  });
});

describe("CW-D06a: task, checkout, lifecycle and acceptance", () => {
  it("requires description, derives the open checkout and Git default target, and protects a manual title", () => {
    const p = setup();
    expect(p.dispatch({ type: "create_task", projectId: "project", description: "  ", checkout: { kind: "main" } }).ok).toBe(false);
    const id = task(p);
    expect(p.snapshot().tasks[0]).toMatchObject({ titleState: "pending", targetBranch: null, checkout: { kind: "main", branch: "feature/open" } });
    send(p, { type: "generate_title", taskId: id, fail: true });
    expect(p.snapshot().tasks[0].titleState).toBe("failed");
    send(p, { type: "rename_task", taskId: id, title: "My title" });
    send(p, { type: "generate_title", taskId: id });
    expect(p.snapshot().tasks[0].title).toBe("My title");
    expect(generateMockTitle("fix the race. Extra text")).toBe("Fix the race");
    ready(p, id);
    expect(boardPhase(p.snapshot(), id)).toBe("ready_to_merge");
    expect(p.dispatch({ type: "integrate", taskId: id, method: "squash", confirmed: false, digest: "mock-v1", targetBranch: "trunk", now: NOW }).ok).toBe(false);
    send(p, { type: "integrate", taskId: id, method: "squash", confirmed: true, digest: "mock-v1", targetBranch: "trunk", now: NOW });
    expect(p.snapshot().tasks[0].integration).toMatchObject({ method: "squash", targetBranch: "trunk" });
    expect(boardPhase(p.snapshot(), id)).toBe("done");
  });
  it("stops and continues without deleting work; an unknown stop blocks close and archive", () => {
    const p = setup(), id = task(p, { checkout: { kind: "worktree", branch: "cw/race", path: "worktrees/race" } });
    send(p, { type: "start_task", taskId: id });
    const checkout = p.snapshot().tasks[0].checkout;
    send(p, { type: "stop_task", taskId: id, confirmed: false });
    expect(p.snapshot().tasks[0].status).toBe("stopping");
    expect(p.dispatch({ type: "close_task", taskId: id, choice: "as_is", confirmed: true, now: NOW }).ok).toBe(false);
    send(p, { type: "confirm_stop", taskId: id });
    expect(p.snapshot().tasks[0].status).toBe("paused");
    send(p, { type: "continue_task", taskId: id });
    expect(p.snapshot().tasks[0].status).toBe("working");
    expect(p.snapshot().tasks[0].checkout).toEqual(checkout);
    send(p, { type: "close_task", taskId: id, choice: "as_is", confirmed: true, now: NOW });
    expect(p.snapshot().tasks[0]).toMatchObject({ status: "done", closedAs: "as_is", notVerifiedByCortex: true });
    expect(p.dispatch({ type: "continue_task", taskId: id }).ok).toBe(false);
    expect(p.snapshot().tasks[0].checkout).toEqual(checkout);
  });
  it("cancel requires confirmation, keeps work and withdraws pending messages", () => {
    const p = setup(), id = task(p);
    send(p, { type: "start_task", taskId: id });
    send(p, { type: "send_message", taskId: id, text: "Later", mode: "queue" });
    expect(p.dispatch({ type: "close_task", taskId: id, choice: "cancel", confirmed: false, now: NOW }).ok).toBe(false);
    send(p, { type: "close_task", taskId: id, choice: "cancel", confirmed: true, now: NOW });
    expect(p.snapshot().tasks[0]).toMatchObject({ closedAs: "cancel", work: "unmerged" });
    expect(p.snapshot().messages[0].delivery).toBe("withdrawn");
  });
  it("preserves all check outcomes; failed, missing and stale evidence cannot silently pass", () => {
    const p = setup(), id = task(p);
    send(p, { type: "start_task", taskId: id });
    send(p, { type: "publish_result", taskId: id, digest: "mock-v1", checks: [
      { name: "a", outcome: "passed", detail: "mock" }, { name: "b", outcome: "failed", detail: "mock" },
      { name: "c", outcome: "not_run", detail: "mock" }, { name: "d", outcome: "unverified", detail: "mock" },
    ] });
    expect(p.dispatch({ type: "accept_result", taskId: id, digest: "mock-v1" }).ok).toBe(false);
    send(p, { type: "publish_result", taskId: id, digest: "mock-v2", checks: [{ name: "mock", outcome: "passed", detail: "simulation" }] });
    send(p, { type: "accept_result", taskId: id, digest: "mock-v2" });
    send(p, { type: "workspace_changed", taskId: id, digest: "mock-v3" });
    expect(boardPhase(p.snapshot(), id)).toBe("review");
    expect(p.dispatch({ type: "integrate", taskId: id, method: "pr", confirmed: true, digest: "mock-v2", targetBranch: "trunk", now: NOW }).ok).toBe(false);
  });
  it("Cortex conflict resolution returns a fresh diff through review, never merges", () => {
    const p = setup(), id = task(p); ready(p, id);
    send(p, { type: "set_conflict", taskId: id });
    send(p, { type: "resolve_with_cortex", taskId: id });
    expect(p.snapshot().tasks[0].conflict).toBe("resolving");
    send(p, { type: "finish_resolution", taskId: id, failed: true });
    expect(p.snapshot().tasks[0].conflict).toBe("failed");
    send(p, { type: "resolve_with_cortex", taskId: id });
    send(p, { type: "finish_resolution", taskId: id, failed: false });
    expect(p.snapshot().tasks[0].result?.checks[0].outcome).toBe("not_run");
    expect(p.snapshot().tasks[0].integration).toBeNull();
    // It returns through Ready to merge with no acceptance: merging stays blocked until the owner accepts that version.
    expect(boardPhase(p.snapshot(), id)).toBe("ready_to_merge");
    expect(p.snapshot().tasks[0].acceptance).toBeNull();
    expect(p.dispatch({ type: "integrate", taskId: id, method: "squash", confirmed: true, digest: p.snapshot().tasks[0].result!.digest, targetBranch: "trunk", now: NOW }).ok).toBe(false);
  });
  it("a PR is not a merge or permission to archive unmerged work", () => {
    const p = setup(), id = task(p); ready(p, id);
    send(p, { type: "integrate", taskId: id, method: "pr", confirmed: true, digest: "mock-v1", targetBranch: "trunk", now: NOW });
    expect(p.snapshot().tasks[0]).toMatchObject({ status: "ready_to_merge", work: "unmerged" });
    expect(p.dispatch({ type: "archive_task", taskId: id, now: NOW }).ok).toBe(false);
  });
});

describe("CW-D06b: sprint planning and dependency queue", () => {
  function plan(mode: "manual" | "automatic" = "automatic") {
    const p = setup();
    const id = send(p, { type: "create_sprint", projectId: "project", description: "Synthetic sprint", runMode: mode }).entityId!;
    send(p, { type: "plan_question", sprintId: id, question: "Which layout?" });
    expect(p.dispatch({ type: "publish_plan", sprintId: id, items: [] }).ok).toBe(false);
    send(p, { type: "answer_planning", sprintId: id, answer: "Compact" });
    const items = [{ key: "a", description: "First task", dependsOn: [] }, { key: "b", description: "Second task", dependsOn: ["a"] }];
    send(p, { type: "publish_plan", sprintId: id, items });
    send(p, { type: "materialize_plan", sprintId: id, version: 1 });
    send(p, { type: "materialize_plan", sprintId: id, version: 1 });
    expect(p.snapshot().tasks).toHaveLength(2);
    return { p, id, tasks: p.snapshot().tasks };
  }
  it("rejects missing, cyclic and self dependencies without changing the plan", () => {
    const p = setup(), id = send(p, { type: "create_sprint", projectId: "project", description: "Plan", runMode: "manual" }).entityId!;
    for (const items of [
      [{ key: "a", description: "A", dependsOn: ["missing"] }],
      [{ key: "a", description: "A", dependsOn: ["a"] }],
      [{ key: "a", description: "A", dependsOn: ["b"] }, { key: "b", description: "B", dependsOn: ["a"] }],
    ]) expect(p.dispatch({ type: "publish_plan", sprintId: id, items }).ok).toBe(false);
    expect(p.snapshot().sprints[0].plan).toBeNull();
  });
  it("manual starts no neighbor and blocks dependent tasks", () => {
    const { p, tasks } = plan("manual");
    expect(p.dispatch({ type: "start_task", taskId: tasks[1].id }).ok).toBe(false);
    send(p, { type: "start_task", taskId: tasks[0].id });
    expect(p.snapshot().tasks.map(t => t.status)).toEqual(["working", "draft"]);
  });
  it("automatic runs sequentially, pauses for a question, and never integrates", () => {
    const { p, id, tasks } = plan();
    send(p, { type: "run_sprint", sprintId: id });
    expect(p.snapshot().tasks.map(t => t.status)).toEqual(["working", "draft"]);
    send(p, { type: "task_issue", taskId: tasks[0].id, reason: "Question needs owner" });
    expect(p.snapshot().sprints[0].queueState).toBe("paused");
    send(p, { type: "clear_issue", taskId: tasks[0].id });
    send(p, { type: "publish_result", taskId: tasks[0].id, digest: "mock-a", checks: [{ name: "a", outcome: "passed", detail: "mock" }] });
    send(p, { type: "accept_result", taskId: tasks[0].id, digest: "mock-a" });
    send(p, { type: "run_sprint", sprintId: id });
    expect(p.snapshot().tasks.map(t => t.status)).toEqual(["ready_to_merge", "working"]);
    expect(p.snapshot().tasks.every(t => !t.integration)).toBe(true);
    send(p, { type: "pause_sprint", sprintId: id });
    expect(p.snapshot().tasks[1].status).toBe("working");
  });
  it("inherits the sprint target while preserving explicit task overrides", () => {
    const { p, id, tasks } = plan();
    send(p, { type: "set_target", sprintId: id, branch: "release" });
    send(p, { type: "set_target", taskId: tasks[1].id, branch: "hotfix" });
    ready(p, tasks[0].id);
    send(p, { type: "integrate", taskId: tasks[0].id, method: "squash", confirmed: true, digest: "mock-v1", targetBranch: "release", now: NOW });
    expect(p.snapshot().tasks[0].integration?.targetBranch).toBe("release");
    expect(p.snapshot().tasks[1].targetBranch).toBe("hotfix");
  });
});

describe("CW-D06c: archive and retention", () => {
  it("waits seven days in Done; asks about unmerged work; refusal stays outside archive", () => {
    const p = setup(), id = task(p);
    send(p, { type: "close_task", taskId: id, choice: "as_is", confirmed: true, now: NOW });
    send(p, { type: "archive_due", now: NOW + 7 * DAY - 1 });
    expect(p.snapshot().tasks[0].archive).toBeNull();
    send(p, { type: "archive_due", now: NOW + 7 * DAY });
    expect(p.snapshot().tasks[0].archiveQuestion).toBe(true);
    send(p, { type: "answer_archive", taskId: id, discard: false, now: NOW + 7 * DAY });
    expect(p.snapshot().tasks[0].archive).toBeNull();
    send(p, { type: "archive_due", now: NOW + 14 * DAY });
    send(p, { type: "answer_archive", taskId: id, discard: true, now: NOW + 14 * DAY });
    expect(p.snapshot().tasks[0].archive?.at).toBe(NOW + 14 * DAY);
    expect(p.snapshot().tasks[0].work).toBe("discard_confirmed");
  });
  it.each(["live", "unknown"] as const)("blocks purge for %s processes and unexpected changes; machine retention uses archive time", process => {
    const p = setup(), id = task(p); ready(p, id);
    send(p, { type: "integrate", taskId: id, method: "squash", confirmed: true, digest: "mock-v1", targetBranch: "trunk", now: NOW });
    send(p, { type: "archive_due", now: NOW + 7 * DAY });
    send(p, { type: "purge_due", now: NOW + 37 * DAY - 1 });
    expect(p.snapshot().tasks).toHaveLength(1);
    send(p, { type: "set_safety", taskId: id, process, unexpectedChanges: false });
    expect(p.dispatch({ type: "purge_task", taskId: id, confirmed: true, now: NOW + 37 * DAY }).ok).toBe(false);
    send(p, { type: "set_safety", taskId: id, process: "stopped", unexpectedChanges: true });
    send(p, { type: "purge_due", now: NOW + 37 * DAY });
    expect(p.snapshot().tasks).toHaveLength(1);
    expect(p.snapshot().tasks[0].purgeBlocker).toBeTruthy();
    send(p, { type: "set_safety", taskId: id, process: "stopped", unexpectedChanges: false });
    send(p, { type: "set_retention", machineId: "mock-machine-00000", days: 40 });
    send(p, { type: "purge_due", now: NOW + 37 * DAY });
    expect(p.snapshot().tasks).toHaveLength(1);
    send(p, { type: "purge_due", now: NOW + 47 * DAY });
    expect(p.snapshot().tasks).toHaveLength(0);
    expect(p.snapshot().purged[0].mock).toBe(true);
  });
  it("restore returns to Done and clears discard permission", () => {
    const p = setup(), id = task(p);
    send(p, { type: "close_task", taskId: id, choice: "cancel", confirmed: true, now: NOW });
    send(p, { type: "archive_task", taskId: id, now: NOW });
    send(p, { type: "answer_archive", taskId: id, discard: true, now: NOW });
    send(p, { type: "restore_task", taskId: id });
    expect(p.snapshot().tasks[0]).toMatchObject({ status: "done", archive: null, work: "unmerged" });
    expect(p.dispatch({ type: "set_retention", machineId: "mock-machine-00000", days: 0 }).ok).toBe(false);
  });
});

describe("CW-D02: task session, queue and delivery", () => {
  function running() {
    const p = setup(), id = task(p, { checkout: { kind: "worktree", branch: "cw/x", path: "worktrees/x" } });
    send(p, { type: "start_task", taskId: id, now: NOW });
    return { p, id, worker: p.snapshot().workers[0].id };
  }
  const message = (p: WorkspacePort, id: string) => p.snapshot().messages.find(m => m.id === id)!;
  it("changes settings on one task only, and refuses what this machine does not offer", () => {
    const p = setup(), a = task(p), b = task(p, { checkout: { kind: "worktree", branch: "cw/b", path: "worktrees/b" } });
    send(p, { type: "set_task_settings", taskId: a, settings: { model: "claude-sonnet-5", effort: "low", mode: "guarded", language: "ru" } });
    expect(p.snapshot().tasks[0].settings).toEqual({ model: "claude-sonnet-5", effort: "low", mode: "guarded", language: "ru" });
    expect(p.snapshot().tasks[1].settings.model).toBe("opus[1m]");
    expect(p.dispatch({ type: "set_task_settings", taskId: b, settings: { model: "local" } }).ok).toBe(false);
    expect(p.dispatch({ type: "set_task_settings", taskId: b, settings: { effort: "max" } }).ok).toBe(false);
  });
  it("keeps the queue editable and unsent; a sent message leaves it for good", () => {
    const { p, id } = running();
    const a = send(p, { type: "send_message", taskId: id, text: "First", mode: "queue" }).entityId!;
    const b = send(p, { type: "send_message", taskId: id, text: "Second", mode: "queue" }).entityId!;
    const c = send(p, { type: "send_message", taskId: id, text: "Third", mode: "queue" }).entityId!;
    const queued = () => p.snapshot().messages.filter(m => m.delivery === "queued").map(m => m.text);
    send(p, { type: "edit_queued", messageId: a, text: "First, edited" });
    send(p, { type: "move_queued", messageId: c, before: a });
    expect(queued()).toEqual(["Third", "First, edited", "Second"]);
    send(p, { type: "move_queued", messageId: c, before: null });
    expect(queued()).toEqual(["First, edited", "Second", "Third"]);
    expect(p.dispatch({ type: "edit_queued", messageId: a, text: "  " }).ok).toBe(false);
    send(p, { type: "send_queued", messageId: b, now: NOW });
    expect(message(p, b).delivery).toBe("recorded");
    expect(p.dispatch({ type: "edit_queued", messageId: b, text: "Too late" }).ok).toBe(false);
    expect(p.dispatch({ type: "remove_queued", messageId: b }).ok).toBe(false);
    send(p, { type: "remove_queued", messageId: c });
    expect(queued()).toEqual(["First, edited"]);
  });
  it("a queued message stays off the record until it is sent", () => {
    const { p, id } = running();
    const before = p.snapshot().sessions[0].timeline.length;
    const q = send(p, { type: "send_message", taskId: id, text: "Later", mode: "queue" }).entityId!;
    expect(p.snapshot().sessions[0].timeline.length).toBe(before);
    send(p, { type: "send_queued", messageId: q, now: NOW });
    expect(p.snapshot().sessions[0].timeline.at(-1)).toMatchObject({ kind: "message", messageId: q });
  });
  it("any message resumes a paused task only when asked to, and a stopping task takes none", () => {
    const { p, id } = running();
    send(p, { type: "stop_task", taskId: id, confirmed: true, now: NOW });
    expect(p.dispatch({ type: "send_message", taskId: id, text: "Hello", mode: "send_now" }).ok).toBe(false);
    send(p, { type: "send_message", taskId: id, text: "Hello", mode: "send_now", resume: true, now: NOW });
    expect(p.snapshot().tasks[0].status).toBe("working");
    send(p, { type: "stop_task", taskId: id, confirmed: false, now: NOW });
    expect(p.dispatch({ type: "send_message", taskId: id, text: "Hello", mode: "send_now", resume: true }).ok).toBe(false);
  });
  it("moves a direct message through its delivery states; only an undelivered one can be withdrawn", () => {
    const { p, id, worker } = running();
    const m = send(p, { type: "send_message", taskId: id, workerId: worker, text: "Direct", mode: "send_now", now: NOW }).entityId!;
    expect(p.snapshot().workers[0].activity.at(-1)).toMatchObject({ kind: "message", messageId: m });
    send(p, { type: "simulate_delivery", messageId: m, delivery: "recorded" });
    send(p, { type: "withdraw_message", messageId: m });
    expect(message(p, m).delivery).toBe("withdrawn");
    const d = send(p, { type: "send_message", taskId: id, workerId: worker, text: "Again", mode: "send_now" }).entityId!;
    send(p, { type: "simulate_delivery", messageId: d, delivery: "delivered", now: NOW });
    expect(message(p, d).deliveredAt).toBe(NOW);
    expect(p.dispatch({ type: "withdraw_message", messageId: d }).ok).toBe(false);
  });
  it("sends an undelivered message on to Cortex as a new steer", () => {
    const { p, id, worker } = running();
    const m = send(p, { type: "send_message", taskId: id, workerId: worker, text: "Direct", mode: "send_now" }).entityId!;
    expect(p.dispatch({ type: "redirect_message", messageId: m }).ok).toBe(false);
    send(p, { type: "simulate_delivery", messageId: m, delivery: "failed", failure: "Connection dropped" });
    expect(message(p, m).failure).toBe("Connection dropped");
    const copy = send(p, { type: "redirect_message", messageId: m, now: NOW }).entityId!;
    expect(message(p, m).delivery).toBe("withdrawn");
    expect(message(p, copy)).toMatchObject({ workerId: null, delivery: "recorded", text: "Direct" });
  });
  it("a question holds the task in Needs you until every open one is answered", () => {
    const { p, id, worker } = running();
    const a = send(p, { type: "ask_decision", taskId: id, workerId: worker, text: "Which?", options: [{ label: "One" }], now: NOW }).entityId!;
    const b = send(p, { type: "ask_decision", taskId: id, workerId: null, text: "And?", options: [], now: NOW }).entityId!;
    expect(p.snapshot().tasks[0].status).toBe("needs_you");
    expect(p.snapshot().workers[0].status).toBe("waiting_you");
    send(p, { type: "answer_decision", taskId: id, decisionId: a, answer: "One", now: NOW });
    expect(p.snapshot().tasks[0].status).toBe("needs_you");
    expect(p.snapshot().workers[0].status).toBe("working");
    expect(p.dispatch({ type: "answer_decision", taskId: id, decisionId: a, answer: "Again" }).ok).toBe(false);
    send(p, { type: "answer_decision", taskId: id, decisionId: b, answer: "Fine" });
    expect(p.snapshot().tasks[0]).toMatchObject({ status: "working", issue: null });
  });
  it("a paused task cannot take an answer, and its question stays open", () => {
    const { p, id, worker } = running();
    const q = send(p, { type: "ask_decision", taskId: id, workerId: worker, text: "Which?", options: [], now: NOW }).entityId!;
    send(p, { type: "stop_task", taskId: id, confirmed: true, now: NOW });
    expect(p.dispatch({ type: "answer_decision", taskId: id, decisionId: q, answer: "One" }).ok).toBe(false);
    expect(p.snapshot().sessions[0].timeline.at(-1)).toMatchObject({ kind: "line", text: "You stopped the task" });
    send(p, { type: "continue_task", taskId: id, now: NOW });
    expect(p.snapshot().tasks[0].status).toBe("needs_you");
    expect(p.snapshot().sessions[0].decisions[0].answer).toBeNull();
  });
  it("elapsed time runs only while a worker runs, and a stop cuts its running command", () => {
    const { p, id, worker } = running();
    send(p, { type: "worker_activity", workerId: worker, text: "pytest", entry: { kind: "command", command: { state: "running", exit: null, took: null, tail: "", reason: null } } });
    send(p, { type: "stop_task", taskId: id, confirmed: true, now: NOW + 5000 });
    expect(p.snapshot().workers[0]).toMatchObject({ status: "paused", elapsedMs: 5000, runningSince: null });
    expect(p.snapshot().workers[0].activity.at(-1)!.command!.state).toBe("stopped");
    send(p, { type: "continue_task", taskId: id, now: NOW + 9000 });
    expect(p.snapshot().workers[0].runningSince).toBe(NOW + 9000);
    send(p, { type: "close_task", taskId: id, choice: "cancel", confirmed: true, now: NOW + 10000 });
    expect(p.snapshot().workers[0]).toMatchObject({ status: "cancelled", elapsedMs: 6000 });
  });
  it("closing withdraws open questions and undelivered messages, and writes the outcome", () => {
    const { p, id, worker } = running();
    send(p, { type: "ask_decision", taskId: id, workerId: null, text: "Which?", options: [], now: NOW });
    const pending = send(p, { type: "send_message", taskId: id, workerId: worker, text: "Direct", mode: "send_now" }).entityId!;
    send(p, { type: "simulate_delivery", messageId: pending, delivery: "recorded" });
    const steer = send(p, { type: "send_message", taskId: id, text: "Steer", mode: "send_now" }).entityId!;
    send(p, { type: "close_task", taskId: id, choice: "as_is", confirmed: true, now: NOW });
    expect(p.snapshot().sessions[0].decisions[0].withdrawn).toBe(true);
    expect(message(p, pending).delivery).toBe("withdrawn");
    expect(message(p, steer).delivery).toBe("recorded");
    expect(p.snapshot().sessions[0].timeline.at(-1)).toMatchObject({ kind: "line", text: "You closed the task as is, not verified", at: NOW });
    expect(p.dispatch({ type: "close_task", taskId: id, choice: "cancel", confirmed: true, now: NOW }).ok).toBe(false);
  });
  it("stopping one command or one worker never stops the task", () => {
    const { p, id, worker } = running();
    send(p, { type: "worker_activity", workerId: worker, text: "pytest", entry: { kind: "command", command: { state: "running", exit: null, took: null, tail: "", reason: null } } });
    const sequence = p.snapshot().workers[0].activity.at(-1)!.sequence;
    expect(p.dispatch({ type: "stop_command", workerId: worker, sequence, reason: " " }).ok).toBe(false);
    send(p, { type: "stop_command", workerId: worker, sequence, reason: "Hangs" });
    expect(p.dispatch({ type: "stop_command", workerId: worker, sequence, reason: "Twice" }).ok).toBe(false);
    send(p, { type: "settle_command", workerId: worker, sequence, state: "stopped", exit: null });
    expect(p.snapshot().tasks[0].status).toBe("working");
    expect(p.snapshot().workers[0].status).toBe("working");
    const m = send(p, { type: "send_message", taskId: id, workerId: worker, text: "Direct", mode: "send_now" }).entityId!;
    send(p, { type: "stop_worker", workerId: worker, reason: "Wrong path", now: NOW });
    expect(p.snapshot().workers[0].status).toBe("cancelled");
    expect(p.snapshot().tasks[0].status).toBe("working");
    expect(message(p, m)).toMatchObject({ delivery: "failed", failure: "Not delivered: the worker was stopped first" });
    expect(p.dispatch({ type: "stop_worker", workerId: worker }).ok).toBe(false);
  });
});

describe("CW-D07: worker, message and usage contracts", () => {
  it("keeps steer, queue and direct worker delivery distinct and simulated", () => {
    const p = setup(), id = task(p);
    send(p, { type: "start_task", taskId: id });
    const worker = p.snapshot().workers[0];
    send(p, { type: "send_message", taskId: id, text: "Steer", mode: "send_now" });
    send(p, { type: "send_message", taskId: id, text: "Queue", mode: "queue" });
    const message = send(p, { type: "send_message", taskId: id, workerId: worker.id, text: "Direct", mode: "send_now" }).entityId!;
    expect(p.snapshot().messages.map(m => m.delivery)).toEqual(["recorded", "queued", "sending"]);
    send(p, { type: "simulate_delivery", messageId: message, delivery: "delivered" });
    expect(p.dispatch({ type: "simulate_delivery", messageId: message, delivery: "sending" }).ok).toBe(false);
    send(p, { type: "flush_queue", taskId: id });
    expect(p.snapshot().messages[1].delivery).toBe("recorded");
    send(p, { type: "worker_activity", workerId: worker.id, text: "Inspecting synthetic code" });
    send(p, { type: "finish_worker", workerId: worker.id, outcome: "completed", summary: "Synthetic result", checks: [{ name: "Manual", outcome: "unverified", detail: "not observed" }] });
    expect(p.snapshot().workers[0]).toMatchObject({ status: "completed", result: { checks: [{ outcome: "unverified" }] } });
    expect(p.snapshot().workers[0].activity.length).toBeGreaterThan(0);
    expect(p.dispatch({ type: "send_message", taskId: id, workerId: worker.id, text: "Too late", mode: "send_now" }).ok).toBe(false);
  });
  it("automatic success advances before acceptance, and cancellation still pauses the queue", () => {
    const p = setup();
    const sprintId = send(p, { type: "create_sprint", projectId: "project", description: "Automatic", runMode: "automatic" }).entityId!;
    send(p, { type: "publish_plan", sprintId, items: [
      { key: "a", description: "First", dependsOn: [] }, { key: "b", description: "Second", dependsOn: ["a"] },
    ] });
    send(p, { type: "materialize_plan", sprintId, version: 1 });
    const [first, second] = p.snapshot().tasks;
    send(p, { type: "run_sprint", sprintId });
    send(p, { type: "publish_result", taskId: first.id, digest: "mock-a", checks: [{ name: "a", outcome: "passed", detail: "mock" }] });
    expect(p.snapshot().tasks[0].acceptance).toBeNull();
    expect(p.snapshot().tasks[1].status).toBe("working");
    send(p, { type: "accept_result", taskId: first.id, digest: "mock-a" });
    expect(p.snapshot().tasks[1].status).toBe("working");
    expect(p.snapshot().tasks[0].status).toBe("ready_to_merge");
    send(p, { type: "close_task", taskId: second.id, choice: "cancel", confirmed: true, now: NOW });
    expect(p.dispatch({ type: "run_sprint", sprintId }).ok).toBe(false);
  });
  it("rejects shared-checkout concurrent writers and malformed worktree paths", () => {
    const p = setup(), first = task(p), second = task(p);
    send(p, { type: "start_task", taskId: first });
    expect(p.dispatch({ type: "start_task", taskId: second }).ok).toBe(false);
    expect(p.dispatch({ type: "create_task", projectId: "project", description: "Bad worktree", checkout: { kind: "worktree", path: "../outside", branch: "cw/test" } }).ok).toBe(false);
  });
  it("unknown results and active workers cannot be accepted by stale or unsolicited actions", () => {
    const p = setup(), id = task(p);
    send(p, { type: "start_task", taskId: id });
    send(p, { type: "stop_task", taskId: id, confirmed: false });
    expect(p.dispatch({ type: "publish_result", taskId: id, digest: "late", checks: [{ name: "late", outcome: "passed", detail: "mock" }] }).ok).toBe(false);
    expect(p.dispatch({ type: "archive_task", taskId: id, now: NOW }).ok).toBe(false);
  });
  it("shows only Codex and Claude weekly usage; unknown is not zero", () => {
    const rows = weeklyUsage([
      { engine: "codex", connected: true, usedPercent: 43, resetAt: "2030-01-08T00:00:00Z", observedAt: "2030-01-01T00:00:00Z" },
      { engine: "claude", connected: true, usedPercent: null, resetAt: null, observedAt: null },
      { engine: "opencode", connected: true, usedPercent: 10, resetAt: null, observedAt: null },
    ]);
    expect(rows.map(r => r.engine)).toEqual(["codex", "claude"]);
    expect(rows[1].usedPercent).toBeNull();
    expect(rows.every(r => r.window === "weekly")).toBe(true);
    expect(weeklyUsage([{ engine: "codex", connected: true, usedPercent: 101, resetAt: null, observedAt: null }])[0].usedPercent).toBeNull();
  });
});
