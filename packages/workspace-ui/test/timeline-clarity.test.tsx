import { createSignal } from "solid-js";
import { render } from "solid-js/web";
import { afterEach, expect, it } from "vitest";
import { emptyWorkspace, emptySession, LOCAL_MACHINE_ID, type Worker } from "@cortex/workspace-domain";
import { Workspace } from "../src";
import { ResultText, resultPreview } from "../src/result-text";
import { WorkingIndicator } from "../src/worker";
import { flush, projection, scriptedPort, withTask } from "./support";
let dispose: (() => void) | undefined;
afterEach(() => { dispose?.(); document.body.innerHTML = ""; localStorage.clear(); });
const settle = async () => { for (let i=0;i<8;i++) await flush(); };
async function mount(path = `/m/${LOCAL_MACHINE_ID}/p/p/board`, state = withTask(emptyWorkspace(), "Fixture task"), setup?: (p: ReturnType<typeof scriptedPort>) => void) {
  state.machines[0].connection = "connected";
  const p = scriptedPort({capabilities:{everything:true,operations:[]}});
  setup?.(p);
  history.replaceState(null, "", path);
  const root = document.createElement("div"); document.body.append(root);
  dispose = render(() => <Workspace port={p.port} />,root);
  await settle(); p.emit(projection("1",state)); await settle();
  return p;
}
it("starts a board task once, keeps its button busy and explains a refusal on the card",async () => {
  const state = withTask(emptyWorkspace(), "Fixture task");
  const p = await mount(undefined, state);
  const start = [...document.querySelectorAll<HTMLButtonElement>(".card button")].find(b=>b.textContent==="Start")!;
  expect(start).toBeTruthy();
  start.click(); start.click();
  expect(p.commands.filter(c=>c.operation==="start_task")).toHaveLength(1);
  expect(start.getAttribute("aria-busy")).toBe("true");
  state.tasks[0].title = "Runtime-named task";
  p.emit(projection("2", structuredClone(state))); await settle();
  expect(document.querySelector(".card .btn-primary")).toBe(start);
  expect(start.disabled).toBe(true);
  p.receipt(false); await settle();
  expect(document.querySelector(".card [role=alert]")?.textContent).toContain("Refused");
  expect(start.disabled).toBe(false);
  start.click();
  expect(p.commands.filter(c=>c.operation==="start_task")).toHaveLength(2);
  p.receipt(); await settle();
});
it("opens worker rows directly in the full stream and hides result bookkeeping", async () => {
  const state=withTask(emptyWorkspace(),"Fixture task");
  state.tasks[0].status="working"; state.tasks[0].process="live";
  state.workers=[{id:"w",taskId:"t",executionId:"e",role:"frontend-dev",status:"working",lane:"execution",goal:"Inspect UI",now:null,waitsFor:null,scope:[],route:null,attempt:null,elapsedMs:null,runningSince:null,context:null,usage:null,result:null,
  activity:[
    {sequence:1,kind:"tool",text:"mcp__cortex__project_read",at:null,args:JSON.stringify({kind:"read",title:"Read",path:"index.html"})},
    ...[2,3,4,5].map(sequence=>({sequence,kind:"tool" as const,text:"mcp__cortex__result_submit",at:null}))
  ]} satisfies Worker];
  const session=emptySession("t");
  session.timeline=[{id:"workers",kind:"workers",title:"Workers",workerIds:["w"]}];
  state.sessions=[session];
  const port = await mount(`/m/${LOCAL_MACHINE_ID}/t/t`,state);
  expect(document.querySelector(".ws-working")).not.toBeNull();
  (document.querySelector(".wrow") as HTMLButtonElement).click(); await settle();
  expect(document.querySelector(".livepane .stream")).not.toBeNull();
  expect(document.querySelector(".detail")).toBeNull();
  expect(document.body.textContent).not.toMatch(/Full view|Submit result|mcp__cortex__/);
  const fold=document.querySelector<HTMLButtonElement>(".livepane button.fold")!;
  expect(fold.getAttribute("aria-expanded")).toBe("false");
  expect(fold.textContent).toContain("Read 1 file");
  fold.click(); await settle();
  (document.querySelector(".livepane .step-row") as HTMLButtonElement).click(); await settle();
  expect(document.querySelector(".ws-step-path")?.textContent).toBe("index.html");
  (document.querySelector(".ws-step-path") as HTMLButtonElement).click(); await settle();
  expect(document.querySelector(".ws-step-detail [role=alert]")?.textContent).toContain("Checkout is unavailable");
  state.repos=[{projectId:"p",remote:"",tree:["index.html"],sources:{},commits:[],branches:[],fetchedAt:null,checkouts:[{id:"main",projectId:"p",kind:"main",path:"/fixture",branch:"main",files:[],sources:{},taskId:null,label:null,base:"main",ahead:0,behind:0,behindOf:{},draft:"",updateConflict:null}]}];
  port.port.readFile=async()=>({text:"<h1>Local coffee</h1>",revision:"fixture"});
  port.emit(projection("2", structuredClone(state))); await settle();
  (document.querySelector(".ws-step-path") as HTMLButtonElement).click(); await settle();
  expect(location.search).toContain("file=index.html");
  expect(document.querySelector(".livepane")).toBeNull();
  expect(document.querySelector('[role=tab][aria-label="index.html"]')).not.toBeNull();
});
it("keeps a calm live indicator mounted across factual activity updates",()=>{
  const [activity,setActivity]=createSignal([{sequence:1,kind:"tool" as const,text:"Read",at:null,args:JSON.stringify({kind:"read",status:"in_progress"})}]);
  const root=document.createElement("div");document.body.append(root);
  dispose=render(()=><WorkingIndicator activity={activity()}/>,root);
  const indicator=root.querySelector(".ws-working");
  expect(indicator?.textContent).toBe("Reading…");
  setActivity([{sequence:2,kind:"tool",text:"Edit",at:null,args:JSON.stringify({kind:"edit",status:"in_progress"})}]);
  expect(root.querySelector(".ws-working")).toBe(indicator);
  expect(indicator?.textContent).toBe("Editing…");
  setActivity([]);
  expect(indicator?.textContent).toBe("Working…");
});
it("uses an explicit drag image with the card's dimensions",async()=>{
  await mount();
  const card=document.querySelector<HTMLElement>(".card")!;
  let ghost:HTMLElement|undefined;
  const event=new Event("dragstart",{bubbles:true});
  Object.defineProperty(event,"dataTransfer",{value:{setData(){},setDragImage(el:HTMLElement){ghost=el;}}});
  card.dispatchEvent(event);
  expect(ghost?.style.width).toBe(`${card.getBoundingClientRect().width}px`);
  expect(ghost?.style.height).toBe(`${card.getBoundingClientRect().height}px`);
  expect(ghost?.getAttribute("aria-hidden")).toBe("true");
});
it("always uses normal conversation detail, with no verbosity selector",async()=>{
  localStorage.setItem("cw.ws.conversationDetail",JSON.stringify("detailed"));
  await mount(`/m/${LOCAL_MACHINE_ID}/t/t`);
  expect(document.querySelector('[popovertarget="m-view"]')).toBeNull();
});
it("keeps finished worker previews and note bodies readable", async () => {
  const state = withTask(emptyWorkspace(), "Finished task");
  state.tasks[0].status = "review";
  state.workers = [{ id:"w", taskId:"t", executionId:"e", role:"frontend-dev", status:"completed", lane:"execution", goal:"Inspect UI", now:null, waitsFor:null, scope:[], route:null, attempt:null, elapsedMs:null, runningSince:null, context:null, usage:null, result:null,
    activity:[{ sequence:1,kind:"note",text:"[final response] Readable" + String.fromCharCode(92) + "_note.",at:null }] } satisfies Worker];
  Object.assign(state.workers[0], { result:{summary:"Readable" + String.fromCharCode(92) + "_summary.",checks:[],reportId:null} });
  await mount(`/m/${LOCAL_MACHINE_ID}/t/t/workers/w`,state);
  expect(document.querySelector(".wcard[aria-selected=true] .l2")?.textContent).toBe("Readable_summary.");
  expect(document.querySelector(".ev-note")?.textContent).toBe("Readable_note.");
  const slash = String.fromCharCode(92), tick = String.fromCharCode(96);
  const code = tick + "C:" + slash + "work" + slash + "_file" + tick;
  expect(resultPreview("File" + slash + "_name and " + code)).toBe("File_name and " + code);
});

it("keeps history rows and the reader's position through a live tail update", async () => {
  const state=withTask(emptyWorkspace(),"Long history");
  state.tasks[0].status="working"; state.tasks[0].process="live";
  state.workers=[{id:"w",taskId:"t",executionId:"e",role:"frontend-dev",status:"working",lane:"execution",goal:"Inspect UI",now:null,waitsFor:null,scope:[],route:null,attempt:null,elapsedMs:null,runningSince:null,context:null,usage:null,result:null,
    activity:Array.from({length:220},(_,i)=>({sequence:i+1,kind:"note" as const,text:"Observation "+i,at:null}))} satisfies Worker];
  const port=await mount(`/m/${LOCAL_MACHINE_ID}/t/t/workers/w`,structuredClone(state));
  const rows=[...document.querySelectorAll(".ev-note")];
  const scroller=document.querySelector<HTMLElement>(".stream")!;
  scroller.scrollTop=-100;
  scroller.dispatchEvent(new Event("scroll"));
  state.workers[0].activity.at(-1)!.text="Live tail changed";
  port.emit(projection("2",structuredClone(state))); await settle();
  expect(document.querySelectorAll(".ev-note")).toHaveLength(220);
  expect(document.querySelectorAll(".ev-note")[0]).toBe(rows[0]);
  expect(document.querySelectorAll(".ev-note")[219]).toBe(rows[219]);
  expect(rows[0].textContent).toBe("Observation 0");
  expect(rows[219].textContent).toBe("Live tail changed");
  expect(scroller.scrollTop).toBe(-100);
});

const feedWorker = (patch: Partial<Worker> = {}): Worker => ({id:"w",taskId:"t",executionId:"e",role:"frontend-dev",status:"completed",lane:"execution",goal:"Build the page",now:null,waitsFor:null,scope:[],route:{model:"opus-5.5",effort:"high"},attempt:null,elapsedMs:null,runningSince:null,context:null,usage:null,
  activity:[
    {sequence:1,kind:"note",text:"Looking at the page.",at:null},
    {sequence:2,kind:"tool",text:"mcp__cortex__project_read",at:null,args:JSON.stringify({kind:"read",title:"Read",path:"src/a.py"})},
    {sequence:3,kind:"command",text:"npm test",at:null,args:JSON.stringify({kind:"execute",title:"Bash",command:"npm test",output:"ok"}),command:{state:"native",exit:null,took:null,tail:"ok",reason:null}},
    {sequence:4,kind:"edit",text:"mcp__cortex__project_edit",at:null,args:JSON.stringify({kind:"edit",title:"Edit",path:"src/b.py",oldText:"a",newText:"b"})},
    {sequence:5,kind:"note",text:"That is done.",at:null},
  ],
  result:{summary:"All done.",checks:[],reportId:"r_1"}, ...patch} as Worker);

it("reads a worker's feed like Conversation: messages, a collapsed block, then the result last",async()=>{
  const state=withTask(emptyWorkspace(),"Feed task");
  state.tasks[0].status="review"; state.workers=[feedWorker()];
  await mount(`/m/${LOCAL_MACHINE_ID}/t/t/workers/w`,state);
  const stream=document.querySelector(".stream-in")!;
  expect([...stream.children].at(-1)!.classList.contains("ws-worker-result")).toBe(true);
  expect(stream.querySelector("details")).toBeNull();
  expect(document.querySelector('[aria-label="Filter activity"]')).toBeNull();
  expect([...stream.querySelectorAll(".ev-note")].map(n=>n.textContent)).toEqual(["Looking at the page.","That is done."]);
  const fold=stream.querySelector<HTMLButtonElement>("button.fold")!;
  expect(fold.getAttribute("aria-expanded")).toBe("false");
  expect(fold.textContent).toBe("Edited 1 file, ran 1 command, and read 1 file");
  expect(stream.textContent).not.toContain("npm test");
  fold.click(); await settle();
  const row=(label:string)=>[...stream.querySelectorAll<HTMLElement>(".step-row")].find(r=>r.querySelector(".step-l")?.textContent===label)!;
  expect(row("Read").querySelector(".step-t")?.textContent).toBe("src/a.py");
  expect(row("Shell").querySelector(".step-t")?.textContent).toBe("npm test");
  expect(row("Read").querySelector(".i-eye")).not.toBeNull();
  expect(row("Shell").querySelector(".i-terminal")).not.toBeNull();
  expect(row("Edit").querySelector(".i-pencil")).not.toBeNull();
  row("Shell").click(); await settle();
  expect(stream.querySelector(".ws-step-detail .args")?.textContent).toBe("ok");
});

it("retains an older page when the same worker refreshes while it is loading",async()=>{
  const state=withTask(emptyWorkspace(),"Paged worker");
  const older={sequence:0,kind:"note" as const,text:"Earlier observation",at:null};
  state.workers=[feedWorker({activityCursor:"older",activity:[{sequence:1,kind:"note",text:"Latest observation",at:null}]})];
  let resolve!: (page:{activity:Worker["activity"];activityCursor:null})=>void;
  const port=await mount(`/m/${LOCAL_MACHINE_ID}/t/t/workers/w`,structuredClone(state),p=>{
    p.port.readWorkerActivity=()=>new Promise(r=>{resolve=r;});
  });
  const olderButton=()=>[...document.querySelectorAll<HTMLButtonElement>(".stream button")].find(b=>b.textContent==="Load older activity");
  olderButton()!.click(); await settle();
  state.workers[0].now="A refreshed fact";
  port.emit(projection("2",structuredClone(state))); await settle();
  resolve({activity:[older],activityCursor:null}); await settle();
  expect(document.querySelector(".stream")?.textContent).toContain("Earlier observation");
  expect(document.querySelector(".stream")?.textContent).toContain("Latest observation");
  expect(olderButton()).toBeUndefined();
});

it("opens the worker result on a card with its report, without a disclosure triangle",async()=>{
  const state=withTask(emptyWorkspace(),"Feed task");
  state.tasks[0].status="review"; state.workers=[feedWorker({status:"failed",result:{summary:"Could not finish.",checks:[],reportId:"r_1"} as Worker["result"]})];
  await mount(`/m/${LOCAL_MACHINE_ID}/t/t/workers/w`,state);
  const card=document.querySelector(".ws-worker-result")!;
  expect(card.tagName).toBe("DIV");
  expect(card.querySelector(".rcard-h")?.textContent).toContain("Worker result");
  expect(card.querySelector(".rcard-h .btn")?.textContent).toBe("Open report");
  expect(card.querySelector(".i-x-circle")).not.toBeNull();
});

it("shows the recorded model and effort wherever an agent is named, and omits them when none is recorded",async()=>{
  const state=withTask(emptyWorkspace(),"Routes");
  state.tasks[0].status="working"; state.tasks[0].process="live"; state.tasks[0].coordinatorRoute={model:"gpt-6-luna",effort:"medium"};
  state.workers=[feedWorker({status:"working",result:null}),feedWorker({id:"w2",role:"reviewer",status:"working",route:null,result:null})];
  const session=emptySession("t");
  session.timeline=[{id:"workers",kind:"workers",title:"Workers",workerIds:["w","w2"]}];
  state.sessions=[session];
  await mount(`/m/${LOCAL_MACHINE_ID}/t/t/workers/w`,state,p=>{p.port.readWorkerActivity=async()=>({activity:[],activityCursor:null,messages:[]});});
  expect(document.querySelector(".d-grid")?.textContent).toMatch(/Model.*opus.5\.5.*high/i);
  const worker=[...document.querySelectorAll<HTMLElement>(".wcard")].find(c=>c.textContent?.includes("frontend-dev"))!;
  expect(worker.querySelector(".l3")?.textContent).toMatch(/opus.5\.5/i);
  expect([...document.querySelectorAll<HTMLElement>(".wcard")].find(c=>c.textContent?.includes("reviewer"))!.querySelector(".l3")?.textContent).not.toMatch(/opus/i);
  expect(document.querySelector('.wcard[href$="/workers/cortex"] .l3')?.textContent ?? "").toMatch(/gpt-6-luna|luna/i);
  dispose?.(); document.body.innerHTML="";
  await mount(`/m/${LOCAL_MACHINE_ID}/t/t`,state);
  expect(document.querySelector(".wrow .route")?.textContent).toMatch(/opus.5\.5/i);
});

it("shows the coordinator's recorded feed instead of an empty note",async()=>{
  const state=withTask(emptyWorkspace(),"Coordinator");
  state.tasks[0].status="working"; state.tasks[0].process="live"; state.tasks[0].coordinatorRoute={model:"gpt-6-luna",effort:"high"};
  let page={activity:[{sequence:1,kind:"note",text:"Planning the work.",at:null},{sequence:2,kind:"tool",text:"mcp__cortex__plan_submit",at:null}],activityCursor:null,messages:[]} as Awaited<ReturnType<NonNullable<ReturnType<typeof scriptedPort>["port"]["readWorkerActivity"]>>>;
  const ids:string[]=[];
  const p=await mount(`/m/${LOCAL_MACHINE_ID}/t/t/workers/cortex`,structuredClone(state),x=>{x.port.readWorkerActivity=async(taskId,id)=>{ids.push(`${taskId}:${id}`);return page;};});
  await settle();
  expect(ids).toContain("t:coordinator:t");
  const pane=document.querySelector(".livepane")!;
  expect(pane.getAttribute("aria-label")).toBe("Coordinator activity");
  expect(pane.querySelector('[aria-label="Find in this activity"]')).not.toBeNull();
  expect(pane.textContent).toContain("Planning the work.");
  expect(pane.textContent).not.toContain("Nothing to report");
  expect(pane.querySelector("button.fold")?.textContent).toContain("Used 1 other tool");
  expect(pane.querySelector(".composer")).toBeNull();
  page={...page,activity:[...page.activity!,{sequence:3,kind:"note",text:"Second step.",at:null}]};
  state.tasks[0].title="Coordinator renamed";
  p.emit(projection("2",structuredClone(state))); await settle();
  expect(document.querySelector(".livepane")?.textContent).toContain("Second step.");
});

it("never shows a native hover title on a screen",async()=>{
  const state=withTask(emptyWorkspace(),"Titles");
  state.tasks[0].status="working"; state.tasks[0].process="live"; state.workers=[feedWorker({status:"working",result:null})];
  const session=emptySession("t"); session.timeline=[{id:"workers",kind:"workers",title:"Workers",workerIds:["w"]}]; state.sessions=[session];
  for (const path of [`/m/${LOCAL_MACHINE_ID}/p/p/board`,`/m/${LOCAL_MACHINE_ID}/t/t`,`/m/${LOCAL_MACHINE_ID}/t/t/workers/w`,"/settings/app/appearance"]) {
    await mount(path,structuredClone(state));
    expect([...document.querySelectorAll("[title]")].map(e=>e.outerHTML.slice(0,120))).toEqual([]);
    dispose?.(); document.body.innerHTML="";
  }
});

it("unescapes stored punctuation in prose but preserves inline and fenced code",()=>{
  const root = document.createElement("div"); document.body.append(root);
  const text = String.raw`Open index\.html at file\:\/\/\/tmp; **done\.** Use ` + "`" + String.raw`C:\work\file` + "`.\n\n```sh\n" + String.raw`printf '\\n'` + "\n```";
  dispose=render(()=><ResultText text={text} />,root);
  expect(root.textContent).toContain("index.html at file:///tmp");
  expect(root.querySelector("strong")?.textContent).toBe("done.");
  expect(root.querySelector("code")?.textContent).toBe(String.raw`C:\work\file`);
  expect(root.querySelector("pre")?.textContent).toBe(String.raw`printf '\\n'`);
});
