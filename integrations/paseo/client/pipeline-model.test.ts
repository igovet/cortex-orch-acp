import { strict as assert } from "node:assert";
import { buildPipeline } from "./pipeline-model.ts";

const worker = (id: string, status: string, title: string) => ({
  taskId: "t", id, assignmentRef: `ref-${id}`, nodeKey: "delegate:x", title, role: "backend-dev", status,
  route: { provider: "codex", model: "gpt-5.6-luna", modelName: "Luna", effort: "medium" },
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

// The base fixture's "ref-a" delegation carries a failed check: a "reported"
// worker with a non-passed check must show as "issues", never "done".
let view = buildPipeline(base);
assert.deepEqual(view.steps.map((s) => [s.kind, s.status]), [
  ["request", "done"], ["plan", "done"], ["worker", "issues"], ["worker", "active"], ["finish", "pending"],
]);
assert.equal(view.steps[2].title, "Implement");
assert.equal(view.steps[2].subtitle, "backend-dev · Luna · medium");

// When catalog evidence is missing, keep the actual route value visible and
// mark it as an alias instead of presenting it as a concrete model name.
const unresolvedWorker = worker("a", "reported", "Implement");
unresolvedWorker.route.modelName = null;
const unresolved = buildPipeline({ ...base, workers: [unresolvedWorker] });
assert.equal(unresolved.steps[2].subtitle, "backend-dev · gpt-5.6-luna (alias) · medium");
assert.deepEqual(view.steps[2].badges, [{ text: "Accepted", tone: "success" }, { text: "1 issue", tone: "warning" }]);
assert.deepEqual(view.steps[2].items.map((i) => i.status), ["done", "failed"]);
assert.deepEqual(view.steps[0].followUps, ["Continue"]);
assert.equal(view.done, 2);

// P1 regression: a "reported" or "completed" status word must never hide a
// non-passed check behind the success icon, for every non-passed outcome.
for (const outcome of ["failed", "not_run", "unverified"]) {
  for (const rawStatus of ["reported", "completed"]) {
    const issueWorker = worker("a", rawStatus, "Implement");
    view = buildPipeline({ ...base, delegations: { ...base.delegations,
      "ref-a": { order: 10, result: { summary: "x", checks: [{ name: "check", outcome }] }, decision: null } },
      workers: [issueWorker] });
    assert.deepEqual([view.steps[2].status, view.steps[2].badges], ["issues", [{ text: "1 issue", tone: "warning" }]]);
  }
}

// The same statuses with every check passed stay "done".
for (const rawStatus of ["reported", "completed"]) {
  const cleanWorker = worker("a", rawStatus, "Implement");
  view = buildPipeline({ ...base, delegations: { ...base.delegations,
    "ref-a": { order: 10, result: { summary: "x", checks: [{ name: "check", outcome: "passed" }] }, decision: null } },
    workers: [cleanWorker] });
  assert.equal(view.steps[2].status, "done");
}

const partialWorker = worker("a", "completed_with_issues", "Implement");
view = buildPipeline({ ...base, workers: [partialWorker] });
assert.deepEqual([view.steps[2].status, view.steps[2].badges.at(-1)], ["issues", { text: "1 issue", tone: "warning" }]);

// A worker whose graph/runtime status string the popover does not recognize
// (e.g. a stray "needs_revision" work-graph attempt state, or any other
// value the runtime-nodes join could not normalize) must not fall back to
// the unstarted numbered placeholder once it has actually reported a
// result. The reported checks are the authoritative terminal signal.
const unrecognizedStatusWorker = worker("a", "needs_revision", "Implement");
view = buildPipeline({ ...base, workers: [unrecognizedStatusWorker] });
assert.deepEqual([view.steps[2].status, view.steps[2].badges.at(-1)], ["issues", { text: "1 issue", tone: "warning" }]);

// The same unrecognized status, but with every reported check passed, is a
// reported success, not a pending placeholder.
const unrecognizedStatusCleanWorker = worker("a", "needs_revision", "Implement");
view = buildPipeline({ ...base, delegations: { ...base.delegations,
  "ref-a": { order: 10, result: { summary: "Fixed", checks: [{ name: "adds", outcome: "passed" }] }, decision: null } },
  workers: [unrecognizedStatusCleanWorker] });
assert.equal(view.steps[2].status, "done");

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
  id, role: "worker-debugger", objective: `Diagnose ${id}. Read-only.`, stage: "execution", status, assignmentRef, waiting, result: null,
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

// A runtime-rejected report is visibly completed with issues, rather than
// looking like an unstarted numbered node. A failure without a result stays a hard failure.
const result = { summary: "Implemented with one failed check", checks: [{ name: "typecheck", outcome: "passed" }, { name: "browser flow", outcome: "failed" }] };
const issueNode = { ...node("issues", "failed", null), result };
view = buildPipeline({ ...waiting, plan: { state: "EXECUTING", blocker: null, nodes: [issueNode, node("hard-failure", "failed", null)] } });
assert.deepEqual(view.steps.slice(2, 4).map((step) => [step.status, step.badges, step.items.map((item) => item.status)]), [
  ["issues", [{ text: "1 issue", tone: "warning" }], ["done", "failed"]],
  ["failed", [], []],
]);

// A planned node whose raw runtime status is neither "completed" nor
// "failed" (any value NODE_STATUS does not enumerate) still shows its
// reported outcome rather than the unstarted number, once it has a result.
const unrecognizedStatusNode = { ...node("unrecognized", "needs_revision", null), result };
view = buildPipeline({ ...waiting, plan: { state: "EXECUTING", blocker: null, nodes: [unrecognizedStatusNode] } });
assert.deepEqual([view.steps[2].status, view.steps[2].badges], ["issues", [{ text: "1 issue", tone: "warning" }]]);

const cleanResult = { summary: "All checks passed", checks: [{ name: "typecheck", outcome: "passed" }] };
const unrecognizedStatusCleanNode = { ...node("clean", "needs_revision", null), result: cleanResult };
view = buildPipeline({ ...waiting, plan: { state: "EXECUTING", blocker: null, nodes: [unrecognizedStatusCleanNode] } });
assert.equal(view.steps[2].status, "done");

// P1 regression, node path: a "completed" raw status must never hide a
// non-passed check behind the success icon, for every non-passed outcome.
for (const outcome of ["failed", "not_run", "unverified"]) {
  const completedIssueNode = { ...node("completed-issue", "completed", null), result: { summary: "x", checks: [{ name: "check", outcome }] } };
  view = buildPipeline({ ...waiting, plan: { state: "EXECUTING", blocker: null, nodes: [completedIssueNode] } });
  assert.deepEqual([view.steps[2].status, view.steps[2].badges], ["issues", [{ text: "1 issue", tone: "warning" }]]);
}

// "completed" with every check passed stays "done".
const completedCleanNode = { ...node("completed-clean", "completed", null), result: { summary: "x", checks: [{ name: "check", outcome: "passed" }] } };
view = buildPipeline({ ...waiting, plan: { state: "EXECUTING", blocker: null, nodes: [completedCleanNode] } });
assert.equal(view.steps[2].status, "done");

console.log("pipeline model tests passed");
