import {render} from "solid-js/web";
import {emptyWorkspace, emptySession} from "@cortex/workspace-domain";
import {Workspace} from "../src";
import {withTask} from "./support";
import "../src/mockup-styles.css";
import "../src/workspace.css";

// Disposable, opt-in WebKit stress data. No service, engines or real stores.
const s=withTask(emptyWorkspace(),"Inspect source-backed project documentation");
s.machines[0].connection="connected";
Object.assign(s.tasks[0],{status:"working",process:"live",executionId:"e"});
const paragraph="Compared the documented behavior with the current source and checked every affected control. No remote systems were changed.";
const section=(i:number)=>`## Finding ${i}: published verification\n\n${paragraph}\n\n| Check | Outcome |\n| --- | --- |\n| Local links | **Passed** |\n| Source | Inspected |\n\n1. Read the exact source\n2. Re-run the focused check\n\n\`\`\`sh\npython3 -B check.py --part ${i}\nprintf 'literal\\.text'\n\`\`\`\n`;
const report="Cortex draft ID: `d_fixture`\n\n# Verdict: REQUEST_CHANGES\n\n"+Array.from({length:1800},(_,i)=>section(i+1)).join("\n")+"\nFinal report paragraph.";
const activity=Array.from({length:2400},(_,i)=>({sequence:i+1,kind:"note" as const,text:`### Observation ${i+1}\n\n${paragraph}\n\n- Preserved source\n- Verified locally\n\n\`\`\`sh\npython3 -B check.py --part ${i+1}\n\`\`\``,at:null}));
s.workers=[{id:"w",taskId:"t",executionId:"e",role:"frontend-dev",status:"completed",lane:"execution",goal:"Review the project documentation",now:null,waitsFor:null,scope:[],route:null,attempt:null,elapsedMs:null,runningSince:null,context:null,usage:null,activity,result:{summary:"Review complete",checks:[],reportId:"r_fixture"}}];
const session=emptySession("t");
session.timeline=activity.map(a=>({id:"entry"+a.sequence,kind:"cortex" as const,text:a.text}));
s.sessions=[session];
let listeners=new Set<any>(),version=0;
const emit=(update=false)=>{
 if(update) {
  activity.at(-1)!.text += "\n\nLive update.";
  const last = session.timeline.at(-1);
  if(last?.kind === "cortex") last.text = activity.at(-1)!.text;
 }
 listeners.forEach(f=>f({kind:"projection",scope:"workspace",version:String(++version),state:structuredClone(s)}));
};
const reads:number[]=[];
const port:any={mode:"real",readReport:async(_t:string,reportId:string,cursor?:string)=>{
 const at=Number(cursor??0);reads.push(at); const end=at+4000,next=end<report.length?String(end):null;
 return{reportId,title:"Verdict: REQUEST_CHANGES",sha256:"a".repeat(64),markdown:report.slice(at,end),totalCharacters:report.length,nextCursor:next,partial:!!next};
},query:async(op:string)=>({"capabilities.get":{everything:true,operations:[]},"project.snapshot":[],"project.facts":[],"machine.facts":[]}[op]),
subscribe:(f:any)=>{listeners.add(f);setTimeout(emit,0);return()=>listeners.delete(f)},command:async()=>{throw Error("This is a read-only fixture")}};
history.replaceState(null,"",new URLSearchParams(location.search).get("view")==="conversation"?"/m/local/t/t":"/m/local/t/t/workers/w");
render(()=><Workspace port={port}/>,document.getElementById("root")!);
(window as any).fixture={emit,reads,characters:report.length};
