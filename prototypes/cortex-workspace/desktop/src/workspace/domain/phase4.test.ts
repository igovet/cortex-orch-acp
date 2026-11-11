import { describe, expect, it } from "vitest";
import { createMockWorkspace } from "../adapter/mock";
import { DAY, dependencyProblem, previewClear, previewPurge, runBlocker, runOrder } from "./index";
import type { PlanItem, WorkspaceCommand, WorkspacePort } from "./index";

const NOW = Date.UTC(2030, 0, 1), LOCAL = "mock-machine-00000";
function send(port: WorkspacePort, command: WorkspaceCommand) {
  const receipt = port.dispatch(command);
  expect(receipt.ok, receipt.message).toBe(true);
  return receipt;
}
const refuse = (port: WorkspacePort, command: WorkspaceCommand) => { const r = port.dispatch(command); expect(r.ok).toBe(false); return r.message; };
function setup() {
  const port = createMockWorkspace();
  send(port, { type: "add_project", id: "project", machineId: LOCAL, name: "Synthetic", repository: "fictional/project", openBranch: "feature/open", defaultBranch: "trunk" });
  return port;
}
const ITEMS: PlanItem[] = [
  { key: "a", title: "First", description: "First task", dependsOn: [] },
  { key: "b", title: "Second", description: "Second task", dependsOn: ["a"] },
  { key: "c", title: "Third", description: "Third task", dependsOn: ["a"] },
  { key: "d", title: "Fourth", description: "Fourth task", dependsOn: ["b", "c"] },
];
function sprint(mode: "manual" | "automatic", items = ITEMS) {
  const p = setup(), id = send(p, { type: "create_sprint", projectId: "project", description: "Synthetic sprint", runMode: mode }).entityId!;
  send(p, { type: "publish_plan", sprintId: id, items });
  send(p, { type: "materialize_plan", sprintId: id, version: 1 });
  return { p, id, ids: p.snapshot().tasks.map(t => t.id) };
}
const statuses = (p: WorkspacePort) => p.snapshot().tasks.map(t => t.status);
const queue = (p: WorkspacePort) => p.snapshot().sprints[0];
const passed = (p: WorkspacePort, id: string, digest: string) => {
  send(p, { type: "publish_result", taskId: id, digest, checks: [{ name: "Mock", outcome: "passed", detail: "mock" }] });
  send(p, { type: "accept_result", taskId: id, digest });
};

it("does not invent contained-work closure in the simulator", () => {
  const p = setup(), taskId = send(p, { type: "create_task", projectId: "project", description: "Local fixture", checkout: { kind: "main" } }).entityId!;
  expect(refuse(p, { type: "close_task", taskId, choice: "already_in_target", confirmed: true, now: NOW })).toContain("real service only");
  expect(p.snapshot().tasks[0].status).toBe("draft");
});

describe("sprint planning conversation", () => {
  it.each(["main", "worktree"] as const)("keeps the sprint-wide %s choice in the demo and creates only drafts", kind => {
    const p = setup(), id = send(p, { type: "create_sprint", projectId: "project", description: "Plan it", runMode: "manual", checkout: { kind } }).entityId!;
    expect(queue(p).checkout).toEqual({ kind });
    expect(p.snapshot().tasks).toHaveLength(0);
    send(p, { type: "publish_plan", sprintId: id, items: ITEMS });
    send(p, { type: "materialize_plan", sprintId: id, version: 1 });
    expect(p.snapshot().tasks).toHaveLength(4);
    expect(p.snapshot().tasks.every(t => t.checkout.kind === kind && t.status === "draft" && t.executionId === null)).toBe(true);
    if (kind === "worktree") expect(new Set(p.snapshot().tasks.map(t => t.checkout.kind === "worktree" && t.checkout.path)).size).toBe(4);
  });
  it("keeps questions, answers and notes, and runs no task while planning", () => {
    const p = setup(), id = send(p, { type: "create_sprint", projectId: "project", description: "Plan it", runMode: "manual" }).entityId!;
    send(p, { type: "plan_note", sprintId: id, text: "I read the code" });
    send(p, { type: "plan_question", sprintId: id, question: "Which one?", options: ["A", "B"], now: NOW });
    expect(queue(p).planning).toBe("question");
    send(p, { type: "answer_planning", sprintId: id, answer: "A\nand a second line" });
    expect(queue(p).conversation.map(e => e.kind)).toEqual(["note", "question", "answer"]);
    expect(queue(p).conversation[1].options).toEqual(["A", "B"]);
    expect(queue(p).conversation[2].text).toContain("\n");
    expect(p.snapshot().tasks).toHaveLength(0);
  });
  it("edits the plan into a new version and keeps the old one; a no-change edit makes no version", () => {
    const p = setup(), id = send(p, { type: "create_sprint", projectId: "project", description: "Plan it", runMode: "manual" }).entityId!;
    send(p, { type: "publish_plan", sprintId: id, items: ITEMS });
    send(p, { type: "edit_plan", sprintId: id, items: ITEMS });
    expect(queue(p).plan!.version).toBe(1);
    send(p, { type: "edit_plan", sprintId: id, items: [{ ...ITEMS[0], title: "Renamed" }, ...ITEMS.slice(1)] });
    expect(queue(p).plan!.version).toBe(2);
    expect(queue(p).history!.map(h => h.version)).toEqual([1]);
    expect(queue(p).plan!.items[0].title).toBe("Renamed");
    refuse(p, { type: "edit_plan", sprintId: id, items: [{ ...ITEMS[0], dependsOn: ["d"] }, ...ITEMS.slice(1)] });
    expect(queue(p).plan!.version).toBe(2);
  });
  it("explains a dependency it will not apply, in plan numbers", () => {
    expect(dependencyProblem(ITEMS, "b", "d")).toBe("Not applied: 2 after 4 makes a cycle, because 4 already waits for 2.");
    expect(dependencyProblem(ITEMS, "a", "a")).toContain("cannot wait for itself");
    expect(dependencyProblem(ITEMS, "a", "z")).toContain("no task");
    expect(dependencyProblem(ITEMS, "c", "b")).toBeNull();
    expect(runOrder(ITEMS)).toEqual(["a", "b", "c", "d"]);
  });
  it("asking for changes reopens a ready plan, and a question blocks creating tasks", () => {
    const p = setup(), id = send(p, { type: "create_sprint", projectId: "project", description: "Plan it", runMode: "manual" }).entityId!;
    send(p, { type: "publish_plan", sprintId: id, items: ITEMS });
    send(p, { type: "plan_message", sprintId: id, text: "Split the second task" });
    expect(queue(p).planning).toBe("planning");
    send(p, { type: "publish_plan", sprintId: id, items: ITEMS });
    send(p, { type: "plan_question", sprintId: id, question: "One more?" });
    refuse(p, { type: "materialize_plan", sprintId: id, version: 2 });
    expect(p.snapshot().tasks).toHaveLength(0);
  });
  it("cancelling keeps the draft and answers; resuming returns to where it was", () => {
    const p = setup(), id = send(p, { type: "create_sprint", projectId: "project", description: "Plan it", runMode: "manual" }).entityId!;
    send(p, { type: "plan_question", sprintId: id, question: "Which?" });
    send(p, { type: "cancel_planning", sprintId: id });
    expect(queue(p)).toMatchObject({ planning: "cancelled", description: "Plan it" });
    refuse(p, { type: "plan_message", sprintId: id, text: "Hello" });
    send(p, { type: "resume_planning", sprintId: id });
    expect(queue(p).planning).toBe("question");
  });
  it("creates titled, linked tasks once, whatever the number of clicks", () => {
    const { p, id } = sprint("manual");
    send(p, { type: "materialize_plan", sprintId: id, version: 1 });
    const tasks = p.snapshot().tasks;
    expect(tasks.map(t => t.title)).toEqual(["First", "Second", "Third", "Fourth"]);
    expect(tasks[3].dependencies).toEqual([tasks[1].id, tasks[2].id]);
    refuse(p, { type: "plan_message", sprintId: id, text: "Too late" });
  });
});

describe("owner automatic rules", () => {
  it("advances passing results without acceptance, never merging", () => {
    const { p, id, ids } = sprint("automatic");
    send(p, { type: "run_sprint", sprintId: id });
    send(p, { type: "publish_result", taskId: ids[0], digest: "auto", checks: [{ name: "Check", outcome: "passed", detail: "Mock" }] });
    expect(statuses(p).slice(0, 2)).toEqual(["ready_to_merge", "working"]);
    expect(p.snapshot().tasks[0].acceptance).toBeNull();
    refuse(p, { type: "integrate", taskId: ids[0], digest: "auto", targetBranch: "trunk", method: "squash", confirmed: true, now: NOW });
  });
  it("cannot publish past an unanswered question or a stop", () => {
    const { p, id, ids } = sprint("automatic");
    send(p, { type: "run_sprint", sprintId: id });
    send(p, { type: "ask_decision", taskId: ids[0], workerId: null, text: "Choose", options: [], now: NOW });
    const publish = { type: "publish_result" as const, taskId: ids[0], digest: "question", checks: [{ name: "Check", outcome: "passed" as const, detail: "Mock" }] };
    refuse(p, publish);
    expect(statuses(p).slice(0, 2)).toEqual(["needs_you", "draft"]);
    const decisionId = p.snapshot().sessions[0].decisions[0].id;
    send(p, { type: "answer_decision", taskId: ids[0], decisionId, answer: "A", now: NOW });
    send(p, { type: "stop_task", taskId: ids[0], confirmed: true, now: NOW });
    refuse(p, publish);
    expect(statuses(p).slice(0, 2)).toEqual(["paused", "draft"]);
  });
  it("runs skipped checks automatically and allows only technical overrides", () => {
    const { p, id, ids } = sprint("automatic");
    send(p, { type: "run_sprint", sprintId: id });
    send(p, { type: "publish_result", taskId: ids[0], digest: "skip", checks: [{ name: "Check", outcome: "not_run", detail: "Skipped by system" }] });
    expect(p.snapshot().tasks[0].result!.checks[0]).toMatchObject({ outcome: "not_run", running: true });
    refuse(p, { type: "accept_result", taskId: ids[0], digest: "skip", openChecks: true });
    refuse(p, { type: "finish_check", taskId: ids[0], digest: "stale", checkIndex: 0, outcome: "passed", detail: "Mock" });
    send(p, { type: "finish_check", taskId: ids[0], digest: "skip", checkIndex: 0, outcome: "failed", detail: "Mock assertion failed" });
    refuse(p, { type: "accept_result", taskId: ids[0], digest: "skip", openChecks: true });
    send(p, { type: "rerun_check", taskId: ids[0], digest: "skip", checkIndex: 0 });
    send(p, { type: "finish_check", taskId: ids[0], digest: "skip", checkIndex: 0, outcome: "not_run", detail: "Cannot start", technicalReason: "Mock database unavailable" });
    refuse(p, { type: "accept_result", taskId: ids[0], digest: "skip" });
    send(p, { type: "accept_result", taskId: ids[0], digest: "skip", openChecks: true });
    expect(p.snapshot().tasks[0].acceptance!.open!.not_run).toBe(1);
    expect(statuses(p)[1]).toBe("draft");
    send(p, { type: "rerun_check", taskId: ids[0], digest: "skip", checkIndex: 0 });
    expect(p.snapshot().tasks[0].acceptance).toBeNull();
    send(p, { type: "finish_check", taskId: ids[0], digest: "skip", checkIndex: 0, outcome: "passed", detail: "Mock pass" });
    expect(p.snapshot().tasks[0].result!.checks[0].technicalReason).toBeUndefined();
  });
});

describe("sprint run modes", () => {
  it("manual: starting a task never starts its neighbours, and a dependent stays locked", () => {
    const { p, id, ids } = sprint("manual");
    expect(runBlocker(p.snapshot(), queue(p))).toBe("Select Automatic to run the queue");
    refuse(p, { type: "run_sprint", sprintId: id });
    refuse(p, { type: "start_task", taskId: ids[1] });
    send(p, { type: "start_task", taskId: ids[0] });
    expect(statuses(p)).toEqual(["working", "draft", "draft", "draft"]);
    passed(p, ids[0], "mock-a");
    expect(statuses(p)).toEqual(["ready_to_merge", "draft", "draft", "draft"]);
    send(p, { type: "start_task", taskId: ids[2] });
    expect(statuses(p)).toEqual(["ready_to_merge", "draft", "working", "draft"]);
    refuse(p, { type: "start_task", taskId: ids[1] });
  });
  it("automatic: one task at a time in dependency order, and every task stops at Ready to merge", () => {
    const { p, id, ids } = sprint("automatic");
    send(p, { type: "run_sprint", sprintId: id });
    for (const [n, digest] of ["a", "b", "c", "d"].entries()) {
      expect(p.snapshot().tasks.filter(t => t.status === "working").map(t => t.id)).toEqual([ids[n]]);
      passed(p, ids[n], `mock-${digest}`);
    }
    expect(statuses(p)).toEqual(Array(4).fill("ready_to_merge"));
    expect(queue(p).queueState).toBe("complete");
    expect(p.snapshot().tasks.every(t => !t.integration && t.work === "unmerged")).toBe(true);
  });
  it("automatic: a question, a failed check or a conflict pauses the queue with the reason, and it continues after", () => {
    const { p, id, ids } = sprint("automatic");
    send(p, { type: "run_sprint", sprintId: id });
    send(p, { type: "ask_decision", taskId: ids[0], workerId: null, text: "Which?", options: [], now: NOW });
    expect(queue(p)).toMatchObject({ queueState: "paused", pauseReason: "Question waiting" });
    expect(statuses(p)[1]).toBe("draft");
    expect(runBlocker(p.snapshot(), queue(p))).toContain("needs you first");
    const decision = p.snapshot().sessions[0].decisions[0].id;
    send(p, { type: "answer_decision", taskId: ids[0], decisionId: decision, answer: "That", now: NOW });
    send(p, { type: "run_sprint", sprintId: id });
    expect(queue(p).queueState).toBe("running");
    send(p, { type: "publish_result", taskId: ids[0], digest: "mock-a", checks: [{ name: "Mock", outcome: "failed", detail: "mock", technicalReason: "Mock runner unavailable" }] });
    expect(queue(p)).toMatchObject({ queueState: "paused", pauseReason: "Result has failed, not run or unverified checks" });
    expect(runBlocker(p.snapshot(), queue(p))).toBe("Review First first");
    send(p, { type: "accept_result", taskId: ids[0], digest: "mock-a", openChecks: true });
    expect(statuses(p).slice(0, 2)).toEqual(["ready_to_merge", "draft"]);
    send(p, { type: "run_sprint", sprintId: id });
    expect(statuses(p)[1]).toBe("working");
    send(p, { type: "set_conflict", taskId: ids[1] });
    expect(queue(p)).toMatchObject({ queueState: "paused", pauseReason: "Merge conflict" });
  });
  it("pausing the queue starts nothing new and leaves the active task running", () => {
    const { p, id, ids } = sprint("automatic");
    send(p, { type: "run_sprint", sprintId: id });
    send(p, { type: "pause_sprint", sprintId: id });
    expect(statuses(p)[0]).toBe("working");
    passed(p, ids[0], "mock-a");
    expect(statuses(p).slice(1)).toEqual(["draft", "draft", "draft"]);
    send(p, { type: "run_sprint", sprintId: id });
    expect(statuses(p)[1]).toBe("working");
  });
  it("switching mode never touches running work, and a running queue waits for an explicit run", () => {
    const { p, id } = sprint("automatic");
    send(p, { type: "set_run_mode", sprintId: id, runMode: "manual" });
    expect(queue(p).queueState).toBe("idle");
    send(p, { type: "set_run_mode", sprintId: id, runMode: "automatic" });
    send(p, { type: "run_sprint", sprintId: id });
    send(p, { type: "set_run_mode", sprintId: id, runMode: "manual" });
    expect(queue(p).queueState).toBe("paused");
    expect(statuses(p)[0]).toBe("working");
  });
});

describe("archive rules", () => {
  function merged(p: WorkspacePort, description = "Merged work") {
    const id = send(p, { type: "create_task", projectId: "project", description, checkout: { kind: "main" } }).entityId!;
    send(p, { type: "start_task", taskId: id });
    send(p, { type: "publish_result", taskId: id, digest: `d-${id}`, checks: [{ name: "Mock", outcome: "passed", detail: "mock" }] });
    send(p, { type: "accept_result", taskId: id, digest: `d-${id}` });
    send(p, { type: "integrate", taskId: id, method: "squash", confirmed: true, digest: `d-${id}`, targetBranch: "trunk", now: NOW });
    return id;
  }
  it("unmerged work: delete only on confirmation, otherwise it stays out of the Archive and asks again later", () => {
    const p = setup(), id = send(p, { type: "create_task", projectId: "project", description: "Spike", checkout: { kind: "main" } }).entityId!;
    send(p, { type: "close_task", taskId: id, choice: "as_is", confirmed: true, now: NOW });
    send(p, { type: "archive_task", taskId: id, now: NOW });
    expect(p.snapshot().tasks[0]).toMatchObject({ archive: null, archiveQuestion: true });
    send(p, { type: "answer_archive", taskId: id, discard: false, now: NOW });
    expect(p.snapshot().tasks[0]).toMatchObject({ archive: null, archiveQuestion: false, work: "unmerged", status: "done" });
    expect(p.snapshot().tasks[0].archiveAskAfter).toBe(NOW + 7 * DAY);
    send(p, { type: "archive_due", now: NOW + 7 * DAY - 1 });
    expect(p.snapshot().tasks[0].archiveQuestion).toBe(false);
    send(p, { type: "archive_due", now: NOW + 7 * DAY });
    expect(p.snapshot().tasks[0].archiveQuestion).toBe(true);
    send(p, { type: "answer_archive", taskId: id, discard: true, now: NOW + 7 * DAY });
    expect(p.snapshot().tasks[0].work).toBe("discard_confirmed");
    expect(p.snapshot().tasks[0].archive?.at).toBe(NOW + 7 * DAY);
  });
  it("auto archive follows the machine's own days, and restore restarts the clock", () => {
    const p = setup(), id = merged(p);
    send(p, { type: "save_storage", machineId: LOCAL, retentionDays: 10, autoArchiveDays: 3 });
    send(p, { type: "archive_due", now: NOW + 2 * DAY });
    expect(p.snapshot().tasks[0].archive).toBeNull();
    send(p, { type: "archive_due", now: NOW + 3 * DAY });
    expect(p.snapshot().tasks[0].archive).not.toBeNull();
    send(p, { type: "restore_task", taskId: id, now: NOW + 4 * DAY });
    expect(p.snapshot().tasks[0]).toMatchObject({ status: "done", archive: null, doneAt: NOW + 4 * DAY });
    send(p, { type: "archive_due", now: NOW + 5 * DAY });
    expect(p.snapshot().tasks[0].archive).toBeNull();
    refuse(p, { type: "save_storage", machineId: LOCAL, retentionDays: 0, autoArchiveDays: 3 });
  });
  it("a bulk delete previews what goes, skips what is blocked and deletes only the rest", () => {
    const p = setup(), a = merged(p, "A"), b = merged(p, "B");
    send(p, { type: "archive_task", taskId: a, now: NOW });
    send(p, { type: "archive_task", taskId: b, now: NOW });
    send(p, { type: "set_safety", taskId: b, process: "stopped", unexpectedChanges: true });
    const preview = previewPurge(p.snapshot(), [a, b]);
    expect(preview.deletable).toEqual([a]);
    expect(preview.skipped).toEqual([expect.objectContaining({ id: b, reason: "Unexpected workspace changes" })]);
    expect(preview.sizeKnown).toBe(false);
    refuse(p, { type: "purge_many", taskIds: [a, b], confirmed: false, now: NOW });
    send(p, { type: "purge_many", taskIds: [a, b], confirmed: true, now: NOW });
    expect(p.snapshot().tasks.map(t => t.id)).toEqual([b]);
    refuse(p, { type: "purge_many", taskIds: [b], confirmed: true, now: NOW });
  });
  it("a block on new files clears only on the owner's word; a live process cannot be waved through", () => {
    const p = setup(), a = merged(p);
    send(p, { type: "archive_task", taskId: a, now: NOW });
    send(p, { type: "set_safety", taskId: a, process: "unknown", unexpectedChanges: true });
    refuse(p, { type: "clear_purge_block", taskId: a, confirmed: true });
    send(p, { type: "set_safety", taskId: a, process: "stopped", unexpectedChanges: true });
    refuse(p, { type: "clear_purge_block", taskId: a, confirmed: false });
    send(p, { type: "clear_purge_block", taskId: a, confirmed: true });
    send(p, { type: "purge_task", taskId: a, confirmed: true, now: NOW });
    expect(p.snapshot().tasks).toHaveLength(0);
  });
  it("Run cleanup archives what is due, purges what is overdue and keeps a receipt for that machine", () => {
    const p = setup(), a = merged(p, "Overdue"), b = merged(p, "Blocked");
    send(p, { type: "archive_task", taskId: a, now: NOW });
    send(p, { type: "archive_task", taskId: b, now: NOW });
    send(p, { type: "set_safety", taskId: b, process: "live", unexpectedChanges: false });
    send(p, { type: "run_cleanup", machineId: LOCAL, now: NOW + 31 * DAY });
    expect(p.snapshot().tasks.map(t => t.id)).toEqual([b]);
    expect(p.snapshot().tasks[0].purgeBlocker).toBe("Live or unknown process");
    expect(p.snapshot().machines[0].cleanup).toMatchObject({ removed: 1, skipped: 1 });
  });
  it("Clear old tasks never touches unmerged work, and guards unknown processes unless forced", () => {
    const p = setup(), a = merged(p, "Old merged"), b = merged(p, "Old unknown"), c = send(p, { type: "create_task", projectId: "project", description: "Old spike", checkout: { kind: "main" } }).entityId!;
    send(p, { type: "close_task", taskId: c, choice: "cancel", confirmed: true, now: NOW });
    send(p, { type: "set_safety", taskId: b, process: "unknown", unexpectedChanges: false });
    const state = p.snapshot();
    expect(previewClear(state, LOCAL, NOW, false)).toEqual({ remove: [a], guarded: 1, kept: 1 });
    expect(previewClear(state, LOCAL, NOW, true)).toEqual({ remove: [a, b], guarded: 0, kept: 1 });
    expect(previewClear(state, LOCAL, NOW - 1, false).remove).toEqual([]);
    refuse(p, { type: "clear_old", machineId: LOCAL, cutoff: NOW, force: false, confirmed: false, now: NOW });
    send(p, { type: "clear_old", machineId: LOCAL, cutoff: NOW, force: false, confirmed: true, now: NOW });
    expect(p.snapshot().tasks.map(t => t.id).sort()).toEqual([b, c].sort());
  });
});

describe("machine settings", () => {
  it("offline settings are read-only, with nothing queued", () => {
    const p = setup();
    send(p, { type: "add_remote", id: "remote", name: "Remote" });
    const remote = () => p.snapshot().machines[1];
    expect(refuse(p, { type: "save_storage", machineId: "remote", retentionDays: 5, autoArchiveDays: 5 })).toContain("read-only");
    expect(refuse(p, { type: "run_cleanup", machineId: "remote", now: NOW })).toContain("offline");
  });
  it("pairs a remote only when the fingerprints match, and reconnecting admits possible gaps", () => {
    const p = setup();
    refuse(p, { type: "pair_machine", id: "m2", name: "GPU box", code: "7KQ2-M9XD", fingerprintsMatch: false });
    refuse(p, { type: "pair_machine", id: "m2", name: "GPU box", code: "nope", fingerprintsMatch: true });
    expect(p.snapshot().machines).toHaveLength(1);
    expect(send(p, { type: "pair_machine", id: "m2", name: "GPU box", code: "7KQ2-M9XD", fingerprintsMatch: true }).entityId).toBe("m2");
    expect(p.snapshot().machines[1]).toMatchObject({ kind: "remote", connection: "connected", historyGap: false });
    send(p, { type: "set_connection", machineId: "m2", connection: "disconnected", now: NOW });
    expect(p.snapshot().machines[1].lastConfirmedAt).toBe(new Date(NOW).toISOString());
    send(p, { type: "set_connection", machineId: "m2", connection: "connected" });
    expect(p.snapshot().machines[1].historyGap).toBe(true);
    refuse(p, { type: "remove_machine", machineId: LOCAL, confirmed: true });
    refuse(p, { type: "remove_machine", machineId: "m2", confirmed: false });
    send(p, { type: "remove_machine", machineId: "m2", confirmed: true });
    expect(p.snapshot().machines).toHaveLength(1);
  });
  it("keeps the worktree folder a full path", () => {
    const p = setup();
    send(p, { type: "set_worktree_root", machineId: LOCAL, path: "~/worktrees/" });
    expect(p.snapshot().machines[0].worktreeRoot).toBe("~/worktrees");
    refuse(p, { type: "set_worktree_root", machineId: LOCAL, path: "relative/path" });
    refuse(p, { type: "set_worktree_root", machineId: LOCAL, path: "~/a/../b" });
  });
});
