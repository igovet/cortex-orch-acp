import assert from "node:assert/strict";
import { emulatesSteer, steerRefusal } from "./provider.ts";

// "Provider does not support prompt.steer": Cortex advertises steer, then
// refuses each one so Paseo replaces the running turn itself. Delivering it
// as a follow-up raised "Active turn changed before steering could be delivered".
assert.equal(emulatesSteer(["prompt.message", "prompt.steer"], ["prompt.message"]), true);
assert.equal(emulatesSteer(["prompt.message"], ["prompt.message"]), false);
assert.equal(emulatesSteer(["prompt.steer"], ["prompt.steer"]), false);
const refusal = steerRefusal({ type: "session.prompt", sessionId: "s",
  prompt: { clientMessageId: "m", delivery: "steer", input: { type: "message", content: [] } } }) as any;
assert.equal(refusal.type, "session.prompt_result");
assert.equal(refusal.sessionId, "s");
assert.equal(refusal.clientMessageId, "m");
assert.equal(refusal.result.type, "failed");
assert.equal(steerRefusal({ type: "session.prompt", sessionId: "s",
  prompt: { clientMessageId: "m", delivery: "auto", input: { type: "message", content: [] } } }), null);
assert.equal(steerRefusal({ type: "session.cancel", sessionId: "s" }), null);
console.log("ok");
