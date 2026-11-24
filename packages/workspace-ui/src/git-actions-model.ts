import type { CheckoutOperation, ProjectRepo, RepoCheckout, Task, WorkspaceCommand } from "@cortex/workspace-domain";
import { behindTarget, finishBlocked, inConflict } from "./review-model";
import { sameBranch, type TaskView } from "./selectors";

/** Every action the title band's Git button knows, in the fixed order of its menu. */
export type GitItemId = "commit" | "fetch" | "pull" | "push" | "pull_push" | "resolve" | "continue" | "abort_pull" | "update" | "merge" | "pr" | "view_pr" | "archive";
export type GitItem = { id: GitItemId; label: string; icon: string; why?: string; sub?: string };
/** `null` id: nothing to do here, said with the reason. */
export type GitPrimary = { id: GitItemId | null; label: string; icon: string; why?: string; strong?: boolean };
export type GitFacts = {
  /** The checkout the button acts on: the task's own, or the project folder when no task is on view. */
  checkout: RepoCheckout | null; repo: ProjectRepo | null;
  /** The task on view. Without one, Merge, PR and Archive are left out. */
  task: TaskView | null;
  /** The task page's composer mode (sessionModel), which says whether its result is under review. */
  mode?: string;
  /** The action this button started and is still waiting for, and the step it is on when it has several. */
  running: GitItemId | null; progress?: string;
  /** Why the machine does not offer a command here, or nothing (ws.gate). */
  gate: (type: WorkspaceCommand["type"]) => string | undefined;
  /** Whether the machine offers a command at all; an operation it has no way to run is left out rather than shown off. */
  offered: (type: WorkspaceCommand["type"]) => boolean;
};

export const RUNNING = "Another Git action from this button is still running.";
export const LIVE_WRITER = "A task is working in this checkout.";
const NO_UPSTREAM = "No upstream branch";
const NOTHING_TO_PULL = "Nothing to pull in the last fetched state. Fetch to check the upstream.";
const NOTHING_TO_PUSH = "Nothing to push";
const NOTHING_TO_COMMIT = "Nothing to commit";
const MERGING = "A merge of this task is in progress. Resolve or abort it first.";
/** Local and remote both have commits the other lacks (last fetched state): a fast-forward is impossible and Cortex never force-pushes. */
export const DIVERGED = "The local and remote histories diverged. Choose Pull… to merge or rebase the remote commits, then push.";
export const PULL_FIRST = "The remote has commits you do not have. Pull first.";
/** A merge or rebase over uncommitted changes could not be aborted exactly, and nothing here stashes. */
export const DIRTY = "Commit your uncommitted changes first. A merge or rebase over them could not be aborted exactly, and nothing is stashed.";
export const DETACHED = "This checkout is not on a branch (detached HEAD)";
/** What every other Git action says while a pull is stopped in the checkout. */
export const STOPPED = "A pull is stopped in this checkout. Continue or abort it first.";

/**
 * Why a task cannot go to the Archive, or undefined: the machine archives only a task its own record closed, merged or
 * closed from this app (`task_lifecycle.rs`, "Only a finished task can go to the Archive."). A task the runtime alone ended,
 * Finish on its stop question for one, reads as done but has no such record: its runtime phase is still shown
 * (`overlayTask` clears it only for a recorded status).
 */
export function archiveBlocked(t: Pick<Task, "status" | "archive" | "observedPhase" | "warnings">): string | undefined {
  if (t.warnings?.includes("contradictory_evidence")) return "The records disagree. The task and its evidence must stay in place.";
  if (t.archive) return "Already in the Archive";
  if (t.status !== "done") return "Available when the task is done";
  return t.observedPhase === undefined ? undefined : "Only a task merged or closed in this app can go to the Archive. This one was ended by the runtime.";
}

/** The checkout has an upstream to pull from and compare with. Its counts are against the target branch otherwise. */
export function hasUpstream(repo: ProjectRepo | null, c: RepoCheckout): boolean {
  return c.upstream !== undefined ? !!c.upstream : !!repo?.remote.trim();
}

/** The ahead/behind words beside the branch: only against an upstream, where they mean something to pull or push. */
export function syncCounts(f: Pick<GitFacts, "repo" | "checkout">): { ahead: number; behind: number; files: number } | null {
  const c = f.checkout;
  if (!c) return null;
  const up = !!f.repo?.remote.trim() && hasUpstream(f.repo, c);
  return { ahead: up ? c.ahead : 0, behind: up ? c.behind : 0, files: c.files.length };
}

/** Uncommitted changes to tracked files. Untracked files do not count: Git refuses to overwrite one. */
export const trackedChanges = (c: RepoCheckout) => c.files.filter(f => f.stage !== "untracked").length;

/**
 * A merge or rebase stopped in the checkout that the Git screens own: a pull's, or one started in the terminal. A merge the
 * task's update or integration left there is the task's own, and its conflict view offers the ways out.
 */
export function stoppedPull(c: RepoCheckout | null, task: Pick<Task, "conflict"> | null | undefined): CheckoutOperation | null {
  const op = c?.operation ?? null;
  if (!op) return null;
  return task && inConflict(task as Task) ? null : op;
}
/** "merge" or "rebase", as the owner reads it in a label. */
export const operationWord = (op: CheckoutOperation) => op.kind === "rebase" ? "rebase" : "merge";

const integrationOpen = (f: GitFacts) => {
  const v = f.task, t = v?.task;
  if (!v || !t || v.project.hasGit === false) return false;
  if (t.status !== "ready_to_merge" || f.mode !== "review" || t.integrationFacts?.alreadyInTarget) return false;
  return !sameBranch(t, v.target) || !!t.integrationFacts?.commitAvailable;
};
const mergeLabel = (v: TaskView) => sameBranch(v.task, v.target) ? `Commit to ${v.target}…` : v.task.acceptance ? "Merge…" : "Review and merge…";
const notReady = (v: TaskView, mode: string | undefined) => v.task.status !== "ready_to_merge" || mode !== "review" ? "Result not accepted yet" : undefined;

/** Why the task's branch cannot be updated from its target now, or undefined. */
export function updateBlocked(c: RepoCheckout | null, v: TaskView): string | undefined {
  const t = v.task;
  if (inConflict(t)) return MERGING;
  if (c?.operation) return STOPPED;
  if (t.process === "live") return `${LIVE_WRITER} Stop it first.`;
  if (!["review", "ready_to_merge"].includes(t.status)) return "Available for a finished task";
  if (!t.result) return "Available once this task has a result";
  if (behindTarget(c, v.target) === 0) return `Up to date with ${v.target}`;
  if (c && trackedChanges(c)) return `The task's worktree has uncommitted changes. Commit them first; nothing is stashed.`;
  return undefined;
}

export const PR_UNKNOWN = "GitHub CLI did not report this pull request's address, so it was not recorded. Find it on GitHub.";

/** Only offer to open a recorded PR. Keep the missing-address reason even when another gate is closed. */
export function viewPrItem(task: Task, f: Pick<GitFacts, "offered" | "gate">): GitItem | undefined {
  if (task.integration?.method !== "pr" || !f.offered("open_pull_request")) return;
  return { id: "view_pr", label: "View PR", icon: "external", why: task.integration.url ? f.gate("open_pull_request") : PR_UNKNOWN };
}

/** The menu: always the same items in the same order, each on or off with its reason. Task-only items need a task on view. */
export function gitItems(f: GitFacts): GitItem[] {
  const c = f.checkout, v = f.task, t = v?.task;
  const busy = f.running ? RUNNING : undefined;
  const merging = t && inConflict(t) ? MERGING : undefined;
  const stopped = stoppedPull(c, t);
  const held = merging ?? (stopped ? STOPPED : undefined);
  const up = c ? hasUpstream(f.repo, c) : false;
  const remote = !!f.repo?.remote.trim();
  const boundMissing = !c && t?.checkout.kind === "worktree";
  const unknown = c ? undefined : boundMissing ? "Task worktree unavailable; restore the original checkout and refresh." : "Git state not read yet";
  const diverged = up && !!c && c.ahead > 0 && c.behind > 0;
  const detached = c && !c.branch ? DETACHED : undefined;
  // Bringing remote commits into a branch that has its own: a merge or rebase the owner chooses, never over uncommitted work.
  const reconcile = () => detached ?? (trackedChanges(c!) ? DIRTY : t?.process === "live" ? `${LIVE_WRITER} Stop it first.` : f.gate("git_pull"));
  const items: GitItem[] = [
    { id: "commit", label: "Commit…", icon: "commit", why: busy ?? unknown ?? held ?? (c!.files.length ? f.gate("git_commit") : NOTHING_TO_COMMIT) },
  ];
  // A repository without a remote is local-only: it is offered no network action at all, rather than dead ones.
  if (remote) items.push(
    { id: "fetch", label: "Fetch", icon: "refresh", why: busy ?? (boundMissing ? unknown : undefined) ?? f.gate("git_fetch") },
    { id: "pull", label: diverged ? "Pull…" : "Pull", icon: "download", why: busy ?? unknown ?? held ?? (!up ? NO_UPSTREAM : !c!.behind ? NOTHING_TO_PULL : diverged ? reconcile() : detached ?? f.gate("git_pull")) },
    { id: "push", label: "Push", icon: "upload", why: busy ?? unknown ?? held ?? (up && !c!.ahead ? NOTHING_TO_PUSH : diverged ? `${PULL_FIRST} ${DIVERGED}` : f.gate("git_push")) },
    { id: "pull_push", label: "Pull and push", icon: "refresh", why: busy ?? unknown ?? held ?? (!up ? NO_UPSTREAM : !c!.behind ? NOTHING_TO_PULL : !c!.ahead ? NOTHING_TO_PUSH : diverged ? DIVERGED : f.gate("git_pull") ?? f.gate("git_push")) },
  );
  // Only while a pull, or the owner's own merge or rebase, is stopped here: its way through and its way back.
  if (stopped) {
    const left = stopped.conflicts.length, word = operationWord(stopped);
    items.push(
      { id: "resolve", label: "Resolve conflicts", icon: "alert", why: busy ?? (left ? undefined : "No conflicts left. Continue the pull."), sub: left ? `${left} file${left === 1 ? "" : "s"} in conflict` : undefined },
      { id: "continue", label: `Continue ${word}`, icon: "check", why: busy ?? (left ? `Resolve ${left} conflicting file${left === 1 ? "" : "s"} and mark ${left === 1 ? "it" : "them"} resolved first` : f.gate("git_pull_continue")) },
      { id: "abort_pull", label: `Abort ${word}…`, icon: "x", why: busy ?? f.gate("git_pull_abort") },
    );
  }
  if (!v || !t) return items;
  const same = sameBranch(t, v.target);
  // Only where the machine has an operation that brings the target into the task's own branch, and only for a task with a worktree of its own.
  if (!same && t.checkout.kind === "worktree" && f.offered("update_from_target")) {
    items.push({ id: "update", label: `Update from ${v.target}…`, icon: "compare", why: busy ?? updateBlocked(c, v) ?? f.gate("update_from_target") });
  }
  if (v.project.hasGit !== false && !t.integrationFacts?.alreadyInTarget) {
    items.push({ id: "merge", label: same ? `Commit to ${v.target}…` : "Merge locally…", icon: "merge", why: busy ?? (boundMissing ? unknown : undefined) ?? notReady(v, f.mode) ?? (same && !t.integrationFacts?.commitAvailable ? t.integrationFacts?.reason ?? "Nothing of this task to commit" : finishBlocked(t) ?? f.gate("integrate")) });
    if (!same && remote) {
      const pr = t.integration?.method === "pr";
      items.push({ id: "pr", label: "Create PR…", icon: "pr", why: busy ?? (boundMissing ? unknown : undefined) ?? (pr ? "A pull request is already open for this version" : v.machine.ghAvailable === false ? "GitHub CLI not available" : notReady(v, f.mode) ?? finishBlocked(t) ?? f.gate("integrate")) });
    }
  }
  const viewPr = viewPrItem(t, f);
  if (viewPr) items.push(viewPr);
  items.push({ id: "archive", label: "Archive task", icon: "archive", why: busy ?? archiveBlocked(t) ?? f.gate("archive_task") });
  return items;
}

/** One action for the state the checkout and the task are in, chosen in this order; nothing to do is said, never guessed. */
export function gitPrimary(f: GitFacts, items = gitItems(f)): GitPrimary {
  const of = (id: GitItemId) => items.find(i => i.id === id)!;
  if (f.running) {
    const i = of(f.running);
    return { id: f.running, label: f.progress ?? PROGRESS[f.running] ?? i.label, icon: "loader", why: RUNNING };
  }
  const c = f.checkout, v = f.task;
  // A stopped pull comes first: until it is continued or aborted, nothing else of this checkout can run.
  const stopped = stoppedPull(c, v?.task);
  if (stopped) {
    if (stopped.conflicts.length) return { id: "resolve", label: "Resolve conflicts", icon: "alert", why: of("resolve").why, strong: true };
    const i = of("continue");
    return { id: "continue", label: i.label, icon: "check", why: i.why, strong: true };
  }
  // The task's own result decides first: in a shared folder its files are what the commit to the target takes.
  if (v && integrationOpen(f)) { const m = of("merge"); return { id: "merge", label: mergeLabel(v), icon: "merge", why: m.why, strong: true }; }
  if (!c) return { id: null, label: "Git", icon: "branch", why: v?.task.checkout.kind === "worktree" ? "Task worktree unavailable; restore the original checkout and refresh." : "Git state not read yet" };
  if (c.files.length) { const i = of("commit"); return { id: "commit", label: "Commit…", icon: "commit", why: i.why }; }
  const remote = !!f.repo?.remote.trim(), up = remote && hasUpstream(f.repo, c);
  // Diverged: Pull… asks how to bring the remote commits in; a push can follow once the branch has them.
  if (up && c.ahead && c.behind) { const i = of("pull"); return { id: "pull", label: "Pull…", icon: "download", why: i.why }; }
  if (up && c.behind) { const i = of("pull"); return { id: "pull", label: "Pull", icon: "download", why: i.why }; }
  if (up && c.ahead) { const i = of("push"); return { id: "push", label: "Push", icon: "upload", why: i.why }; }
  return { id: null, label: "Up to date", icon: "check", why: !remote ? `${NOTHING_TO_COMMIT}. This repository is local only.` : !up ? `${NOTHING_TO_COMMIT}. ${NO_UPSTREAM} to compare with.` : "Nothing to commit, pull or push in the last fetched state" };
}

const PROGRESS: Partial<Record<GitItemId, string>> = { commit: "Committing…", fetch: "Fetching…", pull: "Pulling…", push: "Pushing…", pull_push: "Pulling…", continue: "Continuing…", abort_pull: "Aborting…", update: "Updating…", merge: "Merging…" };
