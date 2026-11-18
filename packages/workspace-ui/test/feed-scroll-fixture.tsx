import {render} from "solid-js/web";
import {emptyWorkspace,emptySession,type Activity,type PortUpdate,type WorkspacePort} from "@cortex/workspace-domain";
import {Workspace} from "../src";
import {conversationOf} from "../src/port/history";
import {STEP_PREFIX} from "../src/tool-names";
import {createRealPort} from "../src/port/real";
import {withTask} from "./support";
import {conversationWorld} from "./conversation-world";
import {pagesWorld} from "./pages-world";

const params=new URLSearchParams(location.search), view=params.get("view")??"worker";
const fixture:any={};
(window as any).fixture=fixture;
// The app's own theme choice, read at start: "light" or "dark".
if(params.get("theme")) localStorage.setItem("cw.theme",params.get("theme")!);
// The sidebar dragged to a width, as the owner can (prefs.ts SIDEBAR_MAX is the widest).
if(params.get("sidebar")) localStorage.setItem("cw.ws.sidebarWidth",params.get("sidebar")!);
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
} else if(view.startsWith("finished")) {
  // A task with a result to review: a long conversation that ends in a tall result card, a team of 15 in the dock's workers bar
  // and the result composer. `finished-live` keeps the coordinator working, so the feed ends in its Working line instead, and
  // `finished-worker` opens a finished worker's page: its feed ends in the worker's result, over a dock with no composer.
  const s=conversationWorld(), live=view==="finished-live";
  const task=s.tasks.find(t=>t.id==="c-done")!, session=s.sessions.find(x=>x.taskId==="c-done")!;
  if(live) Object.assign(task,{status:"working",process:"live",executionId:"e-c-done"});
  const checks=Array.from({length:12},(_,i)=>({name:"Worker check "+(i+1)+": the refresh path keeps its lock under load",detail:"Reported by the worker",outcome:"passed" as const,source:"worker" as const}));
  task.result={...task.result!,checks:[...task.result!.checks,...checks]};
  s.workers=s.workers.filter(w=>w.taskId!=="c-done");
  const notes=(i:number):Activity[]=>Array.from({length:60},(_,k)=>({sequence:k+1,kind:"note" as const,text:"### Part "+i+", note "+(k+1)+"\n\nRead the rotation code and the migration.\n\n- Kept the lock\n",at:null}));
  for(let i=0;i<15;i++) s.workers.push({id:"wd"+i,taskId:"c-done",executionId:"e-c-done",role:i===14?"code-reviewer":"backend-dev-"+i,status:i===3?"cancelled":"completed",lane:i===14?"review":"execution",goal:"Lock the rotation and add a version column",now:null,waitsFor:null,scope:[],route:null,attempt:null,elapsedMs:184000,runningSince:null,context:null,usage:null,activity:i===0?notes(i):[],result:{summary:"Finished part "+i+".",checks:[]}});
  const prose=(i:number)=>"### Step "+i+"\n\nChecked the refresh handler and the migration.\n\n- Kept the old error message\n- Added `version` to the token row\n";
  session.timeline=[...session.timeline,...Array.from({length:40},(_,i)=>({id:"long"+i,kind:"cortex" as const,text:prose(i)}))];
  let version=1;const listeners=new Set<(u:PortUpdate)=>void>();
  const emit=()=>listeners.forEach(f=>f({kind:"projection",scope:"workspace",version:String(++version),state:structuredClone(s)}));
  const port:WorkspacePort={mode:"real",query:(async(op:string)=>op==="capabilities.get"?{everything:true,operations:[]}:op==="project.snapshot"?[{kind:"projection",scope:"workspace",version:"1",state:structuredClone(s)}]:[]) as WorkspacePort["query"],subscribe:f=>{listeners.add(f);return()=>listeners.delete(f);},command:async()=>{throw Error("Read-only fixture");}};
  history.replaceState(null,"","/m/"+s.machines[0].id+"/t/c-done"+(view==="finished-worker"?"/workers/wd0":""));
  render(()=><Workspace port={port}/>,document.getElementById("root")!);
  let n=0;
  Object.assign(fixture,{
    // The same facts read again: equal data, new objects.
    same:()=>emit(),
    // A facts read whose result card changed: one more check, and a longer summary.
    facts:()=>{n++;task.result={...task.result!,checks:[...task.result!.checks,{name:"Worker check extra "+n,detail:"Reported later",outcome:"passed",source:"worker"}],summary:task.result!.summary+"\n\nAlso re-read round "+n+"."};emit();},
    // A forward tail read: no new rows, every row a new object.
    tail:()=>{session.timeline=session.timeline.map(e=>({...e}));emit();},
    // The team bar's text changes (a worker's line).
    team:()=>{const w=s.workers.find(x=>x.id==="wd14")!;w.result={summary:"No findings, round "+(++n)+".",checks:[]};emit();},
    stream:()=>{const last=session.timeline.at(-1)!;if(last.kind==="cortex")last.text+="\n\nMore streamed content "+(++n)+".";emit();},
  });
} else if(view==="sprint") {
  // A planning conversation long enough to scroll, over the planning composer.
  const s=pagesWorld("planning"), sprint=s.sprints[0];
  sprint.conversation=[...sprint.conversation,...Array.from({length:40},(_,i)=>({kind:"note" as const,text:"### Round "+i+"\n\nThe lockout depends on the rate limits.\n\n- Kept the three tasks\n- Added a check\n"}))];
  let version=1;const listeners=new Set<(u:PortUpdate)=>void>();
  const emit=()=>listeners.forEach(f=>f({kind:"projection",scope:"workspace",version:String(++version),state:structuredClone(s)}));
  const port:WorkspacePort={mode:"real",query:(async(op:string)=>op==="capabilities.get"?{everything:true,operations:[]}:op==="project.snapshot"?[{kind:"projection",scope:"workspace",version:"1",state:structuredClone(s)}]:[]) as WorkspacePort["query"],subscribe:f=>{listeners.add(f);return()=>listeners.delete(f);},command:async()=>{throw Error("Read-only fixture");}};
  history.replaceState(null,"","/m/"+s.machines[0].id+"/p/"+sprint.projectId+"/s/"+sprint.id+"/planning");
  render(()=><Workspace port={port}/>,document.getElementById("root")!);
  Object.assign(fixture,{emit,append:()=>{sprint.conversation.push({kind:"note",text:"### Later\n\nOne more note."});emit();}});
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
