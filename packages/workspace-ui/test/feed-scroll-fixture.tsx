import {render} from "solid-js/web";
import {emptyWorkspace,emptySession,type Activity,type PortUpdate,type WorkspacePort} from "@cortex/workspace-domain";
import {Workspace} from "../src";
import {conversationOf} from "../src/port/history";
import {STEP_PREFIX} from "../src/tool-names";
import {createRealPort} from "../src/port/real";
import {withTask} from "./support";

const params=new URLSearchParams(location.search), view=params.get("view")??"worker";
const fixture:any={};
(window as any).fixture=fixture;
if(view.startsWith("startup")) {
  let resolved=false, fail=view==="startup-error";
  const info=await fetch("/fixture").then(r=>r.json());
  const transport:any={
    request:async(operation:string,message:any)=>{
      if(fail && operation==="machine.call" && message.op==="product.read") throw Error("Disposable product read refused");
      const response=await fetch("/rpc",{method:"POST",body:JSON.stringify({operation,message})}).then(r=>r.json());
      if(response.error) { (fixture.errors??=[]).push(operation+": "+response.error); throw Error(response.error); }
      // Artificially slow the real product read, not a fabricated product response.
      if(operation==="machine.call"&&message.op==="product.read") await new Promise(r=>setTimeout(r,1400));
      return response.result;
    },subscribe:()=>()=>{}
  };
  const port=createRealPort({transport,actorId:"fixture",registry:true});
  const listeners=new Set<any>(), subscribe=port.subscribe;
  port.subscribe=f=>{listeners.add(f);const stop=subscribe(f);return()=>{listeners.delete(f);stop();};};
  history.replaceState(null,"","/m/local/p/"+info.projectId+"/board");
  const samples:any[]=[]; let frames=0;
  const sample=()=>{samples.push({ms:performance.now(),loading:!!document.querySelector('[role=status]'),cards:[...document.querySelectorAll(".card")].map(e=>e.textContent),columns:[...document.querySelectorAll(".col")].map(e=>e.textContent)}); if(++frames<210)requestAnimationFrame(sample);else resolved=true;};
  render(()=><Workspace port={port}/>,document.getElementById("root")!);
  requestAnimationFrame(sample);
  Object.assign(fixture,{samples,info,done:()=>resolved,failRefresh:()=>{fail=true;listeners.forEach(f=>f({kind:"refresh"}));},recover:()=>{fail=false;[...document.querySelectorAll("button")].find(b=>b.textContent?.trim()==="Retry"||b.textContent?.trim()==="Try again")?.click();},refresh:()=>{resolved=false;frames=0;samples.length=0;listeners.forEach(f=>f({kind:"refresh"}));requestAnimationFrame(sample);}});
} else {
  const s=withTask(emptyWorkspace(),"Follow a streamed review");
  s.machines[0].connection="connected";
  Object.assign(s.tasks[0],{status:"working",process:"live",executionId:"e"});
  const prose=(i:number)=>"### Observation "+i+"\n\nReviewed the source and kept the reader’s place.\n\n"+(i%3===0?"| Check | State |\n| --- | --- |\n| Local | Passed |\n\n":"")+"- No remote systems changed\n- Preserve the local result\n";
  const activity:Activity[]=Array.from({length:Number(params.get("length")??240)},(_,i)=>({sequence:i+1,kind:"note",text:prose(i+1),at:null}));
  s.workers=[{id:"w",taskId:"t",executionId:"e",role:"frontend-dev",status:"working",lane:"execution",goal:"Read streaming content without losing your place",now:null,waitsFor:null,scope:[],route:null,attempt:null,elapsedMs:null,runningSince:null,context:null,usage:null,activity,result:null}];
  const session=emptySession("t");session.timeline=activity.map(a=>({id:"entry"+a.sequence,kind:"cortex",text:a.text}));s.sessions=[session];
  if(view.includes("tools")) {
    const details=[
      {title:"mcp__cortex__project_read",kind:"read",path:"src/app.py"},
      {title:"mcp__cortex__outside_write",kind:"edit",path:"src/notes.md",newText:"Saved locally."},
      {title:"mcp__codebase_memory_mcp__get_code_snippet",kind:"other"},
      {title:"mcp__cortex__read_skill",kind:"read"},
      {title:"mcp__cortex__result_submit",kind:"edit"},
      {title:"printf 'ready'",kind:"execute",command:"printf 'ready'",output:"ready",status:"completed"},
    ];
    details.forEach((d,i)=>activity.push({sequence:10001+i,kind:"tool",text:d.title,at:null,args:JSON.stringify(d)}));
    session.timeline.push(...conversationOf(details.map((d,i)=>({historyId:"tool"+i,revision:"1",kind:"tool",text:STEP_PREFIX+JSON.stringify(d)})),{description:"",answers:[]}).map(x=>x.entry));
  }
  let version=1;const listeners=new Set<(u:PortUpdate)=>void>();
  const emit=()=>listeners.forEach(f=>f({kind:"projection",scope:"workspace",version:String(++version),state:structuredClone(s)}));
  const port:WorkspacePort={mode:"real",query:(async(op:string)=>op==="capabilities.get"?{everything:true,operations:[]}:op==="project.snapshot"?[{kind:"projection",scope:"workspace",version:"1",state:structuredClone(s)}]:[]) as WorkspacePort["query"],subscribe:f=>{listeners.add(f);return()=>listeners.delete(f);},command:async()=>{throw Error("Read-only fixture");}};
  history.replaceState(null,"",view.startsWith("conversation")?"/m/local/t/t":"/m/local/t/t/workers/w");
  render(()=><Workspace port={port}/>,document.getElementById("root")!);
  Object.assign(fixture,{emit,expand:()=>{
    activity[0].text+="\n\n"+prose(0).repeat(12);
    const first=session.timeline[0]; if(first.kind==="cortex")first.text=activity[0].text;emit();
  },append:()=>{
    const n=activity.length+1; activity.push({sequence:n,kind:"note",text:prose(n),at:null});
    session.timeline.push({id:"entry"+n,kind:"cortex",text:prose(n)});emit();
  },stream:()=>{
    activity.at(-1)!.text+="\n\nMore streamed content.\n\n"+prose(activity.length);
    const last=session.timeline.at(-1)!;if(last.kind==="cortex")last.text=activity.at(-1)!.text;emit();
  }});
}
