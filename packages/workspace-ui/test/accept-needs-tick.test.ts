import { expect, it } from "vitest";
import type { Task } from "@cortex/workspace-domain";
import { acceptNeedsTick } from "../src/review-model";

const task = (over: Partial<Task>) => ({ result: { version: 1, digest: "d", summary: "", checks: [{ name: "runtime", outcome: "passed", detail: "" }] }, ...over }) as Task;

it("asks for the owner's tick when the machine requires acknowledgement although every listed check passed", () => {
  expect(acceptNeedsTick(task({}))).toBe(false);
  expect(acceptNeedsTick(task({ reviewActions: { accept: true, reason: null, unblockAction: null, acknowledgeOpenChecks: true } }))).toBe(true);
  expect(acceptNeedsTick(task({ reviewActions: { accept: true, reason: null, unblockAction: null, acknowledgeOpenChecks: false } }))).toBe(false);
});
