import { SCENARIO_DETAILS, type Fixture, type Scenario, type Task } from "../../fixtures";
import type { MockStore } from "../../store";
import type { GlyphName } from "./glyphs";

export type StoreState = ReturnType<MockStore["state"]>;
export type Decision = Fixture["decisions"][number];
export type Check = Fixture["checks"][number];
export type Result = Fixture["results"][number];
export type Execution = Fixture["executions"][number];
export type FixtureEvent = Fixture["events"][number];
export type Tone = "neutral" | "info" | "warn" | "danger" | "success" | "mock";

/** Candidate board phases (owner decision). Columns are read-only projections. */
export const COLUMNS = [
  { id: "draft", name: "Draft" },
  { id: "discussion", name: "Discussion" },
  { id: "planned", name: "Planned" },
  { id: "execution", name: "In execution" },
  { id: "review", name: "Result review" },
  { id: "accepted", name: "Accepted (mock)" },
  { id: "closed", name: "Closed" },
] as const;
export type ColumnId = typeof COLUMNS[number]["id"];
export const columnName = (id: ColumnId) => COLUMNS.find(c => c.id === id)!.name;

/**
 * Concept-local projection of the four shared fixture phases onto the seven
 * candidate columns. "done" means runtime completion, which is not human
 * acceptance, so it stays in Result review until a mock acceptance exists.
 */
const PHASE_COLUMN: Record<Task["phase"], ColumnId> = { backlog: "discussion", working: "execution", review: "review", done: "review" };

export type ExecLabel = "Not started" | "Running" | "Waiting" | "Idle" | "Failed" | "Cancelled" | "Unknown";
export const EXEC_STYLE: Record<ExecLabel, { tone: Tone; glyph: GlyphName }> = {
  "Not started": { tone: "neutral", glyph: "none" },
  Running: { tone: "info", glyph: "half" },
  Waiting: { tone: "warn", glyph: "pause" },
  Idle: { tone: "neutral", glyph: "minus" },
  Failed: { tone: "danger", glyph: "x" },
  Cancelled: { tone: "neutral", glyph: "slash" },
  Unknown: { tone: "neutral", glyph: "question" },
};
const EXEC_MAP: Record<Execution["state"], ExecLabel> = { running: "Running", blocked: "Waiting", waiting: "Waiting", failed: "Failed", complete: "Idle" };

export type WaitLabel = "Your answer needed" | "Your permission needed" | "Result review needed";
export const WAIT_GLYPH: Record<WaitLabel, GlyphName> = { "Your answer needed": "message", "Your permission needed": "key", "Result review needed": "filecheck" };
export const WAIT_LABELS = Object.keys(WAIT_GLYPH) as WaitLabel[];
export const EXEC_LABELS = Object.keys(EXEC_STYLE) as ExecLabel[];

export const CHECK_STYLE: Record<Check["outcome"] | "unverified", { label: string; tone: Tone; glyph: GlyphName; order: number }> = {
  failed: { label: "Failed", tone: "danger", glyph: "x", order: 0 },
  not_run: { label: "Not run", tone: "neutral", glyph: "dash", order: 1 },
  unverified: { label: "Unverified", tone: "neutral", glyph: "question", order: 2 },
  passed: { label: "Passed", tone: "success", glyph: "check", order: 3 },
};

export type Conn = "connected" | "offline" | "disconnected";
export const connOf = (s: Scenario): Conn => s === "offline" ? "offline" : s === "disconnected" ? "disconnected" : "connected";
export const CONFLICT_TASK_ID = SCENARIO_DETAILS.conflict.taskId!;

export const hhmm = (iso: string) => `${iso.slice(11, 16)} UTC`;
export const stamp = (iso: string) => `${iso.slice(0, 10)} ${iso.slice(11, 16)} UTC`;

export type EventIndex = { byTask: Map<string, number[]>; last: Map<string, string>; all: number[] };
export function buildEventIndex(events: readonly FixtureEvent[]): EventIndex {
  const byTask = new Map<string, number[]>(), last = new Map<string, string>(), all: number[] = new Array(events.length);
  events.forEach((e, i) => {
    all[i] = i;
    let list = byTask.get(e.taskId);
    if (!list) byTask.set(e.taskId, list = []);
    list.push(i);
    last.set(e.taskId, e.occurredAt);
  });
  return { byTask, last, all };
}

export type TaskView = {
  id: string; title: string; description: string; ordinal: number; column: ColumnId;
  exec: ExecLabel; lastKnown: ExecLabel | null; execution?: Execution;
  wait: WaitLabel | null; answered: { decision: Decision; answer: string } | null;
  decisions: Decision[]; openDecisions: Decision[]; result?: Result; checks: Check[];
  accepted: boolean; stale: boolean; failedCheck: boolean; created: boolean; draft: boolean;
  updatedAt: string | null; conflict: boolean;
};

function group<T>(items: readonly T[], key: (t: T) => string) {
  const map = new Map<string, T[]>();
  for (const item of items) { const k = key(item); map.set(k, [...(map.get(k) ?? []), item]); }
  return map;
}

export function deriveViews(s: StoreState, idx: EventIndex, drafts: ReadonlySet<string>, conn: Conn): TaskView[] {
  const f = s.fixture;
  const execBy = new Map(f.executions.map(e => [e.taskId, e] as const));
  const resultBy = new Map(f.results.map(r => [r.taskId, r] as const));
  const decisionsBy = group(f.decisions, d => d.taskId), checksBy = group(f.checks, c => c.taskId);
  return f.tasks.map((task, ordinal): TaskView => {
    const execution = execBy.get(task.id), result = resultBy.get(task.id);
    const decisions = decisionsBy.get(task.id) ?? [], checks = checksBy.get(task.id) ?? [];
    const openDecisions = decisions.filter(d => !(d.id in s.responses));
    const answeredDecision = decisions.find(d => d.id in s.responses);
    const accepted = !!result && s.acceptedResultIds.includes(result.id);
    let exec: ExecLabel = execution ? EXEC_MAP[execution.state] : "Not started";
    let lastKnown: ExecLabel | null = null;
    if (conn !== "connected" && execution) { lastKnown = exec; exec = "Unknown"; }
    const wait: WaitLabel | null = openDecisions.some(d => d.kind === "permission") ? "Your permission needed"
      : openDecisions.some(d => d.kind === "question") ? "Your answer needed"
      : result?.state === "complete" && !accepted ? "Result review needed" : null;
    const draft = drafts.has(task.id);
    return {
      id: task.id, title: task.title, description: task.description, ordinal,
      column: accepted ? "accepted" : draft ? "draft" : PHASE_COLUMN[task.phase],
      exec, lastKnown, execution, wait,
      answered: answeredDecision ? { decision: answeredDecision, answer: s.responses[answeredDecision.id] } : null,
      decisions, openDecisions, result, checks, accepted, stale: result?.state === "stale",
      failedCheck: checks.some(c => c.outcome === "failed"),
      created: task.id.startsWith("mock-created-task-"), draft,
      updatedAt: idx.last.get(task.id) ?? null, conflict: task.id === CONFLICT_TASK_ID,
    };
  });
}

export type TabId = "discussion" | "decisions" | "result" | "code" | "git" | "workers";
export const TABS: { id: TabId; label: string }[] = [
  { id: "discussion", label: "Discussion & plan" }, { id: "decisions", label: "Decisions" },
  { id: "result", label: "Result & diff" }, { id: "code", label: "Code" }, { id: "git", label: "Git" }, { id: "workers", label: "Workers" },
];

export type NeedItem = { key: string; kind: "Permission" | "Question" | "Result acceptance"; title: string; view: TaskView; tab: TabId; actionable: boolean; note?: string };
export function needsYou(views: readonly TaskView[]): NeedItem[] {
  const items: NeedItem[] = [];
  for (const v of views) {
    for (const d of v.openDecisions) items.push({ key: d.id, kind: d.kind === "permission" ? "Permission" : "Question", title: d.title, view: v, tab: "decisions", actionable: true });
    if (v.result?.state === "complete" && !v.accepted) items.push({ key: v.result.id, kind: "Result acceptance", title: `Review result ${v.result.id} revision ${v.result.version}`, view: v, tab: "result", actionable: true });
    if (v.result?.state === "stale") items.push({ key: v.result.id, kind: "Result acceptance", title: `Result ${v.result.id} is stale`, view: v, tab: "result", actionable: false, note: "Acceptance is disabled: the workspace changed after this result." });
  }
  return items;
}

/** Decisions count shown on the task tab: open decisions plus an acceptable result. */
export const decisionCount = (v: TaskView) => v.openDecisions.length + (v.result?.state === "complete" && !v.accepted ? 1 : 0);

export type Filters = { phase: "all" | ColumnId; wait: "all" | "none" | WaitLabel; exec: "all" | ExecLabel };
export const applyFilters = (views: readonly TaskView[], f: Filters) => views.filter(v =>
  (f.phase === "all" || v.column === f.phase)
  && (f.wait === "all" || (f.wait === "none" ? !v.wait : v.wait === f.wait))
  && (f.exec === "all" || v.exec === f.exec));

export type SortKey = "title" | "phase" | "exec" | "wait" | "updated";
export function sortViews(views: readonly TaskView[], key: SortKey, dir: 1 | -1) {
  const col = (v: TaskView) => COLUMNS.findIndex(c => c.id === v.column);
  const val = (v: TaskView): string | number => key === "title" ? v.title : key === "phase" ? col(v) : key === "exec" ? v.exec : key === "wait" ? v.wait ?? "~" : v.updatedAt ?? "";
  return [...views].sort((a, b) => { const x = val(a), y = val(b); return (x < y ? -1 : x > y ? 1 : a.ordinal - b.ordinal) * dir; });
}

export const cardLabel = (v: TaskView, conn: Conn) => [
  v.title, `Phase ${columnName(v.column)}`, `Execution ${v.exec}${v.lastKnown ? `, last known ${v.lastKnown}` : ""}`,
  v.wait ?? (v.answered ? "Answered (mock)" : ""), v.stale ? "Result stale" : "", v.failedCheck ? "Check failed" : "",
  v.id, v.created ? "Mock" : "", conn !== "connected" ? "cached state" : "",
].filter(Boolean).join(". ");
