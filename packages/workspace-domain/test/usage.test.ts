import { describe, expect, it } from "vitest";
import { applyMachineFacts, emptyWorkspace, LOCAL_MACHINE_ID, weeklyUsage } from "../src";

const at = "2026-11-16T12:00:00Z", reset = "2026-11-17T02:59:00Z";

describe("engine weekly usage", () => {
  it("keeps Claude's general and Fable windows by themselves", () => {
    const [claude] = weeklyUsage([{ engine: "claude", connected: true, usedPercent: 67, resetAt: reset, observedAt: at, windows: [
      { name: "seven_day", usedPercent: 67, resetAt: reset, observedAt: at },
      { name: "seven_day_fable", usedPercent: 2, resetAt: reset, observedAt: at },
    ] }]);
    expect(claude.windows?.map(w => [w.name, w.usedPercent])).toEqual([["seven_day", 67], ["seven_day_fable", 2]]);
  });

  it("treats an impossible percentage or an unknown window name as unknown, never clamped or shown", () => {
    const [claude] = weeklyUsage([{ engine: "claude", connected: true, usedPercent: 140, resetAt: null, observedAt: null, windows: [
      { name: "seven_day", usedPercent: 101, resetAt: null, observedAt: null },
      { name: "seven_day_fable", usedPercent: Number.NaN, resetAt: null, observedAt: null },
      { name: "scoped_week", usedPercent: 50, resetAt: null, observedAt: null },
    ] }]);
    expect(claude.usedPercent).toBeNull();
    expect(claude.windows).toEqual([
      { name: "seven_day", usedPercent: null, resetAt: null, observedAt: null },
      { name: "seven_day_fable", usedPercent: null, resetAt: null, observedAt: null },
    ]);
    expect(weeklyUsage([{ engine: "codex", connected: true, usedPercent: 33, resetAt: reset, observedAt: at }])[0]).not.toHaveProperty("windows");
    expect(weeklyUsage([{ engine: "opencode", connected: true, usedPercent: 3, resetAt: null, observedAt: null }])).toEqual([]);
  });

  it("replaces the previous usage with a newer machine read", () => {
    const first = weeklyUsage([{ engine: "claude", connected: true, usedPercent: 54, resetAt: reset, observedAt: at, windows: [{ name: "seven_day_fable", usedPercent: 0, resetAt: reset, observedAt: at }] }]);
    const later = weeklyUsage([{ engine: "claude", connected: true, usedPercent: 67, resetAt: reset, observedAt: "2026-11-16T12:05:00Z", windows: [{ name: "seven_day_fable", usedPercent: 2, resetAt: reset, observedAt: "2026-11-16T12:05:00Z" }] }]);
    const one = applyMachineFacts(emptyWorkspace(), { machineId: LOCAL_MACHINE_ID, engineUsage: first, patch: {} });
    const two = applyMachineFacts(one, { machineId: LOCAL_MACHINE_ID, engineUsage: later, patch: {} });
    expect(two.engineUsage[0].windows?.[0].usedPercent).toBe(2);
    expect(two.engineUsage[0].observedAt).toBe("2026-11-16T12:05:00Z");
  });
});
