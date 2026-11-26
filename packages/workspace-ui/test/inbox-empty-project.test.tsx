import { render } from "solid-js/web";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { emptyConfig, LOCAL_MACHINE_ID } from "@cortex/workspace-domain";
import { Workspace } from "../src";
import { createRealPort } from "../src/port/real";
import { capabilitiesWire, flush, scriptedTransport, wireTask } from "./support";

/**
 * Two projects: `busy` has a running task, `fresh` has no task yet (a new project's board reads at watermark "0"). Another
 * `slow` project's board read can be held. Neither an empty project nor a slow one may keep Inbox or Home on their skeleton.
 */
function machine(options: { slow?: Promise<void> } = {}) {
  const projects = [{ projectId: "busy", displayName: "Busy", canonicalRoot: "/busy", activation: {} }, { projectId: "fresh", displayName: "Fresh", canonicalRoot: "/fresh", activation: {} },
    ...(options.slow ? [{ projectId: "slow", displayName: "Slow", canonicalRoot: "/slow", activation: {} }] : [])];
  const task = wireTask({ taskId: "task-1", description: "Task 1", version: "1", executionId: "exec-1", phase: "execution", executionState: "running", runtimeState: "EXECUTING" });
  const envelope = (projectId: string, high: string, items: unknown[]) => ({ protocol: { major: 2, mapping: 2 }, projectId, generation: "g", version: high, highWatermark: high, data: { type: "board", value: { items, nextCursor: null, highWatermark: high } } });
  return scriptedTransport({
    "service.capabilities": () => ({ methods: ["native.query", "native.board"], machine: true }),
    "projects.list": () => projects,
    "capabilities.get": () => capabilitiesWire(["capabilities.get", "project.snapshot", "tasks.create", "tasks.start", "decisions.list", "decisions.answer", "history.page"]),
    "native.board": async raw => {
      const id = (raw as { projectId: string }).projectId;
      if (id === "slow") await options.slow;
      return id === "busy" ? envelope(id, "3", [task]) : envelope(id, "0", []);
    },
    "native.query": raw => ({ protocol: { major: 2, mapping: 2 }, projectId: (raw as { projectId: string }).projectId, generation: "g", version: "2", highWatermark: "2", data: { type: "history", value: { items: [], nextCursor: null, highWatermark: "2", tailCursor: "nt1.1" } } }),
    "decisions.list": () => ({ items: [], nextCursor: null, highWatermark: "1" }),
    "machine.call": async raw => {
      const op = (raw as { op: string }).op;
      if (op === "overview") return { ok: true, result: { name: "This machine" } };
      if (op === "project.config.read") return { ok: true, result: { value: {}, tokens: {} } };
      if (op === "config.read") return { ok: true, result: { config: emptyConfig(), connections: [], tokens: {} } };
      if (op === "workers") return { ok: true, result: [] };
      return { ok: true, result: op === "repo.snapshot" ? null : { tasks: {}, sprints: [] } };
    },
  });
}

let dispose: (() => void) | undefined;
beforeEach(() => { localStorage.clear(); sessionStorage.clear(); });
afterEach(() => { dispose?.(); dispose = undefined; document.body.innerHTML = ""; vi.useRealTimers(); history.replaceState(null, "", "/"); });
const settle = async (n = 20) => { for (let i = 0; i < n; i++) await flush(); };
function open(path: string, svc: ReturnType<typeof machine>) {
  history.replaceState(null, "", path);
  const root = document.createElement("div"); document.body.append(root);
  dispose = render(() => <Workspace port={createRealPort({ transport: svc.transport, actorId: "owner", registry: true })} />, root);
  return root;
}

it.each([["Inbox", `/m/${LOCAL_MACHINE_ID}/inbox`], ["Home", `/m/${LOCAL_MACHINE_ID}/`]])("%s leaves its skeleton once every project has answered, though one holds no task yet", async (_name, path) => {
  const root = open(path, machine());
  await settle();
  expect(root.querySelector(".ws-skel-list")).toBeNull();
  expect(root.textContent).toMatch(/Nothing needs you/);
});

it.each([["Inbox", `/m/${LOCAL_MACHINE_ID}/inbox`], ["Home", `/m/${LOCAL_MACHINE_ID}/`]])("%s does not wait for a project whose first read is still slow, and shows no error", async (_name, path) => {
  let release!: () => void;
  const root = open(path, machine({ slow: new Promise<void>(resolve => { release = resolve; }) }));
  await settle();
  // The slow project is still unread: with nothing to show yet the skeleton stays, and nothing says it failed.
  expect(root.querySelector(".ws-first-notice")).toBeNull();
  release();
  await settle();
  expect(root.querySelector(".ws-skel-list")).toBeNull();
  expect(root.textContent).toMatch(/Nothing needs you/);
});
