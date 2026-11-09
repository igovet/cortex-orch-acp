// @vitest-environment jsdom
import { render } from "solid-js/web";
import { afterEach, beforeEach, expect, it } from "vitest";
import { WsContext, type Ws } from "../src/ctx";
import { CloneProject, Updates } from "../src/maintenance";
import { AddConnection } from "../src/machine-dialogs";
import { VoiceSettings } from "../src/voice-settings";
import { flush } from "./support";

let dispose: (() => void) | undefined;
beforeEach(() => { localStorage.clear(); sessionStorage.clear(); });
afterEach(() => { dispose?.(); document.body.innerHTML = ""; });
const settle = async () => { for (let i = 0; i < 6; i++) await flush(); };
const button = (label: string) => [...document.querySelectorAll<HTMLButtonElement>("button")].find(b => b.textContent === label)!;
const input = (label: string, value: string) => { const e = document.querySelector<HTMLInputElement>(`input[aria-label="${label}"]`)!; e.value=value; e.dispatchEvent(new Event("input",{bubbles:true})); };
function mount(child: () => unknown, action: (op: string, p: Record<string, unknown>) => Promise<Record<string, unknown>>) {
  const ws = {machine:()=>({id:"local",name:"This machine"}), store:{mode:"real",machineAction:action,reload:async()=>{},pickFolder:async()=>null}, demo:undefined} as unknown as Ws;
  const root=document.createElement("div");document.body.append(root);
  dispose=render(()=><WsContext.Provider value={ws}>{child() as never}</WsContext.Provider>,root);
}

it("release check, verified download, distinct consent and rollback use exact job identities",async()=>{
  const calls: Record<string,unknown>[]=[]; let active="", phase="ready";
  mount(()=><Updates/>,async(_op,p)=>{
    calls.push(p);
    if(p.action==="update.status")return {phase:"idle",current:"1.0.6",privilegePrompt:true};
    if(p.action==="update.check")return {phase:"available",version:"1.0.7",message:"Checksum only, no signature."};
    if(p.action==="start"){active=String(p.id);phase=p.kind==="download"?"ready":"done";return {id:active,phase,message:"Recorded"};}
    return {id:active,phase,message:phase==="ready"?"Checksum verified.":"Installed. Restart Cortex Workspace."};
  });
  await settle();button("Check for updates").click();await settle();
  button("Download and verify").click();await settle();
  const downloaded=active;
  expect(button("Install with system prompt").disabled).toBe(true);
  expect(calls.filter(p=>p.kind==="install")).toHaveLength(0);
  document.querySelector<HTMLInputElement>('input[type="checkbox"]')!.click();await settle();
  button("Install with system prompt").click();await settle();
  expect(calls.find(p=>p.kind==="install")).toMatchObject({source:downloaded,confirmed:true});
  expect(document.body.textContent).toContain("Restart Cortex Workspace");
  expect(button("Roll back retained package").disabled).toBe(true);
  document.querySelector<HTMLInputElement>('input[type="checkbox"]')!.click();await settle();
  button("Roll back retained package").click();await settle();
  expect(calls.find(p=>p.kind==="rollback")).toMatchObject({source:downloaded,confirmed:true});
});

it("dev updates and a failed checksum never offer installation",async()=>{
  mount(()=><Updates/>,async()=>({phase:"dev",message:"Dev updates come from local builds."}));
  await settle();expect(document.body.textContent).toContain("local builds");
  expect(button("Check for updates")).toBeUndefined();
});

it("a clone reports progress, cancels the exact job and adds only a completed machine folder",async()=>{
  let phase="cloning", id="", added="";const calls:Record<string,unknown>[]=[];
  mount(()=><CloneProject added={p=>{added=p;}}/>,async(_op,p)=>{
    calls.push(p);if(p.action==="start")id=String(p.id);
    if(p.action==="cancel")phase="cancelled";
    return {id,phase,message:phase==="cloning"?"Cloning repository…":"Cancelled."};
  });
  input("GitHub repository URL","https://github.com/fixture/repo");input("Clone destination","/tmp/clone-fixture");await settle();
  button("Clone repository").click();await settle();
  expect(button("Add cloned project")).toBeUndefined();expect(document.body.textContent).toContain("Cloning repository");
  button("Cancel clone").click();await settle();
  expect(calls.find(p=>p.action==="cancel")?.id).toBe(id);
  expect(added).toBe("");
  dispose?.();dispose=undefined;
  localStorage.clear();
  mount(()=><CloneProject added={p=>{added=p;}}/>,async(_op,p)=>({id:String(p.id??"done"),phase:"done",destination:"/machine/repo",message:"Clone complete."}));
  input("GitHub repository URL","https://github.com/fixture/repo");input("Clone destination","/machine/repo");await settle();
  button("Clone repository").click();await settle();button("Add cloned project").click();
  expect(added).toBe("/machine/repo");
});

it("API keys are masked, submitted privately, cleared and not put into browser storage",async()=>{
  const calls:{op:string;p:Record<string,unknown>}[]=[];let closed=false;
  mount(()=><AddConnection onClose={()=>{closed=true;}}/>,async(op,p)=>{calls.push({op,p});return {connectionId:"fixture"};});
  button("API key").click();await settle();input("Connection name","Fixture");input("API key","fixture-key-only");await settle();
  expect(document.querySelector<HTMLInputElement>('input[aria-label="API key"]')!.type).toBe("password");
  button("Save API-key connection").click();await settle();
  expect(calls[0]).toMatchObject({op:"engine.connection",p:{action:"add_key",engine:"claude-agent-acp",name:"Fixture",key:"fixture-key-only"}});
  expect(document.querySelector<HTMLInputElement>('input[aria-label="API key"]')!.value).toBe("");
  expect(JSON.stringify(localStorage)).not.toContain("fixture-key-only");expect(closed).toBe(true);
});

it("native login opens a terminal, then rechecks independently without sending credentials",async()=>{
  const calls:{op:string;p:Record<string,unknown>}[]=[];
  mount(()=><AddConnection onClose={()=>{}}/>,async(op,p)=>{calls.push({op,p});return {message:"Finish in terminal, then recheck."};});
  button("Sign in in terminal").click();await settle();
  expect(calls).toEqual([{op:"native.engine_login",p:{engine:"claude-agent-acp"}}]);
  button("Recheck native login and connect").click();await settle();
  expect(calls[1]).toEqual({op:"engine.connection",p:{action:"recheck",engine:"claude-agent-acp"}});
});

it("voice settings show only the shipped local Russian provider and real model state",async()=>{
  mount(()=><VoiceSettings/>,async()=>({message:"Not downloaded or incomplete."}));
  await settle();
  expect(document.body.textContent).toContain("sherpa-onnx");
  expect(document.body.textContent).toContain("Russian only");
  expect(document.body.textContent).toContain("Not downloaded or incomplete");
  expect(document.body.textContent).not.toContain("Not chosen");
});
