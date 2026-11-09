import { dependencyReady, isOpen, openPlanningQuestion, runBlockerOf, runOrder, type PlanItem, type Sprint, type Task, type TaskSession } from "@cortex/workspace-domain";
import type { Ask } from "./questions";
import { checksLine } from "./review-model";
import { readyLabel, type TaskView, type WorkspaceIndex } from "./selectors";

export const itemTitle = (item: PlanItem) => item.title ?? item.description;

/** The planning run's open questions, every one of them. A sprint with no run behind it, as in the demo, asks from its conversation. */
export function planningAsks(sprint: Sprint, session: TaskSession | undefined): Ask[] {
  if (sprint.planning === "created" || sprint.planning === "cancelled") return [];
  const open = (session?.decisions ?? []).filter(d => d.kind === "question" && !d.suspended && (isOpen(d) || d.held));
  if (open.length) return open.map(d => ({ id: d.id, text: d.text, from: "", at: d.at, unavailable: d.unavailable, options: d.options, answer: d.answer }));
  const asked = sprint.planning === "question" ? openPlanningQuestion(sprint) : null;
  return asked ? [{ id: "planning", text: asked.text, from: "", at: asked.at ?? null, options: (asked.options ?? []).map(label => ({ label })), answer: null }] : [];
}

export type ThreadEntry = { key: string; kind: "qa" | "note" | "message" | "event"; text: string; question?: string; at: number | null };
/** The planning conversation in order. A settled question reads with its answer, however each side was recorded. */
export function planningThread(sprint: Sprint, session: TaskSession | undefined): ThreadEntry[] {
  const asked = (session?.decisions ?? []).filter(d => d.kind === "question");
  const settled = asked.filter(d => d.answer && !d.held), open = openPlanningQuestion(sprint), out: ThreadEntry[] = [];
  let question: string | null = null;
  const flush = (key: string) => { if (question !== null) out.push({ key: `note:${key}`, kind: "note", text: question, at: null }); question = null; };
  sprint.conversation.forEach((entry, i) => {
    if (entry === open) return;
    if (entry.kind === "question") { flush(String(i)); question = entry.text; return; }
    if (entry.kind !== "answer") { flush(String(i)); out.push({ key: `${entry.kind}:${i}`, kind: entry.kind, text: entry.text, at: entry.at ?? null }); return; }
    // An answer still waiting for the rest of its batch stays on its tab.
    if (asked.some(d => d.held && d.answer?.text === entry.text)) return;
    const at = settled.findIndex(d => d.answer!.text === entry.text), decision = at < 0 ? null : settled.splice(at, 1)[0];
    out.push({ key: `qa:${decision?.id ?? i}`, kind: "qa", question: decision?.text ?? question ?? "", text: entry.text, at: decision?.answer?.at ?? entry.at ?? null });
    question = null;
  });
  flush("end");
  return [...out, ...settled.map((d): ThreadEntry => ({ key: `qa:${d.id}`, kind: "qa", question: d.text, text: d.answer!.text, at: d.answer!.at }))];
}
const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? "" : "s"}`;

type Word = { text: string; tone: string; icon: string; spin?: boolean; group: "ready" | "working" | "next" | "blocked" | "merged" | "closed" };
/** One word for a task inside a sprint. A dependency that has not unlocked reads as blocked, never as queued. */
function wordOf(view: TaskView, next: boolean, index: WorkspaceIndex): Word {
  const t = view.task;
  if (t.conflict !== "none") return { text: "Conflict", tone: "st-bad", icon: "alert", group: "working" };
  if (view.unknown) return { text: view.fact.text, tone: "st-idle", icon: "help", group: "working" };
  switch (t.status) {
    case "working": return { text: "Working", tone: "st-run", icon: "loader", spin: true, group: "working" };
    case "paused": return { text: "Paused", tone: "st-idle", icon: "pause", group: "working" };
    case "stopping": return { text: "Stopping", tone: "st-warn", icon: "hourglass", group: "working" };
    case "needs_you": return { text: "Needs you", tone: "st-warn", icon: "question", group: "working" };
    case "review": return { text: "Review", tone: "st-idle", icon: "eye", group: "working" };
    case "ready_to_merge": return { text: readyLabel(t, view.target), tone: "st-merge", icon: "merge", group: "ready" };
    case "failed": return { text: "Run failed", tone: "st-bad", icon: "x-circle", group: "working" };
    case "cancelled": return { text: "Run cancelled", tone: "st-idle", icon: "x-circle", group: "working" };
    case "done": return t.integration?.method === "commit" ? { text: `Committed to ${t.integration.targetBranch}`, tone: "st-idle", icon: "check", group: "closed" } : t.work === "merged" ? { text: "Merged", tone: "st-merge", icon: "merge", group: "merged" }
      : { text: t.closedAs === "external" ? "Done outside the app" : t.closedAs === "cancel" ? "Cancelled" : "Closed as is", tone: "st-idle", icon: t.closedAs === "cancel" ? "x-circle" : "circle", group: "closed" };
    case "draft": {
      if (t.dependencies.some(id => { const d = index.byId.get(id); return !d || !dependencyReady(d.task); })) return { text: "Blocked", tone: "st-idle", icon: "hourglass", group: "blocked" };
      return next ? { text: "Next", tone: "st-idle", icon: "circle", group: "next" } : { text: "Queued", tone: "st-idle", icon: "circle", group: "blocked" };
    }
  }
}

export type PlanRow = { item: PlanItem; no: number; deps: number[]; view: TaskView | null; word: Word | null; waits: number[] };
/** The plan as a table. Once tasks exist each row also carries its task, in plan order. */
export function planRows(index: WorkspaceIndex, sprint: Sprint, items: PlanItem[]): PlanRow[] {
  const created = sprint.materializedVersion !== null && sprint.taskIds.length === items.length;
  const views = created ? sprint.taskIds.map(id => index.byId.get(id) ?? null) : [];
  const nextId = views.find(v => v?.task.status === "draft" && !v.task.dependencies.some(id => { const d = index.byId.get(id); return !d || !dependencyReady(d.task); }))?.id;
  const no = (key: string) => items.findIndex(i => i.key === key) + 1;
  return items.map((item, i) => {
    const view = views[i] ?? null;
    const unready = (key: string) => { const d = views[no(key) - 1]; return !d || !dependencyReady(d.task); };
    return { item, no: i + 1, deps: item.dependsOn.map(no), view, word: view ? wordOf(view, view.id === nextId, index) : null, waits: item.dependsOn.filter(unready).map(no) };
  });
}

const short = (text: string) => text.length > 15 ? `${text.slice(0, 14).trimEnd()}…` : text;
const NODE_W = 120, NODE_H = 34, COL = 176, ROW = 46, PAD = 12;
/** A left-to-right layered drawing: a task sits one column past its last dependency. */
export function planGraph(items: PlanItem[], selected: string | null) {
  const depth = new Map<string, number>();
  const depthOf = (key: string, seen: string[] = []): number => {
    if (depth.has(key)) return depth.get(key)!;
    if (seen.includes(key)) return 0;
    const item = items.find(i => i.key === key);
    const d = item && item.dependsOn.length ? Math.max(...item.dependsOn.map(k => depthOf(k, [...seen, key]))) + 1 : 0;
    depth.set(key, d);
    return d;
  };
  const columns: string[][] = [];
  items.forEach(i => { (columns[depthOf(i.key)] ??= []).push(i.key); });
  const tallest = Math.max(1, ...columns.map(c => c.length));
  const height = tallest * ROW + PAD * 2 - (ROW - NODE_H), width = (columns.length - 1) * COL + NODE_W + PAD * 2;
  const at = new Map<string, { x: number; y: number }>();
  columns.forEach((keys, c) => keys.forEach((key, r) => at.set(key, { x: PAD + c * COL, y: PAD + (tallest - keys.length) * ROW / 2 + r * ROW })));
  const no = (key: string) => items.findIndex(i => i.key === key) + 1;
  const edges = items.flatMap(item => item.dependsOn.filter(d => at.has(d)).map(dep => {
    const a = at.get(dep)!, b = at.get(item.key)!, x1 = a.x + NODE_W, y1 = a.y + NODE_H / 2, x2 = b.x, y2 = b.y + NODE_H / 2, mid = (x1 + x2) / 2;
    return { key: `${dep}>${item.key}`, d: y1 === y2 ? `M${x1} ${y1} L${x2} ${y2}` : `M${x1} ${y1} C${mid} ${y1} ${mid} ${y2} ${x2} ${y2}`, on: dep === selected || item.key === selected };
  }));
  const nodes = items.map(item => ({ key: item.key, ...at.get(item.key)!, w: NODE_W, h: NODE_H, label: `${no(item.key)} ${short(itemTitle(item))}`, on: item.key === selected }));
  const leads = items.filter(i => items.some(x => x.dependsOn.includes(i.key))).map(i => `Task ${no(i.key)} leads to ${items.filter(x => x.dependsOn.includes(i.key)).map(x => no(x.key)).join(", ")}.`);
  return { nodes, edges, width, height, label: leads.join(" ") || "No task depends on another." };
}
export function planCaption(items: PlanItem[], selected: string | null) {
  const order = `Automatic run order, one at a time: ${runOrder(items).map(k => items.findIndex(i => i.key === k) + 1).join(", ")}.`;
  const item = items.find(i => i.key === selected);
  if (!item) return order;
  const no = (key: string) => items.findIndex(i => i.key === key) + 1;
  const after = items.filter(i => i.dependsOn.includes(item.key)).map(i => no(i.key));
  const needs = item.dependsOn.map(no);
  const parts = [needs.length ? `needs ${needs.join(", ")}` : "starts first", ...(after.length ? [`unblocks ${after.join(", ")}`] : [])];
  return `Task ${no(item.key)} ${parts.join(" and ")}. ${order}`;
}

export type SummaryRow = { view: TaskView; word: Word; checks: string; branch: string };
const checksOf = (task: Task, word: Word, index: WorkspaceIndex): string => {
  if (task.result) return checksLine(task.result.checks);
  if (word.group === "blocked" && word.text === "Blocked") {
    const dep = task.dependencies.map(id => index.byId.get(id)).find(d => d && !dependencyReady(d.task));
    return dep ? `Waits for ${dep.task.title}` : "Waits for a dependency";
  }
  return task.status === "draft" ? "Not started" : "No result yet";
};
/** An index of real facts: a missing check is never read as a pass, and nothing here is a percentage of progress. */
export function sprintSummary(index: WorkspaceIndex, sprint: Sprint) {
  const views = sprint.taskIds.flatMap(id => { const v = index.byId.get(id); return v ? [v] : []; });
  const rows = planRows(index, sprint, views.map(v => ({ key: v.id, title: v.task.title, description: v.task.description, dependsOn: v.task.dependencies.filter(d => views.some(x => x.id === d)) })));
  const next = views.find(v => v.task.status === "draft" && v.task.dependencies.every(id => { const d = index.byId.get(id); return !!d && dependencyReady(d.task); }));
  const list: SummaryRow[] = views.map(view => {
    const word = wordOf(view, view.id === next?.id, index);
    return { view, word, checks: checksOf(view.task, word, index), branch: view.task.checkout.branch };
  });
  const count = (group: Word["group"]) => list.filter(r => r.word.group === group).length;
  return { rows: list, planRows: rows, ready: count("ready"), working: count("working"), next: count("next"), blocked: count("blocked"), merged: count("merged"), closed: count("closed") };
}
export const createLabel = (n: number, version: number) => `Create ${plural(n, "task")} from Plan v${version}`;

/**
 * Why a sprint shows fewer task rows than it has, in words, or null when every recorded task is here. A task that could
 * not be loaded is never read as a deleted one: the membership stays, and Retry only reads again.
 */
export function sprintGap(index: WorkspaceIndex, sprint: Sprint): { title: string; text: string; retry: boolean } | null {
  const loaded = sprint.taskIds.filter(id => index.byId.has(id)).length, missing = sprint.taskIds.length - loaded;
  if (sprint.planning !== "created") return { title: "No tasks yet", text: sprint.plan ? "The plan is ready. Create its tasks on the Planning tab; nothing runs until you start them." : "Tasks appear here once planning proposes them and you create them.", retry: false };
  if (missing > 0) return { title: "Task details unavailable", text: `${missing} of ${plural(sprint.taskIds.length, "sprint task")} could not be loaded. The sprint keeps them, and Retry reads them again without creating anything.`, retry: true };
  if (!sprint.taskIds.length) return { title: "This sprint has no tasks", text: "Add one with New task. It is saved as a draft and starts only when you start it.", retry: false };
  return null;
}
/**
 * Why a created sprint is not moving although work is left, with the task to open, or null. Automatic queues name what
 * Run sprint would refuse; manual sprints name the dependency the first unstarted task waits for.
 */
export function queueStall(index: WorkspaceIndex, sprint: Sprint): { reason: string; taskId: string | null } | null {
  if (sprint.planning !== "created" || sprint.queueState === "running" || sprint.queueState === "complete") return null;
  if (sprint.taskIds.some(id => !index.byId.has(id))) return null;
  if (sprint.runMode === "automatic") { const b = runBlockerOf(index.state, sprint); return b?.taskId ? b : null; }
  const drafts = sprint.taskIds.map(id => index.byId.get(id)!).filter(v => v.task.status === "draft");
  const ready = (v: TaskView) => v.task.dependencies.every(id => { const d = index.byId.get(id); return !!d && dependencyReady(d.task); });
  if (!drafts.length || drafts.some(ready)) return null;
  const first = drafts[0], dep = first.task.dependencies.map(id => index.byId.get(id)).find(d => !d || !dependencyReady(d.task));
  return dep ? { reason: `${first.task.title} waits for ${dep.task.title} to reach Ready to merge`, taskId: dep.id } : { reason: `${first.task.title} waits for a task that is not loaded`, taskId: null };
}
/** Why the sprint cannot go to the Archive yet, or null. Every recorded task must be loaded and done, not only the visible ones. */
export function sprintArchiveBlocked(index: WorkspaceIndex, sprint: Sprint): string | null {
  if (!sprint.taskIds.length) return "This sprint has no tasks to archive";
  if (sprint.taskIds.some(id => !index.byId.has(id))) return "Some of this sprint's tasks are not loaded yet. Retry, then archive";
  return sprint.taskIds.every(id => index.byId.get(id)!.task.status === "done") ? null : "Available when every task is done";
}
