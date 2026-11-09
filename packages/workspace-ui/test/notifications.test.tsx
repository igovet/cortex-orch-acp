// @vitest-environment jsdom
import { createSignal } from "solid-js";
import { render } from "solid-js/web";
import { afterEach, expect, it, vi } from "vitest";
import { emptyWorkspace, emptySession, type WorkspaceState } from "@cortex/workspace-domain";
import { WsContext, type Ws } from "../src/ctx";
import { QuestionNotifications } from "../src/notifications";
import { draftTask, flush } from "./support";

let dispose: (()=>void) | undefined;
afterEach(()=>{dispose?.(); document.body.innerHTML="";});
function mount(open=false, enabled=true, failure=false) {
  const initial=emptyWorkspace();
  initial.machines[0].connection="connected";
  initial.projects=[{id:"p",name:"P",machineId:initial.machines[0].id,repository:"/p",openBranch:"main",defaultBranch:"main"}];
  initial.tasks=[draftTask()];
  const session=emptySession("t");
  session.decisions=[{id:"q",kind:"question",from:"worker",workerId:null,at:1,text:"Which?",options:[],answer:null,withdrawn:false}];
  initial.sessions=[session];
  const [state,setState]=createSignal(initial), notify=vi.fn(async()=>{if(failure)throw new Error("unavailable");});
  const ws={store:{mode:"real",state},router:{route:()=>open?{name:"task",task:"t"}:{name:"home"}},panels:{notifications:()=>enabled},notify} as unknown as Ws;
  const root=document.createElement("div"); document.body.append(root);
  dispose=render(()=><WsContext.Provider value={ws}><QuestionNotifications/></WsContext.Provider>,root);
  return {notify,setState,state};
}
it("notifies once for an unseen question off-task, not on every refresh or reconnect", async()=>{
  const h=mount();
  await flush(); expect(h.notify).toHaveBeenCalledWith("Fix the race");
  h.setState(s=>({...s})); await flush(); expect(h.notify).toHaveBeenCalledTimes(1);
  h.setState(s=>({...s,machines:s.machines.map(m=>({...m,connection:"disconnected"}))}));
  h.setState(s=>({...s,machines:s.machines.map(m=>({...m,connection:"connected"}))}));
  await flush(); expect(h.notify).toHaveBeenCalledTimes(1);
  h.setState(s=>({...s,sessions:s.sessions.map(t=>({...t,decisions:t.decisions.map(d=>({...d,id:"q2"}))}))}));
  await flush(); expect(h.notify).toHaveBeenCalledTimes(2);
});
it("keeps the open task and disabled notifications quiet", async()=>{
  const h=mount(true); await flush(); expect(h.notify).not.toHaveBeenCalled();
  dispose?.();
  const off=mount(false,false); await flush(); expect(off.notify).not.toHaveBeenCalled();
});
it("names a delivery failure instead of pretending a notification arrived",async()=>{
  mount(false,true,true); await flush(); expect(document.querySelector('[role="alert"]')?.textContent).toContain("Questions remain in Inbox");
});
it("notifies for a hidden planning run through its visible sprint",async()=>{
  const h=mount(false); h.notify.mockClear();
  h.setState(s=>({...s,tasks:[],sprints:[{id:"sprint",projectId:"p",description:"Plan",title:"Sprint work",planningTaskId:"t",runMode:"manual",targetBranch:null,planning:"question",conversation:[],plan:null,materializedVersion:null,taskIds:[],queueState:"idle",pauseReason:null}],sessions:s.sessions.map(t=>({...t,decisions:t.decisions.map(d=>({...d,id:"planning-q"}))}))}));
  await flush(); expect(h.notify).toHaveBeenCalledWith("Sprint work");
});
