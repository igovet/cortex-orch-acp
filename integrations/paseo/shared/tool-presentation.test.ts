import assert from "node:assert/strict";
import test from "node:test";
import { KNOWN_CORTEX_TOOL_NAMES, presentToolSemantics } from "./tool-presentation.ts";

const cortexTools = [
  "create_task", "set_governance", "create_draft", "read_draft", "write_report",
  "list_reports", "read_report", "delegate", "list_events", "clarify", "cancel",
  "decide", "integrate", "update_plan", "read_plan", "prepare_candidate",
  "request_check", "read_check", "assess_check", "list_sources", "read_source",
  "read_skill", "read_capsule", "read_large_output", "read_diagnostics",
  "ask_question", "publish_result",
];

test("maps every advertised Cortex MCP operation to a human semantic row", () => {
  assert.deepEqual([...KNOWN_CORTEX_TOOL_NAMES].sort(), [...cortexTools].sort());
  for (const name of cortexTools) {
    const row = presentToolSemantics({ name: `mcp__cortex__${name}`, input: {} });
    assert.ok(row.action.length > 0, name);
    assert.ok(row.icon.length > 0, name);
    assert.ok(!row.label.includes("mcp__"), name);
    assert.ok(!row.label.includes("{"), name);
  }
});

test("uses native Read/Edit/Shell/Skill/Governance action and target", () => {
  assert.deepEqual(presentToolSemantics({ title: "Read — src/greeting.py", input: {} }), {
    kind: "read", icon: "FileText", action: "Read", target: "src/greeting.py", label: "Read — src/greeting.py", expandable: false, systemOperation: true, visible: true,
  });
  assert.deepEqual(presentToolSemantics({ title: "Edit seed.txt", input: { file_path: "/project/seed.txt" } }), {
    kind: "edit", icon: "FilePenLine", action: "Edit", target: "/project/seed.txt", label: "Edit — /project/seed.txt", expandable: true, systemOperation: false, visible: true,
  });
  assert.deepEqual(presentToolSemantics({ title: "Terminal", input: { command: "pwd" } }), {
    kind: "shell", icon: "Terminal", action: "Shell", target: "pwd", label: "Shell — pwd", expandable: true, systemOperation: false, visible: true,
  });
  assert.equal(presentToolSemantics({ title: "Skill", input: { skill: "knowledge-harvest" } }).label, "Skill — knowledge-harvest");
  assert.equal(presentToolSemantics({ title: "Skill", input: { skill: "knowledge-harvest" } }).expandable, false);
  assert.equal(presentToolSemantics({ title: "Governance", input: { mode: "light" } }).label, "Governance — light");
  assert.equal(presentToolSemantics({ name: "mcp__cortex__read_report", input: { report_id: "r_abc", author: "worker-backend" } }).label, "Read report — by worker-backend");
  assert.equal(presentToolSemantics({ name: "mcp__cortex__read_report", input: { report_id: "r_abc" } }).systemOperation, true);
  assert.equal(presentToolSemantics({ name: "mcp__cortex__write_report", title: "Edit report", input: { report_ref: "r_abc" } }).expandable, false);
  assert.equal(presentToolSemantics({ name: "mcp__cortex__set_governance", input: { mode: "light" } }).label, "Set governance — light");
  assert.equal(presentToolSemantics({ name: "mcp__cortex__read_report", input: { report_id: "r_abc" } }).label, "Read report — r_abc");
  assert.equal(presentToolSemantics({ name: "mcp__cortex__update_plan", input: { action: "add_assignment", node_key: "frontend" } }).label, "Update plan — frontend");
});

test("hides only the coordinator's internal capsule reads", () => {
  assert.equal(presentToolSemantics({ name: "mcp__cortex__read_capsule", input: {} }).visible, false);
  assert.equal(presentToolSemantics({ title: "Read — capsule", input: {} }).visible, false);
  assert.equal(presentToolSemantics({ title: "Read — src/capsule", input: {} }).visible, true);
  assert.equal(presentToolSemantics({ name: "mcp__cortex__read_report", input: { report_id: "capsule" } }).visible, false);
});

test("hides every Cortex MCP alias and project-local .cortex access", () => {
  assert.equal(presentToolSemantics({ name: "mcp__cortex__read_report", input: {} }).visible, false);
  assert.equal(presentToolSemantics({ name: "mcp.cortex.read_report", input: {} }).visible, false);
  assert.equal(presentToolSemantics({ name: "Read", input: { file_path: "/project/.cortex/cortex.sqlite3" } }).visible, false);
  assert.equal(presentToolSemantics({ name: "Search", input: { query: "/project/.cortex" } }).visible, false);
  assert.equal(presentToolSemantics({ name: "Read", input: { file_path: "/project/src/cortex.ts" } }).visible, true);
  assert.equal(presentToolSemantics({ name: "Read", input: {} }).visible, false);
  assert.equal(presentToolSemantics({ name: "Editing files", toolKind: "edit", input: { content: [{ type: "diff", path: "/project/.cortex/draft.md", oldText: "a", newText: "b" }] } }).visible, false);
  assert.equal(presentToolSemantics({ name: "Read", toolKind: "read", input: { locations: [{ path: "/project/src/app.py" }] } }).target, "/project/src/app.py");
  assert.equal(presentToolSemantics({ name: "Edit", toolKind: "edit", rawInput: {}, input: { content: [{ type: "diff", path: "/project/src/app.py", oldText: "a", newText: "b" }] } }).target, "/project/src/app.py");
  assert.equal(presentToolSemantics({ name: "Edit", toolKind: "edit", rawInput: {}, input: { content: [{ type: "diff", path: "/project/.cortex/draft.md", oldText: "a", newText: "b" }] } }).visible, false);
});

test("humanizes unknown names and keeps expanded payload out of the collapsed label", () => {
  const row = presentToolSemantics({ name: "mcp__other__search_code", input: { query: "secret" } });
  assert.equal(row.label, "Search Code");
  assert.equal(row.target, null);
  assert.equal(presentToolSemantics({ name: "mcp.cortex.publish_result", input: { report_ref: "r_abc" } }).label, "Publish result — r_abc");
  assert.equal(presentToolSemantics({ name: "mcp__other__search_code", input: { query: "{\"secret\":true}" } }).label, "Search Code");
});
