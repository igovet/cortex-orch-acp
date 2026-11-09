import { createRoot } from "solid-js";
import { afterEach, describe, expect, it } from "vitest";
import { emptyWorkspace, type WorkspaceState } from "@cortex/workspace-domain";
import { createWorkspaceStore } from "../src/store";
import { flush, projection, scriptedPort, withTask } from "./support";

let dispose: (() => void) | undefined;
afterEach(() => { dispose?.(); dispose = undefined; });
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

  it("stops listening when the screen goes away", async () => {
    const s = scriptedPort();
    storeOn(s.port);
    expect(s.listeners.size).toBe(1);
    dispose?.(); dispose = undefined;
    expect(s.listeners.size).toBe(0);
  });
});
