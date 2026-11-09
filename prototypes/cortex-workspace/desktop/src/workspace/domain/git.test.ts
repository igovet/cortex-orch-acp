import { describe, expect, it } from "vitest";
import { loadFixtures } from "../../fixtures";
import { createMockWorkspace } from "../adapter/mock";
import { WORKSPACE_CASES as C } from "../adapter/fixtures";
import { boardPhase, repoOf, taskCheckout, worktreeWork } from "./index";
import type { Task, WorkspaceCommand, WorkspacePort, WorkspaceState } from "./index";

const NOW = Date.UTC(2030, 0, 1, 3);
const port = () => createMockWorkspace(loadFixtures().workspace);
function send(p: WorkspacePort, command: WorkspaceCommand) {
  const receipt = p.dispatch(command);
  expect(receipt.ok, receipt.message).toBe(true);
  return receipt;
}
const task = (p: WorkspacePort, id: string) => p.snapshot().tasks.find(t => t.id === id)!;
function rerunOrdinaryChecks(p: WorkspacePort, id: string) {
  const t = task(p, id);
  t.result!.checks.forEach((c, checkIndex) => {
    if (c.outcome === "passed" || c.technicalReason) return;
    send(p, { type: "rerun_check", taskId: id, digest: t.result!.digest, checkIndex });
    send(p, { type: "finish_check", taskId: id, digest: t.result!.digest, checkIndex, outcome: "passed", detail: "Simulated pass, no command ran" });
  });
}
const checkoutOf = (s: WorkspaceState, t: Task) => taskCheckout(s, t)!;
const merge = (p: WorkspacePort, id: string, extra: Partial<Extract<WorkspaceCommand, { type: "integrate" }>> = {}) => {
  const t = task(p, id);
  return p.dispatch({ type: "integrate", taskId: id, method: "squash", confirmed: true, digest: t.acceptance?.digest ?? t.result!.digest, targetBranch: "main", now: NOW, ...extra });
};

describe("acceptance and overrides", () => {
  it("accepting open checks takes an explicit override, and keeps them on the record", () => {
    const p = port(), t = task(p, C.review);
    expect(p.dispatch({ type: "accept_result", taskId: C.review, digest: t.result!.digest }).ok).toBe(false);
    expect(p.dispatch({ type: "accept_result", taskId: C.review, digest: t.result!.digest, openChecks: true }).ok).toBe(false);
    rerunOrdinaryChecks(p, C.review);
    send(p, { type: "accept_result", taskId: C.review, digest: t.result!.digest, openChecks: true, now: NOW });
    expect(task(p, C.review)).toMatchObject({ status: "ready_to_merge", acceptance: { version: 2, open: { failed: 0, not_run: 2, unverified: 0 } } });
  });
  it("never overrides a stale version", () => {
    const p = port(), id = C.stale, t = task(p, id);
    expect(p.dispatch({ type: "accept_result", taskId: id, digest: t.result!.digest, openChecks: true }).ok).toBe(false);
  });
  it("request changes reopens a result and drops any acceptance", () => {
    const p = port();
    send(p, { type: "request_changes", taskId: C.merge, text: "Add an index", now: NOW });
    expect(task(p, C.merge)).toMatchObject({ status: "working", process: "live", acceptance: null });
    expect(p.snapshot().messages.some(m => m.taskId === C.merge && m.text === "Add an index" && m.delivery === "recorded")).toBe(true);
    expect(merge(p, C.merge).ok).toBe(false);
  });
  it("comments gather into one message tied to file and line", () => {
    const p = port();
    send(p, { type: "add_comment", taskId: C.review, path: "cedar/settings.py", line: 42, text: "Why 60?" });
    send(p, { type: "send_comments", taskId: C.review, now: NOW });
    const sent = p.snapshot().messages.filter(m => m.taskId === C.review);
    expect(sent).toHaveLength(1);
    expect(sent[0].text).toContain("2 comments on the diff:");
    expect(sent[0].text).toContain("cedar/settings.py:42 Why 60?");
    expect(p.snapshot().reviews.find(r => r.taskId === C.review)!.comments.every(c => c.sent)).toBe(true);
    expect(p.dispatch({ type: "send_comments", taskId: C.review }).ok).toBe(false);
  });
});

describe("integration", () => {
  it("merges only the confirmed target, and a changed target must be confirmed again", () => {
    const p = port();
    expect(merge(p, C.merge, { confirmed: false }).ok).toBe(false);
    send(p, { type: "set_target", taskId: C.merge, branch: "release/2.4" });
    expect(merge(p, C.merge).ok).toBe(false);
    expect(merge(p, C.merge, { targetBranch: "release/2.4" }).ok).toBe(true);
    expect(task(p, C.merge)).toMatchObject({ status: "done", work: "merged", integration: { method: "squash", targetBranch: "release/2.4" } });
  });
  it("deletes the worktree after merging only when nothing would be lost", () => {
    const p = port();
    rerunOrdinaryChecks(p, C.review);
    send(p, { type: "accept_result", taskId: C.review, digest: task(p, C.review).result!.digest, openChecks: true });
    expect(merge(p, C.review, { deleteWorktree: true }).ok).toBe(true);
    expect(taskCheckout(p.snapshot(), task(p, C.review))).toBeNull();
    const q = port(), id = C.merge, state = q.snapshot();
    expect(checkoutOf(state, task(q, id)).files).toHaveLength(0);
  });
  it("keeps a worktree with uncommitted files after a merge", () => {
    const seed = loadFixtures().workspace;
    const target = seed.tasks.find(t => t.id === C.merge)!;
    taskCheckout(seed, target)!.files.push({ path: "notes.md", status: "U", add: 1, del: 0, stage: "untracked", hunks: [] });
    const p = createMockWorkspace(seed);
    expect(merge(p, C.merge, { deleteWorktree: true }).ok).toBe(true);
    expect(taskCheckout(p.snapshot(), task(p, C.merge))).not.toBeNull();
  });
  it("a pull request leaves the work unmerged and cannot be opened twice", () => {
    const p = port();
    expect(merge(p, C.merge, { method: "pr", title: "T", draft: true }).ok).toBe(true);
    expect(task(p, C.merge)).toMatchObject({ status: "ready_to_merge", work: "unmerged", integration: { method: "pr", draft: true } });
    expect(merge(p, C.merge, { method: "pr" }).ok).toBe(false);
  });
});

describe("conflicts", () => {
  it("updating a stale branch can stop at a conflict, and aborting restores everything", () => {
    const p = port(), before = task(p, C.merge);
    send(p, { type: "update_from_target", taskId: C.merge, now: NOW });
    expect(task(p, C.merge)).toMatchObject({ conflict: "conflict", status: "needs_you", acceptance: null });
    expect(boardPhase(p.snapshot(), C.merge)).toBe("needs_you");
    send(p, { type: "abort_merge", taskId: C.merge, now: NOW });
    expect(task(p, C.merge)).toMatchObject({ status: "ready_to_merge", conflict: "none", merge: null, acceptance: before.acceptance, result: before.result });
  });
  it("finishing by hand needs every block, then returns through Ready to merge unaccepted", () => {
    const p = port(), blocks = task(p, C.rollups).merge!;
    expect(blocks.files[1].blocks[0].choice).toBe("ours");
    expect(p.dispatch({ type: "finish_merge", taskId: C.rollups }).ok).toBe(false);
    send(p, { type: "resolve_block", taskId: C.rollups, path: blocks.files[0].path, blockId: "load", choice: "both" });
    send(p, { type: "finish_merge", taskId: C.rollups, now: NOW });
    const t = task(p, C.rollups);
    expect(t).toMatchObject({ conflict: "none", status: "ready_to_merge", acceptance: null, resolvedBy: "you" });
    expect(t.result).toMatchObject({ version: 2, checks: [{ outcome: "not_run" }] });
    expect(merge(p, C.rollups).ok).toBe(false);
  });
  it("Cortex keeps the owner's choices, and a failed attempt changes nothing", () => {
    const p = port();
    send(p, { type: "resolve_with_cortex", taskId: C.rollups, guidance: "Keep tz", now: NOW });
    expect(task(p, C.rollups)).toMatchObject({ conflict: "resolving", status: "working", process: "live", merge: { guidance: "Keep tz" } });
    expect(boardPhase(p.snapshot(), C.rollups)).toBe("working");
    expect(p.dispatch({ type: "resolve_block", taskId: C.rollups, path: task(p, C.rollups).merge!.files[0].path, blockId: "load", choice: "ours" }).ok).toBe(false);
    send(p, { type: "finish_resolution", taskId: C.rollups, failed: true, now: NOW });
    const failed = task(p, C.rollups);
    expect(failed).toMatchObject({ conflict: "failed", status: "needs_you", process: "stopped" });
    expect(failed.merge!.failure).toContain("usage_rollups.py");
    expect(failed.merge!.files[0].blocks[0].choice).toBeNull();
    send(p, { type: "resolve_with_cortex", taskId: C.rollups, now: NOW });
    send(p, { type: "finish_resolution", taskId: C.rollups, failed: false, now: NOW });
    const done = task(p, C.rollups);
    expect(done.merge!.files.map(f => f.blocks[0].choice)).toEqual(["cortex", "ours"]);
    expect(done).toMatchObject({ conflict: "none", status: "ready_to_merge", acceptance: null, resolvedBy: "cortex", integration: null });
    expect(done.result!.checks.filter(c => c.outcome === "not_run")).toHaveLength(1);
  });
  it("Stop hands the conflict back, and Close waits for the merge to be aborted", () => {
    const p = port();
    expect(p.dispatch({ type: "close_task", taskId: C.rollups, choice: "as_is", confirmed: true, now: NOW }).ok).toBe(false);
    send(p, { type: "resolve_with_cortex", taskId: C.rollups, now: NOW });
    send(p, { type: "stop_task", taskId: C.rollups, confirmed: true, now: NOW });
    expect(task(p, C.rollups)).toMatchObject({ conflict: "conflict", status: "needs_you", process: "stopped" });
  });
  it("a task that never had a result can still leave a conflict", () => {
    const p = port();
    send(p, { type: "abort_merge", taskId: C.conflict, now: NOW });
    expect(task(p, C.conflict)).toMatchObject({ status: "paused", conflict: "none" });
    send(p, { type: "continue_task", taskId: C.conflict, now: NOW });
  });
});

describe("Git and worktrees", () => {
  const session = (p: WorkspacePort) => checkoutOf(p.snapshot(), task(p, C.session));
  it("stages, commits only what is staged, and refuses an empty commit", () => {
    const p = port(), id = session(p).id;
    expect(session(p).files.map(f => f.stage)).toEqual(["staged", "unstaged", "untracked"]);
    send(p, { type: "git_stage", checkoutId: id, path: "tests/auth/test_race.py", staged: true });
    expect(session(p).files[2]).toMatchObject({ status: "A", stage: "staged" });
    send(p, { type: "git_stage", checkoutId: id, path: "tests/auth/test_race.py", staged: false });
    expect(session(p).files[2]).toMatchObject({ status: "U", stage: "untracked" });
    expect(p.dispatch({ type: "git_commit", checkoutId: id, message: " " }).ok).toBe(false);
    send(p, { type: "git_commit", checkoutId: id, message: "Add the version column", now: NOW });
    expect(session(p).files.map(f => f.path)).toEqual(["auth/refresh.py", "tests/auth/test_race.py"]);
    expect(session(p).ahead).toBe(3);
    expect(repoOf(p.snapshot(), task(p, C.session).projectId)!.commits[0]).toMatchObject({ message: "Add the version column", files: 1, pushed: false });
    expect(p.dispatch({ type: "git_commit", checkoutId: id, message: "Nothing staged" }).ok).toBe(false);
  });
  it("reverting needs confirmation, and a whole hunk can go on its own", () => {
    const p = port(), id = session(p).id;
    expect(p.dispatch({ type: "git_revert", checkoutId: id, path: "auth/refresh.py", confirmed: false }).ok).toBe(false);
    send(p, { type: "git_revert", checkoutId: id, path: "auth/refresh.py", hunk: 0, confirmed: true });
    expect(session(p).files.some(f => f.path === "auth/refresh.py")).toBe(false);
  });
  it("an owner commit invalidates an acceptance of that worktree", () => {
    const seed = loadFixtures().workspace, target = seed.tasks.find(t => t.id === C.merge)!;
    taskCheckout(seed, target)!.files.push({ path: "notes.md", status: "M", add: 1, del: 0, stage: "staged", hunks: [] });
    const p = createMockWorkspace(seed);
    send(p, { type: "git_commit", checkoutId: checkoutOf(seed, target).id, message: "Owner edit", now: NOW });
    expect(task(p, C.merge)).toMatchObject({ status: "review", acceptance: null });
    expect(merge(p, C.merge).ok).toBe(false);
  });
  it("push confirms remote and branch; amending a pushed commit needs confirmation", () => {
    const p = port(), c = session(p), repo = () => repoOf(p.snapshot(), task(p, C.session).projectId)!;
    expect(p.dispatch({ type: "git_push", checkoutId: c.id, remote: "origin", branch: c.branch, confirmed: false }).ok).toBe(false);
    expect(p.dispatch({ type: "git_push", checkoutId: c.id, remote: "upstream", branch: c.branch, confirmed: true }).ok).toBe(false);
    send(p, { type: "git_push", checkoutId: c.id, remote: "origin", branch: c.branch, confirmed: true });
    expect(repo().commits.filter(x => x.branch === c.branch).every(x => x.pushed)).toBe(true);
    expect(p.dispatch({ type: "git_commit", checkoutId: c.id, message: "Reworded", amend: true }).ok).toBe(false);
    send(p, { type: "git_commit", checkoutId: c.id, message: "Reworded", amend: true, confirmed: true });
    expect(repo().commits[0].message).toBe("Reworded");
  });
  it("branch names are validated and unique", () => {
    const p = port(), projectId = task(p, C.session).projectId;
    expect(p.dispatch({ type: "create_branch", projectId, name: "bad name" }).ok).toBe(false);
    expect(p.dispatch({ type: "create_branch", projectId, name: "release/2.4" }).ok).toBe(false);
    send(p, { type: "create_branch", projectId, name: "topic/new", at: "6be10a4" });
    expect(repoOf(p.snapshot(), projectId)!.branches.at(-1)).toMatchObject({ name: "topic/new", where: "Local only, from 6be10a4" });
  });
  it("a worktree with unmerged work is deleted only by an explicit discard, and never while its task runs", () => {
    const p = port(), repo = () => repoOf(p.snapshot(), task(p, C.session).projectId)!;
    const orphan = repo().checkouts.find(c => !c.taskId && c.kind === "worktree")!;
    expect(worktreeWork(p.snapshot(), orphan)).toMatchObject({ task: null, files: 1, commits: 2 });
    expect(p.dispatch({ type: "delete_worktree", checkoutId: orphan.id, confirmed: true, discard: false }).ok).toBe(false);
    send(p, { type: "delete_worktree", checkoutId: orphan.id, confirmed: true, discard: true });
    expect(repo().checkouts.some(c => c.id === orphan.id)).toBe(false);
    const live = session(p);
    expect(p.dispatch({ type: "delete_worktree", checkoutId: live.id, confirmed: true, discard: true }).ok).toBe(false);
    const merged = checkoutOf(p.snapshot(), task(p, C.mergedWorktree));
    expect(worktreeWork(p.snapshot(), merged)).toMatchObject({ files: 0, commits: 0 });
    send(p, { type: "delete_worktree", checkoutId: merged.id, confirmed: true, discard: false });
    expect(repo().checkouts.some(c => c.id === merged.id)).toBe(false);
  });
  it("an orphaned worktree can become a draft task", () => {
    const p = port(), repo = () => repoOf(p.snapshot(), task(p, C.session).projectId)!;
    const orphan = repo().checkouts.find(c => !c.taskId && c.kind === "worktree")!;
    const id = send(p, { type: "attach_worktree", checkoutId: orphan.id }).entityId!;
    expect(task(p, id)).toMatchObject({ status: "draft", checkout: { branch: orphan.branch } });
    expect(repo().checkouts.find(c => c.id === orphan.id)!.taskId).toBe(id);
  });
  it("Git refuses to act on an offline machine", () => {
    const p = port(), projectId = task(p, C.session).projectId, machineId = p.snapshot().projects.find(x => x.id === projectId)!.machineId;
    send(p, { type: "set_connection", machineId, connection: "disconnected" });
    expect(p.dispatch({ type: "git_fetch", projectId, now: NOW }).ok).toBe(false);
  });
});
