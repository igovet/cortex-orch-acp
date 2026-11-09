import { SCENARIO_DETAILS, type Fixture, type Scenario, type Task } from "../../fixtures";
import type { MockStore } from "../../store";

// Concept C presentation model. Everything here is derived from the shared
// CW-D01 fixture and store; nothing adds facts that the fixture does not hold.
export type State = ReturnType<MockStore["state"]>;
export type FixtureEvent = Fixture["events"][number];
export type Execution = Fixture["executions"][number];
export type Decision = Fixture["decisions"][number];
export type Result = Fixture["results"][number];
export type Check = Fixture["checks"][number];
export type Outcome = Check["outcome"] | "unverified";
export type Connection = "connected" | "offline" | "disconnected";
export type Tone = "danger" | "warn" | "success" | "neutral" | "info" | "mock";
export type GroupId = "decision" | "blocked" | "changed" | "running" | "quiet";

export const GROUPS: readonly { id: GroupId; title: string }[] = [
  { id: "decision", title: "Needs your decision" },
  { id: "blocked", title: "Blocked" },
  { id: "changed", title: "Changed since last visit" },
  { id: "running", title: "Running" },
  { id: "quiet", title: "Quiet tasks" },
];

export const CONFLICT_TASK_ID = SCENARIO_DETAILS.conflict.taskId ?? "mock-task-00006";

export const PHASES: readonly { id: Task["phase"]; label: string }[] = [
  { id: "backlog", label: "Backlog" },
  { id: "working", label: "Working" },
  { id: "review", label: "Review" },
  { id: "done", label: "Done" },
];
export const phaseLabel = (phase: Task["phase"]) => PHASES.find(p => p.id === phase)?.label ?? phase;

export function connectionOf(scenario: Scenario): Connection {
  return scenario === "disconnected" ? "disconnected" : scenario === "offline" ? "offline" : "connected";
}

export function blockReason(conn: Connection, loading: boolean): string | null {
  if (conn === "disconnected") return "Disconnected; actions unavailable.";
  if (conn === "offline") return "Offline: showing the last confirmed state. Actions unavailable.";
  if (loading) return "Loading tasks; actions are unavailable until loading finishes.";
  return null;
}

export const stamp = (iso: string) => `${iso.slice(0, 10)} ${iso.slice(11, 16)} UTC`;
export const clock = (iso: string) => `${iso.slice(11, 16)} UTC`;

const taskIndex = new WeakMap<readonly FixtureEvent[], Map<string, FixtureEvent[]>>();
const execIndex = new WeakMap<readonly FixtureEvent[], Map<string, FixtureEvent[]>>();
function indexBy(events: readonly FixtureEvent[], key: "taskId" | "executionId", cache: typeof taskIndex) {
  let map = cache.get(events);
  if (!map) {
    map = new Map();
    for (const event of events) {
      const value = event[key];
      if (!value) continue;
      const list = map.get(value);
      if (list) list.push(event); else map.set(value, [event]);
    }
    cache.set(events, map);
  }
  return map;
}
export const eventsForTask = (f: Fixture, taskId: string) => indexBy(f.events, "taskId", taskIndex).get(taskId) ?? [];
export const eventsForExecution = (f: Fixture, executionId: string) => indexBy(f.events, "executionId", execIndex).get(executionId) ?? [];

export const executionFor = (f: Fixture, taskId: string) => f.executions.find(e => e.taskId === taskId) ?? null;
export const resultFor = (f: Fixture, taskId: string) => f.results.find(r => r.taskId === taskId) ?? null;
export const decisionFor = (f: Fixture, taskId: string) => f.decisions.find(d => d.taskId === taskId) ?? null;
export const checksForTask = (f: Fixture, taskId: string) => f.checks.filter(c => c.taskId === taskId);
export const isCreated = (taskId: string) => taskId.startsWith("mock-created-task-");
export const hasChanges = (f: Fixture, taskId: string) => !!resultFor(f, taskId) || taskId === CONFLICT_TASK_ID;

export const OUTCOME_ORDER: Record<Outcome, number> = { failed: 0, not_run: 1, unverified: 2, passed: 3 };
export const OUTCOME_LABEL: Record<Outcome, string> = { failed: "Failed", not_run: "Not run", unverified: "Unverified", passed: "Passed" };
export const OUTCOME_TONE: Record<Outcome, Tone> = { failed: "danger", not_run: "neutral", unverified: "neutral", passed: "success" };
export const OUTCOME_GLYPH: Record<Outcome, string> = { failed: "✕", not_run: "⊖", unverified: "?", passed: "✓" };

export function countChecks(checks: readonly Check[]) {
  const counts: Record<Outcome, number> = { passed: 0, failed: 0, not_run: 0, unverified: 0 };
  for (const check of checks) counts[check.outcome]++;
  return counts;
}
export const evidenceLine = (checks: readonly Check[]) => {
  const n = countChecks(checks);
  return `${n.failed} Failed · ${n.not_run} Not run · ${n.unverified} Unverified · ${n.passed} Passed`;
};

export function executionLabel(e: Execution | null, conn: Connection): string {
  if (!e) return "Not started";
  if (conn !== "connected" && e.state === "running") return "Unknown";
  return ({ running: "Running", blocked: "Waiting", waiting: "Waiting", failed: "Failed", complete: "Idle" } as const)[e.state];
}
export function executionTone(label: string): Tone {
  return label === "Running" ? "info" : label === "Waiting" ? "warn" : label === "Failed" ? "danger" : "neutral";
}
export const executionGlyph = (label: string) =>
  ({ Running: "◐", Waiting: "‖", Failed: "✕", Idle: "−", Unknown: "?", "Not started": "−" } as Record<string, string>)[label] ?? "−";

export function waitLabel(e: Execution | null): string | null {
  if (!e?.waitReason) return null;
  return e.waitReason === "permission" ? "Your permission needed" : "Your answer needed";
}

export type QueueItem = {
  key: string; taskId: string; title: string; group: GroupId;
  badge: string; glyph: string; tone: Tone; ask: string; meta: string;
};

/** Fixed fixture ranking: sections in GROUPS order, items in fixture order. Never re-sorted. */
export function buildQueue(s: State, tasks: readonly Task[], conn: Connection): QueueItem[] {
  const items: QueueItem[] = [];
  if (!tasks.length) return items;
  const f = s.fixture;
  const byId = new Map(tasks.map(t => [t.id, t] as const));
  const used = new Set<string>();
  const lastEvent = (id: string) => {
    const events = eventsForTask(f, id);
    return events.length ? `Last event ${clock(events[events.length - 1].occurredAt)} · observed` : "No events";
  };
  const push = (task: Task, group: GroupId, badge: string, glyph: string, tone: Tone, ask: string, meta: string) => {
    used.add(task.id);
    items.push({ key: task.id, taskId: task.id, title: task.title, group, badge, glyph, tone, ask, meta });
  };
  for (const d of f.decisions) {
    const task = byId.get(d.taskId);
    if (!task || used.has(task.id)) continue;
    const label = d.kind === "permission" ? "Permission" : "Question";
    const glyph = d.kind === "permission" ? "◆" : "?";
    const answer = s.responses[d.id];
    if (answer) push(task, "changed", label, glyph, "neutral", `Answered (mock): ${answer}`, "Recorded in memory only");
    else push(task, "decision", label, glyph, "warn", d.title, `${d.kind === "permission" ? "Your permission needed" : "Your answer needed"} · requested: unknown`);
  }
  for (const r of f.results) {
    const task = byId.get(r.taskId);
    if (!task || used.has(task.id)) continue;
    if (r.state === "stale") push(task, "decision", "Stale", "↺", "warn", "Result changed; review again", `Result review needed · ${r.id}`);
    else if (s.acceptedResultIds.includes(r.id)) push(task, "changed", "Accepted (mock)", "✓", "mock", `Accepted (mock) · ${r.id}`, "Not runtime evidence");
    else push(task, "decision", "Result acceptance", "▣", "warn", `Review result ${r.id} v${r.version}`, "Result review needed · produced: unknown");
  }
  for (const e of f.executions) {
    const task = byId.get(e.taskId);
    if (!task || used.has(task.id) || e.state !== "failed") continue;
    const failed = checksForTask(f, task.id).filter(c => c.outcome === "failed").length;
    push(task, "blocked", "Failed check", "✕", "danger", `Execution failed · ${failed} check failed`, lastEvent(task.id));
  }
  for (const task of tasks) {
    if (used.has(task.id)) continue;
    if (isCreated(task.id)) push(task, "changed", "New", "+", "neutral", "Not started (mock)", "Created in memory · no events");
    else if (task.id === CONFLICT_TASK_ID) push(task, "changed", "Conflict", "≠", "danger", `Merge stopped: ${f.git.fileIds.length} file has conflicts on ${f.git.branch}`, lastEvent(task.id));
  }
  for (const e of f.executions) {
    const task = byId.get(e.taskId);
    if (!task || used.has(task.id) || e.state !== "running") continue;
    const label = executionLabel(e, conn);
    push(task, "running", label, executionGlyph(label), executionTone(label),
      conn === "connected" ? "Execution running · observed" : "Last confirmed running; current state unknown", lastEvent(task.id));
  }
  for (const task of tasks) {
    if (used.has(task.id)) continue;
    const label = executionLabel(executionFor(f, task.id), conn);
    push(task, "quiet", label, executionGlyph(label), "neutral", "Nothing needs you", lastEvent(task.id));
  }
  return items;
}

export type DiffLine = { kind: "del" | "add"; text: string };
/** Synthetic diff derived from the fixture file record (base → local). */
export function fileDiffs(f: Fixture) {
  return f.files.map(file => ({ file, lines: [{ kind: "del", text: file.base }, { kind: "add", text: file.local }] as DiffLine[] }));
}
