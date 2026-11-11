import { createConnection } from "node:net";
import { render } from "solid-js/web";
import { afterEach, beforeAll, expect, it } from "vitest";
import { Workspace } from "../src";
import { createRealPort, type RealTransport } from "../src/port/real";
import type { WorkspaceCommand } from "@cortex/workspace-domain";

const socket = process.env.WORKSPACE_CONTRACT_SOCKET;
type Fixture = { projectId: string; taskId: string; root: string; hasGit: boolean; technical: boolean; blocked: boolean };
let fixture: Fixture;
const calls: { operation: string; message: any }[] = [];
const replies: any[] = [];
function rpc(operation: string, message: unknown = {}): Promise<any> {
  return new Promise((resolve, reject) => {
    const stream = createConnection(socket!);
    let data = "";
    stream.setTimeout(12000, () => stream.destroy(new Error("fixture RPC timeout")));
    stream.on("error", reject);
    stream.on("connect", () => stream.write(JSON.stringify({ operation, message }) + "\n"));
    stream.on("data", chunk => {
      data += chunk.toString();
      if (!data.includes("\n")) return;
      stream.end();
      const reply = JSON.parse(data);
      if (reply.error) reject(new Error(reply.error)); else resolve(reply.result);
    });
  });
}
const transport: RealTransport = {
  async request(operation, message) { calls.push({ operation, message }); const reply = await rpc(operation, message); if (operation === "machine.call" && (message as any).params?.action === "accept") replies.push(reply); return reply; },
  subscribe() { return () => {}; },
};
const port = () => createRealPort({ transport, actorId: "local-owner", registry: true });
const command = (p: ReturnType<typeof port>, payload: WorkspaceCommand) =>
  p.command({ requestId: crypto.randomUUID(), operation: payload.type, expectedVersion: null, payload });
const text = () => document.body.textContent ?? "";
const button = (label: string) => [...document.querySelectorAll<HTMLButtonElement>("button")].find(b => b.textContent?.trim() === label);
let dispose: (() => void) | undefined;
async function until(test: () => boolean) {
  const deadline = Date.now() + 12000;
  while (!test() && Date.now() < deadline) await new Promise(r => setTimeout(r, 25));
  expect(test(), document.querySelector(".ws-receipt")?.textContent ?? text()).toBe(true);
}
async function mount(path: string) {
  localStorage.clear(); sessionStorage.clear();
  history.replaceState(null, "", path);
  const root = document.createElement("div"); document.body.append(root);
  dispose = render(() => <Workspace port={port()} />, root);
}
afterEach(() => { dispose?.(); dispose = undefined; document.body.innerHTML = ""; });
beforeAll(async () => { if (socket) fixture = await rpc("fixture"); });

it.skipIf(!socket)("Accept binds the real source version, rejects stale review, and persists acceptance for Git and plain folders", async () => {
  const p = port();
  await p.query("capabilities.get", {});
  const [update] = await p.query("project.snapshot", {});
  expect(update.scope).toBe("project");
  if (update.scope !== "project") throw new Error("project projection required");
  let result = update.slice.tasks[0].result!;
  expect(result.version).toBe(1);
  expect(result.sourceVersion).not.toBe("1");
  const staleVersion = await rpc("machine.call", { op: "product.task", projectId: fixture.projectId,
    params: { action: "accept", taskId: fixture.taskId, digest: result.digest, version: result.version } });
  expect(staleVersion).toMatchObject({ ok: false, code: fixture.blocked ? "checks_required" : "stale_result" });
  const refused = await command(p, { type: "accept_result", taskId: fixture.taskId, digest: "old-result", now: Date.now() });
  expect(refused.ok).toBe(false);
  expect((await rpc("machine.call", { op: "product.read", projectId: fixture.projectId })).result.tasks[fixture.taskId].acceptance).toBeUndefined();

  await mount(`/m/local/t/${fixture.taskId}/changes/diff`);
  await until(() => !!document.querySelector(".rv-bar"));
  if (!fixture.hasGit) await until(() => text().includes("Changes were applied in the project folder"));
  if (!fixture.hasGit) {
    expect(text()).toContain("Changes were applied in the project folder");
    expect(text()).toContain("No diff without Git");
    expect(button("Commit…")).toBeUndefined();
    // Real no-Git file snapshot + Code read, not invented diff data.
    const facts = (await p.query("project.facts", {}))[0];
    expect(facts.repo?.tree).toContain("feature.py");
    const file = await rpc("machine.call", { op: "repo.file", projectId: fixture.projectId,
      params: { checkoutId: fixture.projectId + ":main", file: "feature.py" } });
    expect(file.result.text).toBe("pass\n");
  }
  history.pushState(null, "", `/m/local/t/${fixture.taskId}/changes`);
  window.dispatchEvent(new PopStateEvent("popstate"));
  await until(() => !!button("Accept v1"));
  if (fixture.blocked) {
    expect(button("Accept v1")!.getAttribute("aria-disabled")).toBe("true");
    expect(button("Rerun check")).toBeDefined();
    expect(button("Ask to fix")).toBeDefined();
    button("Accept v1")!.click();
    expect(document.querySelector('dialog[aria-label="Accept with open checks"]')?.hasAttribute("open")).not.toBe(true);
    const refusal = await command(p, { type: "accept_result", taskId: fixture.taskId, digest: result.digest, now: Date.now() });
    expect(refusal.ok).toBe(false);
    const current = await rpc("fixture.result");
    expect(current.acceptanceAvailable).toBe(false);
    expect(current.checks.some((c: any) => c.outcome === "failed" && c.exitCode === 127)).toBe(true);
    expect((await rpc("machine.call", { op: "product.read", projectId: fixture.projectId })).result.tasks[fixture.taskId].acceptance).toBeUndefined();
    return;
  }
  expect(button("Accept v1")!.getAttribute("aria-disabled")).toBeNull();
  if (fixture.technical) {
    expect(text()).not.toContain("verification_executable_unavailable");
    expect(text()).not.toMatch(/\br_[a-f0-9]{12}\b/);
    expect(text()).toContain("not available on this machine");
    button("Rerun check")!.click();
    await until(() => document.querySelector(".ws-receipt")?.textContent?.includes("acknowledgement") === true);
    expect(text()).toContain("not available on this machine");
    const [current] = await p.query("project.snapshot", {});
    if (current.scope !== "project") throw new Error("project required");
    result = current.slice.tasks[0].result!;
  }
  button("Accept v1")!.click();
  if (fixture.technical) {
    await until(() => text().includes("knowing these checks did not pass"));
    const dialog = document.querySelector('dialog[aria-label="Accept with open checks"]')!;
    const confirm = dialog.querySelector<HTMLButtonElement>("[data-bar=\"dialog\"] .btn-primary")!;
    expect(confirm.getAttribute("aria-disabled")).toBe("true");
    confirm.click();
    expect((await rpc("machine.call", { op: "product.read", projectId: fixture.projectId })).result.tasks[fixture.taskId].acceptance).toBeUndefined();
    const tick = dialog.querySelector<HTMLInputElement>('input[type="checkbox"]')!;
    tick.click();
    expect(tick.checked).toBe(true);
    expect(confirm.isConnected).toBe(true);
    expect(confirm.getAttribute("aria-disabled")).toBeNull();
    expect(confirm.disabled).toBe(false);
    confirm.click();
    await until(() => replies.length > 0);
    const observed = await rpc("machine.call", {op:"product.read",projectId:fixture.projectId});
    expect(observed.result.tasks[fixture.taskId].acceptance, JSON.stringify(replies)).toBeDefined();
    expect(calls.filter(c => c.operation === "machine.call" && c.message.params?.action === "accept").length).toBeGreaterThan(0);
  }
  await until(() => text().includes("Accepted v1"));
  const product = await rpc("machine.call", { op: "product.read", projectId: fixture.projectId });
  expect(product.result.tasks[fixture.taskId].acceptance).toMatchObject({ version: result.sourceVersion, digest: result.digest });
  expect(product.result.tasks[fixture.taskId].status).toBe("ready_to_merge");
  await until(() => text().includes("Accepted v1"));
  expect(text()).not.toContain(`Accepted v${result.sourceVersion}`);
  const sent = calls.filter(c => c.operation === "machine.call" && c.message.op === "product.task" && c.message.params.action === "accept").at(-1)!;
  expect(sent.message.params.version).toBe(result.sourceVersion);
  if (!fixture.hasGit) expect((await rpc("machine.call", { op: "repo.snapshot", projectId: fixture.projectId })).result.hasGit).toBe(false);
});

it.skipIf(!socket)("worker overview does not call a real persisted withdrawn probe a failed check", async () => {
  await mount(`/m/local/t/${fixture.taskId}`);
  await until(() => text().includes("backend-dev"));
  const workers = await rpc("machine.call", { op: "workers", projectId: fixture.projectId, params: { taskIds: [fixture.taskId] } });
  const implementer = workers.result.find((w: any) => w.lane === "execution");
  // The worker's original declaration stays not_run; the runtime's actual
  // failed attempt is in the task result, checked above.
  expect(implementer.result.checks.some((c: any) => c.outcome === "failed")).toBe(false);
  expect(text()).not.toContain("1 check failed");
  if (fixture.blocked) expect(implementer.result.checks[0].outcome).toBe("not_run");
});

it.skipIf(!socket)("the plain-folder Git tab offers explicit init, and its real port action creates an empty repository", async () => {
  if (fixture.hasGit) return;
  await mount(`/m/local/p/${fixture.projectId}/git`);
  await until(() => !!button("Create empty repository"));
  expect(text()).not.toContain("No uncommitted changes");
  button("Create empty repository")!.click();
  await until(() => !!button("Create repository"));
  button("Create repository")!.click();
  await until(() => !button("Create empty repository") || !!document.querySelector(".ws-receipt"));
  const snapshot = await rpc("machine.call", { op: "repo.snapshot", projectId: fixture.projectId });
  expect(snapshot.result.hasGit).toBe(true);
  const init = calls.find(c => c.operation === "machine.call" && c.message.op === "git");
  expect(init?.message.params).toMatchObject({ action: "git_init", confirmed: true });
  const files = await rpc("machine.call", { op: "repo.file", projectId: fixture.projectId,
    params: { checkoutId: fixture.projectId + ":main", file: "feature.py" } });
  expect(files.result.text).toBe("pass\n");
});
