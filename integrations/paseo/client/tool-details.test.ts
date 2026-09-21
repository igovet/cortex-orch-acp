import assert from "node:assert/strict";
import test from "node:test";
import { readableToolText } from "./tool-details.ts";

test("opaque protocol data never becomes expanded JSON", () => {
  for (const value of [{id: 1, ok: true}, '{"id":1,"ok":true}', [], {}, '{bad json'])
    assert.equal(readableToolText(value), null);
});
test("retains readable command output and MCP text without the envelope", () => {
  assert.equal(readableToolText({result: {content: [{type: "text", text: "3 tests passed"}]}}), "3 tests passed");
  assert.equal(readableToolText('{"stdout":"hello","stderr":"notice","exit_code":0}'), "hello\nnotice");
  assert.equal(readableToolText({content: [{type: "text", text: '{"task_id":"t_1"}'}]}), null);
});

// An MCP result whose text is a JSON envelope with `data` (GitKraken git_log_or_diff).
assert.equal(readableToolText({ error: null, result: { content: [{ type: "text",
  text: JSON.stringify({ data: { output: "diff --git a/x b/x" } }) }] } }), "diff --git a/x b/x");

test("a generic tool always shows what it returned, like the chat's Input/Output sections", async () => {
  const { toolDetailText, toolArguments } = await import("./tool-details.ts");
  // Readable text wins over its envelope.
  assert.equal(toolDetailText({ result: { content: [{ type: "text", text: "3 tests passed" }] } }), "3 tests passed");
  // Pure data is shown as formatted JSON of the payload, not the MCP envelope.
  assert.equal(toolDetailText({ error: null, result: { content: [{ type: "text", text: '{"task_id":"t_1"}' }] } }), '{\n  "task_id": "t_1"\n}');
  assert.equal(toolDetailText({ result: { structuredContent: { total: 2 } } }), '{\n  "total": 2\n}');
  for (const empty of [null, "", {}, []]) assert.equal(toolDetailText(empty), null);
  assert.deepEqual(toolArguments({ server: "GitKraken", tool: "git_log_or_diff", arguments: { action: "diff" } }), { action: "diff" });
  assert.deepEqual(toolArguments({ command: "pwd" }), { command: "pwd" });
});
