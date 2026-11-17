import { expect, it } from "vitest";
import { applyNativeTaskDelta, type NativeTaskDelta, type NativeTaskSnapshot } from "../src";
import type { Task } from "@cortex/workspace-contract";
const task = { taskId: "t", version: "1" } as Task;
const snapshot: NativeTaskSnapshot = { projectId: "p", generation: "g", highWatermark: "9007199254740993", version: "1", items: [task] };
const delta: NativeTaskDelta = { projectId: "p", batch: { generation: "g", streamKey: { scope: "project", project_id: "p" }, fromSequence: "9007199254740994", throughSequence: "9007199254740997",
  changes: [{ kind: "task", id: "t", baseVersion: "1", newVersion: "2", operation: { type: "remove" } }] } };
it("applies exact large decimal sequences without modifying the confirmed snapshot and ignores duplicates", () => {
  const next = applyNativeTaskDelta(snapshot, delta);
  expect(next.items).toEqual([]); expect(snapshot.items).toEqual([task]);
  expect(applyNativeTaskDelta(next, delta)).toBe(next);
});
it("fails closed on invalidation, a missing interval, cross-project stream, generation or entity base", () => {
  expect(() => applyNativeTaskDelta(snapshot, { ...delta, batch: {...delta.batch, changes:[{kind:"project",id:"p",baseVersion:"0",newVersion:"1",operation:{type:"invalidate"}}]} })).toThrow("resnapshot");
  expect(() => applyNativeTaskDelta(snapshot, { ...delta, projectId: "foreign" })).toThrow();
  expect(() => applyNativeTaskDelta(snapshot, { ...delta, batch: { ...delta.batch, generation: "other" } })).toThrow();
  expect(() => applyNativeTaskDelta(snapshot, { ...delta, batch: { ...delta.batch, fromSequence: "9007199254740995" } })).toThrow();
  expect(() => applyNativeTaskDelta(snapshot, { ...delta, batch: { ...delta.batch, changes: [{ ...delta.batch.changes[0], baseVersion: "0" }] } })).toThrow();
});
