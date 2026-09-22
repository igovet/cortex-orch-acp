import assert from "node:assert/strict";
import { emulatesSteer, steerAsFollowUp } from "./provider.ts";

// "Provider does not support prompt.steer": a message typed during a
// running turn must still reach Cortex, as the bridge's follow-up.
assert.equal(emulatesSteer(["prompt.message", "prompt.steer"], ["prompt.message"]), true);
assert.equal(emulatesSteer(["prompt.message"], ["prompt.message"]), false);
assert.equal(emulatesSteer(["prompt.steer"], ["prompt.steer"]), false);
const steered = steerAsFollowUp({ type: "session.prompt", sessionId: "s",
  prompt: { clientMessageId: "m", delivery: "steer", input: { type: "message", content: [] } } });
assert.equal(steered.prompt.delivery, "auto");
const other = { type: "session.cancel", sessionId: "s" };
assert.equal(steerAsFollowUp(other), other);
console.log("ok");
