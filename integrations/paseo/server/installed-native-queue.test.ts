/** Explicit no-paid installed-client helper qualification; not a GUI test.
 * Pass the inspected Metro bundle path. Nothing launches or writes to Paseo.
 */
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { runInNewContext } from "node:vm";
import { ProviderEventSchema } from "@getpaseo/plugin/server/provider";
import { QUEUED_MESSAGE_CONFIG_ID, wrapCortexConnection } from "./provider.ts";

const bundlePath = process.argv[2];
assert.ok(bundlePath, "Supply the installed Paseo Metro bundle path");
const bundle = readFileSync(bundlePath, "utf8");

// Evaluate only the selected installed helper module with explicit stand-ins
// for its dependencies, never the application, accounts or daemon.
function helper(marker: string, dependencies: any[] = []) {
  const at = bundle.indexOf(marker);
  assert.ok(at >= 0, `Installed helper missing: ${marker}`);
  const start = bundle.lastIndexOf("__d(", at);
  const end = bundle.indexOf("\n__d(", at);
  assert.ok(start >= 0 && end > at, "Complete Metro module required");
  let factory: any;
  let ids: number[] = [];
  runInNewContext(bundle.slice(start, end), {
    __d(fn: any, _id: number, imports: number[]) { factory = fn; ids = imports; },
  }, { timeout: 1000 });
  assert.equal(ids.length, dependencies.length, "Installed helper dependencies changed");
  const exports: any = {};
  factory({}, (id: number) => {
    const index = ids.indexOf(id);
    assert.ok(index >= 0, "Unexpected helper import");
    return dependencies[index];
  }, undefined, undefined, {}, exports, ids);
  return exports;
}

const attachments = helper("e.userAttachmentsOnly=function");
const actions = helper("e.runAlternateSendAction=function");
let nextId = 0;
const queueHelpers = helper("e.queueComposerMessage=function", [
  attachments, {}, { generateMessageId: () => `native-${++nextId}` },
  { i18n: { t: () => "Failed to send" } },
]);
let state = new Map<string, any[]>();
const queue = {
  read: (id: string) => state.get(id) ?? [],
  write: (update: (value: Map<string, any[]>) => Map<string, any[]>) => { state = update(state); },
};
const file = { kind: "file", attachment: { id: "file-1" } };
const review = { kind: "review" };
let sent = 0;
const action = {
  defaultSendBehavior: "steer", isAgentRunning: true, onQueue: () => {},
  handleSendMessage: () => { sent++; },
  handleQueueMessage: () => queueHelpers.queueComposerMessage({
    agentId: "s", text: " Original ", attachments: [file, review], queue,
  }),
};
actions.runAlternateSendAction(action);
assert.equal(sent, 0);
const original = queue.read("s")[0];
assert.equal(original.text, "Original");
const draft = queueHelpers.editQueuedComposerMessage({ agentId: "s", messageId: original.id, queue });
assert.equal(draft.text, "Original");
assert.deepEqual(draft.attachments, [file]);
assert.equal(queue.read("s").length, 0);
assert.equal(queueHelpers.editQueuedComposerMessage({ agentId: "s", messageId: original.id, queue }), null);

// Only the replacement is delivered through Cortex's real provider wrapper.
const delivered: any[] = [];
const listeners = new Set<(event: any) => void>();
const emit = (event: any) => listeners.forEach((listener) => listener(event));
const wrapped = wrapCortexConnection({
  capabilities: ["prompt.message"],
  async send(input: any) {
    delivered.push(input);
    emit({ type: "request.completed", requestId: input.requestId });
  },
  onEvent(listener: (event: any) => void) { listeners.add(listener); return () => listeners.delete(listener); },
  async close() {},
} as any, ["prompt.message", "prompt.steer"]);
wrapped.onEvent((event) => { ProviderEventSchema.parse(event); });
emit({ type: "session.turn", sessionId: "s", turnId: "active", state: "started" });
const replacement = queueHelpers.queueComposerMessage({ agentId: "s", text: "Edited", attachments: [], queue }).queued;
assert.equal((await queueHelpers.sendQueuedComposerMessageNow({
  agentId: "s", messageId: replacement.id, queue,
  submitMessage: async ({ text }: { text: string }) => wrapped.send({
    type: "session.prompt", sessionId: "s", prompt: {
      clientMessageId: replacement.id, delivery: "steer",
      input: { type: "message", content: [{ type: "text", text }] },
    },
  }),
})).status, "submitted");
assert.equal(delivered.length, 1);
assert.equal(delivered[0].type, "session.configure");
assert.deepEqual(JSON.parse(delivered[0].changes.settings[QUEUED_MESSAGE_CONFIG_ID]), {
  clientMessageId: replacement.id, content: [{ type: "text", text: "Edited" }],
});
assert.equal(queueHelpers.editQueuedComposerMessage({ agentId: "s", messageId: replacement.id, queue }), null);
assert.equal(queue.read("s").length, 0);
await wrapped.close();

// Failed sends restore the native item; missing/consumed items are not edits.
const retry = queueHelpers.queueComposerMessage({ agentId: "s", text: "Retry", attachments: [], queue }).queued;
assert.equal((await queueHelpers.sendQueuedComposerMessageNow({
  agentId: "s", messageId: retry.id, queue, submitMessage: async () => { throw new Error("Capture refused"); },
})).status, "failed");
assert.equal(queue.read("s")[0].id, retry.id);
assert.equal(queueHelpers.editQueuedComposerMessage({ agentId: "s", messageId: retry.id, queue }).text, "Retry");
actions.runDefaultSendAction(action);
assert.equal(sent, 1, "Default Steer sends immediately, bypassing the native queue");
console.log("ok: installed native helpers and Cortex provider (not rendered GUI)");
