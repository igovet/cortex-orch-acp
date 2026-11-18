import { afterEach, expect, it, vi } from "vitest";
import { createRoot } from "solid-js";
import { emptyConfig, type NativeTaskDelta, type TimelineEntry } from "@cortex/workspace-domain";
import { createRealPort } from "../src/port/real";
import { createWorkspaceStore } from "../src/store";
import { STEP_PREFIX } from "../src/tool-names";
import { capabilitiesWire, scriptedTransport, wireTask } from "./support";

/**
 * A native service whose history pages behave like the core's: a whole read pages up to the newest row and names a forward
 * cursor; a forward read returns only the rows after its position. `reset` makes the next forward read refuse as the core
 * does for a history that was removed or written again.
 */
function streamingService(taskIds = ["task-1"]) {
  const versions = new Map(taskIds.map(id => [id, 1]));
  let high = 3, reset = false, overlap = 0;
  const task = (id: string) => wireTask({ taskId: id, version: String(versions.get(id)), executionId: `execution-${id}`, phase: "execution", executionState: "running", runtimeState: "EXECUTING" });
  const rows = new Map<string, Record<string, unknown>[]>(taskIds.map(id => [id, []]));
  const add = (id: string, kind: string, text: string, toolStatus: string | null = null) => {
    const list = rows.get(id)!;
    list.push({ historyId: `${id}:h${list.length}`, sequence: String(list.length + 1), revision: "1", kind, text, toolStatus,
      recordedAt: `2026-11-17T10:00:${String(list.length).padStart(2, "0")}.000000+00:00`, availability: "available" });
  };
  const envelope = (data: unknown) => ({ protocol: { major: 2, mapping: 2 }, projectId: "p", generation: "g", version: "2", highWatermark: String(high), data });
  const reads: { target: string; cursor: string | null }[] = [];
  const history = (payload: { target: string; cursor: string | null; limit: number }) => {
    reads.push({ target: payload.target, cursor: payload.cursor });
    const list = rows.get(payload.target)!, last = list.length - 1;
    let after = -1, top = last;
    if (payload.cursor?.startsWith("nt1.")) {
      if (reset) { reset = false; throw new Error("Refused: history_reset"); }
      after = Number(payload.cursor.slice(4)) - overlap; overlap = 0;
    } else if (payload.cursor) [top, after] = payload.cursor.slice(3).split(".").map(Number);
    const items = list.slice(after + 1, top + 1).slice(0, payload.limit);
    const consumed = after + items.length, next = consumed < top ? `nh.${top}.${consumed}` : null;
    return envelope({ type: "history", value: { items, nextCursor: next, highWatermark: String(top + 1), tailCursor: `nt1.${next ? consumed : top}` } });
  };
  const s = scriptedTransport({
    "service.capabilities": () => ({ methods: ["native.query", "native.board"], machine: true }),
    "projects.list": () => [{ projectId: "p", displayName: "Project", canonicalRoot: "/p", activation: {} }],
    "capabilities.get": () => capabilitiesWire(["capabilities.get", "project.snapshot", "tasks.create", "tasks.start", "decisions.list", "decisions.answer", "history.page"]),
    "native.board": () => envelope({ type: "board", value: { items: taskIds.map(task), nextCursor: null, highWatermark: String(high) } }),
    "native.query": raw => history((raw as { payload: { target: string; cursor: string | null; limit: number } }).payload),
    "decisions.list": () => ({ items: [], nextCursor: null, highWatermark: "1" }),
    "machine.call": raw => {
      const op = (raw as { op: string }).op;
      if (op === "overview") return { ok: true, result: { name: "This machine" } };
      if (op === "project.config.read") return { ok: true, result: { value: {}, tokens: {} } };
      if (op === "config.read") return { ok: true, result: { config: emptyConfig(), connections: [], tokens: {} } };
      return { ok: true, result: op === "repo.snapshot" ? null : op === "workers" ? [] : { tasks: {}, sprints: [] } };
    },
  });
  /** The event the service sends after a row was written: the task's version moved. */
  const changed = (id: string) => {
    const base = versions.get(id)!, from = high + 1;
    versions.set(id, base + 1); high++;
    const delta: NativeTaskDelta = { projectId: "p", batch: { generation: "g", streamKey: { scope: "project", project_id: "p" }, fromSequence: String(from), throughSequence: String(high),
      changes: [{ kind: "task", id, baseVersion: String(base), newVersion: String(base + 1), operation: { type: "upsert", data: { kind: "task_projection", data: task(id) } } }] } };
    s.send({ type: "native_delta", ...delta });
  };
  return { s, add, changed, reads, reset: () => { reset = true; }, overlap: (n: number) => { overlap = n; } };
}

let dispose: (() => void) | undefined;
afterEach(() => { dispose?.(); dispose = undefined; });
function open(s: ReturnType<typeof streamingService>["s"]) {
  const store = createRoot(d => { dispose = d; return createWorkspaceStore(createRealPort({ transport: s.transport, actorId: "owner", registry: true })); });
  const timeline = (id = "task-1") => store.state().sessions.find(x => x.taskId === id)?.timeline ?? [];
  const said = (id = "task-1") => timeline(id).flatMap((e: TimelineEntry) => e.kind === "cortex" ? [e.text] : []);
  return { store, timeline, said };
}
const step = (call: string, status: string) => STEP_PREFIX + JSON.stringify({ title: "Read a.ts", callId: call, kind: "read", path: "a.ts", status });
const whole = (reads: { cursor: string | null }[]) => reads.filter(r => r.cursor === null).length;
const forward = (reads: { cursor: string | null }[]) => reads.filter(r => r.cursor?.startsWith("nt1.")).length;

it("a streamed message grows in place across events, read forward only, and a tool's later update changes its row", async () => {
  const service = streamingService();
  service.add("task-1", "user", "Fix it");
  service.add("task-1", "assistant", "Hel");
  const { said, timeline } = open(service.s);
  await vi.waitFor(() => expect(said()).toEqual(["Hel"]));
  const id = timeline().find(e => e.kind === "cortex")!.id;
  service.add("task-1", "assistant", "lo wor"); service.changed("task-1");
  await vi.waitFor(() => expect(said()).toEqual(["Hello wor"]));
  service.add("task-1", "assistant", "ld."); service.changed("task-1");
  await vi.waitFor(() => expect(said()).toEqual(["Hello world."]));
  // The same message on screen, grown: never a new entry or a second copy.
  expect(timeline().find(e => e.kind === "cortex")!.id).toBe(id);
  service.add("task-1", "tool", step("c1", "pending"), "pending"); service.changed("task-1");
  await vi.waitFor(() => expect(timeline().filter(e => e.kind === "fold")).toHaveLength(1));
  const fold = timeline().find(e => e.kind === "fold")!.id;
  service.add("task-1", "tool", step("c1", "completed"), "completed"); service.changed("task-1");
  await vi.waitFor(() => expect(forward(service.reads)).toBeGreaterThanOrEqual(4));
  await vi.waitFor(() => expect(timeline().filter(e => e.kind === "fold").map(e => e.id)).toEqual([fold]));
  expect(whole(service.reads)).toBe(1);
});

it("a forward read that overlaps the rows held, or a history that was reset, never shows a row twice", async () => {
  const service = streamingService();
  service.add("task-1", "assistant", "One. ");
  const { said } = open(service.s);
  await vi.waitFor(() => expect(said()).toEqual(["One."]));
  // The service hands back a row already held (a read racing a newer one): it is not added again.
  service.overlap(1);
  service.add("task-1", "assistant", "Two. "); service.changed("task-1");
  await vi.waitFor(() => expect(said()).toEqual(["One. Two."]));
  // The history was reset underneath: the next read is a whole one, and replaces what was held.
  service.reset();
  service.add("task-1", "assistant", "Three."); service.changed("task-1");
  await vi.waitFor(() => expect(whole(service.reads)).toBe(2));
  await vi.waitFor(() => expect(said()).toEqual(["One. Two. Three."]));
});

it("after the service restarts, the conversation continues from the rows already held", async () => {
  const service = streamingService();
  service.add("task-1", "assistant", "Before. ");
  const { said } = open(service.s);
  await vi.waitFor(() => expect(said()).toEqual(["Before."]));
  service.s.send({ type: "connection", state: "disconnected" });
  service.s.send({ type: "connection", state: "connected" });
  service.s.send({ type: "refresh" });
  service.add("task-1", "assistant", "After."); service.changed("task-1");
  await vi.waitFor(() => expect(said()).toEqual(["Before. After."]));
  expect(whole(service.reads)).toBe(1);
  expect(forward(service.reads)).toBeGreaterThanOrEqual(1);
});

it("only the conversation on view is read as it streams; another is read when it is opened", async () => {
  const service = streamingService(["task-1", "task-2"]);
  service.add("task-1", "assistant", "First task.");
  service.add("task-2", "assistant", "Second task.");
  const { store, said } = open(service.s);
  const release = store.interest("p", "facts", "task-1");
  await vi.waitFor(() => expect(said("task-1")).toEqual(["First task."]));
  service.add("task-2", "assistant", " More."); service.changed("task-2");
  service.add("task-1", "assistant", " Again."); service.changed("task-1");
  await vi.waitFor(() => expect(said("task-1")).toEqual(["First task. Again."]));
  expect(service.reads.filter(r => r.target === "task-2")).toEqual([]);
  const second = store.interest("p", "facts", "task-2");
  await vi.waitFor(() => expect(said("task-2")).toEqual(["Second task. More."]));
  release(); second();
});
