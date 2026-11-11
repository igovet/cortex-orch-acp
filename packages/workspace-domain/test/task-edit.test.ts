import { expect, it } from "vitest";
import { mapTask, taskEditable } from "../src";
const wire = { taskId:"t", description:"Saved",title:null,titleAvailability:"not_connected",version:"4",executionId:null,phase:"not_started",runtimeState:null,executionState:"not_started",waitReason:null,lastConfirmedAt:null,warnings:[] } as const;
it("only offers the saved request editor before the first start, never after pause, failure or unknown start", () => {
  const task = mapTask({...wire,warnings:[]}, {projectId:"p"});
  expect(task.sourceVersion).toBe("4");
  expect(taskEditable(task)).toBe(true);
  for (const patch of [{executionId:"e"}, {status:"paused" as const}, {status:"failed" as const}, {process:"unknown" as const}, {closedAs:"cancel" as const}, {archive:{at:1,previousStatus:"done" as const}}]) {
    expect(taskEditable({...task,...patch})).toBe(false);
  }
});
