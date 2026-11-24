import { describe, expect, it, vi } from "vitest";
import { createRoot } from "solid-js";
import { createWorkspaceStore } from "../src/store";
import { createRealPort } from "../src/port/real";
import { nativeDelta } from "../src/port/native";
import { capabilitiesWire, scriptedTransport, wireTask } from "./support";
import type { NativeTaskDelta, PortUpdate } from "@cortex/workspace-domain";

const delta = (version = "2", from = "4", through = "5", base = "1"): NativeTaskDelta => ({
  projectId: "p", batch: { generation: "generation", streamKey: { scope: "project", project_id: "p" }, fromSequence: from, throughSequence: through,
    changes: [{ kind: "task", id: "task-1", baseVersion: base, newVersion: version,
      operation: { type: "upsert", data: { kind: "task_projection", data: wireTask({ version, description: "Updated", executionId: "execution", phase: "recovery", executionState: "recovering", runtimeState: "RECOVERING", waitReason: "turn_interrupted" }) } } }] }
});
const historyRow = (i: number, kind: string, text: string) => ({ historyId: `h${i}`, sequence: String(i + 1), revision: "1", kind, text, toolStatus: null,
  recordedAt: `2026-11-17T14:52:${String(i).padStart(2, "0")}.000000+00:00`, availability: "available" });
function fixture() {
  const s = scriptedTransport({
    "service.capabilities": () => ({ methods: ["native.query"] }),
    "projects.list": () => [{ projectId: "p", displayName: "Project", canonicalRoot: "/p", activation: { actorId: "native" } }],
    "capabilities.get": () => capabilitiesWire(["capabilities.get", "project.snapshot", "history.page"]),
    "native.query": raw => {
      const m = raw as { operation: string };
      return { protocol: { major: 2, mapping: 2 }, projectId: "p", generation: "generation", version: "2", highWatermark: "3",
        data: m.operation === "tasks.page"
          ? { type: "tasks", value: { items: [wireTask()], nextCursor: null, highWatermark: "3" } }
          : { type: "history", value: { items: [], nextCursor: null, highWatermark: "0" } } };
    }
  });
  return { s, p: createRealPort({ transport: s.transport, actorId: "native", registry: true }) };
}
describe("native read path", () => {
  it("publishes a 31-task board without waiting for cold conversation reads or showing a loading issue", async () => {
    const { s, p } = fixture();
    s.answers["service.capabilities"] = () => ({ methods: ["native.query", "native.board"] });
    const items = Array.from({length: 31}, (_, i) => wireTask({taskId: "task-" + i, executionId: i === 0 ? "execution" : null}));
    const board = {protocol:{major:2,mapping:2},projectId:"p",generation:"generation",version:"2",highWatermark:"3",
      data:{type:"board",value:{items,nextCursor:null,highWatermark:"3"}}};
    s.answers["native.board"] = () => board;
    let finish!: (value: unknown) => void;
    s.answers["native.query"] = raw => (raw as {operation:string}).operation === "tasks.page"
      ? {...board,data:{...board.data,type:"tasks"}} : new Promise(resolve => {finish=resolve;});
    const updates: PortUpdate[] = [], stop = p.subscribe(u => updates.push(u));
    try {
      await p.query("capabilities.get", {});
      const result = await p.query("project.snapshot", {});
      expect(result[0]?.scope === "project" && result[0].slice.tasks.length).toBe(31);
      expect(updates.some(u => u.kind === "project_state" && u.issue?.includes("loading"))).toBe(false);
      expect(s.calls.filter(c => c.operation === "native.board")).toHaveLength(1);
      if (finish) finish({...board,data:{type:"history",value:{items:[],nextCursor:null,highWatermark:"0"}}});
    } finally {stop();}
  });

  it("keeps sprint facts behind a slow first board instead of publishing unloaded membership", async () => {
    const {s,p}=fixture();
    s.answers["service.capabilities"]=()=>({methods:["native.query","native.board"],machine:true});
    let finish!:(value:unknown)=>void;
    s.answers["native.board"]=()=>new Promise(resolve=>{finish=resolve;});
    const items=Array.from({length:31},(_,i)=>wireTask({taskId:"cold-"+i}));
    const product={tasks:{},sprints:[{id:"cold-sprint",projectId:"p",description:"Cold sprint",runMode:"automatic",
      planning:"created",taskIds:items.map(t=>t.taskId),queueState:"running",pauseReason:null,
      conversation:[],plan:null,targetBranch:null,materializedVersion:1}],messages:[]};
    s.answers["machine.call"]=raw=>{
      const op=(raw as {op:string}).op;
      if(op==="repo.snapshot") return {ok:true,result:null};
      return {ok:true,result:op==="workers"?[]:product};
    };
    const updates:PortUpdate[]=[],stop=p.subscribe(u=>updates.push(u));
    try {
      await p.query("capabilities.get",{});
      expect(await p.query("project.snapshot",{})).toEqual([]);
      expect(await p.query("project.facts",{})).toEqual([]);
      expect(updates.some(u=>u.kind==="project_state" && u.issue)).toBe(false);
      finish({protocol:{major:2,mapping:2},projectId:"p",generation:"generation",version:"2",highWatermark:"3",
        data:{type:"board",value:{items,nextCursor:null,highWatermark:"3"}}});
      await vi.waitFor(()=>expect(updates.some(u=>u.kind==="projection" && u.scope==="project" && u.slice.tasks.length===31)).toBe(true));
    } finally {stop();}
  });
  it("does not expose sprint membership after a failed first board or turn startup readiness into an error", async () => {
    const {s,p}=fixture();
    s.answers["service.capabilities"]=()=>({methods:["native.query","native.board","projects.status"],machine:true});
    const items=Array.from({length:31},(_,i)=>wireTask({taskId:"cold-"+i}));
    const product={tasks:{},sprints:[{id:"cold-sprint",projectId:"p",description:"Cold sprint",runMode:"automatic",
      planning:"created",taskIds:items.map(t=>t.taskId),queueState:"running",pauseReason:null,
      conversation:[],plan:null,targetBranch:null,materializedVersion:1}],messages:[]};
    let cold=true;
    s.answers["native.board"]=()=>{if(cold) throw new Error("snapshot_expired"); return {
      protocol:{major:2,mapping:2},projectId:"p",generation:"generation",version:"2",highWatermark:"3",
      data:{type:"board",value:{items,nextCursor:null,highWatermark:"3"}}};};
    s.answers["projects.status"]=()=>({ready:false,state:"starting",reason:null});
    s.answers["machine.call"]=raw=>({ok:true,result:(raw as {op:string}).op==="repo.snapshot"?null:product});
    const updates:PortUpdate[]=[],stop=p.subscribe(u=>updates.push(u));
    try {
      await p.query("capabilities.get",{});
      expect(await p.query("project.snapshot",{})).toEqual([]);
      expect(await p.query("project.facts",{})).toEqual([]);
      expect(updates.some(u=>u.kind==="facts")).toBe(false);
      cold=false;
      expect((await p.query("project.snapshot",{}))[0]?.scope).toBe("project");
      const facts=await p.query("project.facts",{});
      expect(facts[0]?.sprints?.[0].taskIds).toHaveLength(31);
      expect((await p.query("project.facts",{fresh:true}))[0]?.issue).toBeNull();
      expect(updates.some(u=>u.kind==="project_state" && u.issue)).toBe(false);
    } finally {stop();}
  });

  it("does not let a failed refresh release new sprint members from an unpublished task read",async()=>{
    const {s,p}=fixture();
    s.answers["service.capabilities"]=()=>({methods:["native.query","native.board"],machine:true});
    let items=[wireTask()], fail=false;
    const product=()=>({tasks:{},sprints:[{id:"sp",projectId:"p",description:"Sprint",runMode:"manual",planning:"created",
      taskIds:items.map(t=>t.taskId),queueState:"idle",pauseReason:null,targetBranch:null,conversation:[],plan:null,materializedVersion:1}]});
    s.answers["native.board"]=()=>({protocol:{major:2,mapping:2},projectId:"p",generation:"g",version:"2",highWatermark:"3",
      data:{type:"board",value:{items,nextCursor:null,highWatermark:"3"}}});
    s.answers["machine.call"]=raw=>{
      const op=(raw as {op:string}).op;
      if(op==="product.board" && fail) throw Error("storage_unavailable");
      return {ok:true,result:op==="repo.snapshot"?null:product()};
    };
    const stop=p.subscribe(()=>{});
    try {
      await p.query("capabilities.get",{});
      expect(await p.query("project.snapshot",{})).toHaveLength(1);
      await p.query("project.facts",{});
      items=[...items,wireTask({taskId:"new-task"})];fail=true;
      expect(await p.query("project.snapshot",{})).toEqual([]);
      expect(await p.query("project.facts",{fresh:true})).toEqual([]);
    } finally {stop();}
  });
  it("isolates a refused project and a slow store without hiding any registry entries", async () => {
    const { s, p } = fixture();
    s.answers["projects.list"] = () => ["p", "broken", "slow"].map(id => ({ projectId: id, displayName: id, canonicalRoot: "/" + id, activation: {} }));
    const original = s.answers["native.query"]!;
    let finish!: (value: unknown) => void;
    s.answers["native.query"] = raw => {
      const id = (raw as { projectId: string }).projectId;
      if (id === "broken") throw new Error("storage_format_unsupported");
      if (id === "slow") return new Promise(resolve => { finish = resolve; });
      return original(raw);
    };
    const updates: PortUpdate[] = [], stop = p.subscribe(u => updates.push(u));
    await p.query("capabilities.get", {});
    const reading = p.query("project.snapshot", {});
    const result = await Promise.race([reading, new Promise<"blocked">(resolve => setTimeout(() => resolve("blocked"), 500))]);
    expect(result).not.toBe("blocked");
    expect(Array.isArray(result) && result.some(u => u.scope === "project" && u.slice.project.id === "p")).toBe(true);
    expect(updates.filter(u => u.kind === "project_state").map(u => u.kind === "project_state" && u.project.id)).toEqual(expect.arrayContaining(["p", "broken", "slow"]));
    finish({ ...(original({ operation: "tasks.page" }) as object), projectId: "slow" });
    await vi.waitFor(() => expect(updates.some(u => u.kind === "projection" && u.scope === "project" && u.slice.project.id === "slow")).toBe(true));
    stop();
  });
  it("shows every registered project and stays connected while a store fails, then recovers automatically", async () => {
    const { s, p } = fixture();
    s.answers["projects.list"] = () => ["p", "broken"].map(id => ({projectId:id,displayName:id,canonicalRoot:"/" + id,activation:{}}));
    const original = s.answers["native.query"]!;
    const message = "This format-11 project needs an upgrade before it can be read here.";
    const reason = "legacy_storage_migration_required";
    const refusal = () => { throw new Error(`refused:project_store:${reason}: ${message}`); };
    s.answers["service.capabilities"] = () => ({methods:["native.query", "projects.activate"]});
    s.answers["projects.activate"] = refusal;
    let failed = true;
    s.answers["native.query"] = raw => {
      const id = (raw as {projectId:string}).projectId;
      if (id === "broken" && failed) refusal();
      return { ...(original(raw) as object), projectId:id };
    };
    let dispose!: () => void;
    const store = createRoot(d => {dispose=d;return createWorkspaceStore(p);});
    try {
      await vi.waitFor(() => expect(store.ready()).toBe(true));
      expect(store.connection().state).toBe("connected");
      expect(store.state().projects.map(p => p.id)).toEqual(["p", "broken"]);
      expect(store.state().projects.find(p => p.id === "broken")?.issue).toBeNull(); // One startup failure is retried quietly.
      expect(store.state().tasks.some(t => t.projectId === "p")).toBe(true);
      expect(await p.command({requestId:"activate-broken",operation:"activate_project",expectedVersion:null,
        payload:{type:"activate_project",projectId:"broken"}})).toMatchObject({
        ok:false,outcome:reason,message
      });
      await p.query("project.snapshot", {full:true});
      await p.query("project.snapshot", {full:true});
      await vi.waitFor(() => expect(store.state().projects.find(p => p.id === "broken")).toMatchObject({
        issue:message, issueCause:reason
      }));
      failed = false;
      await vi.waitFor(() => expect(store.state().projects.find(p => p.id === "broken")?.issue).toBeNull(), {timeout:6000});
      expect(store.connection().state).toBe("connected");
    } finally {dispose();}
  });
  it("uses native task/history reads and patches one task without refetching snapshots or machine facts", async () => {
    const { s, p } = fixture();
    await p.query("capabilities.get", {});
    await p.query("project.snapshot", {});
    const updates: PortUpdate[] = [], stop = p.subscribe(u => updates.push(u));
    s.send({ type: "native_delta", ...delta() });
    await vi.waitFor(() => expect(updates.some(u => u.kind === "delta")).toBe(true));
    const update = updates.find(u => u.kind === "delta")!;
    if (update.kind !== "delta" || update.update.scope !== "project") throw new Error("missing project patch");
    expect(update.update.version).toBe("5");
    expect(update.update.slice.tasks[0]).toMatchObject({ id: "task-1", description: "Updated", observedPhase: "recovery" });
    expect(s.calls.filter(c => c.operation === "native.query" && (c.message as any).operation === "tasks.page")).toHaveLength(1);
    expect(s.calls.filter(c => c.operation === "native.query" && (c.message as any).operation === "history.page")).toHaveLength(1);
    expect(s.calls.some(c => ["project.snapshot", "tasks.list", "machine.call"].includes(c.operation))).toBe(false);
    const before = updates.length;
    s.send({ type: "native_delta", ...delta() });
    await new Promise(resolve => setTimeout(resolve, 10));
    expect(updates).toHaveLength(before);
    stop();
  });
  it("treats details superseded by a Working task's events as obsolete, not a read failure, and rereads them", async () => {
    const { s, p } = fixture();
    s.answers["service.capabilities"] = () => ({ methods: ["native.query", "native.board"] });
    s.answers["native.board"] = () => ({ protocol: { major: 2, mapping: 2 }, projectId: "p", generation: "generation", version: "2", highWatermark: "3",
      data: { type: "board", value: { items: [wireTask({ executionId: "execution" })], nextCursor: null, highWatermark: "3" } } });
    // A running task appends events while its conversation is being read.
    let hold = true;
    const pending: (() => void)[] = [];
    const history = { protocol: { major: 2, mapping: 2 }, projectId: "p", generation: "generation", version: "2", highWatermark: "3",
      data: { type: "history", value: { items: [], nextCursor: null, highWatermark: "0" } } };
    s.answers["native.query"] = () => hold ? new Promise(resolve => pending.push(() => resolve(history))) : history;
    const updates: PortUpdate[] = [], stop = p.subscribe(u => updates.push(u));
    let high = 3, version = 1;
    const advance = async () => {
      s.send({ type: "native_delta", ...delta(String(version + 1), String(high + 1), String(high + 1), String(version)) });
      high++; version++;
      await vi.waitFor(() => expect(updates.some(u => u.kind === "delta" && u.update.version === String(high))).toBe(true));
    };
    try {
      await p.query("capabilities.get", {});
      await p.query("project.snapshot", {});
      for (let round = 0; round < 3; round++) {
        if (round) await advance();
        await vi.waitFor(() => expect(pending.length).toBeGreaterThan(0));
        await advance();
        for (const release of pending.splice(0)) release();
        await new Promise(resolve => setTimeout(resolve, 10));
      }
      expect(updates.some(u => u.kind === "project_state" && u.issue)).toBe(false);
      hold = false;
      for (const release of pending.splice(0)) release();
      await vi.waitFor(() => expect(updates.some(u => u.kind === "enrichment" && u.update.version === String(high))).toBe(true));
      expect(updates.some(u => u.kind === "project_state" && u.issue)).toBe(false);
    } finally { stop(); }
  });
  it("says on the task when its conversation could not be read, with the reason, and reads it again on Retry", async () => {
    const { s, p } = fixture();
    s.answers["service.capabilities"] = () => ({ methods: ["native.query", "native.board"] });
    const board = { protocol: { major: 2, mapping: 2 }, projectId: "p", generation: "generation", version: "2", highWatermark: "3",
      data: { type: "board", value: { items: [wireTask({ executionId: "execution", phase: "paused", executionState: "paused", runtimeState: "PAUSED" })], nextCursor: null, highWatermark: "3" } } };
    s.answers["native.board"] = () => board;
    let failing = true;
    s.answers["native.query"] = () => {
      if (failing) throw new Error("storage_unavailable");
      return { ...board, data: { type: "history", value: { items: [historyRow(0, "assistant", "Kept from before the pause")], nextCursor: null, highWatermark: "1" } } };
    };
    let dispose!: () => void;
    const store = createRoot(d => { dispose = d; return createWorkspaceStore(p); });
    const session = () => store.state().sessions.find(x => x.taskId === "task-1");
    try {
      await vi.waitFor(() => expect(session()?.historyRead).toEqual({ state: "failed", reason: "storage_unavailable" }));
      // The failure belongs to that task's conversation; the board and the project are not reported unreadable.
      expect(store.state().projects.find(x => x.id === "p")?.issue).toBeNull();
      failing = false;
      store.retryHistory("task-1");
      await vi.waitFor(() => expect(session()?.timeline.some(e => e.kind === "cortex" && e.text === "Kept from before the pause")).toBe(true));
      expect(session()?.historyRead).toBeUndefined();
    } finally { dispose(); }
  });
  it("shows the retained conversation of a task whose run continues after a service restart, while its board keeps moving", async () => {
    const { s, p } = fixture();
    s.answers["service.capabilities"] = () => ({ methods: ["native.query", "native.board"] });
    // A fresh service generation: the run was recovered and works on, so the watermark moves throughout every read.
    let high = 3, version = 1;
    const running = (v: number) => wireTask({ version: String(v), executionId: "execution", phase: "execution", executionState: "running", runtimeState: "EXECUTING" });
    s.answers["native.board"] = () => ({ protocol: { major: 2, mapping: 2 }, projectId: "p", generation: "restarted", version: "2", highWatermark: String(high),
      data: { type: "board", value: { items: [running(version)], nextCursor: null, highWatermark: String(high) } } });
    const rows = Array.from({ length: 3 }, (_, i) => historyRow(i, i === 1 ? "tool" : "assistant", i === 1 ? "Read src/login.ts" : `Said before the restart ${i}`));
    s.answers["native.query"] = () => new Promise(resolve => setTimeout(() => resolve({ protocol: { major: 2, mapping: 2 }, projectId: "p", generation: "restarted", version: "2", highWatermark: String(high),
      data: { type: "history", value: { items: rows, nextCursor: null, highWatermark: "3" } } }), 30));
    const updates: PortUpdate[] = [], stop = p.subscribe(u => updates.push(u));
    const timeline = (u: PortUpdate) => u.kind === "enrichment" && u.update.scope === "project" ? u.update.slice.sessions.find(x => x.taskId === "task-1") : undefined;
    const events = setInterval(() => {
      const task = running(version + 1);
      s.send({ type: "native_delta", projectId: "p", batch: { generation: "restarted", streamKey: { scope: "project", project_id: "p" }, fromSequence: String(high + 1), throughSequence: String(high + 1),
        changes: [{ kind: "task", id: "task-1", baseVersion: String(version), newVersion: String(version + 1), operation: { type: "upsert", data: { kind: "task_projection", data: task } } }] } });
      high++; version++;
    }, 5);
    try {
      await p.query("capabilities.get", {});
      const first = await p.query("project.snapshot", {});
      const opened = first[0]?.scope === "project" ? first[0].slice.sessions.find(x => x.taskId === "task-1") : undefined;
      // Before the first read completes the page says it is loading, never an empty finished conversation.
      expect(opened?.historyRead).toEqual({ state: "loading" });
      await vi.waitFor(() => expect(updates.some(u => timeline(u)?.timeline.some(e => e.kind === "cortex" && e.text.includes("Said before the restart 0")))).toBe(true), { timeout: 2000 });
      expect(updates.some(u => u.kind === "project_state" && u.issue)).toBe(false);
    } finally { clearInterval(events); stop(); }
  });
  it("retains confirmed content and requests resnapshot on a gap, generation change or base conflict", async () => {
    for (const bad of [{...delta(),batch:{...delta().batch,changes:[{kind:"project" as const,id:"p",baseVersion:"0",newVersion:"1",operation:{type:"invalidate" as const}}]}}, delta("2", "5", "6"), { ...delta(), batch: { ...delta().batch, generation: "restarted" } }, delta("3", "4", "5", "2")]) {
      const { s, p } = fixture();
      await p.query("capabilities.get", {}); await p.query("project.snapshot", {});
      const updates: PortUpdate[] = [], stop = p.subscribe(u => updates.push(u));
      s.send({ type: "native_delta", ...bad });
      await vi.waitFor(() => expect(updates).toContainEqual({ kind: "refresh" }));
      expect(updates.some(u => u.kind === "delta")).toBe(false); stop();
    }
  });
  it("rejects malformed or identity-conflicting batches before applying anything", () => {
    expect(nativeDelta(delta())).not.toBeNull();
    expect(nativeDelta({ ...delta(), projectId: "foreign" })).toBeNull();
    expect(nativeDelta({ ...delta(), batch: { ...delta().batch, throughSequence: 9007199254740992 } })).toBeNull();
    const bad = delta(); bad.batch.changes[0].id = "different";
    expect(nativeDelta(bad)).toBeNull();
  });
});
