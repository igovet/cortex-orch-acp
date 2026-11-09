/** Disposable browser harness. Only the local test server supplies data; no engines or owner stores. */
import { render } from "solid-js/web";
import { emptyWorkspace, type WorkspaceCommand, type WorkspacePort } from "@cortex/workspace-domain";
import { Workspace } from "../src";
import { withTask } from "./support";

const rpc = async (op: string, params = {}) => {
  const r = await fetch("/fixture", {method:"POST",headers:{"Content-Type":"application/json"},body:JSON.stringify({op,params})});
  return r.json();
};
const s = withTask(emptyWorkspace(), "Русская задача");
s.machines[0].connection="connected";
s.machines[0].diagnostics={at:Date.now(),status:"degraded",checks:8,items:[{name:"npm",ok:false,detail:"Not found by the desktop service",action:"Make npm available to the desktop service (not just your terminal), restart the service, then run diagnostics again."}]};
const refresh = async () => {
  const overview=await rpc("overview");
  Object.assign(s.machines[0],overview.result);
  const repo=await rpc("repo.snapshot"); s.repos=[repo.result];
};
await refresh();
const listeners=new Set<(u: never)=>void>();
let version=0;
const emit=()=>{for(const f of listeners) f({kind:"projection",scope:"workspace",version:String(++version),state:structuredClone(s)} as never);};
const commands: WorkspaceCommand[]=[];
const port:WorkspacePort={
  mode:"real",
  async query(operation) {return ({"capabilities.get":{everything:false,operations:["workspace.open_editor","workspace.open_vscode","workspace.set_worktree_root","workspace.save_surface","workspace.integrate","workspace.close_task","workspace.git_draft","workspace.git_commit","workspace.git_stage","tasks.start"]},"project.snapshot":[],"project.facts":[],"machine.facts":[]} as never)[operation];},
  subscribe(f){listeners.add(f);setTimeout(emit,100);return()=>listeners.delete(f);},
  async command(envelope){
    const c=envelope.payload;commands.push(c);
    let reply={ok:true,message:"Recorded"};
    if(c.type==="set_worktree_root") reply=await rpc("machine.update",{patch:{worktreeRoot:c.path}});
    if(c.type==="open_editor") reply=await rpc("repo.locate",{checkoutId:c.checkoutId});
    if(reply.ok){await refresh();emit();}
    return {requestId:envelope.requestId,ok:reply.ok,outcome:reply.ok?"recorded":"refused",message:reply.message??"Saved"};
  }
};
(window as never as {fixture:unknown}).fixture={s,emit,commands};
render(()=><Workspace port={port}/>,document.getElementById("root")!);
