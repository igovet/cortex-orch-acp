// @vitest-environment jsdom
import { render } from "solid-js/web";
import { afterEach, expect, it } from "vitest";
import { emptyWorkspace, taskEditable, taskRenamable, type Task, type WorkspaceCommand } from "@cortex/workspace-domain";
import { WsContext, type Ws } from "../src/ctx";
import { TaskTitle } from "../src/task-chrome";
import { buildIndex } from "../src/selectors";
import { draftTask, flush } from "./support";

let dispose: (() => void) | undefined;
afterEach(() => { dispose?.(); document.body.innerHTML = ""; });

function mount(task: Task) {
  const state = emptyWorkspace(), machine = state.machines[0];
  machine.connection = "connected";
  state.projects = [{id:"p",machineId:machine.id,name:"Project",repository:"/p",openBranch:"main",defaultBranch:"main"}];
  state.tasks = [task];
  const commands: WorkspaceCommand[] = [];
  const ws = {gate:() => undefined,run:async (command: WorkspaceCommand) => { commands.push(command); return {ok:true,requestId:"r",outcome:"recorded"}; }} as unknown as Ws;
  const view = buildIndex(state, 1).byId.get(task.id)!;
  const root = document.createElement("div"); document.body.append(root);
  dispose = render(() => <WsContext.Provider value={ws}><TaskTitle view={view} /></WsContext.Provider>, root);
  return commands;
}

// Live run: a started task's title could not be changed ("I simply can't change the name"). The title is presentation, not
// lifecycle, so the crumb opens the same Save/Cancel editor for a started, finished or closed task; only a cleared one is fixed.
it.each([
  ["working", {status:"working",executionId:"e",process:"live"}],
  ["finished", {status:"ready_to_merge",executionId:"e"}],
  ["closed", {status:"done",executionId:"e",closedAs:"as_is"}],
] as const)("renames a %s task through the title editor", async (_name, patch) => {
  const task = {...draftTask(),...patch} as Task;
  expect(taskEditable(task)).toBe(false);
  expect(taskRenamable(task)).toBe(true);
  const commands = mount(task);
  document.querySelector<HTMLButtonElement>("button.ws-title")!.click();
  await flush();
  const input = document.querySelector<HTMLInputElement>('[aria-label="Task title"]')!;
  expect(input.readOnly).toBe(false);
  input.value = "  Owner name  "; input.dispatchEvent(new Event("input",{bubbles:true}));
  [...document.querySelectorAll<HTMLButtonElement>("button")].find(b => b.textContent === "Save")!.click();
  await flush();
  expect(commands).toEqual([{type:"rename_task",taskId:"t",title:"Owner name"}]);
});

it("keeps a cleared task's title read-only", () => {
  const task = {...draftTask(),status:"done",executionId:"e",archive:{at:1,previousStatus:"done"}} as Task;
  expect(taskRenamable(task)).toBe(false);
  mount(task);
  expect(document.querySelector("button.ws-title")).toBeNull();
  expect(document.querySelector("span.ws-title")?.textContent).toBe("Fix the race");
});
