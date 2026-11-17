// @vitest-environment jsdom
import { render } from "solid-js/web";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { emptySession, emptyWorkspace, LOCAL_MACHINE_ID, type PortCapabilities, type Worker } from "@cortex/workspace-domain";
import { Workspace } from "../src";
import { createRealPort } from "../src/port/real";
import { capabilitiesWire, flush, projection, scriptedPort, scriptedTransport, withTask } from "./support";

let dispose: (() => void) | undefined;
beforeEach(() => { localStorage.clear(); sessionStorage.clear(); });
afterEach(() => { dispose?.(); dispose = undefined; document.body.innerHTML = ""; });
const settle = async () => { for (let i = 0; i < 8; i++) await flush(); };
const OFFERED = ["capabilities.get", "project.snapshot", "tasks.create", "tasks.start", "decisions.list", "decisions.answer", "history.page"];

/** What the native service and a pre-native machine service each tell the port. */
async function offeredBy(methods: string[], attachments: boolean | undefined) {
  const s = scriptedTransport({
    "service.capabilities": () => ({ methods, machine: true }),
    "projects.list": () => [],
    "capabilities.get": () => ({ ...capabilitiesWire(OFFERED), ...(attachments === undefined ? {} : { attachments }) }),
  });
  const port = createRealPort({ transport: s.transport, actorId: "owner", registry: true });
  return (await port.query("capabilities.get", {} as never) as PortCapabilities).operations;
}
describe("attachment capabilities of the native service", () => {
  it("Attach is offered when the service says it retains and delivers files, and worker guidance is not", async () => {
    const operations = await offeredBy(["native.query", "native.board", "machine.call"], true);
    expect(operations).toContain("workspace.attachments");
    expect(operations).toContain("workspace.worker_messages");
    expect(operations).not.toContain("workspace.worker_attachments");
  });
  it("Attach stays off, as before, when the service does not say so", async () => {
    expect(await offeredBy(["native.query", "native.board", "machine.call"], undefined)).not.toContain("workspace.attachments");
    expect(await offeredBy(["native.query", "native.board", "machine.call"], false)).not.toContain("workspace.attachments");
  });
  it("a machine service that is not native still takes files with a worker message", async () => {
    expect(await offeredBy(["machine.call"], true)).toContain("workspace.worker_attachments");
  });
});

const worker = (): Worker => ({ id: "w", taskId: "t", executionId: "e", role: "frontend-dev", status: "working", lane: "execution", goal: "Build", now: null, waitsFor: null, scope: [], route: null, attempt: null, elapsedMs: null, runningSince: null, context: null, usage: null, activity: [], result: null });
async function attachState(operations: string[], path: string) {
  const state = withTask(emptyWorkspace(), "Live");
  state.machines[0].connection = "connected";
  state.tasks[0].status = "working"; state.tasks[0].process = "live"; state.tasks[0].executionId = "e";
  state.workers = [worker()]; state.sessions = [emptySession("t")];
  const capabilities: PortCapabilities = { everything: false, operations };
  const p = scriptedPort({ capabilities });
  history.replaceState(null, "", `/m/${LOCAL_MACHINE_ID}/t/t${path}`);
  const root = document.createElement("div"); document.body.append(root);
  dispose = render(() => <Workspace port={p.port} />, root);
  await settle(); p.emit(projection("1", state)); await settle();
  return document.querySelector<HTMLButtonElement>('[aria-label="Attach files"]')!;
}
describe("each composer says its own reason", () => {
  it("a message to one worker says files cannot go to a single worker, not that attachments are unavailable on this machine", async () => {
    const attach = await attachState(["workspace.attachments", "workspace.worker_messages"], "/workers/w");
    expect(attach.getAttribute("aria-disabled")).toBe("true");
    expect(attach.dataset.why).toMatch(/one worker/);
    expect(attach.dataset.why).not.toMatch(/not available on this machine/);
  });
  it("the same worker composer takes files when the machine can carry them to the worker", async () => {
    const attach = await attachState(["workspace.attachments", "workspace.worker_messages", "workspace.worker_attachments"], "/workers/w");
    expect(attach.getAttribute("aria-disabled")).toBeNull();
  });
  it("the task message box takes files while the worker box does not", async () => {
    const attach = await attachState(["workspace.attachments", "workspace.worker_messages", "workspace.send_message"], "");
    expect(attach.getAttribute("aria-disabled")).toBeNull();
  });
});
