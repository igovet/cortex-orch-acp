import assert from "node:assert/strict";
import test from "node:test";
import { unavailableMessage } from "../shared/account-usage-display.ts";

test("a completed Claude turn without a rate-limit event stays explicit", () => {
  assert.equal(
    unavailableMessage("claude-agent-acp", { available: true, connections: [] }, 1),
    "No Claude rate-limit event observed",
  );
});

test("unavailable states explain the failed layer", () => {
  assert.equal(
    unavailableMessage("codex-acp", { available: true, connections: [] }, 1),
    "No current native plan window reported",
  );
  assert.equal(
    unavailableMessage("codex-acp", { available: true, connections: [] }, 0),
    "No configured connection",
  );
  assert.equal(
    unavailableMessage("claude-agent-acp", { available: false, connections: [] }, 0),
    "Usage reader failed",
  );
  assert.equal(unavailableMessage("claude-agent-acp", null, 0), "Loading…");
});
