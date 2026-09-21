import assert from "node:assert/strict";
import type { TranscriptEntry, WorkerEvent } from "./worker-panel-events.ts";
import { groupToolRuns, summarizeToolRun } from "./worker-panel-groups.ts";

const event = (id: number, changes: Partial<WorkerEvent>): WorkerEvent => ({ id, type: "progress", createdAt: String(id), text: `event ${id}`, ...changes });
const entry = (e: WorkerEvent): TranscriptEntry => ({ event: e, updates: [e] });
const read = (id: number, path: string, status = "completed") => event(id, { activity_ref: `tool:${id}`, title: "mcp.cortex.project_read", status, raw_input: { server: "cortex", tool: "project_read", arguments: { path } } });
const shell = (id: number) => event(id, { activity_ref: `tool:${id}`, title: "mcp.cortex.command_run", status: "completed", raw_input: { server: "cortex", tool: "command_run", arguments: { argv: ["rg", "x"] } } });

const items = groupToolRuns([
  entry(read(1, "a.md")), entry(read(2, "b.md")), entry(read(3, "a.md")), entry(shell(4)),
  entry(event(5, { text: "[worker message] thinking" })),
  entry(read(6, "c.md", "in_progress")),
]);
assert.deepEqual(items.map((item) => item.kind), ["group", "entry", "group"]);
if (items[0].kind !== "group" || items[2].kind !== "group") throw new Error("groups expected");
assert.equal(items[0].entries.length, 4);
assert.equal(items[0].summary, "Ran 1 command and read 2 files");
assert.equal(items[0].loading, false);
assert.equal(items[2].summary, "Read 1 file");
assert.equal(items[2].loading, true);

assert.equal(summarizeToolRun([
  event(1, { title: "mcp.cortex.project_write", raw_input: { server: "cortex", tool: "project_write", arguments: { path: "x" } } }),
  shell(2), shell(3), read(4, "y"), event(5, { title: "mcp.cortex.plan_submit" }),
]), "Edited 1 file, ran 2 commands, read 1 file, and used 1 other tool");

import { nativeRow } from "./worker-panel-groups.ts";
assert.deepEqual(nativeRow(read(1, "docs/a.md")), { label: "Read", summary: "docs/a.md" });
assert.deepEqual(nativeRow(shell(2)), { label: "Shell", summary: "rg x" });
assert.equal(nativeRow(event(3, { title: "mcp.cortex.plan_submit" })).label, "Submit plan");

import { chatToolIcon } from "./worker-panel-groups.ts";
assert.equal(chatToolIcon(read(1, "a.md")), "Eye");
assert.equal(chatToolIcon(shell(2)), "SquareTerminal");
assert.equal(chatToolIcon(event(3, { title: "mcp.cortex.project_write", raw_input: { server: "cortex", tool: "project_write", arguments: { path: "x" } } })), "Pencil");
assert.equal(chatToolIcon(event(4, { title: "mcp.cortex.plan_submit" })), "Wrench");

// External MCP calls arrive as `execute` but are ordinary tools, as in the chat.
const mcpSearch = event(10, { activity_ref: "tool:10", title: "mcp.codebase_memory.search_code", tool_kind: "execute",
  raw_input: { server: "codebase_memory", tool: "search_code", arguments: { pattern: "x" } } });
assert.equal(nativeRow(mcpSearch).label, "Search Code");
assert.equal(chatToolIcon(mcpSearch), "Wrench");
assert.equal(summarizeToolRun([mcpSearch]), "Used 1 other tool");
