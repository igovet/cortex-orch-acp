import { createConnection } from "node:net";
import { createRealPort } from "../src/port/real";
import { render } from "solid-js/web";
import { afterEach, expect, it } from "vitest";
import { dependencyReady, emptyWorkspace, LOCAL_MACHINE_ID, type WorkspaceState } from "@cortex/workspace-domain";
import { Workspace } from "../src";
import { flush, projection, scriptedPort, withTask } from "./support";

let dispose: (() => void) | undefined;
afterEach(() => { dispose?.(); document.body.innerHTML = ""; localStorage.clear(); sessionStorage.clear(); });
const settle = async () => { for (let i = 0; i < 8; i++) await flush(); };
async function mount(s: WorkspaceState, path = `/m/${LOCAL_MACHINE_ID}/p/p/board`) {
  s.machines[0].connection = "connected";
  const p = scriptedPort({ capabilities: { everything: true, operations: [] } });
  history.replaceState(null, "", path);
  const root = document.createElement("div"); document.body.append(root);
  dispose = render(() => <Workspace port={p.port} />, root);
  await settle(); p.emit(projection("1", s)); await settle();
  return p;
}
const socket = process.env.WORKSPACE_CONTRACT_SOCKET;
function rpc(operation: string, message: unknown = {}): Promise<any> {
  return new Promise((resolve, reject) => {
    const s = createConnection(socket!); let data = "";
    s.setTimeout(10000, () => s.destroy(new Error("fixture timeout")));
    s.on("error", reject); s.on("connect", () => s.write(JSON.stringify({ operation, message }) + "\n"));
    s.on("data", chunk => { data += chunk; if (!data.includes("\n")) return; s.end(); const r = JSON.parse(data); if (r.error) reject(new Error(r.error)); else resolve(r.result); });
  });
}
it.skipIf(!socket)("real port closes a stopped task externally and restores it after a fresh window", async () => {
  const f = await rpc("fixture");
  const mountReal = () => {
    const port = createRealPort({ actorId: "local-owner", registry: true, transport: { request: rpc, subscribe: () => () => {} } });
    history.replaceState(null, "", `/m/local/p/${f.projectId}/board`);
    const root = document.createElement("div"); document.body.append(root);
    dispose = render(() => <Workspace port={port} />, root); return root;
  };
  const until = async (fn: () => boolean) => { const end = Date.now() + 8000; while (!fn() && Date.now() < end) await new Promise(r => setTimeout(r, 25)); expect(fn()).toBe(true); };
  let root = mountReal();
  await until(() => !!root.querySelector(".card"));
  externalAction(`Actions for ${root.querySelector(".card-t")!.textContent}`)!.click(); await settle();
  [...document.querySelectorAll<HTMLButtonElement>("dialog button")].find(b => b.textContent === "Close task")!.click();
  await until(() => root.textContent?.includes("Done outside the app") === true && !root.querySelector("dialog[open]"));
  const saved = (await rpc("machine.call", { op: "product.read", projectId: f.projectId })).result.tasks[f.taskId];
  expect(saved).toMatchObject({ status: "done", closedAs: "external", notVerifiedByCortex: true, acceptance: null, integration: null });
  dispose?.(); root.remove();
  root = mountReal(); await until(() => !!root.querySelector('[data-col="done"] .card'));
  expect(root.textContent).toContain("Done outside the app");
  expect(root.querySelector('[data-col="done"] .card')?.textContent).not.toMatch(/Accepted|Merged/);
});
function externalAction(trigger: string) {
  document.querySelector<HTMLButtonElement>(`button[aria-label="${trigger}"]`)!.click();
  return [...document.querySelectorAll<HTMLButtonElement>('#m-task [role="menuitem"]')].find(b => b.textContent?.startsWith("Done outside the app…"));
}
it("external completion satisfies dependencies without accepted or merged flags", () => {
  const s = withTask(emptyWorkspace(), "External");
  Object.assign(s.tasks[0], { status: "done", closedAs: "external" });
  expect(dependencyReady(s.tasks[0])).toBe(true);
  expect(s.tasks[0].acceptance).toBeNull();
  expect(s.tasks[0].integration).toBeNull();
});
it("cards cannot be dragged or dropped to Done; the menu opens confirmation", async () => {
  const p = await mount(withTask(emptyWorkspace(), "External work"));
  const card = document.querySelector<HTMLElement>(".card")!;
  expect(card.draggable).toBe(false);
  expect([...card.querySelectorAll("a")].every(link => !link.draggable)).toBe(true);
  const drop = new Event("drop", { bubbles: true, cancelable: true });
  Object.defineProperty(drop, "dataTransfer", { value: { getData: () => "t" } });
  document.querySelector('[data-col="done"]')!.dispatchEvent(drop);
  expect(drop.defaultPrevented).toBe(false);
  expect(document.querySelector("dialog[open]")).toBeNull();
  expect(p.commands).toHaveLength(0);
  externalAction("Actions for External work")!.click(); await settle();
  expect(document.querySelector("dialog[open]")?.textContent).toContain("Done outside the app");
  expect(p.commands).toHaveLength(0);
  const confirm = [...document.querySelectorAll<HTMLButtonElement>("dialog button")].find(b => b.textContent === "Close task")!;
  confirm.click(); await settle();
  expect(p.commands.at(-1)?.payload).toMatchObject({ type: "close_task", taskId: "t", choice: "external", confirmed: true });
  expect(document.querySelector("dialog[open] .ws-request-status")?.textContent).toBe("Requesting closure…");
  expect(document.querySelector('[data-col="done"] .card')).toBeNull();
  p.receipt(false, undefined, "unknown_outcome", "Closure could not be confirmed.");
  await settle();
  expect(document.querySelector("dialog[open] .ws-request-status")?.textContent).toBe("");
  expect(document.body.textContent).toContain("Closure could not be confirmed.");
  expect(p.commands).toHaveLength(1);
});
it("running cards explain Stop first without opening the dialog or issuing a command", async () => {
  const s = withTask(emptyWorkspace(), "Running"); s.tasks[0].status = "working"; s.tasks[0].process = "live";
  const p = await mount(s);
  const entry = externalAction("Actions for Running")!;
  expect(entry.getAttribute("aria-disabled")).toBe("true");
  expect(entry.textContent).toContain("Stop the task");
  entry.click(); await settle();
  expect(document.querySelector("dialog[open]")).toBeNull();
  expect(p.commands).toHaveLength(0);
});

it("a paused card names the exact blocker and sends only the bound external close", async () => {
  const s = withTask(emptyWorkspace(), "Paused external");
  Object.assign(s.tasks[0], { status: "paused", process: "stopped", executionId: "we_7", sourceVersion: "13",
    warnings: ["external_close_waiting_for_exit"], issue: "backend-dev (worker-fixture)" });
  const p = await mount(s);
  externalAction("Actions for Paused external")!.click(); await settle();
  const dialog = document.querySelector("dialog[open]")!;
  expect(dialog.textContent).toContain("backend-dev (worker-fixture)");
  expect(dialog.textContent).toContain("sprint queue stays paused");
  const check = [...dialog.querySelectorAll("button")].find(b => b.textContent === "Check stop and close")!;
  check.click(); await settle();
  expect(p.commands.map(entry => entry.payload)).toMatchObject([
    {type: "close_task", taskId: "t", choice: "external", executionId: "we_7", expectedVersion: "13", confirmed: true},
  ]);
  p.receipt(false, undefined, "external_stop_pending", "Still waiting for worker-fixture.");
  await settle();
  expect(dialog.textContent).toContain("Still waiting for worker-fixture.");
  expect(document.querySelector("dialog[open]")).toBe(dialog);
  p.emit(projection("2", {...s, tasks: [{ ...s.tasks[0], sourceVersion: "14", warnings: [], issue: null }]})); await settle();
  check.click(); await settle();
  expect(p.commands.at(-1)?.payload).toMatchObject({choice: "external", executionId: "we_7", expectedVersion: "14"});
  p.receipt();
  await settle();
  expect(document.querySelector("dialog[open]")).toBeNull();
});

it("an open external confirmation cannot apply to a newly started execution", async () => {
  const s = withTask(emptyWorkspace(), "New execution");
  const p = await mount(s);
  externalAction("Actions for New execution")!.click(); await settle();
  p.emit(projection("2", {...s, tasks: [{ ...s.tasks[0], status: "paused", process: "stopped", executionId: "new", sourceVersion: "2" }]})); await settle();
  const confirm = [...document.querySelectorAll<HTMLButtonElement>("dialog[open] button")].find(b => b.textContent === "Close task")!;
  expect(confirm.dataset.why).toContain("execution changed");
  confirm.click(); await settle();
  expect(p.commands).toHaveLength(0);
});

it.each([
  ["List", `/m/${LOCAL_MACHINE_ID}/p/p/list`, "Actions for External work"],
  ["task title band", `/m/${LOCAL_MACHINE_ID}/t/t`, "Task actions"],
])("%s uses the same external closure menu", async (_name, route, trigger) => {
  const p = await mount(withTask(emptyWorkspace(), "External work"), route);
  externalAction(trigger)!.click(); await settle();
  expect(document.querySelector("dialog[open] h2")?.textContent).toBe("Done outside the app");
  expect(p.commands).toHaveLength(0);
});

it("the task command palette opens the same confirmation by keyboard", async () => {
  const p = await mount(withTask(emptyWorkspace(), "External work"), `/m/${LOCAL_MACHINE_ID}/t/t`);
  document.dispatchEvent(new KeyboardEvent("keydown", { key: "k", ctrlKey: true, bubbles: true, cancelable: true }));
  await settle();
  const field = document.querySelector<HTMLInputElement>("dialog[open] .ws-pal-in")!;
  field.value = ">done outside";
  field.dispatchEvent(new Event("input", { bubbles: true }));
  field.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));
  await settle();
  expect(document.querySelector("dialog[open] h2")?.textContent).toBe("Done outside the app");
  expect(p.commands).toHaveLength(0);
});

it("a completed task no longer offers external closure", async () => {
  const s = withTask(emptyWorkspace(), "Closed");
  Object.assign(s.tasks[0], { status: "done", closedAs: "external" });
  await mount(s);
  expect(externalAction("Actions for Closed")).toBeUndefined();
});
for (const remote of [false, true]) it(`content already in the branch offers confirmed closure (remote=${remote})`, async () => {
  const s = withTask(emptyWorkspace(), "Contained"); const t = s.tasks[0];
  Object.assign(t, { status: "ready_to_merge", result: { version: 1, digest: "d", summary: "Done", checks: [] }, workspaceDigest: "d",
    integrationFacts: { alreadyInTarget: true, targetBranch: "main", pushStatus: "unknown" } });
  if (remote) s.repos.push({ projectId: "p", remote: "origin", tree: [], sources: {}, commits: [], branches: [], checkouts: [], fetchedAt: null });
  const p = await mount(s, `/m/${LOCAL_MACHINE_ID}/t/t/changes`);
  expect(document.body.textContent).toContain("Already in main");
  if (remote) expect(document.body.textContent).toContain("Push status unknown");
  else expect(document.body.textContent).not.toMatch(/push|upstream/i);
  const labels = [...document.querySelectorAll("button")].map(b => b.textContent?.trim());
  expect(labels).not.toContain("Merge…"); expect(labels).not.toContain("Review and merge…"); expect(labels).not.toContain("Accept v1");
  const close = [...document.querySelectorAll<HTMLButtonElement>("button")].find(b => b.textContent === "Close task…")!;
  close.click(); await settle(); expect(p.commands).toHaveLength(0);
  [...document.querySelectorAll<HTMLButtonElement>("dialog button")].find(b => b.textContent === "Close task")!.click(); await settle();
  expect(p.commands.at(-1)?.payload).toMatchObject({ choice: "already_in_target", confirmed: true });
});
