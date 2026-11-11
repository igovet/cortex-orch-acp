import { createEffect, createRoot } from "solid-js";
import { afterEach, expect, it } from "vitest";
import { render } from "solid-js/web";
import { Workspace } from "../src";
import { emptyWorkspace, type ProjectFacts } from "@cortex/workspace-domain";
import { createWorkspaceStore } from "../src/store";
import { flush, scriptedPort, withTask } from "./support";

let dispose: (() => void) | undefined;
afterEach(() => { dispose?.(); document.body.innerHTML=""; });
const deferred = <T,>() => { let resolve!: (value: T) => void; let reject!: (e: Error) => void; const promise = new Promise<T>((a,b) => {resolve=a; reject=b;}); return {promise,resolve,reject}; };
it("never publishes the raw tasks before sprint/planning/archive facts, on startup or refresh", async () => {
  const s = scriptedPort(), pending = deferred<ProjectFacts[]>();
  let version = "1";
  const raw = withTask(emptyWorkspace(), "Planning run");
  s.port.query = (async (op: string) => op === "capabilities.get" ? {everything:true,operations:[]} : op === "project.snapshot" ? [{kind:"projection",scope:"workspace",version,state:structuredClone(raw)}] : op === "project.facts" ? pending.promise : []) as typeof s.port.query;
  const store = createRoot(d => {dispose=d; return createWorkspaceStore(s.port);});
  const seen: string[][] = [];
  createRoot(d => { const prior=dispose!; dispose=()=>{d();prior();}; createEffect(()=>seen.push(store.state().tasks.map(t=>t.title))); });
  await flush();
  expect(store.state().tasks).toEqual([]);
  pending.resolve([{projectId:"p",openBranch:"main",defaultBranch:"main",repo:null,workers:[],reviews:[],sprints:[],overlays:{t:{planning:true}}}]);
  await flush();
  expect(store.state().tasks).toEqual([]);
  expect(seen.flat()).not.toContain("Planning run");
  version = "2";
  raw.tasks[0].title = "Purged run";
  await store.reload();
  expect(seen.flat()).not.toContain("Purged run");
  // A same-wire-version facts change may legitimately unhide a former planning task.
  s.port.query = (async (op:string)=>op==="capabilities.get"?{everything:true,operations:[]}:op==="project.snapshot"?[{kind:"projection",scope:"workspace",version,state:structuredClone(raw)}]:[]) as typeof s.port.query;
  await store.reload();
  expect(store.state().tasks.map(t=>t.title)).toEqual(["Purged run"]);
  expect(store.ready()).toBe(true);
});
it("keeps the last coherent view when a required facts read fails", async () => {
  const s=scriptedPort(), state=withTask(emptyWorkspace(),"Known");
  const store=createRoot(d=>{dispose=d;return createWorkspaceStore(s.port,{initial:{state,version:"1",capabilities:{everything:true,operations:[]}}});});
  s.port.query=(async(op:string)=>op==="capabilities.get"?{everything:true,operations:[]}:op==="project.snapshot"?[{kind:"projection",scope:"workspace",version:"2",state:withTask(emptyWorkspace(),"Wrong")}]:op==="project.facts"?Promise.reject(new Error("Facts unavailable")):[]) as typeof s.port.query;
  await store.reload();
  expect(store.state().tasks[0].title).toBe("Known");
  expect(store.connection().state).toBe("connected");
  expect(store.readIssue()).toBe("Facts unavailable");
});
it("shows only the startup screen rather than an empty project list or a wrong-page error",async()=>{
  const s=scriptedPort(), pending=deferred<any>();
  s.port.query=(async()=>pending.promise) as typeof s.port.query;
  const root=document.createElement("div");document.body.append(root);
  history.replaceState(null,"","/m/local/t/t");
  dispose=render(()=><Workspace port={s.port}/>,root);
  await flush();
  expect(root.querySelector(".l-startup")).toBeTruthy();
  expect(root.querySelector(".titlebar, .sidebar, .ws-tabstrip")).toBeNull();
  expect(root.textContent).not.toMatch(/No projects on this machine|This task is not on this machine|Offline|Connecting/);
});
