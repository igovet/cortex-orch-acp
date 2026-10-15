import assert from "node:assert/strict";
import test from "node:test";
import { nativeEditSnapshot, nativeToolDetail, presentTimeline } from "./provider.ts";

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

test("maps Codex shell argv to Paseo detail without its bash -lc wrapper", () => {
  const event = nativeToolDetail({
    type: "timeline.item",
    sessionId: "s",
    item: {
      type: "tool_call", id: "shell-argv-1", callId: "shell-argv-1", name: "Shell",
      status: "completed", error: null,
      detail: {
        type: "unknown",
        input: { input: { argv: ["bash", "-lc", "rg --files src"] } },
        output: { stdout: "src/cortex_acp.py\\\\n", exit_code: 0 },
      },
    },
  });
  if (event.type !== "timeline.item" || event.item.type !== "tool_call") throw new Error("wrong event");
  assert.deepEqual(event.item.detail, {
    type: "shell", command: "rg --files src", output: "src/cortex_acp.py\\\\n", exitCode: 0,
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

test("worker cards use the one detail Paseo overview never groups", () => {
  const event = presentTimeline({
    type: "timeline.item",
    sessionId: "s",
    item: {
      type: "tool_call", id: "worker:a1", callId: "worker:a1", name: "Worker: debugger — audit",
      status: "running", error: null, detail: { type: "unknown", input: null, output: null },
    },
  });
  if (event.type !== "timeline.item" || event.item.type !== "tool_call") throw new Error("wrong event");
  assert.deepEqual(event.item.detail, { type: "plan", text: "Worker: debugger — audit" });
  assert.equal(event.item.metadata, undefined);
});

test("a completed worker card's embedded result projection reaches item.metadata.cortexWorkerResult", () => {
  const event = presentTimeline({
    type: "timeline.item",
    sessionId: "s",
    item: {
      type: "tool_call", id: "worker:a1", callId: "worker:a1", name: "Worker: debugger — audit",
      status: "completed", error: null,
      detail: {
        type: "unknown", input: null,
        output: { summary: "Did the thing.", checks: [{ name: "tests", outcome: "passed" }] },
      },
    },
  });
  if (event.type !== "timeline.item" || event.item.type !== "tool_call") throw new Error("wrong event");
  assert.deepEqual(event.item.detail, { type: "plan", text: "Worker: debugger — audit" });
  assert.deepEqual(event.item.metadata, {
    cortexWorkerResult: { summary: "Did the thing.", checks: [{ name: "tests", outcome: "passed" }] },
  });
});

test("a worker card whose rawOutput is not the result shape embeds no metadata", () => {
  const event = presentTimeline({
    type: "timeline.item",
    sessionId: "s",
    item: {
      type: "tool_call", id: "worker:a1", callId: "worker:a1", name: "Worker: debugger — audit",
      status: "failed", error: "boom",
      detail: { type: "unknown", input: null, output: "plain-text failure mirror" },
    },
  });
  if (event.type !== "timeline.item" || event.item.type !== "tool_call") throw new Error("wrong event");
  assert.equal(event.item.metadata, undefined);
});

test("worker timeline history keeps only bounded card fields and its bounded result", () => {
  const transcript = "secret-full-worker-transcript ".repeat(3000);
  const hugeSummary = "🙂".repeat(12000);
  const checks = Array.from({ length: 40 }, (_, index) => ({
    name: "result-check-" + index,
    outcome: "passed",
  }));
  const event = presentTimeline({
    type: "timeline.item",
    sessionId: "session-worker",
    item: {
      type: "tool_call",
      id: "worker-id-1",
      callId: "worker:assignment-1",
      name: "W".repeat(5000),
      status: "failed",
      error: "worker failure ".repeat(500),
      detail: {
        type: "unknown",
        input: { transcript },
        output: { summary: hugeSummary, checks },
      },
      metadata: {
        cortexRawInput: transcript,
        cortexRawOutput: { transcript },
        extraBody: transcript,
      },
    },
  });
  if (event.type !== "timeline.item" || event.item.type !== "tool_call") throw new Error("wrong event");

  assert.equal(event.item.id, "worker-id-1");
  assert.equal(event.item.callId, "worker:assignment-1");
  assert.equal(event.item.status, "failed");
  assert.deepEqual(event.item.detail, { type: "plan", text: event.item.name });
  assert.deepEqual(Object.keys(event.item.metadata ?? {}), ["cortexWorkerResult"]);
  assert.ok(Buffer.byteLength(JSON.stringify(event.item.name), "utf8") <= 1024);
  assert.ok(Buffer.byteLength(JSON.stringify(event.item.error), "utf8") <= 1024);
  assert.ok(Buffer.byteLength(JSON.stringify(event.item.metadata?.cortexWorkerResult), "utf8") <= 16 * 1024);
  assert.ok(Buffer.byteLength(JSON.stringify(event), "utf8") <= 32 * 1024);
  assert.equal(JSON.stringify(event).includes("secret-full-worker-transcript"), false);
  const result = event.item.metadata?.cortexWorkerResult as { summary?: string; checks?: unknown[] } | undefined;
  assert.equal(typeof result?.summary, "string");
  assert.ok(result?.summary?.includes("[truncated"));
  assert.ok((result?.checks?.length ?? 0) > 0);
});

test("generic tool timeline history removes raw metadata and caps retained detail bytes", () => {
  const payload = "TOOL_PAYLOAD_DO_NOT_RETAIN ".repeat(3000);
  const event = presentTimeline({
    type: "timeline.item",
    sessionId: "session-tool",
    item: {
      type: "tool_call",
      id: "mcp-id-1",
      callId: "mcp:call-1",
      name: "Read report",
      status: "completed",
      error: null,
      detail: {
        type: "unknown",
        input: { server: "cortex", tool: "read_report", arguments: { report_id: "r1", body: payload } },
        output: { text: payload },
      },
      metadata: {
        cortexRawInput: { body: payload },
        cortexRawOutput: { body: payload },
        otherUnneededBody: payload,
      },
    },
  });
  if (event.type !== "timeline.item" || event.item.type !== "tool_call") throw new Error("wrong event");

  assert.equal(event.item.id, "mcp-id-1");
  assert.equal(event.item.callId, "mcp:call-1");
  assert.equal(event.item.metadata, undefined);
  assert.ok(Buffer.byteLength(JSON.stringify(event.item.detail), "utf8") <= 16 * 1024);
  assert.ok(Buffer.byteLength(JSON.stringify(event), "utf8") <= 24 * 1024);
});

test("native tool detail preserves useful file identity while clipping large output", () => {
  const content = "read-output-" .repeat(6000);
  const event = presentTimeline({
    type: "timeline.item",
    sessionId: "session-read",
    item: {
      type: "tool_call",
      id: "read-id-1",
      callId: "read:call-1",
      name: "Read — seed.txt",
      status: "completed",
      error: null,
      detail: {
        type: "unknown",
        input: { file_path: "/project/seed.txt" },
        output: { content },
      },
    },
  });
  if (event.type !== "timeline.item" || event.item.type !== "tool_call") throw new Error("wrong event");

  assert.equal(event.item.detail.type, "read");
  if (event.item.detail.type !== "read") throw new Error("wrong native detail");
  assert.equal(event.item.detail.filePath, "/project/seed.txt");
  assert.ok(Buffer.byteLength(JSON.stringify(event.item.detail), "utf8") <= 16 * 1024);
  assert.equal(event.item.metadata, undefined);
});

test("an oversized unknown input never drops the output key Paseo requires", () => {
  const event = presentTimeline({
    type: "timeline.item",
    sessionId: "s",
    item: {
      type: "tool_call", id: "t1", callId: "call-1", name: "mcp_tool",
      status: "running", error: null,
      detail: { type: "unknown", input: { blob: "x".repeat(64 * 1024) }, output: null },
    },
  });
  if (event.type !== "timeline.item" || event.item.type !== "tool_call") throw new Error("wrong event");
  assert.equal(event.item.detail.type, "unknown");
  assert.ok(Object.prototype.hasOwnProperty.call(event.item.detail, "input"));
  assert.ok(Object.prototype.hasOwnProperty.call(event.item.detail, "output"));
});

test("a typed detail whose required key cannot fit falls back to plain_text", () => {
  const event = presentTimeline({
    type: "timeline.item",
    sessionId: "s",
    item: {
      type: "tool_call", id: "t2", callId: "call-2", name: "custom",
      status: "completed", error: null,
      detail: { type: "search", query: "q".repeat(64 * 1024) } as never,
    },
  });
  if (event.type !== "timeline.item" || event.item.type !== "tool_call") throw new Error("wrong event");
  const detail = event.item.detail as Record<string, unknown>;
  assert.ok(detail.type === "plain_text" || typeof detail.query === "string");
});

test("raw Code Mode replay has MCP detail/status parity without inventing dropped output", () => {
  const code = 'return await tools.cortex.command_run({argv: ["pwd"]})';
  const rawOutput = { metadata: { toolCalls: [
    { tool: "search", input: {}, status: "completed" },
    { tool: "cortex.command_run", input: { argv: ["pwd"] }, status: "completed" },
  ] } };
  const event = (name: string, input: any, output: any, status: any = "completed"): any => ({
    type: "timeline.item", sessionId: "s", item: {
      type: "tool_call", id: "code-1", callId: "code-1", name, status, error: null,
      detail: { type: "unknown", input, output },
    },
  });
  const raw = event("execute", { code }, rawOutput);
  const projected: any = presentTimeline(raw);
  assert.equal(projected.item.name, "Shell: pwd");
  assert.deepEqual({ ...projected.item.detail }, { type: "shell", command: "pwd" });
  assert.deepEqual(raw.item.detail.output, rawOutput, "presentation never mutates source");
  const withResult = { ...rawOutput, result: { content: [{ type: "text", text: "/project" }] } };
  const normalized = event("mcp.cortex.command_run",
    { server: "cortex", tool: "command_run", arguments: { argv: ["pwd"] }, code }, withResult);
  assert.deepEqual(presentTimeline(event("execute", { code }, withResult)), presentTimeline(normalized));

  const innerError = { metadata: { toolCalls: [
    { tool: "cortex.answer_submit", input: { answer: "yes" }, status: "error" },
  ] } };
  const failed: any = presentTimeline(event("execute", { code }, innerError));
  assert.equal(failed.item.name, "Submit answer");
  assert.equal(failed.item.status, "failed");
  assert.equal(failed.item.detail.output, null);
  assert.equal(failed.item.error, "Code Mode tool call failed");
  const failedWithText: any = presentTimeline(event("execute", { code },
    { ...innerError, result: { content: [{ type: "text", text: "answer rejected" }] } }));
  assert.equal(failedWithText.item.error, "answer rejected");
  const nativeFailed: any = presentTimeline(event("execute", { code }, innerError, "failed"));
  assert.equal(nativeFailed.item.status, "failed");
  assert.equal(nativeFailed.item.error, "Code Mode tool call failed");
  const metadataError: any = presentTimeline(event("execute", { code },
    { metadata: { ...rawOutput.metadata, error: true } }));
  assert.equal(metadataError.item.status, "failed");
  const running: any = presentTimeline(event("execute", { code }, innerError, "running"));
  assert.equal(running.item.status, "running", "only completed inner errors fail the wrapper");

  const helper: any = presentTimeline(event("execute", { code: "return search({})" },
    { metadata: { toolCalls: [{ tool: "search", input: {}, status: "completed" }] } }));
  assert.equal(helper.item.name, "Search");
  const unresolved: any = presentTimeline(event("execute", {}, null, "running"));
  assert.equal(unresolved.item.name, "Shell", "the bridge has not retained kind or code yet");
  const shell: any = presentTimeline(event("execute", { command: "pwd" }, "/project"));
  assert.equal(shell.item.name, "Shell: pwd");
});

test("maps a Claude Bash row named by its own command to shell detail", () => {
  const event = nativeToolDetail({
    type: "timeline.item",
    sessionId: "s",
    item: {
      type: "tool_call", id: "bash-cat", callId: "bash-cat", name: "cat notes.md | grep todo",
      status: "completed", error: null,
      detail: { type: "unknown", input: { command: "cat notes.md | grep todo" }, output: [{ type: "text", text: "todo: one" }] },
    },
  });
  if (event.type !== "timeline.item" || event.item.type !== "tool_call") throw new Error("wrong event");
  assert.deepEqual(event.item.detail, { type: "shell", command: "cat notes.md | grep todo", output: "todo: one" });
});

test("aliases Claude Edit/Write snake_case input for the bridge edit row", () => {
  const edit = nativeEditSnapshot({ kind: "edit", input: { file_path: "/p/a.py", old_string: "a", new_string: "b" } });
  assert.deepEqual(edit.input, { file_path: "/p/a.py", old_string: "a", new_string: "b", filePath: "/p/a.py", oldString: "a", newString: "b" });
  const write = nativeEditSnapshot({ kind: "edit", input: { file_path: "/p/b.md", content: "# B" } });
  assert.deepEqual(write.input, { file_path: "/p/b.md", content: "# B", filePath: "/p/b.md", newString: "# B" });
  const native = { kind: "edit", input: { filePath: "/p/c", newString: "x", new_string: "y" } };
  assert.equal(nativeEditSnapshot(native), native);
  const shell = { kind: "execute", input: { command: "ls", content: "x" } };
  assert.equal(nativeEditSnapshot(shell), shell);
});
