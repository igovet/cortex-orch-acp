import { createRoot } from "solid-js";
import { afterEach, describe, expect, it, vi } from "vitest";
import { emptyWorkspace, type WorkspaceState } from "@cortex/workspace-domain";
import { createWorkspaceStore } from "../src/store";
import { flush, projection, scriptedPort, withTask } from "./support";

let dispose: (() => void) | undefined;
afterEach(() => { dispose?.(); dispose = undefined; vi.useRealTimers(); });
const storeOn = (port: Parameters<typeof createWorkspaceStore>[0], options: Parameters<typeof createWorkspaceStore>[1] = {}) => createRoot(d => { dispose = d; return createWorkspaceStore(port, options); });
const titles = (state: WorkspaceState) => state.tasks.map(t => t.title);

describe("workspace store", () => {
  it("installs a confirmed projection and starts empty", async () => {
    const s = scriptedPort();
    const store = storeOn(s.port);
    await flush();
    expect(store.state().projects).toEqual([]);
    s.emit(projection("1", withTask(emptyWorkspace(), "First")));
    expect(titles(store.state())).toEqual(["First"]);
    expect(store.version("workspace")).toBe("1");
  });

  it("a command receipt changes nothing on screen until a confirmed projection arrives", async () => {
    const s = scriptedPort();
    const store = storeOn(s.port);
    await flush();
    s.emit(projection("1", withTask(emptyWorkspace(), "Before")));
    const pending = store.dispatch({ type: "create_task", projectId: "p", description: "Next", checkout: { kind: "main" } });
    expect(s.commands).toHaveLength(1);
    expect(titles(store.state())).toEqual(["Before"]);
    s.receipt(true, "t2");
    const receipt = await pending;
    expect(receipt).toMatchObject({ ok: true, entityId: "t2" });
    expect(store.receipt()?.ok).toBe(true);
    expect(titles(store.state())).toEqual(["Before"]);
    s.emit(projection("2", withTask(emptyWorkspace(), "After")));
    expect(titles(store.state())).toEqual(["After"]);
  });

  it("wraps a command in an envelope with its own request identity and the installed version", async () => {
    const s = scriptedPort();
    const store = storeOn(s.port);
    await flush();
    s.emit(projection("7", emptyWorkspace()));
    void store.dispatch({ type: "start_task", taskId: "t" });
    void store.dispatch({ type: "start_task", taskId: "u" });
    const [first, second] = s.commands;
    expect(first).toMatchObject({ operation: "start_task", expectedVersion: "7", payload: { type: "start_task", taskId: "t" } });
    expect(first.requestId).not.toBe(second.requestId);
    expect(first.requestId.length).toBeLessThanOrEqual(128);
  });

  it("runs one action once: a repeat joins the one under way, and it can be sent again after", async () => {
    const s = scriptedPort();
    const store = storeOn(s.port);
    await flush();
    const first = store.dispatch({ type: "start_task", taskId: "t", now: 1 }), repeat = store.dispatch({ type: "start_task", taskId: "t", now: 2 });
    expect(s.commands).toHaveLength(1);
    s.receipt(); await flush(); await flush();
    expect((await first).ok).toBe(true);
    expect(await repeat).toEqual(await first);
    void store.dispatch({ type: "start_task", taskId: "t", now: 3 });
    expect(s.commands).toHaveLength(2);
  });

  it("changes nothing when a read says nothing new, and keeps every untouched record when it does", async () => {
    const s = scriptedPort({ capabilities: { everything: true, operations: [] } });
    const store = storeOn(s.port);
    await flush();
    const state = withTask(emptyWorkspace(), "Fix");
    s.emit(projection("1", state));
    const held = store.state();
    s.emit(projection("2", structuredClone(state)));
    expect(store.state()).toBe(held);
    const changed = structuredClone(state);
    changed.projects[0].name = "Renamed";
    s.emit(projection("3", changed));
    expect(store.state()).not.toBe(held);
    expect(store.state().tasks[0]).toBe(held.tasks[0]);
  });

  it("rejects a stale snapshot by its version, and an equal one too", async () => {
    const s = scriptedPort();
    const store = storeOn(s.port);
    await flush();
    s.emit(projection("5", withTask(emptyWorkspace(), "Current")));
    s.emit(projection("4", withTask(emptyWorkspace(), "Stale")));
    s.emit(projection("5", withTask(emptyWorkspace(), "Duplicate")));
    expect(titles(store.state())).toEqual(["Current"]);
    expect(store.rejected()).toBe(2);
    expect(store.version("workspace")).toBe("5");
    s.emit(projection("9007199254740993", withTask(emptyWorkspace(), "Newer")));
    s.emit(projection("9007199254740992", withTask(emptyWorkspace(), "Older")));
    expect(titles(store.state())).toEqual(["Newer"]);
  });

  it("orders each project on its own", async () => {
    const s = scriptedPort();
    const store = storeOn(s.port);
    await flush();
    const slice = (id: string) => ({ project: { id, machineId: "local", name: id, repository: "/x", openBranch: "", defaultBranch: "" }, tasks: [], sessions: [] });
    s.emit({ kind: "projection", version: "9", scope: "project", slice: slice("a") });
    s.emit({ kind: "projection", version: "1", scope: "project", slice: slice("b") });
    expect(store.state().projects.map(p => p.id)).toEqual(["a", "b"]);
    expect(store.rejected()).toBe(0);
  });

  it("restores a removed project at the same version without accepting late removed snapshots", async () => {
    const s = scriptedPort(), store = storeOn(s.port);
    await flush();
    const slice = { project: { id: "a", machineId: "local", name: "Restored", repository: "/x", openBranch: "", defaultBranch: "" }, tasks: [], sessions: [] };
    s.emit({ kind: "registry", projectIds: ["a"] });
    s.emit({ kind: "projection", version: "9", scope: "project", slice });
    s.emit({ kind: "registry", projectIds: [] });
    expect(store.state().projects).toEqual([]);
    s.emit({ kind: "projection", version: "10", scope: "project", slice });
    expect(store.state().projects).toEqual([]);
    s.emit({ kind: "registry", projectIds: ["a"] });
    s.emit({ kind: "projection", version: "9", scope: "project", slice });
    expect(store.state().projects.map(p => p.name)).toEqual(["Restored"]);
  });

  it("refuses a command the machine does not offer, without reaching the port", async () => {
    const s = scriptedPort({ capabilities: { everything: false, operations: ["tasks.create"] } });
    const store = storeOn(s.port);
    await flush();
    const receipt = await store.dispatch({ type: "stop_task", taskId: "t", confirmed: true });
    expect(receipt).toMatchObject({ ok: false, outcome: "unsupported" });
    expect(s.commands).toEqual([]);
    expect(store.can("tasks.create")).toBe(true);
    expect(store.can("executions.cancel")).toBe(false);
  });

  it("offers nothing until the machine has said what it offers", () => {
    const store = storeOn(scriptedPort().port);
    expect(store.can("tasks.create")).toBe(false);
  });

  it("a failed port call becomes a visible failed receipt", async () => {
    const s = scriptedPort();
    const store = storeOn(s.port);
    await flush();
    s.port.command = () => Promise.reject(new Error("socket closed"));
    const receipt = await store.dispatch({ type: "start_task", taskId: "t" }) ;
    expect(receipt).toMatchObject({ ok: false, outcome: "unknown_outcome" });
    expect(store.receipt()?.ok).toBe(false);
  });
});

describe("a real backend failure", () => {
  it("shows the failure and never selects demo data", async () => {
    const s = scriptedPort({ failQuery: new Error("service down") });
    const store = storeOn(s.port);
    await flush();
    expect(store.mode).toBe("real");
    expect(store.connection()).toMatchObject({ state: "unavailable", reason: "service down" });
    expect(store.state().projects).toEqual([]);
    expect(store.state().tasks).toEqual([]);
    expect(JSON.stringify(store.state())).not.toMatch(/mock/i);
    expect(store.state().machines[0].connection).toBe("disconnected");
    expect(store.can("tasks.create")).toBe(false);
  });

  it("keeps the last confirmed data, marked offline, when the connection drops", async () => {
    const s = scriptedPort();
    const store = storeOn(s.port);
    await flush();
    s.emit(projection("1", withTask(emptyWorkspace(), "Kept")));
    s.emit({ kind: "connection", connection: { state: "disconnected", reason: "closed", at: 5 } });
    expect(titles(store.state())).toEqual(["Kept"]);
    expect(store.state().machines[0].connection).toBe("disconnected");
    s.emit({ kind: "connection", connection: { state: "connected", reason: null, at: 6 } });
    expect(store.state().machines[0].connection).toBe("connected");
  });

  it("rereads everything when the connection comes back", async () => {
    const s = scriptedPort();
    const store = storeOn(s.port);
    await flush();
    s.emit({ kind: "connection", connection: { state: "disconnected", reason: "closed", at: 5 } });
    const asked: string[] = [], query = s.port.query.bind(s.port);
    s.port.query = ((operation: Parameters<typeof query>[0], parameters: never) => { asked.push(operation); return query(operation, parameters); }) as typeof s.port.query;
    s.emit({ kind: "connection", connection: { state: "connected", reason: null, at: 6 } });
    await flush();
    expect(asked).toEqual(["capabilities.get", "project.snapshot", "project.facts", "machine.facts"]);
    expect(store.connection().state).toBe("connected");
  });

  it("retries on request, and recovers without a restart", async () => {
    const s = scriptedPort({ failQuery: new Error("service down") });
    const store = storeOn(s.port);
    await flush();
    s.port.query = (async (operation: string) => operation === "capabilities.get" ? { everything: false, operations: ["tasks.create"] } : []) as typeof s.port.query;
    await store.reload();
    expect(store.connection().state).toBe("connected");
    expect(store.can("tasks.create")).toBe(true);
  });

  it("removes a confirmed deletion before a slow refresh, and fences late snapshots", async () => {
    const s = scriptedPort({capabilities:{everything:true,operations:[]}});
    const state = withTask(emptyWorkspace(),"Delete");
    let finish!: (r: import("@cortex/workspace-domain").CommandReceipt) => void;
    s.port.command = () => new Promise(resolve => { finish = resolve; });
    const store = storeOn(s.port,{initial:{state,version:"1",capabilities:{everything:true,operations:[]}}});
    s.port.query = (() => new Promise(() => {})) as typeof s.port.query;
    const pending = store.dispatch({type:"delete_sprint",projectId:"p",sprintId:"s",confirmed:true});
    expect(store.state().tasks).toHaveLength(1);
    finish({ok:true,requestId:"r",outcome:"recorded",message:"Deleted",deletedTaskIds:["t"]});
    expect((await pending).ok).toBe(true);
    expect(store.state().tasks).toEqual([]);
    s.emit(projection("2",state));
    expect(store.state().tasks).toEqual([]);
  });

  it("settles an acknowledgement without waiting for facts and reads only its project", async () => {
    const s = scriptedPort({capabilities:{everything:true,operations:[]}});
    const state = withTask(emptyWorkspace(),"Before");
    const store = storeOn(s.port,{initial:{state,version:"1",capabilities:{everything:true,operations:[]}}});
    const asked: {operation:string;parameters:unknown}[] = [];
    let release!: (facts: never[]) => void;
    s.port.query = (async (operation: string, parameters: unknown) => {
      asked.push({operation,parameters});
      return operation === "project.facts" ? new Promise<never[]>(resolve => {release=resolve;}) : [];
    }) as typeof s.port.query;
    const before = performance.now();
    const pending = store.dispatch({type:"rename_task",taskId:"t",title:"Confirmed"});
    s.receipt();
    expect((await pending).ok).toBe(true);
    const elapsed = performance.now()-before;
    expect(elapsed).toBeLessThan(100);
    console.info("Receipt to confirmed local rename (fake transport, unresolved facts), ms:", elapsed);
    expect(store.state().tasks[0].title).toBe("Confirmed");
    expect(store.syncing()).toBe(true);
    await flush();
    expect(asked).toEqual([
      {operation:"project.snapshot",parameters:{projectId:"p"}},
      {operation:"project.facts",parameters:{projectId:"p"}},
    ]);
    release([]); await flush();
    expect(store.syncing()).toBe(false);
  });

  it.each(["project.snapshot", "project.facts"])("reconciles facts suppressed during another project's %s even when later polls are unchanged", async heldOperation => {
    const s = scriptedPort({capabilities:{everything:true,operations:[]}});
    const state = withTask(emptyWorkspace(), "A old");
    state.projects.push({...state.projects[0], id:"q", name:"Q"});
    state.tasks.push({...state.tasks[0], id:"u", projectId:"q", title:"B old"});
    const store = storeOn(s.port,{initial:{state,version:"1",capabilities:{everything:true,operations:[]}}});
    const facts = (projectId: string, targetBranch: string) => ({
      projectId, openBranch:"main", defaultBranch:"main", repo:null, workers:[], reviews:[], sprints:[],
      overlays:{[projectId === "p" ? "t" : "u"]:{targetBranch}},
    });
    let releaseA!: () => void, releaseB!: () => void;
    const asked: {operation:string;projectId?:string}[] = [];
    s.port.query = (async (operation: string, parameters: {projectId?:string}) => {
      asked.push({operation,projectId:parameters.projectId});
      if (parameters.projectId === "p" && operation === heldOperation)
        await new Promise<void>(resolve => { releaseA=resolve; });
      if (parameters.projectId === "q" && operation === "project.snapshot")
        await new Promise<void>(resolve => { releaseB=resolve; });
      return operation === "project.facts" ? [facts(parameters.projectId!, "fresh")] : [];
    }) as typeof s.port.query;
    const command = store.dispatch({type:"rename_task",taskId:"t",title:"A new"});
    s.receipt(); await command; await flush();

    // The real port remembers sent facts before notifying; an unchanged poll emits nothing.
    let sent = "";
    const pollB = (branch: string) => {
      if (sent === branch) return;
      sent = branch;
      s.emit({kind:"facts",facts:facts("q",branch)});
    };
    pollB("observed");
    pollB("observed-again");
    pollB("observed-again");
    expect(store.state().tasks.find(t => t.id === "u")?.targetBranch).toBeNull();
    releaseA(); await flush();
    expect(asked.filter(r => r.operation === "project.snapshot").map(r => r.projectId)).toEqual(["p","q"]);
    expect(store.syncing()).toBe(true);
    releaseB(); await flush();
    pollB("observed-again");
    expect(store.state().tasks.find(t => t.id === "u")?.targetBranch).toBe("fresh");
    expect(store.state().tasks.find(t => t.id === "t")?.title).toBe("A new");
    expect(asked).toHaveLength(4);
    expect(store.syncing()).toBe(false);
  });

  it.each([false, true])("keeps suppressed fact scopes when an event refresh is also queued (event first: %s)", async eventFirst => {
    const s = scriptedPort({capabilities:{everything:true,operations:[]}});
    const state = withTask(emptyWorkspace(), "A");
    state.projects.push({...state.projects[0], id:"q", name:"Q"});
    state.tasks.push({...state.tasks[0], id:"u", projectId:"q"});
    const store = storeOn(s.port,{initial:{state,version:"1",capabilities:{everything:true,operations:[]}}});
    const facts = {projectId:"q",openBranch:"main",defaultBranch:"main",repo:null,workers:[],reviews:[],sprints:[],overlays:{u:{targetBranch:"fresh"}}};
    let release!: () => void, full = false;
    const scopes: unknown[] = [];
    s.port.query = (async (operation: string, parameters: {projectId?:string;full?:boolean}) => {
      if (operation === "capabilities.get") return {everything:true,operations:[]};
      if (operation === "project.snapshot") {
        scopes.push(parameters);
        if (parameters.projectId === "p") await new Promise<void>(resolve => { release=resolve; });
        else full = !!parameters.full;
      }
      // Like the real adapter, an event-scoped unscoped read need not include Q.
      return operation === "project.facts" && full ? [facts] : [];
    }) as typeof s.port.query;
    const command = store.dispatch({type:"rename_task",taskId:"t",title:"A new"});
    s.receipt(); await command;
    if (eventFirst) s.emit({kind:"refresh"});
    s.emit({kind:"facts",facts});
    if (!eventFirst) s.emit({kind:"refresh"});
    release(); await flush();
    expect(scopes).toEqual([{projectId:"p"},{full:true}]);
    expect(store.state().tasks.find(t => t.id === "u")?.targetBranch).toBe("fresh");
    expect(store.syncing()).toBe(false);
  });

  it("retries a failed dirty-project read without replaying the acknowledged command", async () => {
    vi.useFakeTimers();
    const s = scriptedPort({capabilities:{everything:true,operations:[]}});
    const state = withTask(emptyWorkspace(), "A");
    state.projects.push({...state.projects[0], id:"q", name:"Q"});
    state.tasks.push({...state.tasks[0], id:"u", projectId:"q"});
    const store = storeOn(s.port,{initial:{state,version:"1",capabilities:{everything:true,operations:[]}}});
    const facts = {projectId:"q",openBranch:"main",defaultBranch:"main",repo:null,workers:[],reviews:[],sprints:[],overlays:{u:{targetBranch:"fresh"}}};
    let release!: () => void, reads = 0;
    s.port.query = (async (operation: string, parameters: {projectId?:string}) => {
      if (operation === "project.snapshot" && parameters.projectId === "p")
        await new Promise<void>(resolve => { release=resolve; });
      if (operation === "project.facts" && parameters.projectId === "q") {
        if (++reads === 1) throw Error("facts unavailable");
        return [facts];
      }
      return [];
    }) as typeof s.port.query;
    const command = store.dispatch({type:"rename_task",taskId:"t",title:"A new"});
    s.receipt(); await command;
    s.emit({kind:"facts",facts});
    release(); await vi.advanceTimersByTimeAsync(0);
    expect(store.readIssue()).toBe("facts unavailable");
    expect(store.state().tasks.find(t => t.id === "u")?.targetBranch).toBeNull();
    await vi.advanceTimersByTimeAsync(3000);
    expect(reads).toBe(2);
    expect(store.state().tasks.find(t => t.id === "u")?.targetBranch).toBe("fresh");
    expect(store.readIssue()).toBeNull();
    expect(s.commands).toHaveLength(1);
  });

  it("does a new scoped read after a pre-command poll, never installing that old reply", async () => {
    const s = scriptedPort({capabilities:{everything:true,operations:[]}});
    const state = withTask(emptyWorkspace(),"Before");
    const store = storeOn(s.port,{initial:{state,version:"1",capabilities:{everything:true,operations:[]}}});
    let finish!: (updates: ReturnType<typeof projection>[]) => void;
    let reads = 0;
    s.port.query = (async (operation: string) => {
      if (operation === "capabilities.get") return {everything:true,operations:[]};
      if (operation === "project.snapshot") {
        reads++;
        return reads === 1 ? new Promise<ReturnType<typeof projection>[]>(resolve => {finish=resolve;}) : [projection("3",withTask(emptyWorkspace(),"Confirmed"))];
      }
      return [];
    }) as typeof s.port.query;
    const reload = store.reload(); await flush();
    const pending = store.dispatch({type:"rename_task",taskId:"t",title:"Confirmed"});
    s.receipt(); await pending;
    finish([projection("2",state)]);
    await reload;
    expect(store.state().tasks[0].title).toBe("Confirmed");
    await flush();
    expect(reads).toBe(2);
    expect(store.syncing()).toBe(false);
  });

  it.each([false, true])("requeues all projects when an unscoped read is fenced (failed read: %s)", async failed => {
    const s = scriptedPort({capabilities:{everything:true,operations:[]}});
    const state = withTask(emptyWorkspace(), "A old");
    state.projects.push({...state.projects[0], id:"q", name:"Q"});
    state.tasks.push({...state.tasks[0], id:"u", projectId:"q", title:"B old"});
    const store = storeOn(s.port,{initial:{state,version:"1",capabilities:{everything:true,operations:[]}}});
    let finish!: () => void, release!: () => void;
    const scopes: unknown[] = [];
    s.port.query = (async (operation: string, parameters: unknown) => {
      if (operation === "capabilities.get") return {everything:true,operations:[]};
      if (operation === "project.snapshot") {
        scopes.push(parameters);
        if (scopes.length === 1) return new Promise<ReturnType<typeof projection>[]>((resolve, reject) => {
          finish = () => failed ? reject(Error("old read failed")) : resolve([projection("2",state)]);
        });
        return new Promise<ReturnType<typeof projection>[]>(resolve => { release = () => {
          const current = structuredClone(state);
          current.tasks[0].title = "A new"; current.tasks[1].title = "B new";
          resolve([projection("3",current)]);
        }; });
      }
      if (operation === "project.facts") return [{projectId:"q", openBranch:"main", defaultBranch:"main", repo:null, workers:[], reviews:[], overlays:{u:{targetBranch:"new-target"}}, sprints:[]}];
      return [];
    }) as typeof s.port.query;
    const reload = store.reload(); await flush();
    const command = store.dispatch({type:"rename_task",taskId:"t",title:"A new"});
    s.receipt(); await command;
    finish(); await reload; await flush();
    expect(store.syncing()).toBe(true);
    expect(store.state().tasks.find(t => t.id === "u")?.title).toBe("B old");
    expect(scopes).toEqual([{}, {full:true}]);
    release(); await flush();
    expect(titles(store.state())).toEqual(["A new", "B new"]);
    expect(store.state().tasks.find(t => t.id === "u")?.targetBranch).toBe("new-target");
    expect(store.syncing()).toBe(false);
  });

  it("returns refusals and unknown outcomes immediately without applying a requested rename", async () => {
    const s = scriptedPort({capabilities:{everything:true,operations:[]}});
    const store = storeOn(s.port,{initial:{state:withTask(emptyWorkspace(),"Before"),version:"1",capabilities:{everything:true,operations:[]}}});
    s.port.query = (() => new Promise(() => {})) as typeof s.port.query;
    const pending = store.dispatch({type:"rename_task",taskId:"t",title:"Not confirmed"});
    s.receipt(false);
    expect((await pending).ok).toBe(false);
    expect(store.state().tasks[0].title).toBe("Before");
    s.port.command = () => Promise.reject(Error("lost"));
    expect((await store.dispatch({type:"rename_task",taskId:"t",title:"Unknown"})).outcome).toBe("unknown_outcome");
    expect(store.state().tasks[0].title).toBe("Before");
  });

  it("refreshes registry and capabilities after project activation or removal", async () => {
    const s = scriptedPort({capabilities:{everything:true,operations:[]}});
    const store = storeOn(s.port,{initial:{state:withTask(emptyWorkspace(),"Before"),version:"1",capabilities:{everything:true,operations:[]}}});
    const asked: string[] = [], query = s.port.query;
    s.port.query = ((operation: Parameters<typeof query>[0], parameters: never) => {asked.push(operation);return query(operation,parameters);}) as typeof query;
    const activate = store.dispatch({type:"activate_project",projectId:"p"});
    s.receipt(); await activate; await flush();
    expect(asked).toContain("capabilities.get");
    asked.length=0;
    const remove = store.dispatch({type:"remove_project",projectId:"p",confirmed:true});
    s.receipt(); await remove; await flush();
    expect(asked).toContain("capabilities.get");
  });

  it("stops listening when the screen goes away", async () => {
    const s = scriptedPort();
    storeOn(s.port);
    expect(s.listeners.size).toBe(1);
    dispose?.(); dispose = undefined;
    expect(s.listeners.size).toBe(0);
  });
});
