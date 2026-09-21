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
