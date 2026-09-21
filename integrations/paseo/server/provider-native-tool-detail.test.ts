import assert from "node:assert/strict";
import test from "node:test";
import { nativeToolDetail } from "./provider.ts";

test("maps Claude Read raw input/output to Paseo read detail", () => {
  const event = nativeToolDetail({
    type: "timeline.item",
    sessionId: "s",
    item: {
      type: "tool_call",
      id: "read-1",
      callId: "read-1",
      name: "Read — seed.txt",
      status: "completed",
      error: null,
      detail: {
        type: "unknown",
        input: { file_path: "/project/seed.txt" },
        output: { content: "1\\tnative-provider-ui-probe\\n2\\t" },
      },
    },
  });
  assert.equal(event.type, "timeline.item");
  if (event.type !== "timeline.item" || event.item.type !== "tool_call") throw new Error("wrong event");
  assert.deepEqual(event.item.detail, {
    type: "read",
    filePath: "/project/seed.txt",
    content: "1\\tnative-provider-ui-probe\\n2\\t",
  });
  assert.deepEqual(event.item.metadata, {
    cortexRawInput: { file_path: "/project/seed.txt" },
    cortexRawOutput: { content: "1\\tnative-provider-ui-probe\\n2\\t" },
  });
});

test("maps Claude Bash raw input/output to Paseo shell detail", () => {
  const event = nativeToolDetail({
    type: "timeline.item",
    sessionId: "s",
    item: {
      type: "tool_call",
      id: "shell-1",
      callId: "shell-1",
      name: "Shell pwd",
      status: "completed",
      error: null,
      detail: {
        type: "unknown",
        input: { command: "pwd", description: "Print working directory" },
        output: { stdout: "/project\\n", exitCode: 0 },
      },
    },
  });
  if (event.type !== "timeline.item" || event.item.type !== "tool_call") throw new Error("wrong event");
  assert.deepEqual(event.item.detail, {
    type: "shell",
    command: "pwd",
    output: "/project\\n",
    exitCode: 0,
  });
});

test("maps Claude Edit aliases to Paseo inline diff detail", () => {
  const event = nativeToolDetail({
    type: "timeline.item",
    sessionId: "s",
    item: {
      type: "tool_call",
      id: "edit-1",
      callId: "edit-1",
      name: "Edit seed.txt",
      status: "completed",
      error: null,
      detail: {
        type: "unknown",
        input: {
          file_path: "/project/seed.txt",
          old_string: "native-provider-ui-probe\\n",
          new_string: "native-provider-ui-probe\\nedited-by-native-claude\\n",
        },
        output: { content: "updated" },
      },
    },
  });
  if (event.type !== "timeline.item" || event.item.type !== "tool_call") throw new Error("wrong event");
  assert.deepEqual(event.item.detail, {
    type: "edit",
    filePath: "/project/seed.txt",
    oldString: "native-provider-ui-probe\\n",
    newString: "native-provider-ui-probe\\nedited-by-native-claude\\n",
  });
});

test("keeps non-file Cortex MCP cards on the generic detail path", () => {
  const event = nativeToolDetail({
    type: "timeline.item",
    sessionId: "s",
    item: {
      type: "tool_call",
      id: "mcp-1",
      callId: "mcp-1",
      name: "Read report",
      status: "completed",
      error: null,
      detail: {
        type: "unknown",
        input: { server: "cortex", tool: "read_report", arguments: { report_id: "r1" } },
        output: { text: "report" },
      },
    },
  });
  if (event.type !== "timeline.item" || event.item.type !== "tool_call") throw new Error("wrong event");
  assert.equal(event.item.detail.type, "unknown");
  assert.equal(event.item.metadata, undefined);
});
