import assert from "node:assert/strict";
import { presentTool } from "./worker-panel-tool.ts";
import { groupWorkerEvents } from "./worker-panel-events.ts";
import type { WorkerEvent } from "./worker-panel-events.js";

const event = (changes: Partial<WorkerEvent>): WorkerEvent => ({ id: 1, type: "progress", createdAt: "now", text: "", ...changes });
const read = presentTool(event({ tool_kind: "read", title: "Read seed.txt", raw_input: { file_path: "/project/seed.txt" }, raw_output: "1\tfirst\n2\t" }));
assert.equal(read.kind, "read");
assert.equal(read.label, "Read seed.txt");
assert.equal(read.output, "1\tfirst\n2\t");
const shell = presentTool(event({ tool_kind: "execute", title: "Terminal", raw_input: { command: "pwd" }, raw_output: "/project" }));
assert.equal(shell.label, "Shell  pwd");
assert.equal(shell.output, "/project");
const edit = presentTool(event({ title: "Edit seed.txt", raw_input: { filePath: "/project/seed.txt", oldString: "first\n", newString: "first\nsecond\n" } }));
assert.equal(edit.kind, "edit");
assert.equal(edit.newText, "first\nsecond\n");
assert.equal(edit.path, "/project/seed.txt");
assert.equal(presentTool(event({ tool_kind: "other", title: "mcp__cortex__publish_result" })).label, "Publish result");
assert.equal(presentTool(event({ title: "Read file" })).kind, "generic");
console.log("worker-panel tool presentation tests passed");
const gatewayRead = presentTool(event({ tool_kind: "execute", title: "mcp.cortex.project_read", raw_input: { server: "cortex", tool: "project_read", arguments: { path: "calc.py" } }, raw_output: { result: { structuredContent: { content: "return a + b" } } } }));
assert.equal(gatewayRead.kind, "read");
assert.equal(gatewayRead.path, "calc.py");
assert.equal(gatewayRead.output, "return a + b");
const gatewayWrite = presentTool(event({ tool_kind: "execute", title: "mcp__cortex__project_write", raw_input: { path: "calc.py", content: "return a + b" } }));
assert.equal(gatewayWrite.kind, "edit");
assert.equal(gatewayWrite.newText, "return a + b");
const gatewayCommand = presentTool(event({ title: "mcp.cortex.command_run", raw_input: { server: "cortex", tool: "command_run", arguments: { argv: ["python3", "-B", "-m", "unittest", "-q"] } } }));
assert.equal(gatewayCommand.command, "python3 -B -m unittest -q");

const claudeCommand = presentTool(event({ title: "mcp__cortex__command_run", raw_input: { argv: ["python3", "-B"] }, raw_output: JSON.stringify({ exit_code: 0, output: "actual stdout" }) }));
assert.equal(claudeCommand.output, "actual stdout");

const claudeShell = presentTool(event({
  id: 91, tool_kind: "execute", title: "Terminal",
  raw_input: { command: "python3 -B -m pytest -q" },
  raw_output: { formatted_output: "17 passed\n" },
}));
const codexShellUpdates = groupWorkerEvents([
  event({ id: 92, activity_ref: "tool:exec-1", tool_kind: "execute", title: "exec", status: "in_progress",
    raw_input: { name: "exec", input: { cmd: "python3 -B -m pytest -q", workdir: "/project" } } }),
  event({ id: 93, activity_ref: "tool:exec-1", tool_kind: "execute", title: "exec", status: "completed",
    raw_output: { output: { stdout: "17 passed\n", exit_code: 0 }, exit_code: 0 } }),
]);
assert.equal(codexShellUpdates.length, 1);
const codexShell = presentTool(codexShellUpdates[0].event);
assert.deepEqual(codexShell, claudeShell);
assert.equal(codexShell.kind, "shell");
assert.equal(codexShell.command, "python3 -B -m pytest -q");
assert.equal(codexShell.output, "17 passed\n");

const mcpTool = presentTool(event({ tool_kind: "execute", title: "mcp.codebase_memory.get_code_snippet",
  raw_input: { server: "codebase_memory", tool: "get_code_snippet", arguments: { qualified_name: "a.b" } },
  raw_output: { error: null, result: { content: [{ type: "text", text: "def b(): pass" }] } } }));
assert.equal(mcpTool.kind, "generic");
