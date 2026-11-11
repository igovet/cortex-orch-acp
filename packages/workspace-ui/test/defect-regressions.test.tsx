import { render } from "solid-js/web";
import { afterEach, beforeEach, expect, it } from "vitest";
import { emptyWorkspace, LOCAL_MACHINE_ID, type Sprint, type Worker } from "@cortex/workspace-domain";
import { Workspace } from "../src";
import { buildIndex, inScope, sprintView } from "../src/selectors";
import { capabilitiesWire, flush, projection, scriptedPort, scriptedTransport, withTask } from "./support";
import { createRealPort } from "../src/port/real";

let dispose: (() => void) | undefined;
beforeEach(() => { localStorage.clear(); sessionStorage.clear(); });
afterEach(() => { dispose?.(); document.body.innerHTML = ""; });
const settle = async () => { for (let i = 0; i < 8; i++) await flush(); };
const buttons = () => [...document.querySelectorAll<HTMLButtonElement>("button")];
const button = (text: string) => buttons().find(b => b.textContent?.trim() === text)!;
const world = () => { const s = withTask(emptyWorkspace(), "Work"); s.machines[0].connection = "connected"; return s; };
async function mount(state: ReturnType<typeof world>, path: string) {
  const s = scriptedPort({ capabilities: { everything: true, operations: [] } });
  history.replaceState(null, "", path);
  const root = document.createElement("div"); document.body.append(root);
  dispose = render(() => <Workspace port={s.port} />, root);
  await settle(); s.emit(projection("1", state)); await settle();
  return s;
}
const taskPath = `/m/${LOCAL_MACHINE_ID}/t/t`;
const review = () => {
  const s = world(), t = s.tasks[0];
  t.status = "review"; t.workspaceDigest = "digest";
  t.result = { version: 1, digest: "digest", summary: "Implemented the change", checks: [{ name: "Focused tests", detail: "Needs another run", outcome: "failed", command: ["pytest"], rerunnable: true }],
    evidence: [{ reference: "r_f530581c44c2", digest: "a".repeat(64) }] };
  return s;
};

it("selects an offered usable route rather than a stale machine default", async () => {
  const s = world();
  s.machines[0].config.surfaces.user_defaults.coordinator = { engine: "claude-agent-acp", model: "opus[1m]", effort: "high", parameters: {} };
  s.options.models = [{ id: "codex-luna", label: "6 Luna", engine: "Codex", efforts: ["low"] }, { id: "offline", label: "Offline", engine: "Claude", unavailable: "Sign in first" }];
  await mount(s, `/m/${LOCAL_MACHINE_ID}/new?project=p`);
  expect(document.querySelector('[aria-label="Coordinator model: 6 Luna"]')).not.toBeNull();
});

it("leaves no unavailable route selected when every offer is unavailable", async () => {
  const s = world();
  s.options.models = [{ id: "offline", label: "Offline", engine: "Claude", unavailable: "Sign in first" }];
  const p = await mount(s, `/m/${LOCAL_MACHINE_ID}/new?project=p`);
  const field = document.querySelector<HTMLTextAreaElement>('[aria-label="Describe the task"]')!;
  field.value = "Do this"; field.dispatchEvent(new Event("input", { bubbles: true }));
  document.querySelector<HTMLButtonElement>('[aria-label="Save draft"]')!.click();
  expect((p.commands[0].payload as { settings: { model: string } }).settings.model).toBe("");
});

it("keeps result evidence identities out of the rendered result", async () => {
  await mount(review(), taskPath);
  expect(document.querySelector(".rcard")).not.toBeNull();
  expect(document.querySelector(".rcard")!.textContent).not.toMatch(/r_f530581c44c2|a{64}/);
});

it("disables rerun while pending, blocks repeat input and releases on refusal", async () => {
  const p = await mount(review(), taskPath);
  const rerun = document.querySelector<HTMLButtonElement>('[aria-label="Rerun Focused tests"]')!;
  rerun.click(); rerun.click();
  expect(p.commands.filter(c => c.operation === "rerun_check")).toHaveLength(1);
  expect(rerun.disabled).toBe(true);
  expect(rerun.getAttribute("aria-busy")).toBe("true");
  p.receipt(false); await settle();
  expect(rerun.disabled).toBe(false);
});

it("disables Send and reopen for the entire request and preserves input on failure", async () => {
  const p = await mount(review(), taskPath);
  button("Request changes…").click(); await settle();
  const field = document.querySelector<HTMLTextAreaElement>("dialog textarea")!;
  field.value = "Please fix"; field.dispatchEvent(new Event("input", { bubbles: true }));
  const send = document.querySelector<HTMLButtonElement>('dialog [aria-label="Send and reopen"]')!; send.click(); send.click();
  expect(p.commands.filter(c => c.operation === "request_changes")).toHaveLength(1);
  expect(send.disabled).toBe(true);
  expect(send.getAttribute("aria-busy")).toBe("true");
  p.receipt(false); await settle();
  expect(send.disabled).toBe(false); expect(field.value).toBe("Please fix");
});

it("shows a worker's role and actual state without claiming it is unassigned", async () => {
  const s = world(); s.tasks[0].status = "working"; s.tasks[0].process = "live"; s.tasks[0].executionId = "e";
  const worker: Worker = { id: "w", taskId: "t", executionId: "e", role: "frontend-dev", status: "completed", lane: "execution", goal: "Implement UI", now: null, waitsFor: null, scope: [], route: null, attempt: null, elapsedMs: null, runningSince: null, context: null, usage: null, activity: [], result: null };
  s.workers = [worker];
  await mount(s, taskPath + "/workers/w");
  expect(document.body.textContent).toContain("frontend-dev");
  expect(document.body.textContent).not.toMatch(/not assigned/i);
});

it("uses the editor's sprint membership for the scoped board and grouping", async () => {
  const s = world();
  const sprint: Sprint = { id: "sp", projectId: "p", title: "Release", description: "Release", runMode: "manual", targetBranch: null, planning: "created", conversation: [], plan: null, materializedVersion: 1, taskIds: ["t"], queueState: "idle", pauseReason: null };
  s.sprints = [sprint]; // Older task projections may omit the reverse link.
  const index = buildIndex(s, Date.now()), summary = index.summaries.get("p")!;
  expect(inScope(summary, { kind: "sprint", id: "sp" }).map(v => v.id)).toEqual(sprintView(index, sprint).views.map(v => v.id));
  expect(index.byId.get("t")!.sprint?.id).toBe("sp");
  expect(inScope(summary, { kind: "standalone" })).toHaveLength(0);
  await mount(s, `/m/${LOCAL_MACHINE_ID}/p/p/s/sp/board`);
  expect(document.querySelector('.card[data-task="t"]')).not.toBeNull();
});

it("reports missing sprint task details instead of pretending the sprint has no tasks", async () => {
  const s = world();
  s.sprints = [{ id: "sp", projectId: "p", title: "Release", description: "Release", runMode: "manual", targetBranch: null, planning: "created", conversation: [], plan: null, materializedVersion: 1, taskIds: ["missing"], queueState: "idle", pauseReason: null }];
  await mount(s, `/m/${LOCAL_MACHINE_ID}/p/p/s/sp/board`);
  expect(sprintView(buildIndex(s, Date.now()), s.sprints[0]).state).not.toContain("0 of 0");
  expect(document.body.textContent).not.toContain("No tasks in this sprint yet");
  expect(document.body.textContent).toContain("Task details unavailable");
  expect(button("Retry")).toBeTruthy();
});

it("offers only logged-in, enabled, non-denied routes from the machine catalogue", async () => {
  const config = world().machines[0].config;
  config.catalogue["codex-acp"].models = ["luna", "disabled", "denied"].map(id => ({ id, routeId: "route-" + id, label: id, efforts: ["low"], observed: true }));
  config.catalogue["claude-agent-acp"].models = [{ id: "opus", label: "Opus", efforts: ["high"], observed: true }];
  config.surfaces.routing.engines["codex-acp"].enabled = ["luna", "denied"];
  config.surfaces.routing.engines["claude-agent-acp"].enabled = ["opus"];
  config.surfaces.routing.deny = ["codex-acp/denied"];
  config.setup["codex-acp"].status = "connected";
  config.setup["claude-agent-acp"].status = "downloaded_no_login";
  const s = scriptedTransport({
    "service.capabilities": () => ({ methods: ["machine.call"], machine: true }),
    "projects.list": () => [],
    "capabilities.get": () => capabilitiesWire(["capabilities.get"]),
    "machine.call": message => ({ ok: true, result: (message as { op: string }).op === "config.read" ? { config, connections: [], tokens: {} } : {} }),
  });
  const port = createRealPort({ transport: s.transport, actorId: "owner", registry: true });
  await port.query("capabilities.get", {});
  const [facts] = await port.query("machine.facts", {});
  expect(facts.options?.models.map(m => m.id)).toEqual(["route-luna"]);
});
