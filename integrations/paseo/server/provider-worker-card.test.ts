import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { presentTimeline, presentWorkerCard } from "./provider.ts";
import { WorkerCardFence } from "./worker-card-fence.ts";
import { CORTEX_PLUGIN_ID, workerRowData, workerRowId } from "../shared/worker-card-item.ts";
import { resolveDurableWorkerCardStatus, workerStatusPresentation } from "../client/worker-display.ts";
import { cardAttention, cardIssueCount } from "../client/worker-card-model.ts";

// One worker card's lifecycle exactly as the installed ACP bridge projects
// it: the same `worker:<ref>` tool call reported from two coordinator turns.
const CALL = "worker:ref1";
const BASE = "Worker: explorer · codex · 6.1 Sol · medium — Read the fixture";

function toolCall(status: string, name: string, error: unknown = null) {
  return {
    type: "timeline.item" as const,
    sessionId: "s1",
    item: { type: "tool_call" as const, id: CALL, callId: CALL, name, status, error,
      detail: { type: "unknown" as const, input: null, output: typeof error === "string" ? error : null } },
  };
}

function project(event: ReturnType<typeof toolCall>) {
  return presentWorkerCard(presentTimeline(event as never)) as unknown as {
    type: string; item: { type: string; id: string; pluginId: string; kind: string; version: number; data: ReturnType<typeof workerRowData> };
  };
}

/** The installed Paseo 0.12.0 client keys a provider plugin item by
 * `<pluginId>/<id>` across turns and replaces its data in place, while a
 * tool_call item is keyed per turn (`turn:<turnId>/<callId>`). Model just
 * those identity rules over a sequence of (turn, event) records. */
function hostRows(records: Array<{ turnId: string; event: { item: Record<string, unknown> } }>) {
  const rows = new Map<string, Record<string, unknown>>();
  for (const { turnId, event } of records) {
    const item = event.item;
    const key = item.type === "plugin" ? `${String(item.pluginId)}/${String(item.id)}` : `turn:${turnId}/${String(item.callId)}`;
    rows.set(key, item);
  }
  return [...rows.values()];
}

test("a worker card becomes one stable-identity plugin item, never a per-record row", () => {
  const running = project(toolCall("running", BASE));
  assert.equal(running.type, "timeline.item");
  assert.equal(running.item.type, "plugin");
  assert.equal(running.item.id, workerRowId(CALL));
  assert.equal(running.item.pluginId, CORTEX_PLUGIN_ID);
  assert.equal(running.item.kind, "worker-row");
  assert.equal(running.item.version, 1);
  assert.deepEqual(running.item.data, { callId: CALL, assignmentRef: "ref1", title: BASE, status: "running" });
});

test("the plugin id is the manifest id the host checks provider plugin items against", () => {
  const manifest = JSON.parse(readFileSync(new URL("../paseo-plugin.json", import.meta.url), "utf8")) as { id: string };
  assert.equal(CORTEX_PLUGIN_ID, manifest.id);
});

test("active-to-cancel across two turns stays one card that reads cancelled, not failed", () => {
  const active = project(toolCall("running", BASE));
  // Stop: the runtime's cancellation arrives after the coordinator turn
  // ended, so the host attributes it to a different turn.
  const cancelled = project(toolCall("failed", `${BASE} · Cancelled`, "Cancelled: Paseo session/cancel"));
  const rows = hostRows([{ turnId: "t1", event: active }, { turnId: "t2", event: cancelled }]);
  assert.equal(rows.length, 1, "one logical worker card");
  const data = rows[0].data as ReturnType<typeof workerRowData>;
  assert.equal(data.status, "failed");
  const label = workerStatusPresentation(resolveDurableWorkerCardStatus(data.title, data.status, "cancelled")).label;
  assert.equal(label, "cancelled");
  assert.equal(cardAttention(label, cardIssueCount(label), data.failureReason), "Cancellation details in the worker tab");
  // Without any title marker (an older runtime), the durable cancellation
  // still settles the card truthfully.
  const unmarked = project(toolCall("failed", BASE, "Cancelled: urgent stop"));
  assert.equal(workerStatusPresentation(resolveDurableWorkerCardStatus(unmarked.item.data.title, unmarked.item.data.status, "cancelled")).label, "cancelled");
  // The tool_call rows the old projection produced split the same lifecycle in two.
  const legacy = hostRows([
    { turnId: "t1", event: toolCall("running", BASE) as never },
    { turnId: "t2", event: toolCall("failed", `${BASE} · Cancelled`, "Cancelled") as never },
  ]);
  assert.equal(legacy.length, 2);
});

test("duplicate live and replayed updates of one identity stay one card with the latest truthful status", () => {
  const fence = new WorkerCardFence();
  const frames = [
    { method: "session/update", params: { update: { sessionUpdate: "tool_call", toolCallId: CALL, status: "in_progress", title: BASE } } },
    { method: "session/update", params: { update: { sessionUpdate: "tool_call_update", toolCallId: CALL, status: "failed", title: `${BASE} · Cancelled` } } },
    // A late duplicate in_progress frame and a replay of the terminal frame.
    { method: "session/update", params: { update: { sessionUpdate: "tool_call_update", toolCallId: CALL, status: "in_progress" } } },
    { method: "session/update", params: { update: { sessionUpdate: "tool_call_update", toolCallId: CALL, status: "failed", title: `${BASE} · Cancelled` } } },
  ];
  for (const frame of frames) fence.observe(frame);
  assert.equal(fence.status(CALL), "failed");
  const settled = fence.toolCall({ id: CALL, status: "in_progress", name: `${BASE} · Cancelled` });
  assert.equal(settled.status, "failed", "a stale non-terminal snapshot never reopens the card");
  const live = project(toolCall("failed", `${BASE} · Cancelled`, "Cancelled: stop"));
  const replayed = project(toolCall("failed", `${BASE} · Cancelled`, "Cancelled: stop"));
  assert.deepEqual(replayed, live, "replay projects the identical item");
  const rows = hostRows([
    { turnId: "t1", event: project(toolCall("running", BASE)) },
    { turnId: "t2", event: live },
    { turnId: "replay", event: replayed },
  ]);
  assert.equal(rows.length, 1);
});

test("a genuine failure stays failed, with its short host reason handed to the worker tab", () => {
  const failed = project(toolCall("failed", BASE, "Not delegated/failed: engine exited 1"));
  assert.equal(failed.item.data.status, "failed");
  assert.equal(failed.item.data.failureReason, "Not delegated/failed: engine exited 1");
  const label = workerStatusPresentation(resolveDurableWorkerCardStatus(failed.item.data.title, failed.item.data.status, "failed")).label;
  assert.equal(label, "failed");
  assert.equal(cardAttention(label, cardIssueCount(label), failed.item.data.failureReason), "Failure details in the worker tab");
  // A durable status that is not settled never turns a failure into anything else.
  assert.equal(resolveDurableWorkerCardStatus(BASE, "failed", "running"), "failed");
  assert.equal(resolveDurableWorkerCardStatus(BASE, "failed", undefined), "failed");
});

test("saved plugin data and durable status cannot hide a failure or open checks", () => {
  const projected = presentWorkerCard(presentTimeline({
    type: "timeline.item", sessionId: "s1",
    item: { type: "tool_call", id: CALL, callId: CALL, name: BASE, status: "completed", error: null,
      detail: { type: "unknown", input: null, output: { cortexWorkerOutcome: { status: "failed", openIssues: 3 } } } },
  } as never)) as unknown as { item: { data: ReturnType<typeof workerRowData> } };
  const stored = JSON.parse(JSON.stringify(projected)).item.data;
  assert.equal(stored.status, "failed");
  assert.equal(cardIssueCount("failed", stored.openIssues), 3);
  // Old persisted plugin rows have only the host's coarse completion. The
  // exact assignment's compact history verdict/count must replace it.
  for (const [durable, issues, expected] of [
    ["failed", 3, "failed"], ["completed", 2, "completed with issues"],
    ["completed", 0, "completed"], ["cancelled", 3, "cancelled"],
  ] as const) {
    const resolved = resolveDurableWorkerCardStatus(BASE, durable === "cancelled" ? "failed" : "completed", durable, issues);
    assert.equal(workerStatusPresentation(resolved).label, expected);
  }
});

test("a delayed same-ACP-status cancellation cannot erase a committed result", () => {
  const fence = new WorkerCardFence();
  const observe = (update: Record<string, unknown>) => fence.observe({ method: "session/update", params: {
    update: { sessionUpdate: "tool_call_update", toolCallId: CALL, ...update },
  } });
  observe({ status: "failed" });
  fence.toolCall({ id: CALL, status: "failed", title: `${BASE} · Cancelled`, output: "Cancelled: replanning" });
  const correction = { id: CALL, status: "failed", title: BASE,
    output: { cortexWorkerOutcome: { status: "failed", openIssues: 1 } } };
  observe({ status: "failed", rawOutput: correction.output });
  assert.deepEqual(fence.toolCall(correction), correction);
  for (const status of ["failed", "in_progress", "completed", "canceled"]) {
    observe({ status });
    assert.deepEqual(fence.toolCall({ id: CALL, status, title: `${BASE} · Cancelled`,
      output: "Cancelled: delayed cleanup" }), correction);
  }
});

test("committed failure repairs an old Cancelled title without guessing from absent or legacy metadata", () => {
  const title = `${BASE} · Cancelled`;
  for (const output of [undefined, { summary: "old", checks: [{ name: "unit", outcome: "failed" }] },
    { cortexWorkerOutcome: { status: "failed", openIssues: 1 } }]) {
    const row = workerRowData({ callId: CALL, name: title, status: "failed",
      metadata: output && "cortexWorkerOutcome" in output ? output : { cortexWorkerResult: output } });
    const saved = JSON.parse(JSON.stringify(row));
    const label = workerStatusPresentation(resolveDurableWorkerCardStatus(
      saved.title, saved.status, "failed", 1, saved.outcomeStatus)).label;
    assert.equal(label, "failed");
    assert.equal(cardIssueCount(label, 1, saved.openIssues), 1);
    // Without a durable read, only an explicit committed outcome may
    // supersede the portable marker. A legacy count alone proves no verdict.
    const offline = workerStatusPresentation(resolveDurableWorkerCardStatus(
      saved.title, saved.status, undefined, saved.openIssues, saved.outcomeStatus)).label;
    assert.equal(offline, saved.outcomeStatus ? "failed" : "cancelled");
  }
});

test("a completed card carries only its open-issue count, and non-worker items pass through unchanged", () => {
  const completed = presentWorkerCard(presentTimeline({
    type: "timeline.item", sessionId: "s1",
    item: { type: "tool_call", id: CALL, callId: CALL, name: BASE, status: "completed", error: null,
      detail: { type: "unknown", input: null, output: { summary: "SECRET SUMMARY", checks: [{ name: "unit", outcome: "failed" }] } } },
  } as never)) as unknown as { item: { data: Record<string, unknown> } };
  assert.equal(completed.item.data.openIssues, 1);
  assert.equal(JSON.stringify(completed).includes("SECRET SUMMARY"), false);
  const native = { type: "timeline.item", sessionId: "s1",
    item: { type: "tool_call", id: "c1", callId: "c1", name: "Read", status: "completed", error: null, detail: { type: "unknown", input: null, output: null } } };
  assert.equal((presentWorkerCard(native as never) as { item: { type: string } }).item.type, "tool_call");
});
