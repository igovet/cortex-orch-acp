import assert from "node:assert/strict";
import { withAccessModeVisuals } from "./provider.ts";

// Cortex's two access modes get Paseo shield visuals; nothing else changes,
// and no mode is ever marked unattended (Paseo must not pick Auto itself).
const state = withAccessModeVisuals({
  mode: "guarded",
  modes: [
    { id: "guarded", label: "Guarded mode" },
    { id: "auto", label: "Auto mode" },
    { id: "other", label: "Other" },
  ],
});
const [guarded, auto, other] = state.modes as unknown as Array<Record<string, unknown>>;
assert.equal(state.mode, "guarded");
assert.deepEqual(guarded, { id: "guarded", label: "Guarded mode", icon: "ShieldCheck", colorTier: "safe" });
assert.deepEqual(auto, { id: "auto", label: "Auto mode", icon: "ShieldOff", colorTier: "dangerous" });
assert.deepEqual(other, { id: "other", label: "Other" });
assert.ok(state.modes.every((mode) => !("isUnattended" in mode)));
console.log("ok");
