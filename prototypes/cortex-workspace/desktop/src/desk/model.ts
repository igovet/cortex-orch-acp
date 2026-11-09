import { createMemo, type Accessor } from "solid-js";
import { selectProjectAggregate, type Fixture, type Project, type Task } from "../fixtures";
import type { MockStore } from "../store";

/** Fixture clock: the last fixture event is 01 Jan 2030 02:46:39 UTC. Relative times are relative to this, not the wall clock. */
export const FIXTURE_NOW = Date.UTC(2030, 0, 1, 2, 46, 40);
export const AS_OF = "01 Jan 2030 02:46 UTC (fixture)";
export const AS_OF_SHORT = "02:46";
export const LAST_VISIT = "01 Jan 2030 02:40 (fixture)";
export const PHASES = ["backlog", "working", "review", "done"] as const;
export type Phase = typeof PHASES[number];
export const PHASE_LABEL: Record<Phase, string> = { backlog: "Backlog", working: "Working", review: "Review", done: "Done" };

export type Exec = "not_started" | "running" | "blocked" | "waiting" | "failed" | "complete" | "unknown";
export const EXEC_LABEL: Record<Exec, string> = {
  not_started: "Not started", running: "Running", blocked: "Blocked", waiting: "Waiting", failed: "Failed", complete: "Complete", unknown: "Unknown",
};
export type Decision = Fixture["decisions"][number];
export type Check = Fixture["checks"][number];
export type Result = Fixture["results"][number];
export type LoadStatus = "loading" | "ready" | "error";

export type TaskView = {
  task: Task; no: number; num: string; ref: string; project: Project; projectIndex: number; slug: string;
  exec: Exec; lastKnown: Exec | null; wait: "permission" | "question" | null; responded: boolean;
  check: "failed" | "not_run" | null; stale: boolean; conflict: boolean; created: boolean;
  capability: string | null; updatedAt: string | null; needsYou: boolean; disconnected: boolean;
};
export type ProjectSummary = {
  project: Project; index: number; slug: string; machineName: string; disconnected: boolean; conflict: boolean;
  taskCount: number; phases: Record<Phase, number>; needsYou: number; failed: number; notRun: number; stale: number;
  running: number | "unknown"; lastActivity: string | null; eventCount: number;
};
export type Attention = { tv: TaskView; label: string; tone: "warn" | "danger" | "info" | "neutral" };

export const slugOf = (p: Project) => p.repository.split("/").pop() ?? p.id;

const VERBS = ["Refine", "Review", "Fix", "Update", "Audit", "Tidy", "Check", "Trim", "Document", "Align", "Test", "Simplify"];
const OBJECTS = ["onboarding copy", "layout tokens", "list sorting", "sync notes", "empty states", "settings page", "keyboard paths", "build output", "import flow", "toolbar icons", "offline banner", "search results"];
/**
 * Generated fixture titles name a random subject ("Orbit synthetic task 2") that reads like another project's task.
 * The desk shows neutral, project-free display titles instead; ids, numbers and fixture semantics are unchanged.
 */
export function displayTitle(title: string) {
  const m = /^[A-Z][a-z]+ synthetic task (\d+)$/.exec(title);
  if (!m) return title;
  const n = Number(m[1]) - 1;
  return `${VERBS[n % VERBS.length]} ${OBJECTS[(Math.floor(n / VERBS.length) + n) % OBJECTS.length]}`;
}
/** Compatibility name for the plain TypeScript mock title policy. */
export { generateMockTitle as draftTitle } from "../workspace/domain";
export const numOf = (no: number) => String(no).padStart(4, "0");

const pad = (n: number) => String(n).padStart(2, "0");
export function hhmm(iso: string | null) {
  if (!iso) return "—";
  const d = new Date(iso);
  return `${pad(d.getUTCHours())}:${pad(d.getUTCMinutes())}`;
}
export function absTime(iso: string | null) {
  if (!iso) return "No activity";
  const d = new Date(iso);
  const months = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
  return `${pad(d.getUTCDate())} ${months[d.getUTCMonth()]} ${d.getUTCFullYear()} ${hhmm(iso)} UTC (fixture)`;
}
/** Relative to the fixture clock. Missing activity is "No activity", never "now" or 0. */
export function relTime(iso: string | null) {
  if (!iso) return "No activity";
  const s = Math.max(0, Math.round((FIXTURE_NOW - Date.parse(iso)) / 1000));
  if (s < 60) return "<1 min ago";
  if (s < 3600) return `${Math.floor(s / 60)} min ago`;
  if (s < 86400) return `${Math.floor(s / 3600)} h ago`;
  return absTime(iso);
}

export type DeskData = ReturnType<typeof createDeskData>;

/**
 * Every counter and every empty state reads these memos, so they can never disagree:
 * empty copy renders only when the same count is 0 and status() is "ready".
 */
export function createDeskData(store: MockStore, loading: Accessor<boolean>, titles: Accessor<Record<string, string>> = () => ({})) {
  const fixture = () => store.state().fixture;
  const status = createMemo<LoadStatus>(() => loading() ? "loading" : store.error() ? "error" : "ready");
  const offline = () => store.state().scenario === "offline";
  const indexById = createMemo(() => new Map(fixture().tasks.map((t, i) => [t.id, i])));
  const lastEvent = createMemo(() => {
    const m = new Map<string, string>();
    for (const e of fixture().events) { const p = m.get(e.taskId); if (!p || e.occurredAt > p) m.set(e.taskId, e.occurredAt); }
    return m;
  });
  const projectIndex = createMemo(() => new Map(fixture().projects.map((p, i) => [p.id, i])));

  const tasks = createMemo<TaskView[]>(() => {
    const f = fixture(), responses = store.state().responses, idx = indexById(), last = lastEvent(), pIdx = projectIndex(), renamed = titles();
    const execs = new Map(f.executions.map(e => [e.id, e]));
    const checks = new Map<string, Check[]>();
    f.checks.forEach(c => checks.set(c.taskId, [...(checks.get(c.taskId) ?? []), c]));
    const stale = new Set(f.results.filter(r => r.state === "stale").map(r => r.taskId));
    const conflicts = new Set(f.files.filter(x => x.state === "conflict").map(x => x.taskId));
    const caps = new Map(f.capabilities.filter(c => !c.available).map(c => [c.taskId, c.name]));
    const open = new Map(f.decisions.filter(d => d.state === "open").map(d => [d.taskId, d]));
    return store.visibleTasks().map(raw => {
      const title = renamed[raw.id] ?? displayTitle(raw.title);
      const task = title === raw.title ? raw : { ...raw, title };
      const project = f.projects[pIdx.get(task.projectId) ?? 0];
      const machine = f.machines.find(m => m.id === project.machineId);
      const disconnected = !machine || machine.state === "disconnected";
      const ex = task.executionId ? execs.get(task.executionId) : undefined;
      let exec: Exec = ex ? ex.state : "not_started";
      let lastKnown: Exec | null = null;
      if (disconnected && (exec === "running" || machine?.currentExecutionState === "unknown") && exec !== "not_started") { lastKnown = exec; exec = "unknown"; }
      const decision = open.get(task.id);
      const responded = decision ? decision.id in responses : false;
      const taskChecks = checks.get(task.id) ?? [];
      const no = (idx.get(task.id) ?? 0) + 1;
      return {
        task, no, num: numOf(no), ref: `#${numOf(no)}`, project, projectIndex: pIdx.get(project.id) ?? 0, slug: slugOf(project),
        exec, lastKnown, wait: ex?.waitReason === "permission" || ex?.waitReason === "question" ? ex.waitReason : null, responded,
        check: taskChecks.some(c => c.outcome === "failed") ? "failed" : taskChecks.some(c => c.outcome === "not_run") ? "not_run" : null,
        stale: stale.has(task.id), conflict: conflicts.has(task.id), created: task.id.startsWith("mock-created-task-"),
        capability: caps.get(task.id) ?? null, updatedAt: last.get(task.id) ?? null,
        needsYou: Boolean(decision) && !responded, disconnected,
      } satisfies TaskView;
    });
  });
  const taskByNum = createMemo(() => new Map(tasks().map(t => [t.num, t])));
  const tasksByProject = createMemo(() => {
    const m = new Map<string, TaskView[]>();
    fixture().projects.forEach(p => m.set(p.id, []));
    tasks().forEach(t => m.get(t.project.id)!.push(t));
    return m;
  });

  const projects = createMemo<ProjectSummary[]>(() => {
    const f = fixture();
    const visible = store.visibleTasks();
    // Aggregates are read from the visible fixture so an empty or failed view never shows baseline counts.
    const view: Fixture = visible === f.tasks ? f : { ...f, tasks: visible, events: [] };
    const byProject = tasksByProject();
    return f.projects.map((project, index) => {
      const agg = selectProjectAggregate(view, project.id)!;
      const list = byProject.get(project.id) ?? [];
      const machine = f.machines.find(m => m.id === project.machineId);
      return {
        project, index, slug: slugOf(project), machineName: machine?.name ?? "Unknown machine",
        disconnected: !machine || machine.state === "disconnected", conflict: project.gitState === "conflict",
        taskCount: list.length, phases: agg.taskCounts,
        needsYou: list.filter(t => t.needsYou).length, failed: agg.failedChecks, notRun: agg.unrunChecks, stale: agg.staleResults,
        running: agg.runningWorkers, lastActivity: agg.lastActivity, eventCount: 0,
      };
    });
  });
  const projectBySlug = createMemo(() => new Map(projects().map(p => [p.slug, p])));
  const eventCounts = createMemo(() => {
    const m = new Map<string, number>();
    if (store.visibleTasks().length) fixture().events.forEach(e => m.set(e.projectId, (m.get(e.projectId) ?? 0) + 1));
    return m;
  });

  const needs = createMemo(() => {
    const f = fixture(), byId = new Map(tasks().map(t => [t.task.id, t]));
    return f.decisions.filter(d => d.state === "open").map(d => ({ decision: d, tv: byId.get(d.taskId) })).filter((x): x is { decision: Decision; tv: TaskView } => Boolean(x.tv?.needsYou));
  });

  function attention(projectId: string): Attention[] {
    const list = tasksByProject().get(projectId) ?? [];
    const out: Attention[] = [];
    list.filter(t => t.needsYou).forEach(tv => out.push({ tv, label: tv.wait === "question" ? "Question" : "Permission", tone: "warn" }));
    list.filter(t => t.check === "failed" || t.exec === "failed").forEach(tv => out.push({ tv, label: "Failed check", tone: "danger" }));
    list.filter(t => t.exec === "running" || t.exec === "unknown").forEach(tv => out.push({ tv, label: tv.exec === "unknown" ? "Unknown" : "Running", tone: tv.exec === "unknown" ? "neutral" : "info" }));
    list.filter(t => t.stale).forEach(tv => out.push({ tv, label: "Result stale", tone: "warn" }));
    list.filter(t => t.conflict).forEach(tv => out.push({ tv, label: "Conflict", tone: "danger" }));
    const seen = new Set<string>();
    return out.filter(a => !seen.has(a.tv.task.id) && seen.add(a.tv.task.id)).slice(0, 5);
  }

  const totals = createMemo(() => {
    const ps = projects();
    return {
      projects: ps.length, tasks: ps.reduce((s, p) => s + p.taskCount, 0),
      connected: ps.filter(p => !p.disconnected).length, needsYou: needs().length,
    };
  });

  return {
    fixture, status, offline, tasks, taskByNum, tasksByProject, projects, projectBySlug, needs, attention, totals, eventCounts,
    checksFor: (taskId: string) => fixture().checks.filter(c => c.taskId === taskId),
    resultFor: (taskId: string) => fixture().results.find(r => r.taskId === taskId),
    decisionsFor: (taskId: string) => fixture().decisions.filter(d => d.taskId === taskId),
    eventsFor: (taskId: string) => fixture().events.filter(e => e.taskId === taskId),
    machineFor: (p: Project) => fixture().machines.find(m => m.id === p.machineId),
  };
}

/** Board filters. Counting "hidden" uses the same task list the column count reads. */
export const FILTERS = ["needsYou", "failed", "notRun", "stale", "running"] as const;
export type FilterId = typeof FILTERS[number];
export const FILTER_LABEL: Record<FilterId, string> = { needsYou: "Needs you", failed: "Failed check", notRun: "Not run", stale: "Result stale", running: "Running" };
export function matchesFilters(tv: TaskView, filters: readonly FilterId[]) {
  if (!filters.length) return true;
  return filters.some(f => f === "needsYou" ? tv.needsYou : f === "failed" ? tv.check === "failed" || tv.exec === "failed"
    : f === "notRun" ? tv.check === "not_run" : f === "stale" ? tv.stale : tv.exec === "running" || tv.lastKnown === "running");
}
