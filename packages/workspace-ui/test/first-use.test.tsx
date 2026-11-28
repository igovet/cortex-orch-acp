// @vitest-environment jsdom
import { lstatSync, mkdirSync, readdirSync, readFileSync, realpathSync, writeFileSync, rmSync } from "node:fs";
import { isAbsolute, relative, resolve, sep } from "node:path";
import { pathToFileURL } from "node:url";
import { spawnSync } from "node:child_process";
import { createRequire } from "node:module";
import { nativeServiceHome, NativeService } from "./native-machine";
import { render } from "solid-js/web";
import { afterEach, expect, it, vi } from "vitest";
import { Workspace } from "../src";
import { createRealPort } from "../src/port/real";
import { conversationOf, type FeedRow } from "../src/port/history";
import { capabilitiesWire, scriptedTransport, snapshot, flush, scriptedPort, withTask, projection } from "./support";
import { emptySession, emptyWorkspace, LOCAL_MACHINE_ID, type UserDefaults, type Worker, type WorkspaceCommand } from "@cortex/workspace-domain";

let dispose: (() => void) | undefined;
const folders: string[] = [];
const services: NativeService[] = [];
// Every test starts the real service and talks to it over its socket.
vi.setConfig({ testTimeout: 60000 });
afterEach(async () => { dispose?.(); dispose = undefined; document.body.innerHTML = ""; for (const service of services.splice(0)) await service.stop(); for (const dir of folders.splice(0)) rmSync(dir, { recursive:true, force:true }); });
// The service answers over a socket, so settling also waits until none of its calls is in flight (and stays idle for a moment).
let inflight = 0;
const settle = async () => {
  for (let i=0;i<10;i++) await flush();
  // Settled once no call is in flight and the page has stopped changing for a few consecutive checks.
  let last = "", stable = 0;
  for (let spent=0; stable<5 && spent<20000; spent+=50) { await new Promise(r=>setTimeout(r,50)); await flush(); const now = document.body.innerHTML; stable = now===last && inflight===0 ? stable+1 : 0; last = now; }
};
const all = (selector: string) => [...document.querySelectorAll<HTMLElement>(selector)];
const button = (text: string) => all("button").find(b => b.textContent?.trim().startsWith(text))!;
const input = (label: string, text: string) => { const el = document.querySelector<HTMLTextAreaElement>(`[aria-label="${label}"]`)!; el.value=text; el.dispatchEvent(new Event("input",{bubbles:true})); };

/** The real native service (configuration, product store, Git), but no account, network, engine or native shell bridge. */
async function machine() {
  const home=nativeServiceHome(); folders.push(home);
  // The service keeps its state under the dev channel's namespace in XDG_STATE_HOME; its worktrees default is below it.
  const repo=resolve(home,"projects/project"), state=resolve(home,"xdg-state/cortex-workspace-dev");
  mkdirSync(repo,{recursive:true});
  const env={PATH:process.env.PATH!,HOME:home,XDG_CONFIG_HOME:resolve(home,"config"),XDG_STATE_HOME:resolve(home,"xdg-state")};
  function git(...args: string[]) {
    const r=spawnSync("git",args,{cwd:repo,env,encoding:"utf8"}); expect(r.status,r.stderr).toBe(0); return r.stdout.trim();
  }
  git("init","-q","-b","main"); git("config","user.name","Fixture"); git("config","user.email","fixture@example.invalid");
  writeFileSync(resolve(repo,"a.txt"),"one\ntwo\n"); git("add","."); git("commit","-qm","Initial");
  const service=await NativeService.start(home,resolve(home,"projects")); services.push(service);
  const registered=await service.call("projects.register",{root:repo,displayName:"Fixture"}) as {projectId:string};
  // The service's own project identity is the one every screen and command uses.
  const id=registered.projectId;
  async function helper(op: string, params: unknown = {}) { inflight++; try { return await service.machine(op,id,params); } finally { inflight--; } }
  let active=false;
  const s=scriptedTransport({
    "service.capabilities":()=>({methods:["machine.call","projects.activate","projects.status"],machine:true}),
    "projects.list":()=>[{projectId:id,displayName:"Fixture",canonicalRoot:repo,activation:active?{}:null}],
    "projects.activate":async()=>{ expect((await helper("config.action",{action:"apply_default_preset"})).ok).toBe(true); active=true; return {ready:true}; },
    "projects.status":()=>({ready:active}),
    "capabilities.get":()=>capabilitiesWire(["capabilities.get","project.snapshot","tasks.create","tasks.start","decisions.list"]),
    "project.snapshot":()=>snapshot("1",[]),
    "machine.call":async message=>{ const {op,params}=message as {op:string;params:unknown}; return op==="project.config.read" ? {ok:true,result:{value:{revisions:{}},tokens:{}}} : await helper(op,params); }
  });
  const p=createRealPort({transport:s.transport,actorId:"owner",registry:true});
  let n=0;
  const command=(payload:WorkspaceCommand)=>p.command({requestId:"first-"+(++n),operation:payload.type,expectedVersion:null,payload});
  const facts=async()=> (await p.query("machine.facts",{}))[0];
  const overview=async()=> (await p.query("machine.overview",{}))[0];
  const ready=async()=>{await p.query("capabilities.get",{}); await p.query("project.snapshot",{}); await p.query("project.facts",{}); await facts();};
  return {id,home,repo,state,env,git,helper,service,s,p,command,facts,overview,ready};
}

/** Resolve the service's exact Dev store in this disposable fixture, whether it is local or home-backed. */
function openFixtureStore(m: Awaited<ReturnType<typeof machine>>) {
  // Load SQLite in Node, outside Vite's jsdom module transform.
  const { DatabaseSync } = createRequire(import.meta.url)("node:sqlite") as typeof import("node:sqlite");
  const project = realpathSync(m.repo);
  const anchorPath = resolve(project, ".cortex-dev/cortex.sqlite3");
  expect(lstatSync(anchorPath).isFile() && realpathSync(anchorPath) === anchorPath).toBe(true);
  const hasProjects = (db: InstanceType<typeof DatabaseSync>) =>
    db.prepare("SELECT 1 FROM sqlite_master WHERE type='table' AND name='workspace_projects'").get() !== undefined;
  const bindingOf = (db: InstanceType<typeof DatabaseSync>) => {
    const binding = db.prepare("SELECT binding_json FROM store_location WHERE singleton=1").get()?.binding_json;
    if (typeof binding !== "string") throw new Error("Fixture store binding is missing");
    return JSON.parse(binding) as { directory: string; project_root: string; channel: string };
  };
  const anchor = new DatabaseSync(anchorPath, { readOnly: true });
  let binding;
  try { binding = hasProjects(anchor) ? undefined : bindingOf(anchor); }
  finally { anchor.close(); }

  let path = anchorPath;
  if (binding) {
    expect(binding).toMatchObject({ project_root: project, channel: ".cortex-dev" });
    expect(isAbsolute(binding.directory)).toBe(true);
    const directory = realpathSync(binding.directory);
    const within = relative(realpathSync(m.state), directory);
    expect(within && within !== ".." && !within.startsWith(".." + sep) && !isAbsolute(within)).toBe(true);
    expect(directory).toBe(binding.directory);
    path = resolve(directory, "cortex.sqlite3");
    expect(lstatSync(path).isFile()).toBe(true);
  }
  // SQLite mode=rw refuses a missing file, including one that disappeared since the binding was verified.
  const uri = pathToFileURL(path);
  uri.searchParams.set("mode", "rw");
  const db = new DatabaseSync(uri.href);
  try {
    expect(hasProjects(db)).toBe(true);
    if (binding) expect(bindingOf(db)).toEqual(binding);
    return db;
  } catch (error) {
    db.close();
    throw error;
  }
}

it("renders pending, uncaptured and empty command output from the native service on both feeds", async () => {
  const m = await machine();
  await m.service.call("projects.activate", { projectId: m.id });
  await expect.poll(async () => (await m.service.call("projects.status", { projectId: m.id }) as { ready: boolean }).ready).toBe(true);
  const at = "2026-11-24T01:00:00Z";
  // Seed only synthetic engine records in the disposable Dev project. No engine is started.
  const db = openFixtureStore(m);
  try {
    db.exec("BEGIN");
    db.prepare("INSERT OR IGNORE INTO workspace_projects(project_id,project_root,activated_by,activated_at) VALUES (?,?,'fixture',?)").run(m.id, m.repo, at);
    db.prepare("INSERT INTO workspace_tasks(task_id,project_id,description,execution_id,created_at) VALUES ('t',?,'Output fixture','e',?)").run(m.id, at);
    db.prepare("INSERT INTO tasks(id,project_root,created_at) VALUES ('runtime',?,?)").run(m.repo, at);
    db.prepare("INSERT INTO runtime_tasks(task_id,state,goal,created_at,updated_at) VALUES ('runtime','EXECUTING','Output fixture',?,?)").run(at, at);
    db.prepare("INSERT INTO acp_host_sessions(session_id,project_root,task_id,client_info,client_capabilities,mcp_servers,created_at) VALUES ('display',?,'runtime','{}','{}','[]',?)").run(m.repo, at);
    db.prepare("INSERT INTO workspace_executions(execution_id,task_id,runtime_task_id,acp_session_id,owner_epoch,bound_at) VALUES ('e','t','runtime','display',1,?)").run(at);
    db.prepare("INSERT INTO runtime_nodes(task_id,revision,node_id,role,objective,dependencies_json,scope_json,stage,status,assignment_ref) VALUES ('runtime',0,'worker','worker-frontend-dev','Output fixture','[]','[]','execution','running','w')").run();
    db.prepare("INSERT INTO delegation_provisioning(idempotency_key,task_id,attempt_id,role_id,step,status,owner_epoch,created_at,updated_at) VALUES ('w','runtime','xa_fixture','worker-frontend-dev','ready','ready',1,?,?)").run(at, at);
    const insert = db.prepare("INSERT INTO host_updates(session_id,seq,update_json,created_at) VALUES ('display',?,?,?)");
    const cases = [
      { command: "still running", status: "in_progress" },
      { command: "not captured", status: "completed" },
      { command: "captured empty", status: "completed", rawOutput: "" },
    ];
    for (const [group, prefix] of ["", "xa_fixture:"].entries()) {
      cases.forEach(({ command, ...fields }, i) => insert.run(group * cases.length + i, JSON.stringify({
        sessionUpdate: "tool_call", toolCallId: `${prefix}exec-${i}`, title: "Bash", kind: "execute", rawInput: { command }, ...fields,
      }), at));
    }
    db.exec("COMMIT");
  } finally { db.close(); }

  const history = await m.service.call("native.query", {
    protocol: { major: 2, mapping: 2 }, requestId: "output-fixture", operation: "history.page", projectId: m.id,
    payload: { target: "t", cursor: null, limit: 20 },
  }) as { data: { type: string; value: { items: FeedRow[] } } };
  expect(history.data.type).toBe("history");
  const coordinator = await m.helper("workers", { taskIds: ["t"], workerId: "coordinator:t" });
  expect(coordinator.ok, JSON.stringify(coordinator)).toBe(true);
  const assignment = await m.helper("workers", { taskIds: ["t"], workerId: "w" });
  expect(assignment.ok, JSON.stringify(assignment)).toBe(true);
  const [coordinatorWorker] = coordinator.result as Worker[];
  const [worker] = assignment.result as Worker[];
  expect(coordinatorWorker.activity).toHaveLength(3);
  expect(worker.activity).toHaveLength(3);

  // Pass native history records through the Conversation mapper and native activity through the
  // worker panel boundaries. Never hand-build any of the projected output fields.
  const state = withTask(emptyWorkspace(), "Output fixture");
  state.machines[0].connection = "connected";
  state.tasks[0].status = "working";
  state.tasks[0].process = "live";
  const session = emptySession("t");
  session.timeline = conversationOf(history.data.value.items, { description: "Output fixture", answers: [] }).map(row => row.entry);
  state.sessions = [session];
  state.workers = [worker];
  for (const suffix of ["", "/workers/cortex", "/workers/w"]) {
    const p = scriptedPort({ capabilities: { everything: true, operations: [] } });
    p.port.readWorkerActivity = async (taskId, workerId) => {
      expect(taskId).toBe("t");
      expect([worker.id, coordinatorWorker.id]).toContain(workerId);
      return workerId === worker.id ? worker : coordinatorWorker;
    };
    window.history.replaceState(null, "", `/m/${LOCAL_MACHINE_ID}/t/t${suffix}`);
    dispose = render(() => <Workspace port={p.port} />, document.body);
    p.emit(projection("1", state));
    await settle();
    all("button.fold").forEach(fold => fold.click());
    await settle();
    const rows = all("button.step-row");
    expect(rows).toHaveLength(3);
    rows.forEach(row => row.click());
    await settle();
    const outputs = rows.map(row => row.closest(".step")?.querySelector('section[aria-label="Output"]'));
    expect(outputs.map(section => section?.children.item(1)?.textContent)).toEqual([
      "Waiting for output…", "No output was recorded.", "Recorded empty.",
    ]);
    dispose(); dispose = undefined;
    document.body.innerHTML = "";
  }
});

it("empty HOME activation refreshes first saves of defaults, limits, profiles and MCP against real files",async()=>{
  const m=await machine(); await m.ready();
  expect((await m.facts()).patch.config!.revisions.user_defaults).toBe(0);
  expect((await m.command({type:"activate_project",projectId:m.id})).ok).toBe(true);
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
});

it("saves and shows a server's explicit opt-out from the sandbox against real files",async()=>{
  const m=await machine(); await m.ready();
  expect((await m.command({type:"activate_project",projectId:m.id})).ok).toBe(true);
  await m.p.query("capabilities.get",{});
  let c=(await m.facts()).patch.config!;
  const server={name:"fixture",enabled:false,command:"/usr/bin/true",args:[],env:[],network:false,readPaths:[],writePaths:[],runtimeDirEnv:[],allowedTools:["read"],readOnlyTools:["read"],coordinatorTools:[],alwaysLoadTools:null,roles:["worker"],engines:null,handTools:[],unsupportedKeys:[],problem:null};
  // Wherever this channel keeps its owner configuration inside the disposable HOME.
  const file=()=>resolve(m.home,(readdirSync(m.home,{recursive:true}) as string[]).find(p=>p.endsWith("cortex/mcp-servers.json"))!);
  for (const [containment,shown] of [["none","none"],["contained",undefined],[undefined,undefined]] as const) {
    const value=[containment ? {...server,containment} : server];
    expect(await m.command({type:"save_surface",machineId:"local",surface:"mcp_servers",value,revision:c.revisions.mcp_servers})).toMatchObject({ok:true});
    c=(await m.facts()).patch.config!;
    expect(c.surfaces.mcp_servers[0].containment).toBe(shown);
    // Contained is the default, so only the opt-out is stored on the entry.
    const stored=JSON.parse(readFileSync(file(),"utf8")).servers.fixture;
    expect(stored.containment).toBe(shown);
  }
});

it("rereads automatic first-run configuration changes without another activation",async()=>{
  const m=await machine(); await m.ready();
  expect((await m.helper("config.action",{action:"apply_default_preset"})).ok).toBe(true);
  expect((await m.facts()).patch.config!.revisions.user_defaults).not.toBe(0);
});

it("does not overwrite a dirty draft's base when a background config read sees another writer",async()=>{
  const m=await machine(); await m.ready();
  const old=(await m.facts()).patch.config!;
  expect((await m.helper("config.action",{action:"apply_default_preset"})).ok).toBe(true);
  await m.facts();
  expect(await m.command({type:"save_surface",machineId:"local",surface:"user_defaults",revision:old.revisions.user_defaults,value:{...old.surfaces.user_defaults,language:"en"}})).toMatchObject({ok:false,outcome:"revision_conflict"});
});

it.each([0,1100])("visible typed/pasted Git draft commits after %i ms, with real git log readback",async(delay)=>{
  const m=await machine();
  m.s.answers["projects.list"]=()=>[{projectId:m.id,displayName:"Fixture",canonicalRoot:m.repo,activation:{}}];
  writeFileSync(resolve(m.repo,"a.txt"),"one\nchanged\n");
  expect((await m.helper("git",{action:"stage",path:"a.txt",staged:true})).ok).toBe(true);
  history.replaceState(null,"",`/m/local/p/${m.id}/git`); localStorage.clear();
  const root=document.createElement("div");document.body.append(root);
  dispose=render(()=><Workspace port={m.p}/>,root); await settle();
  const message=delay?"Pasted commit\n\nBody from clipboard":"Typed commit";
  input("Commit message",message);
  if(delay) await new Promise(r=>setTimeout(r,delay));
  // The Git pane's own Commit, not the title band's Commit…, which opens its dialog.
  all(".commit-box button").find(b => b.textContent?.trim().startsWith("Commit"))!.click(); await settle();
  expect(m.git("log","-1","--format=%B")).toBe(message);
});

it("keeps Git empty-message refusal and explains disabled Pull for a remote without an upstream",async()=>{
  const m=await machine(); m.git("remote","add","origin",resolve(m.home,"remote.git")); await m.ready();
  expect((await m.helper("git",{action:"commit",message:"  "})).ok).toBe(false);
  m.s.answers["projects.list"]=()=>[{projectId:m.id,displayName:"Fixture",canonicalRoot:m.repo,activation:{}}];
  history.replaceState(null,"",`/m/local/p/${m.id}/git`);
  const root=document.createElement("div");document.body.append(root);
  dispose=render(()=><Workspace port={m.p}/>,root); await settle();
  const pull=document.querySelector('[aria-label="Pull"]')!;
  expect(pull.getAttribute("aria-disabled")).toBe("true");
  expect(pull.getAttribute("data-why")).toMatch(/upstream|tracking/i);
});

it("uses the service's channel-safe worktree default for reset, tooltip and example",async()=>{
  const m=await machine(); await m.ready();
  const untouched=m.git("status","--porcelain");
  expect(untouched).toBe("?? .cortex-dev/");
  // Startup facts carry no overview: the worktree folder is read when a screen that shows it opens.
  expect((await m.facts()).patch).not.toHaveProperty("worktreeRoot");
  const initial=(await m.overview()).patch;
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
  expect((await m.overview()).patch.worktreeRoot).toBe(initial.defaultWorktreeRoot);
  // Registering the project made the service reserve its private .cortex-dev folder; saving the folder changes nothing else.
  expect(m.git("status","--porcelain")).toBe(untouched);
});

it("failed Start planning keeps the entered description and creates nothing",async()=>{
  const m=await machine();
  m.s.answers["projects.list"]=()=>[{projectId:m.id,displayName:"Fixture",canonicalRoot:m.repo,activation:{}}];
  history.replaceState(null,"",`/m/local/new?kind=sprint&project=${m.id}`); localStorage.clear();
  const root=document.createElement("div");document.body.append(root);
  dispose=render(()=><Workspace port={m.p}/>,root); await settle();
  button("Sprint").click(); await settle();
  const description=root.querySelector<HTMLTextAreaElement>("textarea")!;
  description.value="Keep this multiline sprint\nwith all the details";
  description.dispatchEvent(new Event("input",{bubbles:true}));
  root.querySelector<HTMLButtonElement>('[aria-label="Start planning"]')!.click(); await settle();
  expect(document.body.textContent).toMatch(/Connect an engine and select an available model/);
  expect(description.value).toBe("Keep this multiline sprint\nwith all the details");
  expect(((await m.helper("product.read")).result as {sprints:unknown[]}).sprints).toEqual([]);
});

it("amend confirmation uses the reactive pasted draft and reads it back from git log",async()=>{
  const m=await machine();
  m.s.answers["projects.list"]=()=>[{projectId:m.id,displayName:"Fixture",canonicalRoot:m.repo,activation:{}}];
  history.replaceState(null,"",`/m/local/p/${m.id}/git`);localStorage.clear();
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
  const m=await machine(); await m.ready(); await m.command({type:"activate_project",projectId:m.id}); await m.facts();
  expect(await m.command({type:"create_sprint",projectId:m.id,description:"Keep description after activation",runMode:"manual",settings:{model:"preset-but-not-connected",effort:"",mode:"guarded",language:"auto"}})).toMatchObject({ok:false,outcome:"model_required"});
  expect(((await m.helper("product.read")).result as {sprints:unknown[]}).sprints).toEqual([]);
});

it.each(["manual","automatic"] as const)("empty model refuses %s sprint before any planning entity is created",async(runMode)=>{
  const m=await machine(); await m.ready();
  expect(await m.command({type:"create_sprint",projectId:m.id,description:"Keep my description",runMode,settings:{model:"",effort:"",mode:"guarded",language:"auto"}})).toMatchObject({ok:false,outcome:"model_required",message:expect.stringMatching(/connect.*model/i)});
  expect(((await m.helper("product.read")).result as {sprints:unknown[]}).sprints).toEqual([]);
  expect(m.s.calls.filter(c=>c.operation==="tasks.create")).toEqual([]);
});
