import assert from "node:assert/strict";
import { presentTool } from "./worker-panel-tool.ts";
import type { WorkerEvent } from "./worker-panel-events.js";

const event = (changes: Partial<WorkerEvent>): WorkerEvent => ({ id: 1, type: "progress", createdAt: "now", text: "", ...changes });
const read = presentTool(event({ tool_kind: "read", title: "Read seed.txt", raw_input: { file_path: "/project/seed.txt" }, raw_output: "1\tfirst\n2\t" }));
assert.equal(read.kind, "read");
assert.equal(read.label, "Read seed.txt");
assert.equal(read.output, "1\tfirst\n2\t");
const shell = presentTool(event({ tool_kind: "execute", title: "Terminal", raw_input: { command: "pwd" }, raw_output: "/project" }));
assert.equal(shell.label, "Shell  pwd");
assert.equal(shell.output, "/project");
const edit = presentTool(event({ title: "Edit seed.txt", raw_input: { file_path: "/project/seed.txt", old_string: "first\n", new_string: "first\nsecond\n" } }));
assert.equal(edit.kind, "edit");
assert.equal(edit.newText, "first\nsecond\n");
assert.equal(presentTool(event({ tool_kind: "other", title: "mcp__cortex__publish_result" })).label, "Publish result");
assert.equal(presentTool(event({ title: "Read file" })).kind, "generic");
console.log("worker-panel tool presentation tests passed");
