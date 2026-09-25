import assert from "node:assert/strict";
import test from "node:test";
import { buildPipeline } from "./pipeline-model.ts";
import { resolveWorkerCardStatus, workerCardMetadata, workerTerminalState } from "./worker-display.ts";
import { workerPopoverStatus } from "./worker-popover-status.ts";

const assignment = (status: string, checks: { name: string; outcome: string }[]) => ({
  taskId: "t", id: "worker", assignmentRef: "worker-ref", nodeKey: "worker", role: "worker-qa-engineer", title: "Real shaped worker",
  status, attempts: [{ id: "attempt", attemptNo: 1, status, failureReason: null, checks, events: [{ id: 1, type: "worker_result", createdAt: "now", text: "result" }] }],
});

function pipeline(worker: ReturnType<typeof assignment>, checks: { name: string; outcome: string }[]) {
  return {
    available: true, tokens: { total: null, coordinator: null }, task: { id: "t", createdAt: "now", state: "running", waitReason: null, waitDetail: null },
    messages: [], requirements: [], plan: null, workers: [worker],
    delegations: { "worker-ref": { order: 1, result: { summary: "Worker result", checks }, decision: null } },
  };
}

// The chat card resolves its status with zero RPC, from the host's own ACP
// status (`host`) plus the runtime's portable title suffix (T035's
// "Completed with issues", and this same channel for "Cancelled" -- see
// orchestrating_runtime.py's `assignment_cancelled` handler); Pipeline and
// the Workers tab instead read the durable pipeline projection
// (`workerPopoverStatus`). Both must still agree on the final label/tone.
test("chat card (zero-RPC title channel), Pipeline, and worker tab agree on one terminal projection", () => {
  const cases = [
    { status: "completed", host: "completed", titleSuffix: "", checks: [], expected: { label: "completed", tone: "success", issueCount: 0, pipeline: "done" } },
    { status: "completed_with_issues", host: "completed", titleSuffix: " · Completed with issues", checks: [{ name: "failing check", outcome: "failed" }], expected: { label: "completed with issues", tone: "warning", issueCount: 1, pipeline: "issues" } },
    { status: "cancelled", host: "failed", titleSuffix: " · Cancelled", checks: [], expected: { label: "cancelled", tone: "muted", issueCount: 0, pipeline: "cancelled" } },
    { status: "failed", host: "failed", titleSuffix: "", checks: [], expected: { label: "failed", tone: "danger", issueCount: 0, pipeline: "failed" } },
  ];
  for (const { status, host, titleSuffix, checks, expected } of cases) {
    const worker = assignment(status, checks);
    const title = `Worker: qa-engineer · claude · Haiku · medium${titleSuffix} — Real shaped worker`;
    const durableStatus = resolveWorkerCardStatus(title, host);
    const terminal = workerTerminalState(durableStatus, checks);
    const chat = workerCardMetadata(title, host);
    const step = buildPipeline(pipeline(worker, checks)).steps.find((item) => item.assignmentRef === "worker-ref");

    assert.equal(workerPopoverStatus(worker), status);
    assert.deepEqual({ label: terminal.label, tone: terminal.tone, issueCount: terminal.issueCount }, {
      label: expected.label, tone: expected.tone, issueCount: expected.issueCount,
    });
    assert.match(chat.subtitle, new RegExp(`${expected.label.replace(/[.*+?^${}()|[\\]\\\\]/g, "\\$&")}$`));
    assert.equal(step?.status, expected.pipeline);
    assert.equal(step?.badges.some((badge) => badge.text === `${expected.issueCount} issue${expected.issueCount === 1 ? "" : "s"}`), expected.issueCount > 0);
  }
});
