import { describe, expect, it } from "vitest";
import { GENERIC_STATUS, STATUS_TEXT, plainPhase, plainReason, plainStop, scrubCodes } from "../src/status-text";

const OWNER_SEEN = ["review_incomplete", "review_cycle_limit", "continuation_cap", "external_blocker", "outcome_unknown", "settlement_blocked", "queue_blocked",
  "task_files_changed", "stale_result", "workspace_drift", "not_run", "owner_integration", "check_preflight_unavailable", "review_in_progress"];

describe("status text", () => {
  it("every known code reads as a sentence with no underscore, in each presentation", () => {
    for (const code of [...Object.keys(STATUS_TEXT), ...OWNER_SEEN]) {
      for (const text of [plainReason(code), plainStop(code), plainPhase(code), scrubCodes(`Automatic continuation: ${code}`), plainReason(`Stopped: ${code}.`)]) {
        expect(text, code).not.toMatch(/[a-z]+_[a-z_]+/);
        expect(text, code).not.toContain("_");
      }
      expect(plainReason(code), code).not.toBe(code);
    }
  });
  it("owner-seen codes have their own sentence", () => {
    for (const code of OWNER_SEEN) expect(Object.hasOwn(STATUS_TEXT, code), code).toBe(true);
  });
  it("an unknown bare code is a generic sentence, never the code", () => {
    expect(plainReason("brand_new_code_9")).toBe(GENERIC_STATUS);
    expect(plainReason("brand_new_code_9", "Run waiting")).toBe("Run waiting");
    expect(plainStop("brand_new_code_9")).toBe(GENERIC_STATUS);
    expect(plainPhase("brand_new_phase")).not.toContain("_");
  });
  it("prose keeps its words and only loses embedded codes", () => {
    expect(plainReason("Задача остановлена: что дальше?")).toBe("Задача остановлена: что дальше?");
    expect(plainReason("Queue paused: review_cycle_limit.")).toBe("Queue paused: the review reached its retry limit.");
    expect(plainReason("  ")).toBe("");
  });
});
