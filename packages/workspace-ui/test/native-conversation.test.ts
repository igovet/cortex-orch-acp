import { expect, it, vi } from "vitest";
import { createRoot } from "solid-js";
import { createRealPort } from "../src/port/real";
import { createWorkspaceStore } from "../src/store";
import { capabilitiesWire, scriptedTransport, wireTask } from "./support";

/** A native service: a board, the core's history pages, and owner prompts that the core records as user turns. */
function nativeService() {
  const task = wireTask({ executionId: "execution", phase: "execution", executionState: "running", runtimeState: "EXECUTING" });
  const rows: Record<string, unknown>[] = [];
  const row = (kind: string, text: string) => rows.push({ historyId: `h${rows.length}`, sequence: String(rows.length + 1), revision: "1", kind, text, toolStatus: null,
    recordedAt: `2026-11-15T10:00:${String(rows.length).padStart(2, "0")}.000000+00:00`, availability: "available" });
  row("assistant", "Planning the work");
  const envelope = (data: unknown) => ({ protocol: { major: 2, mapping: 2 }, projectId: "p", generation: "g", version: "2", highWatermark: "3", data });
  const s = scriptedTransport({
    "service.capabilities": () => ({ methods: ["native.query", "native.board"], machine: true }),
    "projects.list": () => [{ projectId: "p", displayName: "Project", canonicalRoot: "/p", activation: {} }],
    // Exactly what a native project advertises (`native_runtime::capabilities`). Without
    // history.page no conversation is ever read, so no owner message can appear.
    "capabilities.get": () => capabilitiesWire(["capabilities.get", "tasks.create", "tasks.start", "decisions.list", "decisions.get", "decisions.answer", "history.page"]),
    "native.board": () => envelope({ type: "board", value: { items: [task], nextCursor: null, highWatermark: "3" } }),
    "native.query": () => envelope({ type: "history", value: { items: [...rows], nextCursor: null, highWatermark: String(rows.length) } }),
    "decisions.list": () => ({ items: [], nextCursor: null, highWatermark: "1" }),
    "executions.prompt": raw => { row("user", (raw as { text: string }).text); row("assistant", "On it"); return { outcome: "recorded" }; },
    "machine.call": raw => { const op = (raw as { op: string }).op; return { ok: true, result: op === "repo.snapshot" ? null : op === "workers" ? [] : op === "product.task" ? {} : { tasks: {}, sprints: [] } }; },
  });
  return s;
}
function open(s: ReturnType<typeof nativeService>) {
  let dispose!: () => void;
  const store = createRoot(d => { dispose = d; return createWorkspaceStore(createRealPort({ transport: s.transport, actorId: "owner", registry: true })); });
  const said = () => (store.state().sessions.find(x => x.taskId === "task-1")?.timeline ?? []).flatMap(e => e.kind === "said" ? [e.text] : []);
  return { store, said, dispose };
}

it("shows a running task's steering and Request changes text as user turns, in order, live and after reload", async () => {
  const s = nativeService();
  const live = open(s);
  try {
    await vi.waitFor(() => expect(live.store.can("workspace.send_message")).toBe(true));
    expect((await live.store.dispatch({ type: "send_message", taskId: "task-1", text: "Also add a docstring", mode: "send_now" })).ok).toBe(true);
    await vi.waitFor(() => expect(live.said()).toEqual(["Also add a docstring"]));
    expect((await live.store.dispatch({ type: "request_changes", taskId: "task-1", text: "Please rename the helper" })).ok).toBe(true);
    await vi.waitFor(() => expect(live.said()).toEqual(["Also add a docstring", "Please rename the helper"]));
  } finally { live.dispose(); }
  const reopened = open(s);
  try {
    await vi.waitFor(() => expect(reopened.said()).toEqual(["Also add a docstring", "Please rename the helper"]));
  } finally { reopened.dispose(); }
  expect(s.calls.some(c => c.operation === "native.query" && (c.message as { operation: string }).operation === "history.page")).toBe(true);
});
