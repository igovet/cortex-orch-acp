import { createSignal } from "solid-js";
import { loadFixtures, SCENARIO_DETAILS, type Scenario, type Concept } from "./fixtures";

export function createMockStore(concept: Concept = "a") {
  const initial = () => ({
    fixture: loadFixtures(concept), scenario: "default" as Scenario,
    selectedTaskId: "mock-task-00000", device: "desktop" as "desktop" | "phone",
    responses: {} as Record<string, string>, acceptedResultIds: [] as string[],
    conflictResolution: null as "local" | "incoming" | null,
    mock: true as const, outcomes: [] as { action: string; mock: true; message: string }[],
  });
  const [state, setState] = createSignal(initial());
  const record = (action: string, message: string) => {
    const outcome = { action, message, mock: true as const };
    setState(s => ({ ...s, outcomes: [...s.outcomes, outcome] }));
    return outcome;
  };
  return {
    state,
    visibleTasks: () => ["empty", "error"].includes(state().scenario) ? [] : state().fixture.tasks,
    error: () => state().scenario === "error" ? "Synthetic fixture read error" : null,
    selectScenario(scenario: Scenario) {
      setState(s => ({ ...s, scenario, selectedTaskId: SCENARIO_DETAILS[scenario].taskId ?? "" }));
    },
    selectTask(selectedTaskId: string) { setState(s => ({ ...s, selectedTaskId })); },
    setDevice(device: "desktop" | "phone") { setState(s => ({ ...s, device })); },
    createTask(title: string, projectId = "mock-project-00000", description = "Created in memory only.") {
      const s = state();
      const task = { id: `mock-created-task-${s.fixture.tasks.length}`, projectId, title: title.trim() || "Synthetic new task", description, phase: "backlog" as const, executionId: null };
      setState({ ...s, selectedTaskId: task.id, fixture: { ...s.fixture, tasks: [...s.fixture.tasks, task] } });
      return record("create", "Mock task created in memory; no execution started.");
    },
    decide(decisionId: string, answer: string) {
      const decision = state().fixture.decisions.find(d => d.id === decisionId);
      if (!decision) return record("decision", "Mock decision unavailable.");
      setState(s => ({ ...s, responses: { ...s.responses, [decisionId]: answer } }));
      return record("decision", `Mock ${decision.kind} response: ${answer}. No authorization was granted.`);
    },
    acceptResult(resultId: string) {
      const result = state().fixture.results.find(r => r.id === resultId);
      if (result?.state === "complete") setState(s => ({ ...s, acceptedResultIds: [...new Set([...s.acceptedResultIds, resultId])] }));
      return record("result", result?.state === "complete" ? "Mock acceptance recorded in memory. Not runtime evidence or a Git action." : "Mock acceptance blocked: result is stale or unavailable.");
    },
    resolveConflict(choice: "local" | "incoming") {
      setState(s => ({ ...s, conflictResolution: choice }));
      return record("conflict", `Mock ${choice} resolution recorded. No file or Git mutation occurred.`);
    },
    reset() { setState(initial()); },
  };
}
export type MockStore = ReturnType<typeof createMockStore>;
export type ConceptProps = { store: MockStore };
