// @vitest-environment jsdom
import { mkdtempSync, mkdirSync, writeFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { resolve } from "node:path";
import { spawnSync } from "node:child_process";
import { render } from "solid-js/web";
import { afterEach, expect, it } from "vitest";
import { Workspace } from "../src";
import { createRealPort } from "../src/port/real";
import { capabilitiesWire, scriptedTransport, snapshot, flush } from "./support";
import type { UserDefaults, WorkspaceCommand } from "@cortex/workspace-domain";

const ROOT = resolve(import.meta.dirname, "../../..");
let dispose: (() => void) | undefined;
const folders: string[] = [];
afterEach(() => { dispose?.(); dispose = undefined; document.body.innerHTML = ""; for (const dir of folders.splice(0)) rmSync(dir, { recursive:true, force:true }); });
const settle = async () => { for (let i=0;i<10;i++) await flush(); };
const all = (selector: string) => [...document.querySelectorAll<HTMLElement>(selector)];
const button = (text: string) => all("button").find(b => b.textContent?.trim().startsWith(text))!;
const input = (label: string, text: string) => { const el = document.querySelector<HTMLTextAreaElement>(`[aria-label="${label}"]`)!; el.value=text; el.dispatchEvent(new Event("input",{bubbles:true})); };

/** Actual helper/configuration/Git, but no account, network, engine or native shell bridge. */
function machine() {
  const home=mkdtempSync(resolve(tmpdir(),"cw-first-use-")); folders.push(home);
  const repo=resolve(home,"project"), state=resolve(home,"state");
  mkdirSync(repo);
  const env={PATH:process.env.PATH!,HOME:home,XDG_CONFIG_HOME:resolve(home,"config"),XDG_DATA_HOME:resolve(state,"data"),XDG_STATE_HOME:resolve(home,"xdg-state")};
  function git(...args: string[]) {
    const r=spawnSync("git",args,{cwd:repo,env,encoding:"utf8"}); expect(r.status,r.stderr).toBe(0); return r.stdout.trim();
  }
  git("init","-q","-b","main"); git("config","user.name","Fixture"); git("config","user.email","fixture@example.invalid");
  writeFileSync(resolve(repo,"a.txt"),"one\ntwo\n"); git("add","."); git("commit","-qm","Initial");
  function helper(op: string, params: unknown = {}) {
    const r=spawnSync("python3",["-I","-B",resolve(ROOT,"crates/workspace-service/machine_tools.py")],{
      env,encoding:"utf8",timeout:10000,input:JSON.stringify({op,params,projectId:"p",root:repo,stateDir:state,projects:[{id:"p",root:repo,name:"Fixture"}]})
    });
    expect(r.status, r.stderr).toBe(0);
    return JSON.parse(r.stdout);
  }
  let active=false;
  const s=scriptedTransport({
    "service.capabilities":()=>({methods:["machine.call","projects.activate","projects.status"],machine:true}),
    "projects.list":()=>[{projectId:"p",displayName:"Fixture",canonicalRoot:repo,activation:active?{}:null}],
    "projects.activate":()=>{ expect(helper("config.action",{action:"apply_default_preset"}).ok).toBe(true); active=true; return {ready:true}; },
    "projects.status":()=>({ready:active}),
    "capabilities.get":()=>capabilitiesWire(["capabilities.get","project.snapshot","tasks.create","tasks.start","decisions.list"]),
    "project.snapshot":()=>snapshot("1",[]),
    "machine.call":message=>{ const {op,params}=message as {op:string;params:unknown}; return op==="project.config.read" ? {ok:true,result:{value:{revisions:{}},tokens:{}}} : helper(op,params); }
  });
  const p=createRealPort({transport:s.transport,actorId:"owner",registry:true});
  let n=0;
  const command=(payload:WorkspaceCommand)=>p.command({requestId:"first-"+(++n),operation:payload.type,expectedVersion:null,payload});
  const facts=async()=> (await p.query("machine.facts",{}))[0];
  const ready=async()=>{await p.query("capabilities.get",{}); await p.query("project.snapshot",{}); await p.query("project.facts",{}); await facts();};
  return {home,repo,state,env,git,helper,s,p,command,facts,ready};
}

it("empty HOME activation refreshes first saves of defaults, limits, profiles and MCP against real files",async()=>{
  const m=machine(); await m.ready();
  expect((await m.facts()).patch.config!.revisions.user_defaults).toBe(0);
  expect((await m.command({type:"activate_project",projectId:"p"})).ok).toBe(true);
  await m.p.query("capabilities.get",{});
  let c=(await m.facts()).patch.config!;
  let value: UserDefaults={...c.surfaces.user_defaults,language:"en"};
  expect(await m.command({type:"save_surface",machineId:"local",surface:"user_defaults",value,revision:c.revisions.user_defaults})).toMatchObject({ok:true});
  c=(await m.facts()).patch.config!;
  expect(c.surfaces.user_defaults.language).toBe("en");
  value={...c.surfaces.user_defaults,ceiling:{...c.surfaces.user_defaults.ceiling,attempts:3}};
  expect((await m.command({type:"save_surface",machineId:"local",surface:"user_defaults",value,revision:c.revisions.user_defaults})).ok).toBe(true);
  expect((await m.facts()).patch.config!.surfaces.user_defaults.ceiling.attempts).toBe(3);
  for (const command of [
    {type:"create_profile",machineId:"local",name:"first",description:"Fixture"},
    {type:"rename_profile",machineId:"local",profileId:"first",name:"renamed"},
    {type:"activate_profile",machineId:"local",profileId:"renamed",keep:"discard"},
    {type:"deactivate_profile",machineId:"local"},
    {type:"delete_profile",machineId:"local",profileId:"renamed"}
  ] as WorkspaceCommand[]) { const r=await m.command(command); expect(r.ok,JSON.stringify(r)).toBe(true); await m.facts(); }
  expect((await m.facts()).patch.config!.profiles.some(p=>p.name==="renamed")).toBe(false);
  c=(await m.facts()).patch.config!;
  const server={name:"fixture",enabled:false,command:"/usr/bin/python3",args:["-B"],env:[],network:false,readPaths:[],writePaths:[],runtimeDirEnv:[],allowedTools:["read"],readOnlyTools:["read"],coordinatorTools:[],alwaysLoadTools:null,roles:["worker"],engines:null,handTools:[],unsupportedKeys:[],problem:null};
  for (const servers of [[server],[{...server,args:["-I","-B"]}],[]]) {
    expect(await m.command({type:"save_surface",machineId:"local",surface:"mcp_servers",value:servers,revision:c.revisions.mcp_servers})).toMatchObject({ok:true});
    c=(await m.facts()).patch.config!;
    expect(c.surfaces.mcp_servers.map(s=>s.args)).toEqual(servers.map(s=>s.args));
  }
},20000);

it("rereads automatic first-run configuration changes without another activation",async()=>{
  const m=machine(); await m.ready();
  expect(m.helper("config.action",{action:"apply_default_preset"}).ok).toBe(true);
  expect((await m.facts()).patch.config!.revisions.user_defaults).not.toBe(0);
});

it("does not overwrite a dirty draft's base when a background config read sees another writer",async()=>{
  const m=machine(); await m.ready();
  const old=(await m.facts()).patch.config!;
  expect(m.helper("config.action",{action:"apply_default_preset"}).ok).toBe(true);
  await m.facts();
  expect(await m.command({type:"save_surface",machineId:"local",surface:"user_defaults",revision:old.revisions.user_defaults,value:{...old.surfaces.user_defaults,language:"en"}})).toMatchObject({ok:false,outcome:"revision_conflict"});
});

it.each([0,1100])("visible typed/pasted Git draft commits after %i ms, with real git log readback",async(delay)=>{
  const m=machine();
  m.s.answers["projects.list"]=()=>[{projectId:"p",displayName:"Fixture",canonicalRoot:m.repo,activation:{}}];
  writeFileSync(resolve(m.repo,"a.txt"),"one\nchanged\n");
  expect(m.helper("git",{action:"stage",path:"a.txt",staged:true}).ok).toBe(true);
  history.replaceState(null,"","/m/local/p/p/git"); localStorage.clear();
  const root=document.createElement("div");document.body.append(root);
  dispose=render(()=><Workspace port={m.p}/>,root); await settle();
  const message=delay?"Pasted commit\n\nBody from clipboard":"Typed commit";
  input("Commit message",message);
  if(delay) await new Promise(r=>setTimeout(r,delay));
  button("Commit").click(); await settle();
  expect(m.git("log","-1","--format=%B")).toBe(message);
},10000);

it("keeps Git empty-message refusal and explains disabled Pull for a remote without an upstream",async()=>{
  const m=machine(); m.git("remote","add","origin",resolve(m.home,"remote.git")); await m.ready();
  expect(m.helper("git",{action:"commit",message:"  "}).ok).toBe(false);
  m.s.answers["projects.list"]=()=>[{projectId:"p",displayName:"Fixture",canonicalRoot:m.repo,activation:{}}];
  history.replaceState(null,"","/m/local/p/p/git");
  const root=document.createElement("div");document.body.append(root);
  dispose=render(()=><Workspace port={m.p}/>,root); await settle();
  const pull=document.querySelector('[aria-label="Pull"]')!;
  expect(pull.getAttribute("aria-disabled")).toBe("true");
  expect(pull.getAttribute("data-why")).toMatch(/upstream|tracking/i);
});

it("uses the service's channel-safe worktree default for reset, tooltip and example",async()=>{
  const m=machine(); await m.ready();
  const initial=(await m.facts()).patch;
  expect(initial.defaultWorktreeRoot).toBe(resolve(m.state,"data/cortex-workspace/worktrees"));
  expect(initial.worktreeRoot).toBe(initial.defaultWorktreeRoot);
  const custom=resolve(m.home,"custom-trees"); mkdirSync(custom); writeFileSync(resolve(custom,"keep.txt"),"existing work");
  expect((await m.command({type:"set_worktree_root",machineId:"local",path:custom})).ok).toBe(true);
  history.replaceState(null,"","/settings/machine/local/worktrees");
  const root=document.createElement("div");document.body.append(root);
  dispose=render(()=><Workspace port={m.p}/>,root); await settle();
  expect(button("Use default").hasAttribute("title")).toBe(false);
  button("Use default").click();
  expect(document.querySelector<HTMLInputElement>('[aria-label="Worktree folder"]')!.value).toBe(initial.defaultWorktreeRoot);
  expect(document.body.textContent).toContain(initial.defaultWorktreeRoot+"/<project-key>/<task-id>");
  button("Save").click(); await settle();
  expect((await m.facts()).patch.worktreeRoot).toBe(initial.defaultWorktreeRoot);
  expect(m.git("status","--porcelain")).toBe("");
});

it("failed Start planning keeps the entered description and creates nothing",async()=>{
  const m=machine();
  m.s.answers["projects.list"]=()=>[{projectId:"p",displayName:"Fixture",canonicalRoot:m.repo,activation:{}}];
  history.replaceState(null,"","/m/local/new?kind=sprint&project=p"); localStorage.clear();
  const root=document.createElement("div");document.body.append(root);
  dispose=render(()=><Workspace port={m.p}/>,root); await settle();
  button("Sprint").click(); await settle();
  const description=root.querySelector<HTMLTextAreaElement>("textarea")!;
  description.value="Keep this multiline sprint\nwith all the details";
  description.dispatchEvent(new Event("input",{bubbles:true}));
  root.querySelector<HTMLButtonElement>('[aria-label="Start planning"]')!.click(); await settle();
  expect(document.body.textContent).toMatch(/Connect an engine and select an available model/);
  expect(description.value).toBe("Keep this multiline sprint\nwith all the details");
  expect(m.helper("product.read").result.sprints).toEqual([]);
});

it("amend confirmation uses the reactive pasted draft and reads it back from git log",async()=>{
  const m=machine();
  m.s.answers["projects.list"]=()=>[{projectId:"p",displayName:"Fixture",canonicalRoot:m.repo,activation:{}}];
  history.replaceState(null,"","/m/local/p/p/git");localStorage.clear();
  const root=document.createElement("div");document.body.append(root);
  dispose=render(()=><Workspace port={m.p}/>,root);await settle();
  input("Commit message","Amended from UI\n\nPasted body");
  button("Amend last commit").click();await settle();
  const dialog=root.querySelector('dialog[open]')!;
  expect(dialog.textContent).toContain("Amend the last commit?");
  [...dialog.querySelectorAll<HTMLButtonElement>("button")].find(b=>b.textContent==="Amend")!.click();await settle();
  expect(m.git("log","-1","--format=%B")).toBe("Amended from UI\n\nPasted body");
  expect(m.git("rev-list","--count","HEAD")).toBe("1");
});

it("a preset model without a connected catalogue is refused without orphan planning work",async()=>{
  const m=machine(); await m.ready(); await m.command({type:"activate_project",projectId:"p"}); await m.facts();
  expect(await m.command({type:"create_sprint",projectId:"p",description:"Keep description after activation",runMode:"manual",settings:{model:"preset-but-not-connected",effort:"",mode:"guarded",language:"auto"}})).toMatchObject({ok:false,outcome:"model_required"});
  expect(m.helper("product.read").result.sprints).toEqual([]);
});

it.each(["manual","automatic"] as const)("empty model refuses %s sprint before any planning entity is created",async(runMode)=>{
  const m=machine(); await m.ready();
  expect(await m.command({type:"create_sprint",projectId:"p",description:"Keep my description",runMode,settings:{model:"",effort:"",mode:"guarded",language:"auto"}})).toMatchObject({ok:false,outcome:"model_required",message:expect.stringMatching(/connect.*model/i)});
  expect(m.helper("product.read").result.sprints).toEqual([]);
  expect(m.s.calls.filter(c=>c.operation==="tasks.create")).toEqual([]);
});
