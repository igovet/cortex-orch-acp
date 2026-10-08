import assert from "node:assert/strict";
import test from "node:test";
import { KNOWN_CORTEX_TOOL_NAMES, presentToolSemantics } from "./tool-presentation.ts";

const cortexTools = [
  "answer_submit", "replan_request", "scope_request", "escalation_request",
  "project_read", "project_write", "project_edit", "project_delete", "project_mkdir", "project_git", "draft_write", "command_run",
  "create_task", "set_governance", "create_draft", "read_draft", "write_report",
  "list_reports", "read_report", "task_get_context", "plan_submit", "synthesis_submit",
  "user_question_request", "result_submit", "list_sources", "read_source",
  "read_skill", "read_capsule", "read_large_output", "read_diagnostics", "ask_question",
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
    kind: "read", icon: "FileText", action: "Read", target: "src/greeting.py", label: "Read: src/greeting.py", expandable: false, systemOperation: false, visible: true,
  });
  assert.deepEqual(presentToolSemantics({ title: "Edit seed.txt", input: { file_path: "/project/seed.txt" } }), {
    kind: "edit", icon: "FilePenLine", action: "Edit", target: "/project/seed.txt", label: "Edit: /project/seed.txt", expandable: true, systemOperation: false, visible: true,
  });
  assert.equal(presentToolSemantics({ title: "Preparing file…", toolKind: "edit", input: {} }).target, null);
  assert.equal(presentToolSemantics({ title: "Editing files", toolKind: "edit", input: {} }).label, "Edit");
  assert.deepEqual(presentToolSemantics({ title: "Terminal", input: { command: "pwd" } }), {
    kind: "shell", icon: "Terminal", action: "Shell", target: "pwd", label: "Shell: pwd", expandable: true, systemOperation: false, visible: true,
  });
  assert.equal(presentToolSemantics({ title: "Skill", input: { skill: "knowledge-harvest" } }).label, "Skill: knowledge-harvest");
  assert.equal(presentToolSemantics({ title: "Skill", input: { skill: "knowledge-harvest" } }).expandable, true);
  assert.equal(presentToolSemantics({ title: "Governance", input: { mode: "light" } }).label, "Governance: light");
  assert.equal(presentToolSemantics({ name: "mcp__cortex__read_report", input: { report_id: "r_abc", author: "worker-backend" } }).label, "Read report: by worker-backend");
  assert.equal(presentToolSemantics({ name: "mcp__cortex__read_report", input: { report_id: "r_abc" } }).systemOperation, false);
  assert.equal(presentToolSemantics({ name: "mcp__cortex__write_report", title: "Edit report", input: { report_ref: "r_abc" } }).expandable, true);
  assert.equal(presentToolSemantics({ name: "mcp__cortex__set_governance", input: { mode: "light" } }).label, "Set governance: light");
  assert.equal(presentToolSemantics({ name: "mcp__cortex__read_report", input: { report_id: "r_abc" } }).label, "Read report: r_abc");
});

test("retains coordinator capsule and report reads", () => {
  assert.equal(presentToolSemantics({ name: "mcp__cortex__read_capsule", input: {} }).visible, true);
  assert.equal(presentToolSemantics({ title: "Read — capsule", input: {} }).visible, true);
  assert.equal(presentToolSemantics({ title: "Read — src/capsule", input: {} }).visible, true);
  assert.equal(presentToolSemantics({ name: "mcp__cortex__read_report", input: { report_id: "capsule" } }).visible, true);
});

test("retains every Cortex MCP alias and project-local .cortex access", () => {
  assert.equal(presentToolSemantics({ name: "mcp__cortex__read_report", input: {} }).visible, true);
  assert.equal(presentToolSemantics({ name: "mcp.cortex.read_report", input: {} }).visible, true);
  assert.equal(presentToolSemantics({ name: "Read", input: { file_path: "/project/.cortex/cortex.sqlite3" } }).visible, true);
  assert.equal(presentToolSemantics({ name: "Search", input: { query: "/project/.cortex" } }).visible, true);
  assert.equal(presentToolSemantics({ name: "Read", input: { file_path: "/project/src/cortex.ts" } }).visible, true);
  assert.equal(presentToolSemantics({ name: "Read", input: {} }).visible, true);
  assert.equal(presentToolSemantics({ name: "Editing files", toolKind: "edit", input: { content: [{ type: "diff", path: "/project/.cortex/draft.md", oldText: "a", newText: "b" }] } }).visible, true);
  assert.equal(presentToolSemantics({ name: "Read", toolKind: "read", input: { locations: [{ path: "/project/src/app.py" }] } }).target, "/project/src/app.py");
  assert.equal(presentToolSemantics({ name: "Edit", toolKind: "edit", rawInput: {}, input: { content: [{ type: "diff", path: "/project/src/app.py", oldText: "a", newText: "b" }] } }).target, "/project/src/app.py");
  assert.equal(presentToolSemantics({ name: "Edit", toolKind: "edit", rawInput: {}, input: { content: [{ type: "diff", path: "/project/.cortex/draft.md", oldText: "a", newText: "b" }] } }).visible, true);
});

test("humanizes unknown names and keeps expanded payload out of the collapsed label", () => {
  const row = presentToolSemantics({ name: "mcp__other__search_code", input: { query: "secret" } });
  assert.equal(row.label, "Search Code");
  assert.equal(row.target, null);
  assert.equal(presentToolSemantics({ name: "mcp.cortex.result_submit", input: { report: { report_ref: "r_abc" } } }).label, "Submit result: r_abc");
  assert.equal(presentToolSemantics({ name: "mcp.cortex.result_submit", input: { report: { title: "Review", markdown: "# secret body" } } }).label, "Submit result");
  assert.equal(presentToolSemantics({ name: "mcp.cortex.result_submit", input: { report: "r_0123456789ab" } }).label, "Submit result: r_0123456789ab");
  assert.equal(presentToolSemantics({ name: "mcp.cortex.result_submit", input: { report: "# secret body\n\nr_0123456789ab", report_title: "Review" } }).label, "Submit result");
  assert.equal(presentToolSemantics({ name: "mcp.cortex.result_submit", input: { report: "secret" } }).label, "Submit result");
  assert.equal(presentToolSemantics({ name: "mcp__other__search_code", input: { query: "{\"secret\":true}" } }).label, "Search Code");
});

test("gateway labels override generic adapter execution kinds", () => {
  assert.equal(presentToolSemantics({ name: "mcp.cortex.project_read", toolKind: "execute", rawInput: { server: "cortex", tool: "project_read", arguments: { path: "calc.py" } } }).label, "Read: calc.py");
  assert.equal(presentToolSemantics({ name: "mcp__cortex__command_run", rawInput: { argv: ["python3", "-B", "-m", "unittest", "-q"] } }).label, "Shell: python3 -B -m unittest -q");
});

test("unwraps Codex tool envelopes and hides only protocol/wait operations", () => {
  assert.equal(presentToolSemantics({ name: "mcp.cortex.read_skill", rawInput: { server: "cortex", tool: "read_skill", arguments: { skill: "tool-discipline" } } }).label, "Read skill: tool-discipline");
  for (const name of ["agent_thought_chunk", "mcp__claude__wait", "Wait", "thinking", "usage_update"]) {
    assert.equal(presentToolSemantics({ name }).visible, false, name);
  }
  assert.equal(presentToolSemantics({ name: "mcp.cortex.read_report" }).visible, true);
  assert.equal(presentToolSemantics({ name: "Shell", rawInput: { command: "wait" } }).visible, true);
});

 test("does not repeat raw MCP names as targets on title-only lifecycle updates", () => {
  assert.equal(presentToolSemantics({ title: "mcp.cortex.project_read", toolKind: "read" }).label, "Read");
  assert.equal(presentToolSemantics({ title: "mcp.cortex.create_draft", toolKind: "execute" }).label, "Create draft");
  assert.equal(presentToolSemantics({ title: "mcp__cortex__draft_write", toolKind: "edit" }).label, "Write draft");
});

 test("file and skill reads are informational even with returned content", () => {
  for (const name of ["mcp.cortex.project_read", "mcp__cortex__read_skill", "Read"]) {
    assert.equal(presentToolSemantics({ name, rawInput: {path: "README.md"} }).expandable, false);
  }
});

test("a call titled with its own command line shows that command, not its first word", () => {
  const loop = "for f in services/*/service; do grep -c intent \"$f\"/*.py; done";
  assert.equal(presentToolSemantics({ name: loop, rawInput: { command: loop } }).label, `Shell: ${loop}`);
  assert.equal(presentToolSemantics({ name: 'grep -rn "stop" services/trading-engine' }).label,
    'grep -rn "stop" services/trading-engine');
  const long = presentToolSemantics({ name: `grep -rn "${"x".repeat(200)}" src` }).label;
  assert.ok(long.startsWith('grep -rn "xxx') && long.endsWith("…") && long.length === 158);
  assert.equal(presentToolSemantics({ name: "Grep" }).label, "Grep");
  assert.equal(presentToolSemantics({ name: "mcp__other__search_code", input: { query: "q" } }).label, "Search Code");
});

test("normalized OpenCode Code Mode calls render like the Codex and Claude calls", () => {
  // engine.opencode_tool_calls rewrites OpenCode's `execute` wrapper into
  // Codex's MCP spelling; the retained JavaScript never makes it a shell row.
  const code = 'return await tools.cortex.answer_submit({ answer: "yes" })';
  const opencode = presentToolSemantics({
    name: "mcp.cortex.answer_submit", toolKind: "other",
    rawInput: { server: "cortex", tool: "answer_submit", arguments: { answer: "yes" }, code },
  });
  const claude = presentToolSemantics({ name: "mcp__cortex__answer_submit", toolKind: "other", rawInput: { answer: "yes" } });
  assert.equal(opencode.label, "Submit answer");
  assert.deepEqual(opencode, claude);
  const read = presentToolSemantics({
    name: "mcp.cortex.read_report", toolKind: "other",
    rawInput: { server: "cortex", tool: "read_report", arguments: { report_id: "r_abc" }, code: "return 1" },
  });
  assert.equal(read.label, "Read report: r_abc");
  const unresolved = presentToolSemantics({ name: "Code Mode", toolKind: "other", rawInput: { code: "return 1" } });
  assert.notEqual(unresolved.kind, "shell");
  assert.equal(unresolved.label, "Code Mode");
});

test("raw OpenCode replay uses authoritative inner names and provisional code hints", () => {
  const code = 'return await tools.cortex.answer_submit({answer: "yes"})';
  const rawInput = { code };
  assert.deepEqual(presentToolSemantics({ name: "execute", toolKind: "other", rawInput }),
    presentToolSemantics({ name: "mcp__cortex__answer_submit", rawInput: { answer: "yes" } }));
  assert.equal(presentToolSemantics({ name: "execute", toolKind: "other", rawInput,
    rawOutput: { metadata: { toolCalls: [
      { tool: "search", input: {} }, { tool: "cortex.read_report", input: { report_id: "r_abc" } },
    ] } } }).label, "Read report: r_abc");
  assert.equal(presentToolSemantics({ name: "execute", toolKind: "other", rawInput: {} }).label, "Code Mode");
  assert.equal(presentToolSemantics({ name: "execute", rawInput: { code: "return 1" } }).kind, "generic");
  assert.equal(presentToolSemantics({ name: "execute", toolKind: "execute", rawInput: { command: "pwd" } }).label, "Shell: pwd");
  assert.equal(presentToolSemantics({ name: "execute", rawInput: { command: "pwd" } }).label, "Shell: pwd");
});

test("every targeted label survives the installed Paseo client's humanizer verbatim (O1)", () => {
  // JS port of the installed client's tool-display rule (module 4613,
  // function u(name)): return the name unchanged only if it contains one of
  // `:` `.` `/`, or the substring `__`; otherwise strip `.`/`_`/`-`,
  // lowercase, and capitalize the first letter.
  const hostHumanize = (value: string): string => {
    if (/[:./]/.test(value) || value.includes("__")) return value;
    const humanized = value.replace(/[._-]+/g, " ").trim().toLowerCase();
    return humanized.replace(/^(.)/, (char) => char.toUpperCase());
  };
  const targetedLabels = [
    presentToolSemantics({ name: "mcp__cortex__command_run", rawInput: { argv: ["python3", "-m", "pytest", "-q"] } }).label,
    presentToolSemantics({ name: "mcp__cortex__read_report", input: { report_id: "r_21f9bba72f15" } }).label,
    presentToolSemantics({ title: "Terminal", input: { command: "pwd" } }).label,
    presentToolSemantics({ name: "mcp.cortex.result_submit", input: { report: { report_ref: "r_abc" } } }).label,
    presentToolSemantics({ name: "mcp.cortex.result_submit", input: { report: "r_0123456789ab" } }).label,
    presentToolSemantics({ name: "mcp__cortex__set_governance", input: { mode: "light" } }).label,
  ];
  for (const label of targetedLabels) {
    assert.ok(label.length > 0 && label.includes(":"), label);
    assert.equal(hostHumanize(label), label, label);
  }
});

test("project_edit is an expandable diff and line pages have readable ranges", () => {
  const edit = presentToolSemantics({ name: "mcp.cortex.project_edit", input: { path: "app.py", edits: [{ old_string: "a", new_string: "b" }] } });
  assert.equal(edit.kind, "edit");
  assert.equal(edit.expandable, true);
  assert.equal(edit.label, "Edit: app.py");
  const page = presentToolSemantics({ name: "mcp.cortex.project_read", input: { path: "app.py", offset: 21, limit: 10 } });
  assert.equal(page.label, "Read: app.py (lines 21–30)");
  assert.equal(page.expandable, false);
});
