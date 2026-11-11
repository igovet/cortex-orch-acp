import { createRoot } from "solid-js";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createRealPort, FACTS_POLL_MS, FACTS_POLL_MAX_MS, type ProjectLocator } from "../src/port/real";
import { createWorkspaceStore, PUSH_GAP_MS } from "../src/store";
import { capabilitiesWire, scriptedPort, scriptedTransport, snapshot, wireTask } from "./support";

let dispose: () => void = () => {};
beforeEach(() => { vi.useFakeTimers(); });
afterEach(() => { dispose(); vi.useRealTimers(); delete (document as { hidden?: boolean }).hidden; });

const settled = async (ms = 0) => { await vi.advanceTimersByTimeAsync(ms); };

describe("an idle store", () => {
  it("starts no timer of its own", async () => {
    const s = scriptedPort();
    createRoot(d => { dispose = d; return createWorkspaceStore(s.port); });
    await settled(10);
    expect(vi.getTimerCount()).toBe(0);
  });

  it("does not rewrite state when a read finds nothing new", async () => {
    const s = scriptedPort();
    const store = createRoot(d => { dispose = d; return createWorkspaceStore(s.port); });
    await settled(10);
    const state = store.state(), index = store.index(), connection = store.connection();
    s.emit({ kind: "refresh" });
    await settled(PUSH_GAP_MS);
    expect(store.state()).toBe(state);
    expect(store.index()).toBe(index);
    expect(store.connection()).toBe(connection);
  });

  it("answers a burst of machine events with one refresh, then one for the rest", async () => {
    const s = scriptedPort();
    const query = vi.spyOn(s.port, "query");
    createRoot(d => { dispose = d; return createWorkspaceStore(s.port); });
    await settled(10);
    query.mockClear();
    const reads = () => query.mock.calls.filter(([operation]) => operation === "project.snapshot").length;
    for (let n = 0; n < 20; n++) s.emit({ kind: "refresh" });
    await settled(0);
    expect(reads()).toBe(1);
    for (let n = 0; n < 20; n++) s.emit({ kind: "refresh" });
    await settled(0);
    expect(reads()).toBe(1);
    await settled(PUSH_GAP_MS);
    expect(reads()).toBe(2);
    await settled(PUSH_GAP_MS * 5);
    expect(reads()).toBe(2);
    expect(vi.getTimerCount()).toBe(0);
  });
});

describe("the machine poll", () => {
  const project: ProjectLocator = { projectId: "p", name: "Project", repository: "/p", openBranch: "main", defaultBranch: "main" };
  const machine = () => {
    const s = scriptedTransport({
      "service.capabilities": () => ({ methods: ["machine.call"], machine: true }),
      "projects.list": () => [{ projectId: "p", displayName: "Project", canonicalRoot: "/p", activation: { actorId: "a" } }],
      "capabilities.get": () => capabilitiesWire(["capabilities.get", "project.snapshot"]),
      "project.snapshot": () => snapshot("4", []),
      "machine.call": message => ({ ok: true, result: (message as { op: string }).op === "product.read" ? { tasks: {}, sprints: [] } : null }),
    });
    return s;
  };
  const opened = async (s: ReturnType<typeof machine>) => {
    const port = createRealPort({ transport: s.transport, actorId: "a", registry: true, projects: [project] });
    await port.query("capabilities.get", {});
    return port;
  };
  const productReads = (s: ReturnType<typeof machine>) => s.calls.filter(c => c.operation === "machine.call" && (c.message as { op: string }).op === "product.facts").length;

  it("reads nothing while the window is hidden", async () => {
    Object.defineProperty(document, "hidden", { configurable: true, get: () => true });
    const s = machine();
    const stop = (await opened(s)).subscribe(() => {});
    await settled(FACTS_POLL_MAX_MS * 3);
    expect(productReads(s)).toBe(0);
    expect(vi.getTimerCount()).toBe(0);
    stop();
  });

  it("waits longer each time a read finds nothing new", async () => {
    const s = machine();
    const stop = (await opened(s)).subscribe(() => {});
    await settled(FACTS_POLL_MS - 1);
    expect(productReads(s)).toBe(0);
    await settled(1);
    expect(productReads(s)).toBe(1);
    // The first read was news; the second finds nothing and doubles the wait.
    await settled(FACTS_POLL_MS);
    expect(productReads(s)).toBe(2);
    await settled(FACTS_POLL_MS);
    expect(productReads(s)).toBe(2);
    await settled(FACTS_POLL_MS);
    expect(productReads(s)).toBe(3);
    stop();
  });

  it("reconciles another project's consumed invalidation after a command reply", async () => {
    const s = machine();
    s.answers["projects.list"] = () => ["p","q"].map(id => ({projectId:id,displayName:id,canonicalRoot:"/"+id,activation:{actorId:"a"}}));
    let title = "B old", hold = false, finish!: () => void;
    s.answers["project.snapshot"] = message => {
      const id = (message as {projectId:string}).projectId;
      const value = snapshot("4", [wireTask({taskId:id+"-task",title:id === "q" ? title : "A",titleAvailability:"available"})], id);
      if (hold && id === "q") return new Promise(resolve => {finish = () => resolve(value);});
      return value;
    };
    const port = await opened(s);
    const store = createRoot(d => {dispose=d;return createWorkspaceStore(port);});
    await settled();
    hold = true; title = "B new";
    s.send({type:"invalidated",projectId:"q"});
    await settled();
    // The command itself is controlled; all reads and invalidation bookkeeping use the real port.
    port.command = async envelope => ({requestId:envelope.requestId,ok:true,outcome:"recorded",message:"Recorded"});
    expect((await store.dispatch({type:"rename_task",taskId:"p-task",title:"A new"})).ok).toBe(true);
    expect(store.syncing()).toBe(true);
    // A newer event must not narrow the replacement read away from q.
    s.send({type:"invalidated",projectId:"p"});
    hold = false; finish(); await settled();
    expect(store.state().tasks.find(t => t.id === "q-task")?.title).toBe("B new");
    expect(store.syncing()).toBe(false);
    const read = s.calls.filter(c => c.operation === "project.snapshot").map(c => (c.message as {projectId:string}).projectId);
    expect(read).toEqual(["p","q","q","p","q"]);
  });

  it("reads only the project its events came from", async () => {
    const s = machine();
    s.answers["projects.list"] = () => [
      { projectId: "p", displayName: "P", canonicalRoot: "/p", activation: { actorId: "a" } },
      { projectId: "q", displayName: "Q", canonicalRoot: "/q", activation: { actorId: "a" } },
    ];
    s.answers["project.snapshot"] = message => snapshot("4", [], (message as { projectId: string }).projectId);
    const port = await opened(s);
    const stop = port.subscribe(() => {});
    s.send({ type: "invalidated", projectId: "q" });
    await port.query("project.snapshot", {});
    await port.query("project.facts", {});
    const read = s.calls.filter(c => c.operation === "project.snapshot").map(c => (c.message as { projectId: string }).projectId);
    expect(read).toEqual(["q"]);
    stop();
  });
});
