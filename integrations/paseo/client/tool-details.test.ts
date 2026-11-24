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

test("full Input/Output preserve the envelope, sibling fields, and empty values", async () => {
  const { toolDetailText } = await import("./tool-details.ts");
  const input = { server: "example", tool: "work", arguments: { message: "requested", count: 2 } };
  const output = { error: null, result: { content: [{ type: "text", text: "done" }], structuredContent: { total: 2 } } };
  assert.equal(toolDetailText(input), JSON.stringify(input, null, 2));
  assert.equal(toolDetailText(output), JSON.stringify(output, null, 2));
  assert.equal(readableToolText(output), "done");
  assert.equal(toolDetailText({ output: "ok", exit_code: 0 }), '{\n  "output": "ok",\n  "exit_code": 0\n}');
  assert.equal(toolDetailText({}), "{}");
  assert.equal(toolDetailText([]), "[]");
  assert.equal(toolDetailText(""), "");
  assert.equal(toolDetailText(false), "false");
  assert.equal(toolDetailText(undefined), null);
});

test("preview caps characters and lines, then restores the exact retained text", async () => {
  const { toolDetailPreview } = await import("./tool-details.ts");
  const long = "x".repeat(999) + "🙂" + "tail";
  assert.deepEqual(toolDetailPreview(long, false), { text: long.slice(0, 999), canExpand: true });
  assert.deepEqual(toolDetailPreview(long, true), { text: long, canExpand: true });
  assert.equal(toolDetailPreview("line\n".repeat(30), false).text.split("\n").length, 24);
  assert.deepEqual(toolDetailPreview("short", false), { text: "short", canExpand: false });
});
