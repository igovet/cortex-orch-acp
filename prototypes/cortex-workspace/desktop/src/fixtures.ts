import { createWorkspaceFixture } from "./workspace/adapter/fixtures";

export const FIXTURE_VERSION = "cw-fixtures-v3";
export const FIXTURE_SEED = 13014;
export const ROUTES = ["a", "b", "c"] as const;
export type Concept = typeof ROUTES[number];
export const SCENARIOS = ["default", "blocked", "stale", "offline", "disconnected", "conflict", "empty", "error"] as const;
export type Scenario = typeof SCENARIOS[number];
export type Task = {
  id: string; projectId: string; title: string; description: string;
  phase: "backlog" | "working" | "review" | "done";
  executionId: string | null;
};
export type Fixture = ReturnType<typeof generateFixture>;
type Machine = { id: string; kind: "local" | "remote"; name: string; state: "connected" | "disconnected"; lastConfirmedAt: string; currentExecutionState: "unknown" | "confirmed" };

const id = (kind: string, n: number) => `mock-${kind}-${String(n).padStart(5, "0")}`;
export type Project = {
  id: string; name: string; repository: string; machineId: string;
  connection: "connected" | "disconnected"; branch: string;
  gitState: "clean" | "conflict"; lastActivity: string | null;
};

// Counts include named scenario tasks; the final project deliberately has no tasks.
const PROJECT_TASK_COUNTS = [220, 90, 65, 50, 35, 25, 15, 0];
function generateFixture() {
  let seed = FIXTURE_SEED;
  const random = () => ((seed = (Math.imul(seed, 1664525) + 1013904223) >>> 0) / 4294967296);
  const subjects = ["Amber", "Quartz", "Indigo", "Cedar", "Orbit", "Mosaic"];
  const projects: Project[] = [
    ["Mosaic Studio", "fictional/mosaic-studio", "feature/layout"],
    ["Cedar Notes", "fictional/cedar-notes", "main"],
    ["Orbit Ledger", "fictional/orbit-ledger", "feature/import"],
    ["Quartz Search", "fictional/quartz-search", "fix/index"],
    ["Amber Components", "fictional/amber-components", "synthetic-layout"],
    ["Indigo Maps", "fictional/indigo-maps", "feature/tiles"],
    ["Willow Archive", "fictional/willow-archive", "main"],
    ["Harbor Sandbox", "fictional/harbor-sandbox", "main"],
  ].map(([name, repository, branch], n) => ({
    id: id("project", n), name, repository, branch, machineId: id("machine", n === 0 ? 1 : 0),
    connection: n === 0 ? "disconnected" : "connected",
    gitState: n === 4 ? "conflict" : "clean", lastActivity: null,
  }));
  const namedProjects = [0, 1, 2, 3, 3, 1, 4, 5, 6];
  const remaining = [...PROJECT_TASK_COUNTS];
  namedProjects.forEach(n => remaining[n]--);
  const taskProjects = [...namedProjects, ...remaining.flatMap((count, n) => Array<number>(count).fill(n))];
  const tasks: Task[] = Array.from({ length: 500 }, (_, n) => ({
    id: id("task", n), projectId: projects[taskProjects[n]].id,
    title: `${subjects[Math.floor(random() * subjects.length)]} synthetic task ${n + 1}`,
    description: n === 8 ? "Long keyboard case: inspect with Tab, Shift+Tab, Enter, Space and Escape. ".repeat(24) : "Synthetic task for concept comparison.",
    phase: n === 0 ? "working" : n === 3 || n === 4 ? "review" : n === 5 ? "done" : "backlog",
    executionId: n < 6 ? id("execution", n) : null,
  }));
  const events = Array.from({ length: 10000 }, (_, n) => ({
    id: id("event", n), taskId: id("task", n % 500), projectId: tasks[n % 500].projectId,
    executionId: n % 500 < 6 ? id("execution", n % 500) : null,
    sequence: n + 1, occurredAt: new Date(Date.UTC(2030, 0, 1) + n * 1000).toISOString(),
    kind: ["discussion", "plan", "check", "snapshot"][n % 4],
    text: `Synthetic history entry ${n + 1}`, mock: true as const,
  }));
  for (const event of events) {
    const project = projects.find(p => p.id === event.projectId)!;
    project.lastActivity = event.occurredAt;
  }
  return {
    version: FIXTURE_VERSION, seed: FIXTURE_SEED, mock: true as const,
    projects,
    tasks, events,
    workspace: createWorkspaceFixture({ projects, tasks, events }),
    executions: Array.from({ length: 6 }, (_, n) => ({
      id: id("execution", n), taskId: id("task", n), sessionId: id("session", n),
      state: (["running", "blocked", "waiting", "failed", "complete", "complete"] as const)[n],
      waitReason: n === 1 ? "permission" : n === 2 ? "question" : null,
    })),
    decisions: [
      { id: "mock-decision-permission", taskId: id("task", 1), version: 1, kind: "permission" as const, state: "open" as const, title: "Allow mock workspace edit?", effect: "Changes only the in-memory fixture.", scope: "Cedar Notes", rejection: "Mock execution remains blocked." },
      { id: "mock-decision-question", taskId: id("task", 2), version: 1, kind: "question" as const, state: "open" as const, title: "Which synthetic layout?", options: ["Compact", "Comfortable"], effect: "Records a mock answer.", scope: "Synthetic task 3", rejection: "Mock question remains unanswered." },
    ],
    checks: [
      { id: "mock-check-failed", taskId: id("task", 3), outcome: "failed" as const, name: "Synthetic contrast check", detail: "Mock failure: secondary label needs review." },
      { id: "mock-check-passed", taskId: id("task", 5), outcome: "passed" as const, name: "Synthetic structure check", detail: "Mock outcome only; no command was run." },
      { id: "mock-check-unrun", taskId: id("task", 4), outcome: "not_run" as const, name: "Physical device check", detail: "Not available in this fixture." },
    ],
    results: [
      { id: "mock-result-complete", taskId: id("task", 5), version: 1, state: "complete" as const, artifactId: "mock-artifact-complete", artifactDigest: "mock-digest-amber-v1", workspaceDigest: "mock-digest-amber-v1", checkIds: ["mock-check-passed"], mock: true as const },
      { id: "mock-result-stale", taskId: id("task", 4), version: 1, state: "stale" as const, artifactId: "mock-artifact-stale", artifactDigest: "mock-digest-quartz-v1", workspaceDigest: "mock-digest-quartz-v2", checkIds: ["mock-check-unrun"], mock: true as const },
    ],
    machines: [
      { id: id("machine", 0), kind: "local" as const, name: "This machine", state: "connected" as const, lastConfirmedAt: "2030-01-01T00:00:00.000Z", currentExecutionState: "confirmed" as const },
      { id: id("machine", 1), kind: "remote" as const, name: "Synthetic remote", state: "disconnected" as const, lastConfirmedAt: "2030-01-01T00:00:00.000Z", currentExecutionState: "unknown" as const },
    ] as Machine[],
    files: [{ id: "mock-file-00000", projectId: id("project", 4), taskId: id("task", 6), path: "synthetic/layout.ts", state: "conflict" as const, base: "layout = compact", local: "layout = comfortable", incoming: "layout = focused" }],
    git: { projectId: id("project", 4), taskId: id("task", 6), branch: projects[4].branch, state: "conflict" as const, fileIds: ["mock-file-00000"], detail: "Mock merge conflict; no Git operation runs." },
    capabilities: [{ id: "mock-capability-push", projectId: id("project", 5), taskId: id("task", 7), name: "Android push", available: false, reason: "Intentionally unavailable in the mock prototype." }],
    keyboardCase: { taskId: id("task", 8), title: "Keyboard and long-text case", keys: ["Tab", "Shift+Tab", "Enter", "Space", "Escape"] },
  };
}

/** Baseline facts, not live engine telemetry. Unknown current workers stay unknown. */
export function selectProjectAggregate(fixture: Fixture, projectId: string) {
  const project = fixture.projects.find(p => p.id === projectId);
  if (!project) return undefined;
  const tasks = fixture.tasks.filter(t => t.projectId === projectId);
  const taskIds = new Set(tasks.map(t => t.id));
  const belongs = (item: { taskId: string }) => taskIds.has(item.taskId);
  const taskCounts: Record<Task["phase"], number> = { backlog: 0, working: 0, review: 0, done: 0 };
  tasks.forEach(t => taskCounts[t.phase]++);
  const events = fixture.events.filter(belongs);
  const machine = fixture.machines.find(m => m.id === project.machineId);
  return {
    projectId, taskCount: tasks.length, taskCounts,
    decisionsWaiting: fixture.decisions.filter(d => belongs(d) && d.state === "open").length,
    failedChecks: fixture.checks.filter(c => belongs(c) && c.outcome === "failed").length,
    unrunChecks: fixture.checks.filter(c => belongs(c) && c.outcome === "not_run").length,
    runningWorkers: !machine || machine.state === "disconnected" || machine.currentExecutionState === "unknown"
      ? "unknown" as const : fixture.executions.filter(e => belongs(e) && e.state === "running").length,
    staleResults: fixture.results.filter(r => belongs(r) && r.state === "stale").length,
    lastActivity: events.reduce<string | null>((latest, e) => !latest || e.occurredAt > latest ? e.occurredAt : latest, null),
  };
}

export function selectProjectAggregates(fixture: Fixture) {
  return fixture.projects.map(p => selectProjectAggregate(fixture, p.id)!);
}

export function selectProjectTasks(fixture: Fixture, projectId: string) {
  return fixture.tasks.filter(t => t.projectId === projectId);
}

/** Route is intentionally not an input to generation; every concept gets equal data. */
export function loadFixtures(_concept: Concept = "a"): Fixture {
  return generateFixture();
}

export const SCENARIO_DETAILS: Record<Scenario, { title: string; taskId: string | null; message: string }> = {
  default: { title: "Shared fixture ready", taskId: id("task", 0), message: "One mock running execution. No agent is connected." },
  blocked: { title: "Permission blocked", taskId: id("task", 1), message: "Open the mock permission decision. Rejection keeps it blocked." },
  stale: { title: "Stale result", taskId: id("task", 4), message: "Artifact and workspace revisions differ. Mock acceptance is disabled." },
  offline: { title: "Offline", taskId: id("task", 0), message: "Last confirmed 2030-01-01 00:00 UTC. Current execution state is unknown." },
  disconnected: { title: "Machine disconnected", taskId: id("task", 0), message: "Last confirmed 2030-01-01 00:00 UTC. No reconnect or command is sent." },
  conflict: { title: "File and Git conflict", taskId: id("task", 6), message: "Compare base, local and incoming synthetic text before a mock resolution." },
  empty: { title: "No mock tasks in this view", taskId: null, message: "Choose Default to restore the shared dataset." },
  error: { title: "Mock read error", taskId: null, message: "Synthetic load failure. Reset the mock store to recover; no request failed." },
};
