import { render } from "solid-js/web";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { emptyWorkspace, LOCAL_MACHINE_ID, type Message, type Worker, type WorkerRoute } from "@cortex/workspace-domain";
import { Workspace } from "../src";
import { createRealPort } from "../src/port/real";
import { routeLabel, workerModel } from "../src/session-model";
import { capabilitiesWire, flush, scriptedTransport, snapshot, wireTask } from "./support";

/** The coordinator runs on Claude Fable 5.2; runtime routing sent each worker to its own engine and model. The screens must show
 *  each worker's own recorded route, the coordinator's only for the coordinator, and an honest unknown where none is confirmed. */
const OFFERED = ["capabilities.get", "project.snapshot", "tasks.create", "tasks.start", "decisions.list", "decisions.answer", "results.get", "executions.get", "history.page"];
const repo = { projectId: "p", remote: "", tree: ["a.txt"], sources: {}, commits: [], branches: [], fetchedAt: null, openBranch: "dev", defaultBranch: "main",
  checkouts: [{ id: "p:main", projectId: "p", kind: "main", branch: "dev", path: "/cedar", taskId: null, label: null, base: "main", ahead: 0, behind: 0, behindOf: {}, draft: "", sources: {}, updateConflict: null, files: [] }] };
const SOL: WorkerRoute = { engine: "codex-acp", model: "gpt-6.1-sol", effort: "medium" };
const SONNET: WorkerRoute = { engine: "claude-agent-acp", model: "sonnet", effort: "high" };
const worker = (patch: Partial<Worker>): Worker => ({ id: "w1", taskId: "task-1", executionId: "e", role: "backend-dev", status: "working", lane: "execution", goal: "Change the code",
  now: null, waitsFor: null, scope: [], route: null, attempt: null, elapsedMs: null, runningSince: null, context: null, usage: null,
  activity: [{ sequence: 1, text: "Reading", kind: "note", at: null }], result: null, ...patch });
const WORKERS = [
  worker({ id: "w1", role: "backend-dev", route: SOL }),
  worker({ id: "w2", role: "code-reviewer", lane: "review", route: SONNET }),
  worker({ id: "w3", role: "docs-writer", route: null }),
  worker({ id: "w4", role: "test-engineer", status: "pending", route: null, activity: [] }),
];

function machine() {
  const config = structuredClone(emptyWorkspace().machines[0].config);
  config.surfaces.user_defaults = { ...config.surfaces.user_defaults, coordinator: { engine: "claude-agent-acp", model: "fable-5.2", effort: "high", parameters: {} }, mode: "auto" };
  config.catalogue["claude-agent-acp"].models = [{ id: "fable-5.2", label: "Fable 5.2", efforts: ["medium", "high"], observed: true }, { id: "sonnet", label: "Sonnet", efforts: ["medium", "high"], observed: true }];
  config.catalogue["codex-acp"].models = [{ id: "gpt-6.1-sol", label: "Sol", efforts: ["medium", "high"], observed: true }];
  for (const engine of ["claude-agent-acp", "codex-acp"] as const) config.setup[engine].status = "connected";
  config.surfaces.routing.engines["claude-agent-acp"].enabled = ["fable-5.2", "sonnet"];
  config.surfaces.routing.engines["codex-acp"].enabled = ["gpt-6.1-sol"];
  const task = wireTask({ executionId: "e", phase: "execution", runtimeState: "EXECUTING", executionState: "running", coordinatorRoute: { model: "fable-5.2", effort: "high" } });
  const product = { tasks: {}, sprints: [], messages: [] as Message[] };
  return scriptedTransport({
    "service.capabilities": () => ({ methods: ["machine.call", "executions.control"], machine: true }),
    "projects.list": () => [{ projectId: "p", displayName: "Cedar", canonicalRoot: "/cedar", activation: {} }],
    "capabilities.get": () => capabilitiesWire(OFFERED),
    "project.snapshot": () => snapshot("4", [task]),
    "decisions.list": () => ({ items: [], nextCursor: null, highWatermark: "4" }),
    "history.page": () => ({ items: [], nextCursor: null, highWatermark: "0" }),
    "executions.get": () => ({ executionId: "e", taskId: "task-1", version: "2", runtimeTaskId: "r", acpSessionId: "s", runtimePlanRevision: "0", ownerEpoch: "3", state: "running", warnings: [], resultId: null }),
    "machine.call": message => {
      const { op } = message as { op: string };
      if (op === "repo.snapshot") return { ok: true, result: repo };
      if (op === "product.read") return { ok: true, result: product };
      if (op === "workers") return { ok: true, result: WORKERS };
      if (op === "overview") return { ok: true, result: { name: "Desk", retentionDays: 30, autoArchiveDays: 7, worktreeRoot: "~/t", version: "1", diskMb: 2, storage: [], cleanup: null, diagnostics: null } };
      if (op === "config.read") return { ok: true, result: { config, connections: [], tokens: {}, usage: [] } };
      if (op === "project.config.read") return { ok: true, result: { value: { revisions: {} }, tokens: {} } };
      return { ok: true, result: {} };
    },
  });
}

let dispose: (() => void) | undefined;
beforeEach(() => { localStorage.clear(); sessionStorage.clear(); });
afterEach(() => { dispose?.(); dispose = undefined; document.body.innerHTML = ""; });
const settled = async () => { for (let i = 0; i < 12; i++) await flush(); };
async function open(worker: string) {
  history.replaceState(null, "", `/m/${LOCAL_MACHINE_ID}/t/task-1/workers/${worker}`);
  const root = document.createElement("div");
  document.body.append(root);
  dispose = render(() => <Workspace port={createRealPort({ transport: machine().transport, actorId: "owner", registry: true })} />, root);
  await settled();
}
const text = (selector: string) => document.querySelector(selector)?.textContent ?? "";
const card = (role: string) => [...document.querySelectorAll(".wcard")].find(c => c.textContent?.includes(role))?.textContent ?? "";
const selectedWorker = () => [...document.querySelectorAll(".inspector .ins-sec")].find(s => s.textContent?.startsWith("Selected worker"))?.textContent ?? "";

describe("a worker's own route", () => {
  it("labels a recorded route with its engine and that engine's model name, and an unconfirmed one honestly", () => {
    const options = { models: [], efforts: [], modes: [], languages: [] };
    const machines = [{ config: { catalogue: { "codex-acp": { models: [{ id: "gpt-6.1-sol", label: "Sol" }] } } } }] as never;
    expect(routeLabel(SOL, options, machines)).toBe("Codex · Sol, Medium");
    expect(routeLabel(SONNET, options, machines)).toBe("Claude · sonnet, High");
    expect(routeLabel({ model: "opus", effort: "high" }, options)).toBe("opus, High");
    const label = (r: WorkerRoute | null) => routeLabel(r, options, machines);
    expect(workerModel({ route: null, status: "working" }, label)).toBe("Not confirmed");
    expect(workerModel({ route: null, status: "pending" }, label)).toBe("Chosen when it starts");
    expect(workerModel({ route: SOL, status: "completed" }, label)).toBe("Codex · Sol, Medium");
  });

  it("names a model by the engine's recorded name, then the catalogue, and never a bare alias id", () => {
    const options = { models: [], efforts: [], modes: [], languages: [] };
    const machines = [{ config: { catalogue: { "claude-agent-acp": { models: [{ id: "default", label: "Catalogue name" }] } } } }] as never;
    const alias: WorkerRoute = { engine: "claude-agent-acp", model: "default", effort: "high" };
    expect(routeLabel({ ...alias, modelLabel: "Default (Opus 5.5)" }, options, machines)).toBe("Claude · Default (Opus 5.5), High");
    expect(routeLabel(alias, options, machines)).toBe("Claude · Catalogue name, High");
    expect(routeLabel(alias, options, [])).toBe("Claude · Default (engine alias), High");
    expect(routeLabel({ ...alias, model: "haiku" }, options, [])).toBe("Claude · haiku, High");
  });

  it("shows each of two workers on different routes their own engine and model, and the coordinator's model only for the coordinator", async () => {
    await open("w1");
    expect(text(".livepane .d-title")).toContain("backend-dev");
    expect(text(".livepane .d-grid")).toContain("Codex · Sol, Medium");
    expect(text(".livepane .d-grid")).not.toContain("Fable");
    expect(selectedWorker()).toContain("ModelCodex · Sol, Medium");
    expect(selectedWorker()).not.toContain("Fable");
    // The task's route is named as the coordinator's, never as the selected worker's model.
    expect(text(".inspector")).toContain("Coordinator modelFable 5.2, High");
    expect(card("backend-dev")).toContain("Codex · Sol, Medium");
    expect(card("code-reviewer")).toContain("Claude · Sonnet, High");
    expect(card("Coordinator")).toContain("Fable 5.2, High");
    for (const role of ["backend-dev", "code-reviewer", "docs-writer", "test-engineer"]) expect(card(role)).not.toContain("Fable");
    dispose?.(); document.body.innerHTML = "";

    await open("w2");
    expect(text(".livepane .d-grid")).toContain("Claude · Sonnet, High");
    expect(selectedWorker()).toContain("ModelClaude · Sonnet, High");
    expect(text(".livepane .d-grid")).not.toContain("Fable");
    dispose?.(); document.body.innerHTML = "";

    await open("cortex");
    expect(text(".livepane .d-title")).toContain("Coordinator");
    expect(text(".livepane .d-grid")).toContain("Fable 5.2, High");
  });

  it("shows an unconfirmed or not yet started worker's route as unknown, never the coordinator's", async () => {
    await open("w3");
    expect(text(".livepane .d-grid")).toContain("Not confirmed");
    expect(text(".livepane .d-grid")).not.toContain("Fable");
    expect(selectedWorker()).toContain("ModelNot confirmed");
    dispose?.(); document.body.innerHTML = "";

    await open("w4");
    expect(text(".livepane .d-grid")).toContain("Chosen when it starts");
    expect(text(".livepane .d-grid")).not.toContain("Fable");
  });
});
