import { createRoot } from "solid-js";
import { render } from "solid-js/web";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { emptyWorkspace, LOCAL_MACHINE_ID } from "@cortex/workspace-domain";
import { Workspace } from "../src";
import { hrefOf } from "../src/router";
import { createRealPort } from "../src/port/real";
import { readBoards } from "../src/port/board-cache";
import { sideRows, sprintRoute, sprintView } from "../src/selectors";
import { createWorkspaceStore } from "../src/store";
import { capabilitiesWire, scriptedTransport, wireTask } from "./support";

/**
 * A sprint deletion that stopped part-way, as a real service records it: the sprint stays with `deleting`, its archive stamp,
 * the members it set out to delete and the ones it did; every deleted member is a `purged` overlay and is gone from the
 * runtime's board, while the sprint's `taskIds` still name all of them. One member's execution has not confirmed its stop.
 */
const MEMBERS = Array.from({ length: 8 }, (_, i) => `wt_member_${i}`);
const STUCK = MEMBERS[0];
const OLD = "sp_old", NEW = "sp_new";
const settings = { effort: "medium", language: "en", mode: "auto", model: "fixture-model" };
const config = () => {
  const value = structuredClone(emptyWorkspace().machines[0].config);
  value.setup["codex-acp"].status = "connected";
  value.catalogue["codex-acp"].models = [{ id: "fixture-model", label: "Fixture", efforts: [], observed: true }];
  value.surfaces.routing.engines["codex-acp"].enabled = ["fixture-model"];
  return value;
};

function service(options: { streamed?: boolean } = {}) {
  let live = [...MEMBERS], high = 3;
  const product: { tasks: Record<string, Record<string, unknown>>; sprints: Record<string, unknown>[]; messages: unknown[] } = {
    tasks: Object.fromEntries(MEMBERS.map(id => [id, { sprintId: OLD, dependencies: [], planning: false, checkout: { kind: "main" } }])),
    sprints: [{ id: OLD, projectId: "p", description: "Old sprint", title: null, runMode: "automatic", planning: "created", taskIds: [...MEMBERS],
      queueState: "running", pauseReason: null, targetBranch: null, conversation: [], plan: null, materializedVersion: 1, checkout: { kind: "main" }, settings }],
    messages: [],
  };
  const envelope = (data: unknown) => ({ protocol: { major: 2, mapping: 2 }, projectId: "p", generation: "g", version: "2", highWatermark: String(high), data });
  const page = (type: string) => envelope({ type, value: { items: live.map(id => wireTask({ taskId: id, description: id === STUCK ? "Stuck member" : `Member ${id}`, executionId: id === STUCK ? "exec-stuck" : null })), nextCursor: null, highWatermark: String(high) } });
  const s = scriptedTransport({
    "service.capabilities": () => ({ methods: ["native.query", "native.board"], machine: true }),
    "projects.list": () => [{ projectId: "p", displayName: "Project", canonicalRoot: "/p", activation: { actorId: "owner" } }],
    "capabilities.get": () => capabilitiesWire(["capabilities.get", "project.snapshot", "history.page", "tasks.create"]),
    "native.board": () => page("board"),
    "native.query": raw => (raw as { operation: string }).operation === "tasks.page" ? page("tasks") : envelope({ type: "history", value: { items: [], nextCursor: null, highWatermark: "0" } }),
    "decisions.list": () => ({ items: [], nextCursor: null, highWatermark: "1" }),
    "machine.call": raw => {
      const { op, params } = raw as { op: string; params?: Record<string, unknown> };
      if (op === "config.read") return { ok: true, result: { config: config(), connections: [], tokens: {} } };
      if (op === "overview") return { ok: true, result: { name: "This machine" } };
      if (op === "project.config.read") return { ok: true, result: { value: {}, tokens: {} } };
      if (op === "repo.snapshot") return { ok: true, result: null };
      if (op === "workers") return { ok: true, result: [] };
      if (op === "product.sprint" && params?.action === "delete") {
        // Seven members are purged; the stuck one is still waiting for its stop.
        const deleted = MEMBERS.slice(1);
        for (const id of deleted) product.tasks[id] = { purged: true };
        // A streamed client learns of the removals from the runtime's events, after this reply.
        if (!options.streamed) live = [STUCK];
        Object.assign(product.sprints[0], { deleting: true, archivedAt: 1_794_905_642_111, queueState: "paused", pauseReason: "Deletion pending — retry Delete sprint",
          deleteTaskIds: [...MEMBERS], deletedTaskIds: deleted, keptWorktrees: [] });
        return { ok: true, result: { done: false, reason: `Waiting for confirmed stop or cleanup: ${STUCK}: active_execution. Retry Delete sprint.`, taskIds: deleted, keptPaths: [] } };
      }
      if (op === "product.sprint" && params?.action === "create") {
        product.sprints.push({ id: NEW, projectId: "p", description: params.description, title: null, runMode: "automatic", planning: "ready", taskIds: [], queueState: "idle", pauseReason: null,
          targetBranch: null, conversation: [], materializedVersion: null, checkout: { kind: "main" }, settings,
          plan: { version: 1, items: [{ key: "t1", title: "First", description: "First item", dependsOn: [] }] } });
        return { ok: true, result: { sprintId: NEW } };
      }
      return { ok: true, result: structuredClone(product) };
    },
  });
  /** The runtime's events for the purged members, as a live client receives them. */
  const removeDeleted = () => {
    const from = high + 1;
    high++; live = [STUCK];
    s.send({ type: "native_delta", projectId: "p", batch: { generation: "g", streamKey: { scope: "project", project_id: "p" }, fromSequence: String(from), throughSequence: String(high),
      changes: MEMBERS.slice(1).map(id => ({ kind: "task" as const, id, baseVersion: "1", newVersion: "2", operation: { type: "remove" as const } })) } });
  };
  return { s, product, removeDeleted };
}

let stop: (() => void) | undefined;
beforeEach(() => { localStorage.clear(); });
afterEach(() => { stop?.(); stop = undefined; document.body.innerHTML = ""; });

function open(s: ReturnType<typeof service>["s"], port = createRealPort({ transport: s.transport, actorId: "owner", registry: true, boardStorage: localStorage })) {
  return createRoot(dispose => { stop = dispose; return createWorkspaceStore(port); });
}

describe("a partially deleted sprint", () => {
  it("does not hide a sprint created after it, on the board, the sprint page lookup or the sidebar", async () => {
    const { s } = service();
    const store = open(s);
    await vi.waitFor(() => expect(store.state().sprints.map(x => x.id)).toEqual([OLD]), { timeout: 4000 });
    await vi.waitFor(() => expect(store.state().tasks).toHaveLength(8));

    const removed = await store.dispatch({ type: "delete_sprint", projectId: "p", sprintId: OLD, confirmed: true });
    expect(removed).toMatchObject({ ok: false, outcome: "partial_effect", deletedTaskIds: MEMBERS.slice(1) });
    const created = await store.dispatch({ type: "create_sprint", projectId: "p", description: "New sprint", runMode: "automatic", checkout: { kind: "main" }, settings: settings as never });
    expect(created).toMatchObject({ ok: true, entityId: NEW });

    // The id the page navigates to is the one the state holds.
    await vi.waitFor(() => expect(store.state().sprints.find(x => x.id === created.entityId)).toMatchObject({ planning: "ready", taskIds: [] }), { timeout: 4000 });
    // The deleted members are gone; the one waiting for its stop is still there.
    await vi.waitFor(() => expect(store.state().tasks.map(t => t.id)).toEqual([STUCK]));
    expect(store.state().projects[0].issue ?? null).toBeNull();

    const summary = store.index().summaries.get("p")!;
    const rows = sideRows(store.index(), summary).flatMap(r => r.kind === "sprint" ? [r.sprint.id] : []);
    expect(rows).toContain(NEW);
    // The unfinished deletion stays visible, in its own words, until it completes.
    expect(rows).toContain(OLD);
    const old = store.state().sprints.find(x => x.id === OLD)!;
    expect(old.deletion).toEqual({ total: 8, left: [STUCK] });
    expect(sprintView(store.index(), old).state).toBe("Deleting, 1 task left: Stuck member, waiting for its stop");
    // The deleted members count neither as missing details nor as the sprint's tasks.
    expect(sprintView(store.index(), old).missing).toBe(0);

    // The confirmed board is cached again, so the next launch paints both sprints.
    const cached = readBoards(localStorage, "owner");
    expect(cached[0]?.product.sprints.map(x => x.id)).toEqual([OLD, NEW]);
  });

  it("shows the project and both sprints on the next launch, without a read failure", async () => {
    const { s } = service();
    // The deletion stopped part-way and a new sprint was created, before this app opened.
    const call = s.answers["machine.call"]!;
    call({ op: "product.sprint", params: { action: "delete" } });
    call({ op: "product.sprint", params: { action: "create", description: "New sprint" } });
    const store = open(s);
    await vi.waitFor(() => expect(store.state().sprints.map(x => x.id)).toEqual([OLD, NEW]), { timeout: 4000 });
    expect(store.state().tasks.map(t => t.id)).toEqual([STUCK]);
    // Three failed reads would raise the project's read issue; give them the time to.
    await new Promise(resolve => setTimeout(resolve, 50));
    expect(store.state().projects[0].issue ?? null).toBeNull();
    expect(store.index().summaries.get("p")!.sprints.map(x => x.id)).toEqual([OLD, NEW]);
  });

  it("keeps reading the project's facts after the runtime streams the purged members' removal", async () => {
    const { s, removeDeleted } = service({ streamed: true });
    const store = open(s);
    await vi.waitFor(() => expect(store.state().tasks).toHaveLength(8), { timeout: 4000 });
    await store.dispatch({ type: "delete_sprint", projectId: "p", sprintId: OLD, confirmed: true });
    await vi.waitFor(() => expect(store.state().sprints.find(x => x.id === OLD)?.archivedAt).toBeTypeOf("number"), { timeout: 4000 });
    removeDeleted();
    await vi.waitFor(() => expect(store.version("project:p")).toBe("4"));
    const created = await store.dispatch({ type: "create_sprint", projectId: "p", description: "New sprint", runMode: "automatic", checkout: { kind: "main" }, settings: settings as never });
    expect(created).toMatchObject({ ok: true, entityId: NEW });
    await vi.waitFor(() => expect(store.state().sprints.map(x => x.id)).toEqual([OLD, NEW]), { timeout: 4000 });
    expect(store.index().summaries.get("p")!.sprints.map(x => x.id)).toContain(NEW);
  });

  it("retries the deletion and drops the sprint once the service confirms it", async () => {
    const { s, product } = service();
    const store = open(s);
    await vi.waitFor(() => expect(store.state().tasks).toHaveLength(8), { timeout: 4000 });
    await store.dispatch({ type: "delete_sprint", projectId: "p", sprintId: OLD, confirmed: true });
    await vi.waitFor(() => expect(store.state().sprints.find(x => x.id === OLD)?.deletion?.left).toEqual([STUCK]), { timeout: 4000 });

    const original = s.answers["machine.call"]!;
    s.answers["machine.call"] = raw => {
      const { op, params } = raw as { op: string; params?: Record<string, unknown> };
      if (op === "product.sprint" && params?.action === "delete") {
        product.sprints.splice(0, 1);
        product.tasks[STUCK] = { purged: true };
        return { ok: true, result: { done: true, taskIds: [...MEMBERS], keptPaths: [] } };
      }
      return original(raw);
    };
    const original_board = s.answers["native.board"]!;
    s.answers["native.board"] = raw => { const reply = original_board(raw) as { data: { value: { items: unknown[] } } }; reply.data.value.items = []; return reply; };
    s.answers["native.query"] = () => ({ protocol: { major: 2, mapping: 2 }, projectId: "p", generation: "g", version: "2", highWatermark: "3", data: { type: "tasks", value: { items: [], nextCursor: null, highWatermark: "3" } } });
    expect(await store.dispatch({ type: "delete_sprint", projectId: "p", sprintId: OLD, confirmed: true })).toMatchObject({ ok: true });
    await vi.waitFor(() => expect(store.state().sprints.find(x => x.id === OLD)).toBeUndefined());
    expect(store.state().tasks.map(t => t.id)).not.toContain(STUCK);
  });
});

describe("a deletion pass that outlives its reply", () => {
  it("counts what the service has left after the pass ends, and says the reason the service recorded", async () => {
    const { s, product } = service();
    const store = open(s);
    await vi.waitFor(() => expect(store.state().tasks).toHaveLength(8), { timeout: 4000 });
    const original = s.answers["machine.call"]!;
    const markDeleted = (deleted: string[], reason?: string) => {
      for (const id of deleted) product.tasks[id] = { purged: true };
      Object.assign(product.sprints[0], { deleting: true, archivedAt: 1_794_905_642_111, queueState: "paused", pauseReason: "Deletion pending — retry Delete sprint",
        deleteTaskIds: [...MEMBERS], deletedTaskIds: deleted, keptWorktrees: [], ...(reason ? { deleteReason: reason } : {}) });
    };
    s.answers["machine.call"] = raw => {
      const { op, params } = raw as { op: string; params?: Record<string, unknown> };
      if (op === "product.sprint" && params?.action === "delete") {
        // The bounded owner action answers while its pass is still deleting: three are gone so far.
        markDeleted(MEMBERS.slice(5));
        return { ok: true, result: { done: false, reason: "Cortex is still stopping this sprint's runs or deleting its tasks. Retry Delete sprint if it still shows as deleting." } };
      }
      return original(raw);
    };
    const removed = await store.dispatch({ type: "delete_sprint", projectId: "p", sprintId: OLD, confirmed: true });
    expect(removed).toMatchObject({ ok: false, outcome: "partial_effect" });
    await vi.waitFor(() => expect(store.state().sprints.find(x => x.id === OLD)?.deletion?.left).toHaveLength(5), { timeout: 4000 });
    expect(sprintView(store.index(), store.state().sprints.find(x => x.id === OLD)!).state).toBe("Deleting, 5 tasks left, waiting for them to stop");

    // The pass ends with one member that is not quiet yet; it records why and announces the change.
    const reason = "Not every task could be deleted yet. “Stuck member” is still running, or Cortex is still waiting to confirm that its engine processes stopped. Retry Delete sprint.";
    markDeleted(MEMBERS.slice(1), reason);
    s.send({ type: "invalidation", projectId: "p" } as never);
    await vi.waitFor(() => expect(store.state().sprints.find(x => x.id === OLD)?.deletion).toEqual({ total: 8, left: [STUCK], reason }), { timeout: 4000 });
    expect(sprintView(store.index(), store.state().sprints.find(x => x.id === OLD)!).state).toBe(`Deleting, 1 task left: Stuck member. ${reason}`);
    // The next launch paints the same words from the cached board.
    expect(readBoards(localStorage, "owner")[0]?.product.sprints.find(x => x.id === OLD)?.deletion?.reason).toBe(reason);
  });
});

describe("a project read that keeps failing", () => {
  it("keeps the owner's words and retains the read's precise cause", async () => {
    const { s, product } = service();
    // A member that is neither on the board nor deleted: its sprint cannot be shown without it.
    (product.sprints[0].taskIds as string[]).push("wt_ghost");
    const port = createRealPort({ transport: s.transport, actorId: "owner", registry: true });
    const store = open(s, port);
    await port.query("capabilities.get", {});
    for (let i = 0; i < 3; i++) await port.query("project.snapshot", { full: true }).catch(() => []);
    await vi.waitFor(() => expect(store.state().projects[0]).toMatchObject({
      issue: "This project's data could not be read. Other projects remain available. Retrying automatically.",
      issueCause: `Sprint membership could not be read completely. Sprint ${OLD} names 1 task the board does not hold: wt_ghost`,
    }), { timeout: 4000 });
  });
});

describe("the screens of a partially deleted sprint", () => {
  it("closes the confirmation once the deletion has started and says what it still waits for", async () => {
    const { s } = service();
    history.replaceState(null, "", hrefOf({ name: "sprint", machine: LOCAL_MACHINE_ID, project: "p", sprint: OLD, tab: "summary" } as never));
    const root = document.createElement("div"); document.body.append(root);
    stop = render(() => <Workspace port={createRealPort({ transport: s.transport, actorId: "owner", registry: true })} />, root);
    const item = await vi.waitFor(() => { const b = [...root.querySelectorAll<HTMLButtonElement>("button[role=menuitem]")].find(x => x.textContent?.startsWith("Delete sprint…")); expect(b).toBeDefined(); return b!; }, { timeout: 4000 });
    item.click();
    const confirm = await vi.waitFor(() => { const b = [...document.querySelectorAll("button")].find(x => x.textContent === "Delete sprint"); expect(b).toBeDefined(); return b!; });
    confirm.click();
    expect(document.querySelector("dialog[open] .ws-request-status")?.textContent).toBe("Requesting deletion…");
    // Part of the sprint is deleted: the dialog closes instead of offering the same press again.
    await vi.waitFor(() => expect([...document.querySelectorAll("button")].some(x => x.textContent === "Delete sprint")).toBe(false), { timeout: 4000 });
    expect(document.body.textContent).toContain("Waiting for confirmed stop or cleanup");
    expect(document.body.textContent).not.toContain("Requesting deletion…");
    await vi.waitFor(() => expect(root.textContent).toContain("Deleting, 1 task left: Stuck member, waiting for its stop"), { timeout: 4000 });
    expect([...root.querySelectorAll("button")].some(b => b.textContent === "Retry delete…")).toBe(true);
  });

  it("say what is left, keep the new sprint in the sidebar and retry the deletion from the sprint's page", async () => {
    const { s, product } = service();
    const call = s.answers["machine.call"]!;
    call({ op: "product.sprint", params: { action: "delete" } });
    call({ op: "product.sprint", params: { action: "create", description: "New sprint" } });
    s.answers["machine.call"] = raw => {
      const { op, params } = raw as { op: string; params?: Record<string, unknown> };
      if (op === "product.sprint" && params?.action === "delete") {
        product.sprints.splice(0, 1);
        product.tasks[STUCK] = { purged: true };
        return { ok: true, result: { done: true, taskIds: [...MEMBERS], keptPaths: [] } };
      }
      return call(raw);
    };
    history.replaceState(null, "", hrefOf(sprintRoute(LOCAL_MACHINE_ID, { id: OLD, projectId: "p", deletion: { total: 8, left: [STUCK] } } as never)));
    const root = document.createElement("div"); document.body.append(root);
    stop = render(() => <Workspace port={createRealPort({ transport: s.transport, actorId: "owner", registry: true })} />, root);
    await vi.waitFor(() => expect(root.textContent).toContain("Deleting, 1 task left: Stuck member, waiting for its stop"), { timeout: 4000 });
    expect(root.textContent).not.toContain("Sprint not found");
    const side = [...root.querySelectorAll(".task-row")].map(r => r.textContent ?? "");
    expect(side.some(t => t.includes("Sprint: Old sprint") && t.includes("Deleting"))).toBe(true);
    expect(side.some(t => t.includes("Sprint: New sprint"))).toBe(true);
    const retry = [...root.querySelectorAll("button")].find(b => b.textContent === "Retry delete…")!;
    expect(retry).toBeDefined();
    retry.click();
    const confirm = await vi.waitFor(() => { const b = [...document.querySelectorAll("button")].find(x => x.textContent === "Delete sprint"); expect(b).toBeDefined(); return b!; });
    confirm.click();
    await vi.waitFor(() => expect(s.calls.some(c => (c.message as { op?: string; params?: { action?: string } }).op === "product.sprint" && (c.message as { params?: { action?: string } }).params?.action === "delete")).toBe(true));
    await vi.waitFor(() => expect([...root.querySelectorAll(".task-row")].some(r => r.textContent?.includes("Old sprint"))).toBe(false), { timeout: 4000 });
    expect([...root.querySelectorAll(".task-row")].some(r => r.textContent?.includes("New sprint"))).toBe(true);
  });
});
