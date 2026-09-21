import assert from "node:assert/strict";

// A sessionStorage stand-in, shared across module evaluations like the page's.
const store = new Map<string, string>();
(globalThis as { sessionStorage?: unknown }).sessionStorage = {
  getItem: (key: string) => store.get(key) ?? null,
  setItem: (key: string, value: string) => void store.set(key, value),
  removeItem: (key: string) => void store.delete(key),
};

const first = await import("./worker-selection.ts");
const view = (selected: string | null): import("./worker-selection.ts").WorkerPanelView => ({ selected, history: null, historyContext: null,
  nextCursor: null, nextAssignmentCursor: null, expandedEvent: 3, expandedGroups: [1], scroll: { "page:x": 120 } });

assert.equal(first.savedWorkerView("a"), null);
first.selectWorker("a", "ref-1");
first.saveWorkerView("a", view("attempt-7"));

// Paseo re-evaluating the plugin module loses module memory, not the view.
const second = await import("./worker-selection.ts?reevaluated");
assert.deepEqual(second.savedWorkerView("a"), view("attempt-7"));
assert.equal(second.selectedWorker("a"), "ref-1");

// A chat card handing off a worker starts that worker fresh.
second.selectWorker("a", "ref-2");
assert.equal(second.savedWorkerView("a"), null);
assert.equal(second.selectedWorker("a"), "ref-2");
