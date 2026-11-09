import { describe, expect, it } from "vitest";
import { FIXTURE_VERSION, loadFixtures, ROUTES, SCENARIOS, SCENARIO_DETAILS, selectProjectAggregate, selectProjectAggregates, selectProjectTasks } from "./fixtures";
import { createMockStore } from "./store";
import { SCENARIO_SCRIPTS } from "./scenarios";

describe("shared synthetic fixtures", () => {
  it("repeats all data, 500 tasks, 10,000 events and unique stable IDs", () => {
    const a = loadFixtures(), b = loadFixtures();
    expect(a).toEqual(b);
    expect(a.version).toBe(FIXTURE_VERSION);
    for (const [items, count] of [[a.projects, 8], [a.tasks, 500], [a.events, 10000]] as const) {
      expect(items).toHaveLength(count);
      expect(new Set(items.map(x => x.id)).size).toBe(count);
    }
    expect(a.projects.map(x => x.id)).toEqual(b.projects.map(x => x.id));
    expect(a.tasks.map(x => x.id)).toEqual(b.tasks.map(x => x.id));
    expect(a.events.map(x => x.id)).toEqual(b.events.map(x => x.id));
    expect(a.executions.filter(x => x.state === "running")).toHaveLength(1);
    expect(a.results.map(x => x.state)).toEqual(["complete", "stale"]);
    expect(a.decisions.map(x => x.kind)).toEqual(["permission", "question"]);
    expect(a.checks.some(x => x.outcome === "failed")).toBe(true);
    expect(a.capabilities[0].available).toBe(false);
    expect(a.machines[0].state).toBe("connected");
    expect(a.machines[1].state).toBe("disconnected");
    expect(a.files[0].state).toBe("conflict");
    expect(a.git.state).toBe("conflict");
    expect(a.tasks[8].description.length).toBeGreaterThan(1000);
  });
  it.each(ROUTES)("returns identical data for route %s with independent references", route => {
    const fixture = loadFixtures(route);
    expect(fixture).toEqual(loadFixtures("a"));
    fixture.tasks[0].title = "Mutation confined to one load";
    expect(loadFixtures("b").tasks[0].title).not.toBe(fixture.tasks[0].title);
  });
  it.each(SCENARIOS)("selects scenario %s without destroying baseline data", scenario => {
    const store = createMockStore();
    store.selectScenario(scenario);
    expect(store.state().scenario).toBe(scenario);
    expect(store.state().selectedTaskId).toBe(SCENARIO_DETAILS[scenario].taskId ?? "");
    expect(store.state().fixture.tasks).toHaveLength(500);
    expect(store.visibleTasks()).toHaveLength(["empty", "error"].includes(scenario) ? 0 : 500);
    expect(Boolean(store.error())).toBe(scenario === "error");
    store.reset();
    expect(store.visibleTasks()).toHaveLength(500);
    expect(store.error()).toBe(null);
  });
  it("has internally bound task/execution/event/result identities", () => {
    const f = loadFixtures();
    const tasks = new Set(f.tasks.map(x => x.id));
    expect(f.events.every(x => tasks.has(x.taskId))).toBe(true);
    expect(f.executions.every(x => tasks.has(x.taskId))).toBe(true);
    expect(f.results.every(x => tasks.has(x.taskId) && x.checkIds.every(id => f.checks.some(c => c.id === id)))).toBe(true);
  });
  it("assigns every task and event to exactly one existing project with uneven totals", () => {
    const f = loadFixtures();
    expect(f.projects.map(p => selectProjectTasks(f, p.id).length)).toEqual([220, 90, 65, 50, 35, 25, 15, 0]);
    for (const task of f.tasks) {
      expect(f.projects.filter(p => p.id === task.projectId)).toHaveLength(1);
    }
    expect(f.events.every(e => e.projectId === f.tasks.find(t => t.id === e.taskId)?.projectId)).toBe(true);
    expect(f.projects.every(p => f.machines.some(m => m.id === p.machineId && m.state === p.connection))).toBe(true);
    expect(f.projects.find(p => p.id === f.git.projectId)?.gitState).toBe("conflict");
    expect(f.files[0].projectId).toBe(f.tasks.find(t => t.id === f.files[0].taskId)?.projectId);
    expect(f.capabilities[0].projectId).toBe(f.tasks.find(t => t.id === f.capabilities[0].taskId)?.projectId);
  });
  it("derives dashboard aggregates from project facts, retaining unknown telemetry", () => {
    const f = loadFixtures();
    const aggregates = selectProjectAggregates(f);
    for (const p of f.projects) {
      const tasks = f.tasks.filter(t => t.projectId === p.id);
      const ids = new Set(tasks.map(t => t.id));
      const aggregate = selectProjectAggregate(f, p.id)!;
      expect(aggregate).toEqual(aggregates.find(a => a.projectId === p.id));
      expect(aggregate.taskCount).toBe(tasks.length);
      for (const phase of ["backlog", "working", "review", "done"] as const) {
        expect(aggregate.taskCounts[phase]).toBe(tasks.filter(t => t.phase === phase).length);
      }
      expect(aggregate.decisionsWaiting).toBe(f.decisions.filter(d => ids.has(d.taskId) && d.state === "open").length);
      expect(aggregate.failedChecks).toBe(f.checks.filter(c => ids.has(c.taskId) && c.outcome === "failed").length);
      expect(aggregate.unrunChecks).toBe(f.checks.filter(c => ids.has(c.taskId) && c.outcome === "not_run").length);
      expect(aggregate.staleResults).toBe(f.results.filter(r => ids.has(r.taskId) && r.state === "stale").length);
      const timestamps = f.events.filter(e => ids.has(e.taskId)).map(e => e.occurredAt).sort();
      expect(aggregate.lastActivity).toBe(timestamps.at(-1) ?? null);
      expect(p.lastActivity).toBe(aggregate.lastActivity);
      expect(aggregate.runningWorkers).toBe(p.connection === "disconnected" ? "unknown" : f.executions.filter(e => ids.has(e.taskId) && e.state === "running").length);
    }
    expect(aggregates.reduce((sum, a) => sum + a.taskCount, 0)).toBe(500);
    expect(selectProjectAggregate(f, "missing")).toBeUndefined();
    expect(aggregates[7].lastActivity).toBeNull();
    // Recompute after data changes: selectors must not return cached dashboard constants.
    f.checks.splice(0, 1);
    f.decisions.splice(0, 1);
    f.results.splice(1, 1);
    f.tasks[0].phase = "done";
    expect(selectProjectAggregate(f, f.projects[3].id)?.failedChecks).toBe(0);
    expect(selectProjectAggregate(f, f.projects[1].id)?.decisionsWaiting).toBe(0);
    expect(selectProjectAggregate(f, f.projects[3].id)?.staleResults).toBe(0);
    expect(selectProjectAggregate(f, f.projects[0].id)?.taskCounts.working).toBe(0);
    f.machines.splice(0, 1);
    expect(selectProjectAggregate(f, f.projects[1].id)?.runningWorkers).toBe("unknown");
  });
  it("contains no live URLs, emails, credentials or absolute home paths", () => {
    const text = JSON.stringify({ fixture: loadFixtures(), scenarios: SCENARIO_DETAILS, scripts: SCENARIO_SCRIPTS });
    // Stricter than an example-domain allowlist: fixture content has no URL at all.
    expect(text).not.toMatch(/(?:https?|wss?|ftp):\/\/|www\./i);
    expect(text).not.toMatch(/[A-Z0-9._%+-]+@[A-Z0-9.-]+\.[A-Z]{2,}/i);
    expect(text).not.toMatch(/(?:Bearer\s+[A-Za-z0-9]|(?:^|[^A-Za-z0-9])sk-[A-Za-z0-9]|gh[pousr]_|AKIA[A-Z0-9]|-----BEGIN.*PRIVATE KEY)/);
    expect(text).not.toMatch(/(?:\/home\/|\/Users\/|\/root\/|[A-Z]:\\Users\\)/i);
    expect(text).not.toMatch(/"(?:password|secret|token|apiKey)"\s*:/i);
  });
});

describe("memory-only mock actions", () => {
  it("records create, decision and complete result, but blocks stale acceptance", () => {
    const store = createMockStore();
    expect(store.createTask("Synthetic new task").mock).toBe(true);
    expect(store.state().fixture.tasks).toHaveLength(501);
    const created = store.state().selectedTaskId;
    expect(store.decide("mock-decision-permission", "Allow mock").mock).toBe(true);
    expect(store.state().responses["mock-decision-permission"]).toBe("Allow mock");
    expect(store.acceptResult("mock-result-complete").mock).toBe(true);
    expect(store.state().acceptedResultIds).toEqual(["mock-result-complete"]);
    expect(store.acceptResult("mock-result-stale").message).toContain("blocked");
    expect(store.state().acceptedResultIds).toEqual(["mock-result-complete"]);
    store.setDevice("phone");
    store.setDevice("desktop");
    expect(store.state().selectedTaskId).toBe(created);
    expect(store.resolveConflict("local").mock).toBe(true);
    expect(store.state().conflictResolution).toBe("local");
    expect(store.state().outcomes.every(x => x.mock)).toBe(true);
    expect(loadFixtures().tasks).toHaveLength(500);
    store.reset();
    expect(store.state().responses).toEqual({});
    expect(store.state().acceptedResultIds).toEqual([]);
    expect(store.state().outcomes).toEqual([]);
  });
  it("provides all three scripts bound to the fixture revision", () => {
    expect(SCENARIO_SCRIPTS.map(s => s.id)).toEqual(["create-decision-result", "code-git-conflict", "desktop-phone-desktop"]);
    expect(SCENARIO_SCRIPTS.every(s => s.mock && s.fixtureVersion === FIXTURE_VERSION && s.steps.length > 0)).toBe(true);
  });
});
