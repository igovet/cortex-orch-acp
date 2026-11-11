// @vitest-environment jsdom
import { createSignal } from "solid-js";
import { render } from "solid-js/web";
import { afterEach, expect, it, vi } from "vitest";
import { emptyWorkspace, type Sprint, type WorkspaceCommand } from "@cortex/workspace-domain";
import { WsContext, type Ws } from "../src/ctx";
import { EditTask } from "../src/task-edit";
import { TaskAttachments } from "../src/composer";
import { SprintActions } from "../src/sprint";
import { buildIndex } from "../src/selectors";
import { Archive } from "../src/archive";
import { draftTask, flush } from "./support";

let dispose: (() => void) | undefined;
afterEach(() => { dispose?.(); document.body.innerHTML = ""; });
const savedFile = {name:"scope.txt",mimeType:"text/plain",data:"aGk="};
function fixture() {
  const state = emptyWorkspace(), machine = state.machines[0];
  machine.connection = "connected";
  state.projects = [{id:"p",machineId:machine.id,name:"Project",repository:"/p",openBranch:"main",defaultBranch:"main"}];
  state.tasks = [{...draftTask(),sourceVersion:"4",title:"Original"}];
  const [held, set] = createSignal(state);
  const commands: WorkspaceCommand[] = [];
  const read = vi.fn(async () => [savedFile]);
  const close = vi.fn();
  let answer = {ok:false,message:"The task changed",requestId:"r",outcome:"stale_version"};
  const ws = {store:{state:held,index:() => buildIndex(held(),1),mode:"real",readTaskAttachments:read,receipt:() => null,clock:() => 1,now:1},
    machine:() => machine,can:() => true,gate:() => undefined,needs:() => ({}),
    run:async (command: WorkspaceCommand) => { commands.push(command); return answer; },
    voiceAvailable:() => false,voice:undefined,t:() => "",router:{route:() => ({name:"archive",machine:machine.id})},
    go:vi.fn(),open:vi.fn(),newItem:vi.fn()} as unknown as Ws;
  const mount = (view: () => any) => { const root = document.createElement("div"); document.body.append(root); dispose = render(() => <WsContext.Provider value={ws}>{view()}</WsContext.Provider>,root); };
  return {ws,state,held,set,commands,read,close,mount,succeed:() => { answer = {...answer,ok:true,outcome:"recorded"}; }};
}
const button = (label: string) => [...document.querySelectorAll<HTMLButtonElement>("button")].find(b => b.textContent?.trim() === label || b.getAttribute("aria-label") === label)!;
const fill = (label: string, value: string) => { const input = document.querySelector<HTMLInputElement>(`[aria-label="${label}"]`)!; input.value=value; input.dispatchEvent(new Event("input",{bubbles:true})); };
it("loads saved attachments, retains a refused draft and saves all fields with its original version", async () => {
  const f = fixture();
  f.mount(() => <EditTask taskId="t" onClose={f.close} />);
  await flush();
  expect(button("Attach files")).toBeTruthy(); expect(button("Save changes").classList.contains("mc-send")).toBe(true);
  fill("Task title","Edited"); fill("Task description","New prompt");
  button("Save changes").click(); await flush();
  expect(f.commands[0]).toMatchObject({type:"edit_task",taskId:"t",expectedVersion:"4",title:"Edited",description:"New prompt",attachments:[savedFile],checkout:{kind:"main"}});
  expect(f.close).not.toHaveBeenCalled();
  expect(document.body.textContent).toContain("Your changes stay here");
  expect((document.querySelector('[aria-label="Task description"]') as HTMLTextAreaElement).value).toBe("New prompt");
  button("Remove scope.txt").click();
  f.succeed(); button("Save changes").click(); await flush();
  expect(f.commands[1]).toMatchObject({expectedVersion:"4",attachments:[]});
  expect(f.close).toHaveBeenCalledOnce();
});
it("never replaces attachments after a failed read, and turns read-only if Start wins", async () => {
  const f = fixture(); f.read.mockRejectedValueOnce(new Error("offline"));
  f.mount(() => <EditTask taskId="t" onClose={f.close} />);
  await flush();
  button("Save changes").click(); await flush(); expect(f.commands).toEqual([]);
  expect(document.body.textContent).toContain("Nothing was replaced");
  button("Retry").click(); await flush();
  f.set({...f.state,tasks:[{...f.state.tasks[0],executionId:"e",status:"working"}]});
  button("Save changes").click(); await flush(); expect(f.commands).toEqual([]);
  expect((document.querySelector('[aria-label="Task description"]') as HTMLTextAreaElement).disabled).toBe(true);
});
it("rereads attachments for a saved draft edit, not for every live task update", async () => {
  const f = fixture();
  f.mount(() => <TaskAttachments taskId="t" />);
  await flush(); expect(f.read).toHaveBeenCalledTimes(1);
  f.set({...f.state,tasks:[{...f.state.tasks[0],sourceVersion:"5"}]});
  await flush(); expect(f.read).toHaveBeenCalledTimes(2);
  f.set({...f.state,tasks:[{...f.state.tasks[0],sourceVersion:"6",executionId:"e",status:"working"}]});
  await flush(); expect(f.read).toHaveBeenCalledTimes(3);
  f.set({...f.state,tasks:[{...f.state.tasks[0],sourceVersion:"7",executionId:"e",status:"working"}]});
  await flush(); expect(f.read).toHaveBeenCalledTimes(3);
});
it.each([undefined,10])("offers destructive deletion even for archived sprints, retaining a refused confirmation for retry (%s)", async archivedAt => {
  const f = fixture();
  const sprint: Sprint = {id:"s",projectId:"p",title:"Delivery",description:"Scope",runMode:"automatic",planning:"created",conversation:[],plan:null,materializedVersion:1,taskIds:["t"],queueState:"paused",pauseReason:null,targetBranch:null,archivedAt};
  f.mount(() => <SprintActions sprint={sprint} tab="summary" />);
  button("Delete sprint…").click();
  const confirmation = (f.ws.open as ReturnType<typeof vi.fn>).mock.calls[0][0];
  expect(f.commands).toEqual([]);
  expect(confirmation.note).toContain("all 1 tasks");
  expect(confirmation.note).toContain("running work will be stopped");
  expect(await confirmation.go()).toBe(false);
  expect(confirmation.note).toContain("The task changed");
  f.succeed();
  expect(await confirmation.go()).toBe(true);
  expect(f.commands[1]).toEqual({type:"delete_sprint",projectId:"p",sprintId:"s",confirmed:true});
  expect(f.ws.go).toHaveBeenCalledWith({name:"home",machine:f.state.machines[0].id});
});
it("closes a sprint only after clear confirmation, hides it in active lists and retains it in Archive", async () => {
  const f = fixture();
  const sprint: Sprint = {id:"s",projectId:"p",title:"Delivery",description:"Scope",runMode:"automatic",planning:"created",conversation:[],plan:null,materializedVersion:1,taskIds:["t"],queueState:"running",pauseReason:null,targetBranch:null};
  f.state.sprints = [sprint]; f.state.tasks[0].sprintId="s";
  expect(buildIndex(f.state,1).summaries.get("p")!.sprints).toHaveLength(1);
  f.mount(() => <SprintActions sprint={sprint} tab="summary" />);
  button("New task").click(); expect(f.ws.newItem).toHaveBeenCalledWith("task","p","s");
  button("Close sprint…").click();
  const confirmation = (f.ws.open as ReturnType<typeof vi.fn>).mock.calls[0][0];
  expect(f.commands).toEqual([]);
  expect(confirmation.note).toContain("tasks already running continue");
  expect(confirmation.note).toContain("No task is cancelled");
  f.succeed(); await confirmation.go();
  expect(f.commands[0]).toEqual({type:"close_sprint",sprintId:"s",confirmed:true});
  dispose!(); dispose=undefined; document.body.innerHTML="";
  f.set({...f.state,sprints:[{...sprint,archivedAt:10,queueState:"paused"}]});
  const index = buildIndex(f.held(),1);
  expect(index.summaries.get("p")!.sprints).toEqual([]);
  expect(index.byId.get("t")!.sprint).toBeNull();
  f.mount(() => <Archive />);
  expect(document.body.textContent).toContain("Closed sprints");
  expect(document.body.textContent).toContain("Delivery");
  expect(document.body.textContent).toContain("Unfinished tasks remain in the project");
});
