import { checkAcceptanceBlocker } from "@cortex/workspace-domain";
import { repoOf, taskCheckout, worktreeWork } from "@cortex/workspace-domain";
import type { Check, ConflictFile, DiffHunk, DiffLine, FileDiff, MergeState, ProjectRepo, RepoCheckout, Task, TaskChanges, WorkspaceState } from "@cortex/workspace-domain";
import { checkCounts, type ProjectSummary, type TaskView, type WorkspaceIndex } from "./selectors";

/** Read models for review, merge, Code and Git. Every count on those screens comes from here; nothing here changes state. */
export const reviewFor = (state: WorkspaceState, taskId: string): TaskChanges | null => state.reviews.find(r => r.taskId === taskId) ?? null;
const baseName = (path: string) => path.split("/").at(-1) ?? path;
export const fileName = baseName;
export const dirName = (path: string) => path.split("/").slice(0, -1).join("/");

export function checksLine(checks: Check[]) {
  const c = checkCounts(checks);
  const parts = [c.passed ? `${c.passed} passed` : "", c.failed ? `${c.failed} failed` : "", c.not_run ? `${c.not_run} not run` : "", c.unverified ? `${c.unverified} unverified` : ""].filter(Boolean);
  return parts.length ? parts.join(", ") : checks.some(c => c.kind === "manual") ? "Check manually" : "No checks reported";
}
export const openChecks = (checks: Check[]) => checks.filter(c => c.kind !== "manual" && c.outcome !== "passed");
export const needsOverride = (checks: Check[]) => checks.length === 0 || openChecks(checks).length > 0;

export const stale = (task: Task) => !!task.result && (task.workspaceDigest !== task.result.digest || task.result.warnings?.includes("workspace_drift") === true);
/** Why the owner cannot accept this version now, or null when they can. */
export function acceptBlocked(task: Task): string | null {
  if (!task.result) return "Available once a result is published";
  if (stale(task)) return `This version no longer matches the worktree. Request a new result`;
  if (task.conflict !== "none") return "Resolve the conflict first";
  if (task.acceptance) return `You accepted v${task.acceptance.version}`;
  if (task.status === "done") return "This task is closed";
  if (task.reviewActions?.accept === false) return task.reviewActions.reason ?? "Refresh this result before accepting";
  if (task.result.acceptanceAvailable === false) return "Result evidence is unavailable. Request a new result";
  const answer = task.result.kind === "answer" && task.result.acceptanceAvailable === true && task.result.checks.length === 0;
  const checksBlocker = answer ? null : checkAcceptanceBlocker(task.result.checks);
  if (checksBlocker) return checksBlocker;
  return task.status === "review" || task.status === "ready_to_merge" ? null : "Available once this task finishes";
}
export function mergeBlocked(task: Task): string | null {
  if (task.status === "done") return task.work === "merged" ? "This task is already merged" : "This task is closed";
  if (task.conflict !== "none") return "Resolve the conflict first";
  if (!task.acceptance) return task.result ? `Accept v${task.result.version} first. Accepting is your decision, merging is a Git action.` : "Available once this task has an accepted result";
  return task.status === "ready_to_merge" ? null : "Available in Ready to merge";
}
/**
 * Why Review and merge cannot open for a task in Ready to merge, or null. Without an acceptance yet, the merge dialog
 * records it for exactly this version first, so the only blockers are the ones that would refuse that acceptance.
 */
export function finishBlocked(task: Task): string | null {
  if (task.integrationFacts?.alreadyInTarget) return `Already in ${task.integrationFacts.targetBranch}. Close the task instead.`;
  if (task.integrationFacts?.sameBranch) {
    if (!task.integrationFacts.commitAvailable) return task.integrationFacts.reason ?? "Task-owned files are not available for a commit. Close as is or finish outside the app.";
    if (task.conflict !== "none") return "Resolve the conflict first";
    return task.acceptance ? task.status === "ready_to_merge" ? null : "Available after review" : acceptBlocked(task);
  }
  if (task.acceptance) return mergeBlocked(task);
  if (task.status !== "ready_to_merge") return mergeBlocked(task);
  if (task.conflict !== "none") return "Resolve the conflict first";
  return acceptBlocked(task);
}

export type Cx = "none" | "running" | "failed" | "done";
/** Which of the approved conflict states this task is in. */
export const cxOf = (task: Task): Cx => task.conflict === "resolving" ? "running" : task.conflict === "failed" ? "failed" : task.conflict === "conflict" ? "none" : task.merge?.finished ? "done" : "none";
export const inConflict = (task: Task) => task.conflict !== "none";
export const blocksLeft = (files: ConflictFile[]) => files.reduce((n, f) => n + f.blocks.filter(b => !b.choice).length, 0);
export const fileResolved = (f: ConflictFile) => f.blocks.every(b => b.choice);
export function conflictFacts(merge: MergeState) {
  const left = blocksLeft(merge.files), open = merge.files.filter(f => !fileResolved(f));
  return { left, files: merge.files.length, resolved: merge.files.length - open.length, leftFiles: open.length, firstOpen: open[0]?.path ?? merge.files[0]?.path ?? null };
}
export const blockWord = (n: number) => `${n} block${n === 1 ? "" : "s"} left`;
export const conflictLine = (m: MergeState) => {
  const f = conflictFacts(m);
  return f.left ? `${blockWord(f.left)} in ${f.leftFiles} file${f.leftFiles === 1 ? "" : "s"}` : "None left";
};
export const RESOLVE_STEPS = (file: string) => ["Read both sides", `Resolving ${file}`, "Run the checks", "Ready to merge"];

export function targetOptions(state: WorkspaceState, view: TaskView) {
  const repo = repoOf(state, view.project.id), def = view.project.defaultBranch;
  const sprintTarget = view.sprint?.targetBranch ?? null;
  const out: { name: string; sub?: string }[] = [{ name: def, sub: "Git default branch" }];
  for (const b of repo?.branches ?? []) if (!out.some(o => o.name === b.name)) out.push({ name: b.name });
  if (view.sprint) { const name = sprintTarget ?? `sprint/${view.sprint.description.toLowerCase().replace(/\W+/g, "-")}`; if (!out.some(o => o.name === name)) out.push({ name, sub: "Branch of this sprint" }); }
  if (view.target && !out.some(o => o.name === view.target)) out.push({ name: view.target, sub: "Chosen for this task" });
  return out;
}
/** Behind the chosen target, or null when this checkout was never compared with it. */
export function behindTarget(checkout: RepoCheckout | null, target: string): number | null {
  if (!checkout) return null;
  if (target in checkout.behindOf) return checkout.behindOf[target];
  return target === checkout.base ? checkout.behind : null;
}

const changed = (d: DiffLine) => d.kind !== "ctx";
export const keptLines = (file: FileDiff) => file.hunks.reduce((n, h) => n + h.lines.filter(changed).length, 0);
/** A record may keep only part of a file's diff; the numbers above it still describe the whole change. */
export const partialDiff = (file: FileDiff) => keptLines(file) < file.add + file.del;
export const firstChanged = (file: FileDiff) => file.hunks.flatMap(h => h.lines).find(l => l.kind === "add")?.new ?? file.hunks.flatMap(h => h.lines).find(changed)?.old ?? 1;
export const addedLines = (file: FileDiff | undefined) => new Set((file?.hunks ?? []).flatMap(h => h.lines).filter(l => l.kind === "add").map(l => l.new!));

export type SplitRow = { left: DiffLine | null; right: DiffLine | null };
/** Pairs a run of removed lines with the run of added lines after it, so each side stays aligned. */
export function splitRows(hunk: DiffHunk): SplitRow[] {
  const rows: SplitRow[] = [];
  let dels: DiffLine[] = [], adds: DiffLine[] = [];
  const flush = () => {
    for (let i = 0; i < Math.max(dels.length, adds.length); i++) rows.push({ left: dels[i] ?? null, right: adds[i] ?? null });
    dels = []; adds = [];
  };
  for (const line of hunk.lines) {
    if (line.kind === "del") { if (adds.length) flush(); dels.push(line); }
    else if (line.kind === "add") adds.push(line);
    else { flush(); rows.push({ left: line, right: line }); }
  }
  flush();
  return rows;
}
export const withoutSpace = (hunk: DiffHunk): DiffHunk => ({ ...hunk, lines: hunk.lines.filter(l => l.kind === "ctx" || l.text.trim()) });

export type CheckoutFiles = { diffs: FileDiff[]; tree: string[] };
/** What the Code view can show for a checkout: its own changes, or the task's recorded diff when it has no working changes. */
export function checkoutFiles(state: WorkspaceState, repo: ProjectRepo, checkout: RepoCheckout): CheckoutFiles {
  const review = checkout.taskId ? reviewFor(state, checkout.taskId) : null;
  const diffs: FileDiff[] = checkout.files.length ? checkout.files : review?.files ?? [];
  return { diffs, tree: [...new Set([...repo.tree, ...diffs.map(d => d.path)])].sort() };
}
export function sourceOf(repo: ProjectRepo, checkout: RepoCheckout, diffs: FileDiff[], path: string): { lines: string[]; partial: boolean } | null {
  const text = checkout.sources[path] ?? repo.sources[path];
  if (text !== undefined) return { lines: text.replace(/\n$/, "").split("\n"), partial: false };
  // There is no full copy of this file, so show only the lines its diff recorded.
  const file = diffs.find(d => d.path === path);
  if (!file?.hunks.length) return null;
  const known = new Map(file.hunks.flatMap(h => h.lines).filter(l => l.new !== null).map(l => [l.new!, l.text]));
  const last = Math.max(...known.keys());
  return { lines: Array.from({ length: last }, (_, i) => known.get(i + 1) ?? ""), partial: true };
}
export type TreeRow = { kind: "dir" | "file"; path: string; name: string; depth: number };
export function treeRows(paths: string[], collapsed: Set<string>): TreeRow[] {
  const rows: TreeRow[] = [], seen = new Set<string>();
  for (const path of paths) {
    const parts = path.split("/");
    let hidden = false;
    for (let d = 0; d < parts.length - 1; d++) {
      const dir = parts.slice(0, d + 1).join("/");
      if (!hidden && !seen.has(dir)) { seen.add(dir); rows.push({ kind: "dir", path: dir, name: parts[d], depth: d }); }
      if (collapsed.has(dir)) hidden = true;
    }
    if (!hidden) rows.push({ kind: "file", path, name: parts.at(-1)!, depth: parts.length - 1 });
  }
  return rows;
}
export const allDirs = (paths: string[]) => [...new Set(paths.flatMap(p => { const parts = p.split("/"); return parts.slice(0, -1).map((_, i) => parts.slice(0, i + 1).join("/")); }))];

export type SearchHit = { line: number; text: string };
export function searchFiles(repo: ProjectRepo, checkout: RepoCheckout, diffs: FileDiff[], tree: string[], query: string, matchCase: boolean) {
  const needle = matchCase ? query : query.toLowerCase();
  if (!needle) return { files: [] as { path: string; hits: SearchHit[] }[], total: 0 };
  const files = tree.flatMap(path => {
    const hits = (sourceOf(repo, checkout, diffs, path)?.lines ?? []).flatMap((text, i) => (matchCase ? text : text.toLowerCase()).includes(needle) ? [{ line: i + 1, text }] : []);
    return hits.length ? [{ path, hits }] : [];
  });
  return { files, total: files.reduce((n, f) => n + f.hits.length, 0) };
}
export function splitMatch(text: string, query: string, matchCase: boolean) {
  const at = (matchCase ? text : text.toLowerCase()).indexOf(matchCase ? query : query.toLowerCase());
  return at < 0 ? { before: text, hit: "", after: "" } : { before: text.slice(0, at), hit: text.slice(at, at + query.length), after: text.slice(at + query.length) };
}

export type WorktreeState = { label: string; tone: string; icon: string; spin?: boolean };
/** Named from the task it serves. A worktree with no task is orphaned, and that is always said. */
export function worktreeState(state: WorkspaceState, checkout: RepoCheckout): WorktreeState {
  const { task } = worktreeWork(state, checkout);
  if (!task) return { label: "Orphaned", tone: "st-warn", icon: "alert" };
  if (task.conflict === "conflict" || task.conflict === "failed") return { label: "Conflict", tone: "st-bad", icon: "alert" };
  if (task.work === "merged") return { label: "Merged", tone: "st-merge", icon: "merge" };
  if (task.process === "live") return { label: "Active", tone: "st-run", icon: "loader", spin: true };
  return { label: "Idle", tone: "st-idle", icon: "eye" };
}
export type WorktreeRow = { key: string; checkout: RepoCheckout | null; branch: string; path: string; task: TaskView | null; label: string; ahead: number | null; behind: number | null; state: WorktreeState };
/** Worktrees of the project: Git records first, then live tasks whose worktree has no record yet. Their figures stay unknown. */
export function worktreeRows(index: WorkspaceIndex, summary: ProjectSummary, repo: ProjectRepo | null): WorktreeRow[] {
  const recorded = (repo?.checkouts ?? []).filter(c => c.kind === "worktree").map((c): WorktreeRow => {
    const view = c.taskId ? index.byId.get(c.taskId) ?? null : null;
    return { key: c.id, checkout: c, branch: c.branch, path: c.path, task: view, label: view?.task.title ?? c.label ?? "No task", ahead: c.ahead, behind: c.behind, state: worktreeState(index.state, c) };
  });
  const known = new Set(recorded.map(r => r.task?.id));
  const others = summary.views.filter(v => v.task.checkout.kind === "worktree" && !known.has(v.id) && ["needs", "work", "review", "merge"].includes(v.column)).map((v): WorktreeRow => ({
    key: v.id, checkout: null, branch: v.task.checkout.branch, path: (v.task.checkout as { path: string }).path, task: v, label: v.task.title, ahead: null, behind: null,
    state: { label: "No Git record", tone: "st-idle", icon: "help" },
  }));
  return [...recorded, ...others];
}
export const defaultCheckout = (repo: ProjectRepo, id: string | undefined) => repo.checkouts.find(c => c.id === id) ?? repo.checkouts.find(c => c.taskId && c.kind === "worktree") ?? repo.checkouts[0];
export { repoOf, taskCheckout, worktreeWork };
