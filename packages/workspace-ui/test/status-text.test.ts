import { describe, expect, it } from "vitest";
import { GENERIC_STATUS, STATUS_SETTINGS_PAGE, STATUS_TEXT, plainPhase, plainReason, plainStop, scrubCodes } from "../src/status-text";

const OWNER_SEEN = ["review_incomplete", "review_cycle_limit", "continuation_cap", "external_blocker", "outcome_unknown", "settlement_blocked", "queue_blocked",
  "task_files_changed", "stale_result", "workspace_drift", "not_run", "owner_integration", "check_preflight_unavailable", "review_in_progress",
  "no_model_listed_for_review", "no_model_listed_for_work_kind"];

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
  it("a stop only model settings resolve names the action and the settings page", () => {
    // 2026-11-25: no model had Review in its preferred work and the task read "stopped for a reason the app cannot describe".
    expect(plainStop("no_model_listed_for_review")).toBe("No model may review this work. Add Review to a model's Preferred work in Settings › Engines and models");
    expect(plainStop("no_model_listed_for_work_kind")).not.toBe(GENERIC_STATUS);
    for (const code of Object.keys(STATUS_SETTINGS_PAGE)) expect(Object.hasOwn(STATUS_TEXT, code), code).toBe(true);
    expect(STATUS_SETTINGS_PAGE.no_model_listed_for_review).toBe("engines");
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
