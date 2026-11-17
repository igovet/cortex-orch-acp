import { render } from "solid-js/web";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { emptyConfig, LOCAL_MACHINE_ID } from "@cortex/workspace-domain";
import { Workspace } from "../src";
import { createRealPort } from "../src/port/real";
import { writeBoards } from "../src/port/board-cache";
import { capabilitiesWire, flush, scriptedTransport, wireTask } from "./support";

let dispose: (() => void) | undefined;
beforeEach(() => { localStorage.clear(); sessionStorage.clear(); });
afterEach(() => { dispose?.(); dispose = undefined; document.body.innerHTML = ""; });
const settle = async () => { for (let i = 0; i < 8; i++) await flush(); };
const FAILED = "Attachments could not be read";

/** Relaunch with a task tab open: the cached board paints the task before the service has confirmed it. */
function relaunch(attachments: (taskId: string) => unknown) {
  const task = wireTask({ taskId: "saved", description: "Retained task", executionId: "execution", phase: "execution", executionState: "running", runtimeState: "EXECUTING" });
  const product = { tasks: {}, sprints: [] };
  writeBoards(localStorage, "owner", [{ project: { projectId: "p", name: "Project", repository: "/p", openBranch: "main", defaultBranch: "main", activated: true }, items: [task], product }]);
  let cold = true;
  const s = scriptedTransport({
    "service.capabilities": () => ({ methods: ["native.query", "native.board"], machine: true }),
    "projects.list": () => [{ projectId: "p", displayName: "Project", canonicalRoot: "/p", activation: {} }],
    "capabilities.get": () => capabilitiesWire(["capabilities.get", "project.snapshot"]),
    "native.board": () => {
      if (cold) throw Error("snapshot_expired");
      return { protocol: { major: 2, mapping: 2 }, projectId: "p", generation: "g", version: "2", highWatermark: "3", data: { type: "board", value: { items: [task], nextCursor: null, highWatermark: "3" } } };
    },
    "machine.call": raw => {
      const call = raw as { op: string; params?: { taskId?: string; attachments?: boolean } };
      if (call.op === "product.read" && call.params?.attachments) return attachments(call.params.taskId!);
      if (call.op === "overview") return { ok: true, result: { name: "This machine" } };
      if (call.op === "project.config.read") return { ok: true, result: { value: {}, tokens: {} } };
      if (call.op === "config.read") return { ok: true, result: { config: emptyConfig(), connections: [], tokens: {} } };
      return { ok: true, result: call.op === "repo.snapshot" ? null : call.op === "workers" ? [] : product };
    },
  });
  history.replaceState(null, "", `/m/${LOCAL_MACHINE_ID}/t/saved`);
  const root = document.createElement("div"); document.body.append(root);
  dispose = render(() => <Workspace port={createRealPort({ transport: s.transport, actorId: "owner", registry: true, boardStorage: localStorage })} />, root);
  return { s, root, warm: () => { cold = false; s.send({ type: "refresh" }); } };
}
const attachmentReads = (s: ReturnType<typeof scriptedTransport>) => s.calls.filter(c => c.operation === "machine.call" && (c.message as { params?: { attachments?: boolean } }).params?.attachments);

it("a restored task tab with no attachments shows no attachment error while the service confirms the task", async () => {
  const { s, root, warm } = relaunch(() => ({ ok: true, result: [] }));
  await settle();
  expect(root.textContent).toContain("Retained task");
  expect(root.textContent).not.toContain(FAILED);
  warm();
  await vi.waitFor(() => expect(attachmentReads(s).length).toBeGreaterThan(0));
  await settle();
  expect(root.textContent).not.toContain(FAILED);
});

it("a restored task tab still reports a real attachment read failure once the task is confirmed", async () => {
  const { s, root, warm } = relaunch(() => ({ ok: false, code: "storage_unavailable", message: "Storage is unavailable." }));
  await settle();
  expect(root.textContent).not.toContain(FAILED);
  warm();
  await vi.waitFor(() => expect(attachmentReads(s).length).toBeGreaterThan(0));
  await vi.waitFor(() => expect(root.textContent).toContain(FAILED));
});
