import assert from "node:assert/strict";
import { emulatesSteer, QUEUED_STEER_CONFIG_ID, steerText, wrapCortexConnection } from "./provider.ts";

assert.equal(emulatesSteer(["prompt.message", "prompt.steer"], ["prompt.message"]), true);
assert.equal(emulatesSteer(["prompt.message"], ["prompt.message"]), false);
assert.equal(emulatesSteer(["prompt.steer"], ["prompt.steer"]), false);

const prompt = { type: "session.prompt", sessionId: "s", prompt: {
  clientMessageId: "message-1", delivery: "steer",
  input: { type: "message", content: [{ type: "text", text: "Change the title" }] },
} } as any;
assert.equal(steerText(prompt), "Change the title");

const sent: any[] = [];
const bridgeListeners = new Set<(event: any) => void>();
const emit = (event: any) => bridgeListeners.forEach((listener) => listener(event));
const connection: any = {
  capabilities: ["prompt.message"],
  async send(input: any) {
    sent.push(input);
    if (input.type === "session.configure") {
      queueMicrotask(() => emit({ type: "request.completed", requestId: input.requestId }));
    }
  },
  onEvent(listener: (event: any) => void) {
    bridgeListeners.add(listener);
    return () => bridgeListeners.delete(listener);
  },
  async close() {},
};
const wrapped = wrapCortexConnection(connection, ["prompt.message", "prompt.steer"]);
const seen: any[] = [];
wrapped.onEvent((event) => seen.push(event));
emit({ type: "session.turn", sessionId: "s", turnId: "acp:original", state: "started" });
await wrapped.send(prompt);
assert.equal(sent.length, 1);
assert.equal(sent[0].type, "session.configure");
assert.deepEqual(JSON.parse(sent[0].changes.settings[QUEUED_STEER_CONFIG_ID]), {
  clientMessageId: "message-1", text: "Change the title",
});
assert.ok(!sent.some((item) => item.type === "session.interrupt"));
assert.ok(seen.some((event) => event.type === "timeline.item" &&
  event.item.type === "user_message" && event.item.text === "Change the title"));
assert.deepEqual(seen.find((event) => event.type === "session.prompt_result")?.result,
  { type: "steer", turnId: "acp:original" });
assert.ok(!seen.some((event) => event.type === "request.completed"));
seen.length = 0;
sent.length = 0;
await wrapped.send({ ...prompt, prompt: { ...prompt.prompt, clientMessageId: "image-1",
  input: { type: "message", content: [{ type: "image", data: "x", mimeType: "image/png" }] },
} } as any);
assert.equal(sent.length, 0);
assert.equal(seen.find((event) => event.type === "session.prompt_result")?.result.type, "completed");
assert.equal(seen.find((event) => event.type === "session.notice")?.notice.severity, "warning");
console.log("ok");
