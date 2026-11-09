import type { Check, Commit, GitFile, MergeSeed, ProjectRepo, RepoCheckout, Task, WorkspaceCommand, WorkspaceState } from "./types";
import { checkoutOf, repoOf, taskCheckout, worktreeWork } from "@cortex/workspace-domain";
import { editable, entity, nextId, nonempty, online, pauseSprint, requireFact, taskEvent } from "./core";
import { targetBranch } from "@cortex/workspace-domain";
import { addLine } from "./timeline";

const owner = (state: WorkspaceState, checkout: RepoCheckout) => checkout.taskId ? state.tasks.find(t => t.id === checkout.taskId) ?? null : null;
function reachable(state: WorkspaceState, repo: ProjectRepo) {
  requireFact(entity(state.machines, entity(state.projects, repo.projectId).machineId).connection === "connected", "Machine offline; Git is unavailable");
}
/** The owner changed the files a result describes, so an acceptance of that result no longer holds. */
function touched(state: WorkspaceState, checkout: RepoCheckout, why: string, now: number | undefined) {
  const task = owner(state, checkout);
  if (!task?.result) return;
  task.workspaceDigest = `${task.workspaceDigest ?? task.result.digest}-${nextId(state, "edit")}`;
  if (task.acceptance) addLine(state, task, "alert", `${why} Acceptance of v${task.acceptance.version} no longer applies`, now);
  task.acceptance = null; task.resolvedBy = null;
  if (task.status === "ready_to_merge") task.status = "review";
}
function fileOf(checkout: RepoCheckout, path: string): GitFile {
  const file = checkout.files.find(f => f.path === path);
  requireFact(file, "That file has no changes in this checkout");
  return file;
}
function shortSha(state: WorkspaceState) { return (0xc0ffee0 + state.nextId++).toString(16).slice(-7); }
const plain = (x = 14): Commit["graph"] => ({ paths: [{ cls: "g1", d: `M${x} 0V34` }], dot: { cls: "d1", x } });
function addCommit(state: WorkspaceState, repo: ProjectRepo, input: Pick<Commit, "message" | "branch" | "taskId" | "files" | "add" | "del"> & { author?: string; pushed?: boolean }) {
  const commit: Commit = { sha: shortSha(state), author: input.author ?? "You", when: "Now", tags: [], pushed: input.pushed ?? false, graph: plain(), ...input };
  repo.commits.unshift(commit);
  return commit;
}

/** Starts a merge of the target into the task's branch. Nothing is stashed or reset, and what came before is kept for Abort. */
export function beginMerge(state: WorkspaceState, task: Task, seed: MergeSeed | null, now: number | null) {
  const source = targetBranch(state, task);
  task.merge = {
    files: structuredClone(seed?.files ?? []), clean: structuredClone(seed?.clean ?? []), checks: structuredClone(seed?.checks ?? []),
    source, into: task.checkout.branch, startedAt: now, handedAt: null, finishedAt: null, guidance: null, step: 0, failure: null, finished: false,
    before: { status: task.status, acceptance: task.acceptance, result: task.result, digest: task.workspaceDigest }, mock: true,
  };
  task.conflict = "conflict"; task.status = "needs_you"; task.acceptance = null; task.resolvedBy = null;
  pauseSprint(state, task, "Merge conflict");
}
const left = (task: Task) => (task.merge?.files ?? []).filter(f => f.blocks.some(b => !b.choice));
/** Stopping Cortex hands the conflict back as it was: the owner's own choices stay, and nothing was merged. */
export function stopResolution(state: WorkspaceState, task: Task, now: number | undefined) {
  requireFact(task.conflict === "resolving", "Nothing is being resolved");
  task.conflict = "conflict"; task.status = "needs_you"; task.process = "stopped";
  if (task.merge) { task.merge.step = 0; task.merge.handedAt = null; }
  addLine(state, task, "pencil", "You took the conflict back", now, true);
}
/** A resolved merge is a new version of the work, so it needs the owner's review like any other result. */
function resolved(state: WorkspaceState, task: Task, by: "cortex" | "you", checks: Check[], summary: string, now: number | undefined) {
  const version = (task.result?.version ?? 0) + 1;
  task.workspaceDigest = "mock-resolved-" + task.id + "-v" + version;
  task.result = { version, digest: task.workspaceDigest, summary, checks, mock: true };
  task.conflict = "none"; task.status = "ready_to_merge"; task.acceptance = null; task.resolvedBy = by; task.process = "stopped"; task.issue = null;
  if (task.merge) { task.merge.finished = true; task.merge.finishedAt = now ?? null; task.merge.failure = null; }
  const checkout = taskCheckout(state, task);
  if (checkout) checkout.behind = 0;
}

function handleConflict(state: WorkspaceState, task: Task, command: WorkspaceCommand) {
  switch (command.type) {
    case "set_conflict":
      requireFact(["review", "ready_to_merge", "working"].includes(task.status), "Conflict is not valid here");
      beginMerge(state, task, null, command.now ?? null); break;
    case "update_from_target": {
      requireFact(["review", "ready_to_merge"].includes(task.status), "Only a finished task can be updated from its target");
      const checkout = taskCheckout(state, task);
      requireFact(checkout && checkout.kind === "worktree", "No worktree to update");
      const target = targetBranch(state, task);
      if (checkout.updateConflict) {
        beginMerge(state, task, checkout.updateConflict, command.now ?? null);
        addLine(state, task, "alert", `Updating from ${target} stopped at a conflict. Nothing was stashed or reset`, command.now, true);
        break;
      }
      checkout.behind = 0; checkout.behindOf[target] = 0;
      touched(state, checkout, `You updated the branch from ${target}.`, command.now);
      break;
    }
    case "resolve_block": {
      requireFact(["conflict", "failed"].includes(task.conflict), "This conflict is being resolved, or it is already resolved");
      const block = task.merge?.files.find(f => f.path === command.path)?.blocks.find(b => b.id === command.blockId);
      requireFact(block, "Unknown conflict block");
      block.choice = command.choice; break;
    }
    case "finish_merge":
      requireFact(["conflict", "failed"].includes(task.conflict) && task.merge, "No merge to finish");
      requireFact(left(task).length === 0, "Resolve every block first");
      resolved(state, task, "you", [{ name: "Checks after your merge", outcome: "not_run", detail: "You finished the merge by hand, so no check has run on it", source: "runtime" }], "Merge finished by you. Review the diff before merging.", command.now);
      addLine(state, task, "check", `You finished merging ${task.merge!.source}. Result v${task.result!.version} waits for your review`, command.now, true); break;
    case "abort_merge": {
      const merge = task.merge;
      requireFact(merge, "No merge in progress");
      requireFact(!task.acceptance && !task.integration, "This merge was already accepted");
      task.result = merge.before.result; task.workspaceDigest = merge.before.digest; task.acceptance = merge.before.acceptance;
      // A task that never had a result goes back to a state it can be resumed from.
      task.status = merge.before.status === "needs_you" ? task.result ? "review" : "paused" : merge.before.status;
      task.resumeStatus = "working"; task.conflict = "none"; task.merge = null; task.process = "stopped"; task.resolvedBy = null; task.issue = null;
      addLine(state, task, "x", "You aborted the merge. The branch is back as it was", command.now, true); break;
    }
    case "resolve_with_cortex":
      requireFact(["conflict", "failed"].includes(task.conflict), "No unresolved conflict");
      task.conflict = "resolving"; task.status = "working"; task.process = "live";
      if (task.merge) { task.merge.handedAt = command.now ?? null; task.merge.guidance = command.guidance?.trim() || null; task.merge.step = 0; task.merge.failure = null; }
      addLine(state, task, "sparkles", "You handed the conflict over", command.now, true); break;
    case "advance_resolution":
      requireFact(task.conflict === "resolving" && task.merge, "Nothing is being resolved");
      task.merge.step = Math.min(2, task.merge.step + 1); break;
    case "stop_resolution": stopResolution(state, task, command.now); break;
    case "finish_resolution": {
      requireFact(task.conflict === "resolving", "Nothing is being resolved");
      task.process = "stopped";
      if (command.failed) {
        const file = left(task)[0]?.path.split("/").at(-1) ?? "the conflict";
        task.conflict = "failed"; task.status = "needs_you";
        if (task.merge) { task.merge.step = 0; task.merge.failure = `${file} could not be resolved and nothing changed. It needs a decision from you.`; }
        addLine(state, task, "alert", `Could not resolve ${file}`, command.now); break;
      }
      left(task).forEach(f => f.blocks.forEach(b => { if (!b.choice) b.choice = "cortex"; }));
      const checks = task.merge?.checks.length ? structuredClone(task.merge.checks) : [{ name: "Resolution checks", outcome: "not_run" as const, detail: "Simulation does not execute checks" }];
      resolved(state, task, "cortex", checks, "Mock resolved diff; owner review required", command.now);
      addLine(state, task, "check", `Conflict resolved. Result v${task.result!.version} waits for your review`, command.now, true); break;
    }
    default: return false;
  }
  taskEvent(state, task, command.type);
  return true;
}

function unstagedAll(file: GitFile) { return file.hunks.length > 0 && file.hunks.every(h => h.staged); }
function handleCheckout(state: WorkspaceState, command: Extract<WorkspaceCommand, { checkoutId: string }>) {
  const { repo, checkout } = checkoutOf(state, command.checkoutId);
  reachable(state, repo);
  switch (command.type) {
    case "git_stage": {
      const file = fileOf(checkout, command.path);
      if (command.hunk !== undefined) {
        const hunk = file.hunks[command.hunk];
        requireFact(hunk, "Unknown hunk");
        hunk.staged = command.staged;
        file.stage = unstagedAll(file) ? "staged" : file.status === "U" ? "untracked" : "unstaged";
      } else {
        file.hunks.forEach(h => { h.staged = command.staged; });
        file.stage = command.staged ? "staged" : file.status === "A" || file.status === "U" ? "untracked" : "unstaged";
      }
      if (file.stage === "staged" && file.status === "U") file.status = "A";
      if (file.stage === "untracked") file.status = "U";
      break;
    }
    case "git_revert": {
      requireFact(command.confirmed, "Owner confirmation required");
      const file = fileOf(checkout, command.path);
      if (command.hunk !== undefined) {
        const [hunk] = file.hunks.splice(command.hunk, 1);
        requireFact(hunk, "Unknown hunk");
        file.add -= hunk.lines.filter(l => l.kind === "add").length; file.del -= hunk.lines.filter(l => l.kind === "del").length;
      }
      if (command.hunk === undefined || file.hunks.length === 0) checkout.files = checkout.files.filter(f => f !== file);
      touched(state, checkout, `You reverted ${command.path}.`, command.now);
      break;
    }
    case "git_draft": checkout.draft = command.message; break;
    case "git_commit": {
      const message = nonempty(command.message, "Commit message");
      requireFact(!owner(state, checkout)?.merge || owner(state, checkout)?.conflict === "none", "Resolve the conflict first");
      const staged = checkout.files.filter(f => f.stage === "staged");
      if (command.amend) {
        const last = repo.commits.find(c => c.branch === checkout.branch);
        requireFact(last, "No commit to amend on this branch");
        requireFact(!last.pushed || command.confirmed, "That commit is already pushed. Confirm to rewrite it");
        last.message = message; last.pushed = false;
        last.files += staged.length; last.add += staged.reduce((n, f) => n + f.add, 0); last.del += staged.reduce((n, f) => n + f.del, 0);
      } else {
        requireFact(staged.length > 0, "Stage something first");
        addCommit(state, repo, { message, branch: checkout.branch, taskId: checkout.taskId, files: staged.length, add: staged.reduce((n, f) => n + f.add, 0), del: staged.reduce((n, f) => n + f.del, 0) });
        checkout.ahead += 1;
      }
      checkout.files = checkout.files.filter(f => f.stage !== "staged");
      checkout.draft = "";
      touched(state, checkout, "You committed in this checkout.", command.now);
      break;
    }
    case "git_pull":
      requireFact(!checkout.files.some(f => f.stage !== "untracked"), "Commit or revert your changes first. Pull never stashes on its own");
      requireFact(checkout.behind > 0, "Already up to date");
      checkout.behind = 0;
      touched(state, checkout, "You pulled new commits.", command.now);
      break;
    case "git_push":
      requireFact(command.confirmed, "Confirm the remote and branch first");
      requireFact(command.remote === repo.remote && command.branch === checkout.branch, "Remote or branch changed; confirm them again");
      repo.commits.filter(c => c.branch === checkout.branch).forEach(c => { c.pushed = true; });
      break;
    case "revert_commit": {
      requireFact(command.confirmed, "Owner confirmation required");
      const commit = repo.commits.find(c => c.sha === command.sha);
      requireFact(commit, "Unknown commit");
      addCommit(state, repo, { message: `Revert "${commit.message}"`, branch: checkout.branch, taskId: checkout.taskId, files: commit.files, add: commit.del, del: commit.add });
      checkout.ahead += 1;
      touched(state, checkout, `You reverted commit ${commit.sha}.`, command.now);
      break;
    }
    case "delete_worktree": {
      requireFact(command.confirmed, "Owner confirmation required");
      requireFact(checkout.kind === "worktree", "The main checkout cannot be deleted");
      const { task, files, commits } = worktreeWork(state, checkout);
      requireFact(!task || (task.status === "done" && task.process === "stopped"), "Close the task before deleting its worktree");
      // Never removes unmerged or uncommitted work without an explicit decision.
      requireFact(files + commits === 0 || command.discard, "This worktree holds unmerged work. Keep it, or discard it explicitly");
      if (task && task.work !== "merged") task.work = "discard_confirmed";
      repo.checkouts = repo.checkouts.filter(c => c !== checkout);
      if (task) addLine(state, task, "x", files + commits ? `You deleted the worktree and discarded ${files} files, ${commits} commits` : "You deleted the merged worktree", command.now);
      break;
    }
    case "attach_worktree": {
      requireFact(checkout.kind === "worktree" && !checkout.taskId, "Only an orphaned worktree can be attached");
      const id = nextId(state, "task"), project = entity(state.projects, checkout.projectId);
      const description = `Continue the work in ${checkout.branch}`;
      state.tasks.push({
        id, projectId: project.id, sprintId: null, description, title: description, titleState: "generated",
        checkout: { kind: "worktree", branch: checkout.branch, path: checkout.path }, targetBranch: null, dependencies: [],
        status: "draft", resumeStatus: "working", issue: null, executionId: null, result: null, workspaceDigest: null, acceptance: null,
        conflict: "none", merge: null, resolvedBy: null, integration: null, closedAs: null, notVerifiedByCortex: false, doneAt: null, archive: null,
        archiveQuestion: false, archiveAskAfter: null, work: "unmerged", process: "stopped", unexpectedChanges: false, purgeBlocker: null,
        settings: { model: "opus[1m]", effort: "high", mode: "development", language: "auto" }, mock: true,
      });
      checkout.taskId = id; checkout.label = null;
      return id;
    }
  }
  return undefined;
}

/** Returns true when the command was Git's, with the new entity id if it created one. */
export function handleGit(state: WorkspaceState, command: WorkspaceCommand): { id?: string } | null {
  if ("checkoutId" in command && command.type !== "open_editor") return { id: handleCheckout(state, command) };
  switch (command.type) {
    case "git_fetch": {
      const repo = repoOf(state, command.projectId);
      requireFact(repo, "No Git record for this project");
      reachable(state, repo); repo.fetchedAt = command.now; return {};
    }
    case "create_branch": {
      const repo = repoOf(state, command.projectId);
      requireFact(repo, "No Git record for this project");
      reachable(state, repo);
      const name = nonempty(command.name, "Branch name");
      requireFact(!/\s|\.\.|^[-/]|[/.]$/.test(name), "That is not a valid branch name");
      requireFact(!repo.branches.some(b => b.name === name) && !repo.checkouts.some(c => c.branch === name), "That branch already exists");
      repo.branches.push({ name, where: command.at ? `Local only, from ${command.at}` : "Local only", task: null, ahead: 0, behind: 0 });
      return { id: name };
    }
    default: break;
  }
  if (!("taskId" in command) || !command.taskId) return null;
  if (!["set_conflict", "update_from_target", "resolve_block", "finish_merge", "abort_merge", "resolve_with_cortex", "advance_resolution", "stop_resolution", "finish_resolution"].includes(command.type)) return null;
  const task = entity(state.tasks, command.taskId);
  editable(task); online(state, task);
  return handleConflict(state, task, command) ? {} : null;
}

/** After a squash merge: the target has the work, and the worktree goes only if nothing would be lost. */
export function afterMerge(state: WorkspaceState, task: Task, message: string | undefined, deleteWorktree: boolean | undefined, now: number) {
  const repo = repoOf(state, task.projectId), checkout = taskCheckout(state, task);
  if (repo) addCommit(state, repo, { message: message?.split("\n")[0] || task.title, branch: task.integration!.targetBranch, taskId: task.id, files: 0, add: 0, del: 0 });
  if (!repo || !checkout || !deleteWorktree || checkout.kind !== "worktree") return;
  if (checkout.files.length) addLine(state, task, "alert", `Worktree kept: ${checkout.files.length} uncommitted files`, now);
  else { repo.checkouts = repo.checkouts.filter(c => c !== checkout); addLine(state, task, "x", "You deleted the merged worktree", now); }
}
