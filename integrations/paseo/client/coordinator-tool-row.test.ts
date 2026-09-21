import assert from "node:assert/strict";
import test from "node:test";
import { projectCoordinatorToolRow } from "./coordinator-tool-row.ts";

function row(name: string, input: unknown, callId = "coordinator:1") {
  return projectCoordinatorToolRow({
    callId,
    name,
    detail: { type: "unknown", input },
  });
}

test("Cortex system operations become compact rows", () => {
  for (const [tool, expected] of [
    ["read_report", "Read report — by worker-backend"],
    ["update_plan", "Update plan — frontend"],
    ["set_governance", "Set governance — light"],
    ["publish_result", "Publish result — report-1"],
  ] as const) {
    const input = tool === "read_report"
      ? { report_id: "report-1", author: "worker-backend" }
      : tool === "update_plan"
        ? { node_key: "frontend" }
        : tool === "set_governance"
          ? { mode: "light" }
          : { report_ref: "report-1" };
    const projection = row(`Read ${tool}`, { server: "cortex", tool, arguments: input });
    assert.equal(projection?.hidden, true);
    assert.equal(projection?.presentation?.expandable, false);
    assert.equal(projection?.presentation?.label, expected);
  }
});

test("native Read and Skill stay compact; Edit and Shell stay native-expandable", () => {
  assert.equal(row("Read — seed.txt", { file_path: "/project/seed.txt" })?.presentation?.expandable, false);
  assert.equal(row("Skill — knowledge-harvest", { skill: "knowledge-harvest" })?.presentation?.expandable, false);
  assert.equal(row("Edit seed.txt", { file_path: "/project/seed.txt", old_string: "a", new_string: "b" }), undefined);
  assert.equal(row("Shell pwd", { command: "pwd" }), undefined);
});

test("delegate is hidden and worker rows remain owned by direct-click renderer", () => {
  const delegate = row("Delegate", { server: "cortex", tool: "delegate", arguments: { title: "worker" } });
  assert.equal(delegate?.hidden, true);
  assert.equal(row("Worker: explorer", {}, "worker:assignment-1"), undefined);
});

test("internal capsule reads are omitted while real reads remain visible", () => {
  assert.equal(row("Read — capsule", {}).hidden, true);
  assert.equal(row("Read — README.md", { file_path: "/project/README.md" })?.hidden, false);
  assert.equal(row("Read read_report", { server: "cortex", tool: "read_report", arguments: { report_id: "capsule" } })?.hidden, true);
  assert.equal(row("Decide decide", { server: "cortex", tool: "decide", arguments: { task: "task-1" } })?.hidden, true);
});

test("hidden .cortex native operations replace the host row during streaming", () => {
  for (const [name, input] of [
    ["Shell", { command: "cat /project/.cortex/cortex.sqlite3" }],
    ["Edit", { file_path: "/project/.cortex/pipeline.md", old_string: "a", new_string: "b" }],
    ["Search", { query: "/project/.cortex" }],
  ] as const) {
    const projection = row(name, input);
    assert.equal(projection?.hidden, true, name);
    assert.equal(projection?.presentation?.visible, false, name);
  }
  const inProgress = projectCoordinatorToolRow({
    callId: "coordinator:in-progress",
    name: "Shell",
    detail: { type: "unknown", input: { command: "rg .cortex /project" } },
  });
  assert.equal(inProgress?.hidden, true);
});
