import { checkAcceptanceBlocker } from "@cortex/workspace-domain";
import { repoOf, taskCheckout, worktreeWork } from "@cortex/workspace-domain";
import type { Check, ConflictFile, DiffHunk, DiffLine, FileDiff, FolderEntry, FolderOverview, MergeState, ProjectRepo, RepoCheckout, Task, TaskChanges, WorkspaceState } from "@cortex/workspace-domain";
import { checkCounts, staleResult, type ProjectSummary, type TaskView, type WorkspaceIndex } from "./selectors";

/** Read models for review, merge, Code and Git. Every count on those screens comes from here; nothing here changes state. */
export const reviewFor = (state: WorkspaceState, taskId: string): TaskChanges | null => state.reviews.find(r => r.taskId === taskId) ?? null;
const baseName = (path: string) => path.split("/").at(-1) ?? path;
export const fileName = baseName;
export const dirName = (path: string) => path.split("/").slice(0, -1).join("/");

export function checksLine(checks: Check[], options?: { includeManualSkips: boolean }) {
  const c = checkCounts(checks);
  const notRun = c.not_run + (options?.includeManualSkips ? checks.filter(c => c.kind === "manual" && c.outcome === "not_run").length : 0);
  const parts = [c.passed ? `${c.passed} passed` : "", c.failed ? `${c.failed} failed` : "", notRun ? `${notRun} not run` : "", c.unverified ? `${c.unverified} unverified` : ""].filter(Boolean);
  return parts.length ? parts.join(", ") : checks.some(c => c.kind === "manual") ? "Check manually" : "No checks reported";
}
export const openChecks = (checks: Check[]) => checks.filter(c => c.kind !== "manual" && c.outcome !== "passed");
export const needsOverride = (checks: Check[]) => checks.length === 0 || openChecks(checks).length > 0;
/** Prefer the machine's acknowledgement decision: some listed skips are informational only. */
export const acceptNeedsTick = (task: Task) => task.reviewActions?.acknowledgeOpenChecks
  ?? (!!task.result && needsOverride(task.result.checks) && !(task.result.kind === "answer" && task.result.acceptanceAvailable && task.result.checks.length === 0));

export const stale = staleResult;
/** Why the owner cannot accept this version now, or null when they can. */
export function acceptBlocked(task: Task): string | null {
  if (!task.result) return "Available once a result is published";
  if (stale(task)) return `This version no longer matches the worktree. Request a new result`;
  if (task.conflict !== "none") return "Resolve the conflict first";
  if (task.acceptance) return task.acceptance.automatic ? `Automatically accepted v${task.acceptance.version}` : `You accepted v${task.acceptance.version}`;
  if (task.status === "done") return "This task is closed";
  if (task.reviewActions?.accept === false) return task.reviewActions.reason ?? "Refresh this result before accepting";
  // A task that has not finished has no acceptable result yet; that, not missing evidence, is why Accept is off.
  if (task.result.acceptanceAvailable === false && task.status !== "review" && task.status !== "ready_to_merge") return "Available once this task finishes";
  if (task.result.acceptanceAvailable === false) return "Result evidence is unavailable. Request a new result";
  const answer = task.result.kind === "answer" && task.result.acceptanceAvailable === true && task.result.checks.length === 0;
  const checksBlocker = answer ? null : checkAcceptanceBlocker(task.result.checks);
  if (checksBlocker) return checksBlocker;
  return task.status === "review" || task.status === "ready_to_merge" ? null : "Available once this task finishes";
}
/** A reason as one sentence: it gets its full stop only when it has none of its own, so a strip never ends in "..". */
export const sentence = (text: string) => { const t = text.trim(); return !t || /[.!?…]$/.test(t) ? t : `${t}.`; };
/** Whether a result offers Request changes. A closed task has nothing left to change, and one being integrated is past review. */
export const changesOffered = (task: Task) => (task.status === "review" || task.status === "ready_to_merge") && !task.integration;
/** Whether a result card offers Accept at all. A closed task, or one working again after Request changes, has nothing to accept. */
export const acceptOffered = (task: Task) => task.status !== "done" && task.status !== "working" && task.status !== "stopping";
export function mergeBlocked(task: Task): string | null {
  if (task.status === "done") return task.work === "merged" ? "This task is already merged" : "This task is closed";
  if (stale(task)) return "This result is stale. Request a new result or close the task as is";
  if (task.conflict !== "none") return "Resolve the conflict first";
  if (!task.acceptance) return task.result ? `Accept v${task.result.version} first. Accepting is your decision, merging is a Git action.` : "Available once this task has an accepted result";
  return task.status === "ready_to_merge" ? null : "Available in Ready to merge";
}
/** Why a task on its target branch cannot be committed from here, when the machine gave no reason of its own. It says where the
 *  task works and what is missing, never that the task has no files. */
export const noCommitReason = (task: Task) => `Which files this task changed in ${task.checkout.kind === "main" ? "the project folder" : "its checkout"} could not be confirmed, so they cannot be committed from here. Close as is, or commit in Git and use Done outside the app.`;
/**
 * Why Review and merge cannot open for a task in Ready to merge, or null. Without an acceptance yet, the merge dialog
 * records it for exactly this version first, so the only blockers are the ones that would refuse that acceptance.
 */
export function finishBlocked(task: Task): string | null {
  if (stale(task)) return "This result is stale. Request a new result or close the task as is";
  // Its files are still being checked against the target branch: nothing is offered from facts not yet known.
  if (task.integrationFacts?.pending) return task.integrationFacts.reason ?? "Checking this task’s files against its target branch…";
  if (task.integrationFacts?.alreadyInTarget) return `Already in ${task.integrationFacts.targetBranch}. Close the task instead.`;
  if (task.integrationFacts?.sameBranch) {
    if (!task.integrationFacts.commitAvailable) return task.integrationFacts.reason ?? noCommitReason(task);
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
/** A `more` row stands under a listed folder: how many entries were not shown, why it could not be read, or that it is being read. */
export type TreeRow = { kind: "dir" | "file" | "more"; path: string; name: string; depth: number; ignored?: boolean; lazy?: boolean; link?: boolean };
/**
 * What Code's tree knows beyond the repository snapshot: `items` are every path to show, sorted, a folder ending in `/`;
 * a `lazy` folder's entries are read only when the owner opens it; `notes` stand under a folder ("" is the top).
 */
export type CodeTree = { items: string[]; ignored: ReadonlySet<string>; lazy: ReadonlySet<string>; links: ReadonlySet<string>; notes: ReadonlyMap<string, string> };
/** One listed folder: its read, the machine's refusal, or still being read. */
export type FolderView = { entries: FolderEntry[]; more: number; exact: boolean } | { error: string } | { loading: true };
const joinPath = (dir: string, name: string) => dir ? `${dir}/${name}` : name;
export const moreWords = (more: number, exact: boolean) => `${exact ? "" : "At least "}${more.toLocaleString("en-US")} more not shown`;
/**
 * The snapshot's files with what the machine listed on demand: the overview (ignored files and top-most ignored or
 * untracked folders the snapshot leaves out) and each listed folder. A folder no snapshot file is under is lazy.
 */
export function codeTree(paths: string[], overview: FolderOverview | null, folders: ReadonlyMap<string, FolderView>): CodeTree {
  const items = new Set(paths), ignored = new Set<string>(), folderPaths = new Set<string>(), links = new Set<string>(), notes = new Map<string, string>();
  const add = (path: string, dir: boolean, mark: boolean | null | undefined) => {
    items.add(dir ? `${path}/` : path);
    if (dir) folderPaths.add(path);
    if (mark) ignored.add(path);
  };
  for (const e of overview?.entries ?? []) add(e.path, e.dir, e.ignored);
  if (overview?.truncated) notes.set("", "Some ignored files are not listed");
  for (const [dir, view] of folders) {
    if ("loading" in view) notes.set(dir, "Reading…");
    else if ("error" in view) notes.set(dir, view.error);
    else {
      for (const e of view.entries) { add(joinPath(dir, e.name), e.dir, e.ignored); if (e.link) links.add(joinPath(dir, e.name)); }
      if (view.more > 0) notes.set(dir, moreWords(view.more, view.exact));
    }
  }
  const known = new Set(allDirs(paths));
  const lazy = new Set([...folderPaths].filter(d => !known.has(d)));
  return { items: [...items].sort(), ignored, lazy, links, notes };
}
export function treeRows(paths: string[], collapsed: Set<string>, tree?: CodeTree): TreeRow[] {
  const rows: TreeRow[] = [], seen = new Set<string>();
  const closed = (dir: string) => collapsed.has(dir);
  // Anything under an ignored folder is ignored too.
  const ignoredUnder = new Set<string>();
  const isIgnored = (path: string, parent: string) => !!tree && (tree.ignored.has(path) || ignoredUnder.has(parent));
  for (const item of tree?.items ?? paths) {
    const folder = item.endsWith("/"), path = folder ? item.slice(0, -1) : item, parts = path.split("/");
    const dirs = folder ? parts.length : parts.length - 1;
    let hidden = false;
    for (let d = 0; d < dirs; d++) {
      const dir = parts.slice(0, d + 1).join("/");
      if (!hidden && !seen.has(dir)) {
        seen.add(dir);
        const ignored = isIgnored(dir, parts.slice(0, d).join("/"));
        if (ignored) ignoredUnder.add(dir);
        rows.push({ kind: "dir", path: dir, name: parts[d], depth: d, ...(ignored ? { ignored } : {}), ...(tree?.lazy.has(dir) ? { lazy: true } : {}) });
      }
      if (closed(dir)) hidden = true;
    }
    if (!folder && !hidden) {
      const ignored = isIgnored(path, dirName(path));
      rows.push({ kind: "file", path, name: parts.at(-1)!, depth: parts.length - 1, ...(ignored ? { ignored } : {}), ...(tree?.links.has(path) ? { link: true } : {}) });
    }
  }
  if (!tree?.notes.size) return rows;
  // Each note follows the last row inside its folder, while that folder is open.
  const out: TreeRow[] = [], open: string[] = [];
  const close = (until: (dir: string) => boolean) => {
    while (open.length && until(open.at(-1)!)) {
      const dir = open.pop()!, note = tree.notes.get(dir);
      if (note) out.push({ kind: "more", path: dir, name: note, depth: dir.split("/").length });
    }
  };
  for (const row of rows) {
    close(dir => !row.path.startsWith(`${dir}/`));
    out.push(row);
    if (row.kind === "dir" && !closed(row.path)) open.push(row.path);
  }
  close(() => true);
  const top = tree.notes.get("");
  if (top) out.push({ kind: "more", path: "", name: top, depth: 0 });
  return out;
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
