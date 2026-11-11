import { expect, it } from "vitest";
import { applySprintDeletion, capabilityOf, emptyWorkspace, MACHINE_COMMANDS, mapTask } from "../src";
it("removes confirmed sprint tasks from primary and secondary surfaces, preserving other work", () => {
  const state = emptyWorkspace();
  const wire = {description:"Saved",title:null,titleAvailability:"not_connected",version:"1",executionId:null,phase:"not_started",runtimeState:null,executionState:"not_started",waitReason:null,lastConfirmedAt:null,warnings:[]} as const;
  state.tasks = ["deleted","other"].map(taskId => mapTask({...wire,taskId,warnings:[]},{projectId:"p"}));
  state.events = state.tasks.map(t => ({id:t.id,taskId:t.id,projectId:"p",sequence:1,text:"activity"}));
  const next = applySprintDeletion(state,"s",new Set(["deleted"]));
  expect(next.tasks.map(t => t.id)).toEqual(["other"]);
  expect(next.events.map(t => t.taskId)).toEqual(["other"]);
  expect(applySprintDeletion(next,"s",new Set(["deleted"]))).toEqual(next);
  expect(capabilityOf("delete_sprint")).toBe("workspace.delete_sprint");
  expect(MACHINE_COMMANDS).toContain("delete_sprint");
});
