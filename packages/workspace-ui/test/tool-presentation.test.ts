import { execFileSync } from "node:child_process";
import { existsSync } from "node:fs";
import { resolve } from "node:path";
import { expect, it } from "vitest";
import { presentToolSemantics } from "../../../integrations/paseo/shared/tool-presentation";
import { conversationOf } from "../src/port/history";
import { same } from "../src/stable";
import { displayStep, feedOf, oneRowPerCall, STEP_PREFIX, toolLabel, viewOfText } from "../src/tool-names";

it.each(["mcp__cortex__outside_write","mcp.cortex.project_mkdir","mcp__codebase_memory_mcp__get_code_snippet","mcp__cortex__read_report"])("uses the shared Paseo naming policy for %s",name=>{
  expect(toolLabel(name)).toBe(presentToolSemantics({name}).label);
  expect(viewOfText(name).label).not.toMatch(/mcp[_.]/);
});
it("filters the actual operation, not just a friendly title, before rows, totals and failed counts",()=>{
  const hidden = {sequence:1,kind:"tool",text:"mcp__cortex__wait_agent",at:1,error:true,args:JSON.stringify({title:"Wait",kind:"read"})};
  const codeName = {...hidden,sequence:2,text:"Tool activity",code:"mcp__cortex__usage_update"};
  const raw = STEP_PREFIX+JSON.stringify({title:"mcp__cortex__outside_write",path:"a.txt"});
  const run=feedOf([hidden,codeName,{sequence:3,kind:"tool",text:raw,at:2}]);
  expect(run).toHaveLength(1);
  expect("run" in run[0] && run[0].run.items).toHaveLength(1);
  expect(JSON.stringify(run.map(x=>"run" in x?x.run.text:""))).not.toMatch(/mcp__|attention/);
});
// A streamed update must cost and redraw only what it changed (test/scroll-perf.py, `-live`).
it("hands back last time's entries for rows and blocks an update left as they were",()=>{
  const note=(sequence:number,text:string)=>({sequence,kind:"note",text,at:sequence});
  const tool=(sequence:number)=>({sequence,kind:"tool",text:"mcp__cortex__project_read",at:sequence});
  const a=note(1,"## First\n\nwork"),b=tool(2),c=tool(3),d=note(4,"Second");
  const runIds=new Map<number,string>(),first=feedOf([a,b,c,d],runIds);
  const again=feedOf([a,b,c,d,note(5,"Third")],runIds,new Map(first.map(x=>[x.key,x])));
  expect(again.slice(0,3)).toEqual(first);
  again.slice(0,3).forEach((entry,i)=>expect(entry).toBe(first[i]));
  // A block that gained a step is a new entry under the same key.
  const grown=feedOf([a,b,c,tool(6),d],runIds,new Map(first.map(x=>[x.key,x])));
  expect(grown[1].key).toBe(first[1].key);
  expect(grown[1]).not.toBe(first[1]);
  expect(grown[0]).toBe(first[0]);
});
it("reads a step only from an object, as before, and keeps a call's row while its updates are the same",()=>{
  expect(displayStep("## Heading, not a step")).toBeNull();
  expect(displayStep("[1, 2]")).toBeNull();
  expect(displayStep(STEP_PREFIX+" {\"title\":\"Read\",\"path\":\"a.py\"}")).toEqual({title:"Read",path:"a.py"});
  const start={sequence:1,kind:"tool",text:"Read",at:1,toolCallRef:"tool:1"},done={sequence:2,kind:"tool",text:"Read done",at:2,toolCallRef:"tool:1"};
  const [row]=oneRowPerCall([start,done]);
  expect(row).toEqual({...done,sequence:1,at:1});
  expect(oneRowPerCall([start,done])[0]).toBe(row);
});
it("compares facts as data, stopping at the first difference",()=>{
  expect(same({a:[1,{b:"x"}],c:null},{a:[1,{b:"x"}],c:null})).toBe(true);
  expect(same({a:[1,{b:"x"}]},{a:[1,{b:"y"}]})).toBe(false);
  expect(same({a:1},{a:1,b:undefined})).toBe(false);
  expect(same(undefined,{a:1})).toBe(false);
});
it.each(["mcp__cortex__outside_write","mcp.cortex.project_git"])("sanitizes structured Conversation and Worker tool titles: %s",title=>{
  const step=STEP_PREFIX+JSON.stringify({title});
  expect(viewOfText(step).label).not.toMatch(/mcp[_.]/);
  const rows=conversationOf([{historyId:"1",revision:"1",kind:"tool",text:step,toolStatus:"completed"}],{description:"",answers:[]});
  expect(rows[0].entry.kind).toBe("fold");
  const entry=rows[0].entry;
  if(entry.kind==="fold") {
    expect(entry.text).not.toMatch(/mcp[_.]/);
    expect(entry.steps.map(s=>viewOfText(s).label).join(" ")).not.toMatch(/mcp[_.]/);
  }
});
it("keeps repeated structured calls readable rather than appending a count to serialized JSON",()=>{
  const step=STEP_PREFIX+JSON.stringify({title:"mcp__cortex__project_read",kind:"read",path:"a.py"});
  const result=conversationOf([1,2].map(n=>({historyId:String(n),revision:"1",kind:"tool",text:step})),{description:"",answers:[]})[0].entry;
  expect(result.kind==="fold"&&result.steps.map(s=>viewOfText(s).label)).toEqual(["Read","Read"]);
});
it("renders Claude and Codex native/gateway inputs through the real service selector and the shared Paseo semantics",()=>{
  const pkg=existsSync(resolve(process.cwd(),"src/layout.tsx"))?process.cwd():resolve(process.cwd(),"packages/workspace-ui");
  const root=resolve(pkg,"../..");
  const events=[
    {title:"printf 'ready'",tool_kind:"execute",rawInput:{command:"printf 'ready'"}},
    {title:"Read",tool_kind:"read",rawInput:{file_path:"src/a.py"}},
    {title:"mcp__cortex__command_run",tool_kind:"execute",rawInput:{input:{argv:["python3","-B","check.py"]}}},
    {title:"mcp.cortex.project_read",tool_kind:"other",rawInput:{server:"cortex",tool:"project_read",arguments:{path:"src/b.py"}}},
    {title:"mcp__cortex__read_report",tool_kind:"other",rawInput:{report_id:"r_0123456789ab"}},
  ];
  const code="import json,sys;sys.path.insert(0,'src');from cortex_runtime.workspace_api.history import tool_detail;print(json.dumps([tool_detail(e) for e in json.loads(sys.argv[1])]))";
  const details=JSON.parse(execFileSync("python3",["-B","-c",code,JSON.stringify(events)],{cwd:root,encoding:"utf8",timeout:10000}));
  const views=details.map((d:any)=>viewOfText(STEP_PREFIX+JSON.stringify(d)));
  expect(views.map((v:any)=>v.label)).toEqual(["Shell","Read","Shell","Read","Read report: r_0123456789ab"]);
  expect(views.map((v:any)=>v.target)).toEqual(["printf 'ready'","src/a.py","python3 -B check.py","src/b.py",""]);
});
