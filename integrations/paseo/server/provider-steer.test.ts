import assert from "node:assert/strict";
import { ProviderEventSchema } from "@getpaseo/plugin/server/provider";
import { emulatesSteer, QUEUED_MESSAGE_CONFIG_ID, queuedContent, wrapCortexConnection } from "./provider.ts";

assert.equal(emulatesSteer(["prompt.message", "prompt.steer"], ["prompt.message"]), true);
assert.equal(emulatesSteer(["prompt.message"], ["prompt.message"]), false);
assert.equal(emulatesSteer(["prompt.steer"], ["prompt.steer"]), false);

const prompt = { type: "session.prompt", sessionId: "s", prompt: {
  clientMessageId: "message-1", delivery: "steer",
  input: { type: "message", content: [{ type: "text", text: "Change the title" }] },
} } as any;
assert.deepEqual(queuedContent(prompt), [{ type: "text", text: "Change the title" }]);

/** A stand-in for Paseo's ACP bridge; `configure` decides each capture outcome. */
function bridge(configure: (input: any, emit: (event: any) => void) => void) {
  const sent: any[] = [];
  const listeners = new Set<(event: any) => void>();
  const emit = (event: any) => listeners.forEach((listener) => listener(event));
  const connection: any = {
    capabilities: ["prompt.message"],
    async send(input: any) {
      sent.push(input);
      if (input.type === "session.configure") configure(input, emit);
    },
    onEvent(listener: (event: any) => void) {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    async close() {},
  };
  const wrapped = wrapCortexConnection(connection, ["prompt.message", "prompt.steer"]);
  const seen: any[] = [];
  // Paseo parses every provider event with this schema (plugin-provider.js
  // normalizeConnection); an invalid event would throw inside the queue send.
  wrapped.onEvent((event) => seen.push(ProviderEventSchema.parse(event)));
  return { sent, emit, wrapped, seen };
}

/**
 * Paseo 0.9.2's own decision for a steer (agent-manager.js
 * steerOrReplaceActiveTurn, plugin-provider.js steerActiveTurn and
 * prompt): only a `steer` result for the turn it expects is accepted; any
 * other result replaces the running turn (session/cancel, then a new
 * prompt); a rejected send is reported as an error and touches nothing.
 */
async function paseoSteer(wrapped: any, input: any, expectedTurnId: string) {
  let result: any;
  const off = wrapped.onEvent((event: any) => {
    if (event.type === "session.prompt_result" && event.clientMessageId === input.prompt.clientMessageId) {
      result ??= event.result;
    }
  });
  try {
    await wrapped.send(input);
  } catch (error) {
    return { disposition: "error", message: (error as Error).message };
  } finally {
    off();
  }
  assert.ok(result, "a sent steer must produce its prompt result");
  return result.type === "steer" && result.turnId === expectedTurnId
    ? { disposition: "steered" } : { disposition: "replaced" };
}

// Durable capture: accepted for the running turn, no interrupt.
{
  const { sent, emit, wrapped, seen } = bridge((input, emit) =>
    queueMicrotask(() => emit({ type: "request.completed", requestId: input.requestId })));
  emit({ type: "session.turn", sessionId: "s", turnId: "acp:original", state: "started" });
  assert.deepEqual(await paseoSteer(wrapped, prompt, "acp:original"), { disposition: "steered" });
  assert.equal(sent.length, 1);
  assert.equal(sent[0].type, "session.configure");
  assert.deepEqual(JSON.parse(sent[0].changes.settings[QUEUED_MESSAGE_CONFIG_ID]), {
    clientMessageId: "message-1", content: [{ type: "text", text: "Change the title" }],
  });
  assert.ok(!sent.some((item) => item.type === "session.interrupt"));
  assert.ok(seen.some((event) => event.type === "timeline.item" &&
    event.item.type === "user_message" && event.item.text === "Change the title"));
  assert.ok(!seen.some((event) => event.type === "session.notice"));
  assert.ok(!seen.some((event) => event.type === "request.completed"));
}

// The turn ends while Cortex captures the queued message. Paseo still
// expects that turn, and Cortex continues the captured message in the
// background, so it is accepted rather than replayed as a second,
// interrupting prompt.
{
  let finish: () => void = () => {};
  const { emit, wrapped, seen } = bridge((input, emit) => {
    finish = () => emit({ type: "request.completed", requestId: input.requestId });
  });
  emit({ type: "session.turn", sessionId: "s", turnId: "acp:ending", state: "started" });
  const steered = paseoSteer(wrapped, { ...prompt, prompt: { ...prompt.prompt, clientMessageId: "race-1" } },
    "acp:ending");
  await new Promise((resolve) => setImmediate(resolve));
  emit({ type: "session.turn", sessionId: "s", turnId: "acp:ending", state: "completed" });
  finish();
  assert.deepEqual(await steered, { disposition: "steered" });
  assert.ok(!seen.some((event) => event.type === "session.notice"));
}

// Cortex refuses the capture: a truthful warning naming Cortex's reason, and
// Paseo is told the send failed, so it neither interrupts nor replaces the turn.
{
  const { sent, emit, wrapped, seen } = bridge((input, emit) =>
    queueMicrotask(() => emit({ type: "request.failed", requestId: input.requestId,
      error: { message: "This session no longer owns the task." } })));
  emit({ type: "session.turn", sessionId: "s", turnId: "acp:running", state: "started" });
  const outcome = await paseoSteer(wrapped, { ...prompt, prompt: { ...prompt.prompt, clientMessageId: "refused-1" } },
    "acp:running");
  assert.equal(outcome.disposition, "error");
  assert.match(outcome.message!, /no longer owns the task/);
  assert.equal(sent.length, 1);
  assert.ok(!sent.some((item) => item.type === "session.interrupt"));
  assert.ok(!seen.some((event) => event.type === "session.prompt_result"));
  assert.ok(!seen.some((event) => event.type === "timeline.item"));
  const notice = seen.find((event) => event.type === "session.notice")?.notice;
  assert.equal(notice?.severity, "warning");
  assert.equal(notice?.title, "Message was not queued");
  assert.match(notice?.description, /no longer owns the task/);
  assert.match(notice?.description, /Nothing was sent/);
}

// The bridge itself rejects the configure request.
{
  const { wrapped, seen } = bridge(() => { throw new Error("ACP transport is closed"); });
  const outcome = await paseoSteer(wrapped, { ...prompt, prompt: { ...prompt.prompt, clientMessageId: "closed-1" } },
    "acp:running");
  assert.equal(outcome.disposition, "error");
  assert.match(seen.find((event) => event.type === "session.notice")?.notice.description, /ACP transport is closed/);
}

// Image content sent during an active turn: durably queued (not refused),
// carried as an ACP-shaped image content block, and echoed with a short
// placeholder since the synthetic timeline item carries only text.
{
  const { sent, emit, wrapped, seen } = bridge((input, emit) =>
    queueMicrotask(() => emit({ type: "request.completed", requestId: input.requestId })));
  emit({ type: "session.turn", sessionId: "s", turnId: "acp:running", state: "started" });
  const outcome = await paseoSteer(wrapped, { ...prompt, prompt: { ...prompt.prompt, clientMessageId: "image-1",
    input: { type: "message", content: [
      { type: "text", text: "See attached" },
      { type: "image", data: "eA==", mimeType: "image/png" },
    ] },
  } }, "acp:running");
  assert.equal(outcome.disposition, "steered");
  assert.equal(sent.length, 1);
  assert.deepEqual(JSON.parse(sent[0].changes.settings[QUEUED_MESSAGE_CONFIG_ID]), {
    clientMessageId: "image-1", content: [
      { type: "text", text: "See attached" },
      { type: "image", data: "eA==", mimeType: "image/png" },
    ],
  });
  assert.ok(seen.some((event) => event.type === "timeline.item" &&
    event.item.type === "user_message" && event.item.text === "See attached\n[image attached]"));
  assert.ok(!seen.some((event) => event.type === "session.notice"));
}

// An uploaded file is carried as a resource_link content block, addressed
// by its staged path as a file:// URI.
{
  const { sent, emit, wrapped } = bridge((input, emit) =>
    queueMicrotask(() => emit({ type: "request.completed", requestId: input.requestId })));
  emit({ type: "session.turn", sessionId: "s", turnId: "acp:running", state: "started" });
  const outcome = await paseoSteer(wrapped, { ...prompt, prompt: { ...prompt.prompt, clientMessageId: "file-1",
    input: { type: "message", content: [
      { type: "uploaded_file", id: "u1", fileName: "notes.txt", mimeType: "text/plain",
        size: 12, path: "/tmp/notes.txt" },
    ] },
  } }, "acp:running");
  assert.equal(outcome.disposition, "steered");
  const sentContent = JSON.parse(sent[0].changes.settings[QUEUED_MESSAGE_CONFIG_ID]).content;
  assert.deepEqual(sentContent, [
    { type: "resource_link", name: "notes.txt", uri: "file:///tmp/notes.txt", mimeType: "text/plain" },
  ]);
}

// Unsupported attachment content: warned and refused, never passed to
// Paseo's cancel-and-replace fallback.
{
  const { sent, emit, wrapped, seen } = bridge(() => assert.fail("unsupported content must not be configured"));
  emit({ type: "session.turn", sessionId: "s", turnId: "acp:running", state: "started" });
  const outcome = await paseoSteer(wrapped, { ...prompt, prompt: { ...prompt.prompt, clientMessageId: "review-1",
    input: { type: "message", content: [{
      type: "review", mimeType: "application/paseo-review", cwd: "/tmp", mode: "uncommitted", comments: [],
    }] },
  } }, "acp:running");
  assert.equal(outcome.disposition, "error");
  assert.equal(sent.length, 0);
  assert.ok(!seen.some((event) => event.type === "session.prompt_result"));
  assert.equal(seen.find((event) => event.type === "session.notice")?.notice.severity, "warning");
}
console.log("ok");
