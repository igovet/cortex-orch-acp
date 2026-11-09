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
  drag("done"); await settle();
  [...document.querySelectorAll<HTMLButtonElement>("dialog button")].find(b => b.textContent === "Close task")!.click();
  await until(() => root.textContent?.includes("Done outside the app") === true && !root.querySelector("dialog[open]"));
  const saved = (await rpc("machine.call", { op: "product.read", projectId: f.projectId })).result.tasks[f.taskId];
  expect(saved).toMatchObject({ status: "done", closedAs: "external", notVerifiedByCortex: true, acceptance: null, integration: null });
  dispose?.(); root.remove();
  root = mountReal(); await until(() => !!root.querySelector('[data-col="done"] .card'));
  expect(root.textContent).toContain("Done outside the app");
  expect(root.querySelector('[data-col="done"] .card')?.textContent).not.toMatch(/Accepted|Merged/);
});
function drag(column: string) {
  const data = new Map<string, string>();
  const transfer = { setData: (k: string, v: string) => data.set(k, v), getData: (k: string) => data.get(k) ?? "", effectAllowed: "", dropEffect: "" };
  for (const [element, type] of [[document.querySelector(".card")!, "dragstart"], [document.querySelector(`[data-col="${column}"]`)!, "drop"]] as const) {
    const event = new Event(type, { bubbles: true, cancelable: true });
    Object.defineProperty(event, "dataTransfer", { value: transfer }); element.dispatchEvent(event);
  }
}
it("external completion satisfies dependencies without accepted or merged flags", () => {
  const s = withTask(emptyWorkspace(), "External");
  Object.assign(s.tasks[0], { status: "done", closedAs: "external" });
  expect(dependencyReady(s.tasks[0])).toBe(true);
  expect(s.tasks[0].acceptance).toBeNull();
  expect(s.tasks[0].integration).toBeNull();
});
it("only Done opens confirmation; no command until the owner confirms", async () => {
  const p = await mount(withTask(emptyWorkspace(), "External work"));
  expect(document.querySelector(".card")?.getAttribute("draggable")).toBe("true");
  drag("review"); await settle();
  expect(document.body.textContent).toContain("Only Done");
  expect(p.commands).toHaveLength(0);
  drag("done"); await settle();
  expect(document.querySelector("dialog[open]")?.textContent).toContain("Done outside the app");
  expect(p.commands).toHaveLength(0);
  const confirm = [...document.querySelectorAll<HTMLButtonElement>("dialog button")].find(b => b.textContent === "Close task")!;
  confirm.click(); await settle();
  expect(p.commands.at(-1)?.payload).toMatchObject({ type: "close_task", taskId: "t", choice: "external", confirmed: true });
});
it("running cards explain Stop first instead of closing or pretending to pass checks", async () => {
  const s = withTask(emptyWorkspace(), "Running"); s.tasks[0].status = "working"; s.tasks[0].process = "live";
  const p = await mount(s); drag("done"); await settle();
  expect(document.querySelector("dialog[open]")?.textContent).toContain("Stop the task");
  expect([...document.querySelectorAll("dialog button")].some(b => b.textContent === "Close task")).toBe(false);
  expect(p.commands).toHaveLength(0);
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
