import { strict as assert } from "node:assert";
import { buildPipeline } from "./pipeline-model.ts";

const worker = (id: string, status: string, title: string) => ({
  taskId: "t", id, assignmentRef: `ref-${id}`, nodeKey: "delegate:x", title, role: "backend-dev", status,
  route: { provider: "codex", model: "gpt-5.6-luna", effort: "medium" },
  attempts: [{ id: `a-${id}`, attemptNo: 1, status, failureReason: null, events: [] }],
});
const base = {
  available: true,
  tokens: { total: null, coordinator: null },
  task: { id: "t", createdAt: "now", state: "running", waitReason: null, waitDetail: null },
  messages: [{ text: "Fix the counter", createdAt: "now" }, { text: "Continue", createdAt: "later" }],
  requirements: [{ id: "r", statement: "Counter increments", criteria: ["Starts at 0", "Reset works"] }],
  delegations: {
    "ref-b": { order: 20, result: null, decision: null },
    "ref-a": { order: 10, result: { summary: "Fixed", checks: [{ name: "adds", outcome: "passed" }, { name: "width", outcome: "failed" }] }, decision: { decision: "accept", rationale: null } },
  },
  plan: null,
  workers: [worker("b", "running", "QA"), worker("a", "reported", "Implement")],
};

let view = buildPipeline(base);
assert.deepEqual(view.steps.map((s) => [s.kind, s.status]), [
  ["request", "done"], ["plan", "done"], ["worker", "done"], ["worker", "active"], ["finish", "pending"],
]);
assert.equal(view.steps[2].title, "Implement");
assert.equal(view.steps[2].subtitle, "backend-dev · gpt-5.6-luna · medium");
assert.deepEqual(view.steps[2].badges, [{ text: "Accepted", tone: "success" }]);
assert.deepEqual(view.steps[2].items.map((i) => i.status), ["done", "failed"]);
assert.deepEqual(view.steps[0].followUps, ["Continue"]);
assert.equal(view.done, 3);

// Once workers settle the coordinator owns the next move.
view = buildPipeline({ ...base, workers: [worker("a", "reported", "Implement")] });
assert.equal(view.steps.at(-1)?.status, "active");

// Nothing planned yet: the plan is the live step.
view = buildPipeline({ ...base, requirements: [], workers: [], delegations: {} });
assert.deepEqual(view.steps.map((s) => s.status), ["done", "active", "pending"]);

view = buildPipeline({ ...base, task: { ...base.task, state: "accepted" } });
assert.equal(view.steps.at(-1)?.status, "done");
assert.ok(view.steps[1].items.every((i) => i.status === "done"));
assert.equal(view.headline.text, "Completed");

view = buildPipeline({ ...base, task: { ...base.task, state: "idle_awaiting_user", waitDetail: "Needs a decision" } });
assert.equal(view.steps.at(-1)?.status, "blocked");
assert.equal(view.steps.at(-1)?.body, "Needs a decision");
// Accepted runtime plan: nodes appear before any worker starts, a started
// node is shown once (by its worker), and a waiting node says why.
const node = (id: string, status: string, assignmentRef: string | null, waiting: string | null = null) => ({
  id, role: "worker-debugger", objective: `Diagnose ${id}. Read-only.`, stage: "execution", status, assignmentRef, waiting,
});
view = buildPipeline({ ...base, requirements: [], delegations: {}, workers: [worker("a", "running", "Implement")],
  plan: { state: "EXECUTING", blocker: null, nodes: [node("a", "running", "ref-a"), node("b", "pending", null, "Waiting for a")] } });
assert.deepEqual(view.steps.map((s) => [s.kind, s.status, s.title]), [
  ["request", "done", "Request"], ["plan", "done", "Plan"], ["worker", "active", "Implement"],
  ["worker", "pending", "Diagnose b."], ["finish", "pending", "Review and acceptance"],
]);
assert.equal(view.steps[1].subtitle, "2 planned steps");
assert.equal(view.steps[3].subtitle, "debugger · Waiting for a");

// Waiting for a step is not the coordinator working: nothing pulses.
const waiting = { ...base, requirements: [], delegations: {}, workers: [],
  plan: { state: "EXECUTING", blocker: null, nodes: [node("a", "pending", null, "Waiting to start")] } };
view = buildPipeline(waiting);
assert.deepEqual(view.steps.map((s) => s.status), ["done", "done", "pending", "pending"]);
assert.equal(view.steps.at(-1)?.title, "Review and acceptance");
view = buildPipeline({ ...waiting, plan: { ...waiting.plan, state: "RECONCILING", nodes: [node("a", "completed", null)] } });
assert.deepEqual([view.steps.at(-1)?.status, view.steps.at(-1)?.title], ["active", "Coordinator is reviewing"]);
view = buildPipeline({ ...waiting, plan: { ...waiting.plan, state: "VERIFYING", nodes: [node("a", "completed", null)] } });
assert.deepEqual([view.steps.at(-1)?.status, view.steps.at(-1)?.title], ["active", "Running verification"]);
view = buildPipeline({ ...waiting, plan: { state: "PLANNING", blocker: null, nodes: [] } });
assert.deepEqual(view.steps.map((s) => s.status), ["done", "active", "pending"]);

view = buildPipeline({ ...base, requirements: [], delegations: {}, workers: [],
  task: { ...base.task, state: "blocked" },
  plan: { state: "BLOCKED", blocker: "scheduler_capacity_timeout", nodes: [node("a", "cancelled", null)] } });
assert.equal(view.steps[2].status, "cancelled");
assert.equal(view.steps.at(-1)?.body, "scheduler_capacity_timeout");
console.log("pipeline model tests passed");
