import { describe, expect, it } from "vitest";
import { pauseSettingsPage } from "../src/session-model";
import type { TaskView } from "../src/selectors";

const view = (over: Record<string, unknown>) => ({ id: "t1", sprint: null, task: { status: "working", automaticContinuation: null }, ...over }) as unknown as TaskView;
const paused = (reason: string) => view({ task: { status: "paused", automaticContinuation: { pause: { reason } } } });
const held = (reason: string, taskId = "t1", kind = "continuation") =>
  view({ sprint: { runMode: "automatic", automaticQueue: { kind, code: reason, reason, taskId, needsOwner: false } } });

describe("pause reason settings link", () => {
  it("a pause for a missing model listing opens Engines and models", () => {
    expect(pauseSettingsPage(paused("no_model_listed_for_review"))).toBe("engines");
    expect(pauseSettingsPage(paused("no_model_listed_for_work_kind"))).toBe("engines");
  });
  it("other reasons and running tasks have no settings link", () => {
    expect(pauseSettingsPage(paused("external_blocker"))).toBeNull();
    expect(pauseSettingsPage(view({}))).toBeNull();
  });
  it("a queue hold on this task for such a reason links too; another task's hold does not", () => {
    const h = held("no_model_listed_for_review");
    expect(pauseSettingsPage(h)).toBe("engines");
    expect(pauseSettingsPage(held("no_model_listed_for_review", "other"))).toBeNull();
  });
});
