import { createRoot } from "solid-js";
import { render } from "solid-js/web";
import { afterEach, describe, expect, it, vi } from "vitest";
import { emptyWorkspace, LOCAL_MACHINE_ID, type MachineFacts } from "@cortex/workspace-domain";
import { Workspace } from "../src";
import { CATALOGUE_RETRY_MAX_MS, CATALOGUE_RETRY_MS, createWorkspaceStore, MACHINE_WAIT_MS, MODELS_POLL_MS, MODELS_WAIT_MS } from "../src/store";
import { flush, projection, scriptedPort, withTask } from "./support";

let dispose: (() => void) | undefined;
afterEach(() => { dispose?.(); dispose = undefined; document.body.innerHTML = ""; vi.useRealTimers(); });

const held = <T,>() => { let open!: (value: T) => void, fail!: (error: Error) => void; const promise = new Promise<T>((a, b) => { open = a; fail = b; }); return { promise, open, fail }; };
const FRAME = ".titlebar, .sidebar, .ws-tabstrip, .main";
const facts: MachineFacts = { machineId: LOCAL_MACHINE_ID, engineUsage: [], patch: {}, options: {
  models: [{ id: "claude-agent-acp/sonnet", label: "Sonnet", engine: "Claude", efforts: ["high"] }], efforts: [{ id: "high", label: "High" }],
  modes: [{ id: "guarded", label: "Guarded" }], languages: [{ id: "auto", label: "Auto" }] } };

function mount(machine: (call: number) => Promise<MachineFacts[]>) {
  const s = scriptedPort(), state = withTask(emptyWorkspace(), "A task");
  let calls = 0;
  s.port.query = (async (op: string) => op === "capabilities.get" ? { everything: true, operations: [] }
    : op === "project.snapshot" ? [{ kind: "projection", scope: "workspace", version: "1", state }]
    : op === "machine.facts" ? machine(++calls) : []) as typeof s.port.query;
  return s;
}

it("keeps the startup screen until the machine's models and efforts are in, then shows them filled in", async () => {
  const gate = held<MachineFacts[]>(), s = mount(() => gate.promise);
  const root = document.createElement("div"); document.body.append(root);
  history.replaceState(null, "", "/");
  dispose = render(() => <Workspace port={s.port} />, root);
  await flush(); await flush();
  // The task facts are in; the machine's settings are not. No interface yet.
  expect(root.querySelector(".l-startup")).toBeTruthy();
  expect(root.querySelector(FRAME)).toBeNull();
  gate.open([facts]);
  await flush(); await flush();
  expect(root.querySelector(".l-startup")).toBeNull();
  expect(root.querySelector(".titlebar")).toBeTruthy();
  const picker = [...root.querySelectorAll("button.sel")].map(b => b.getAttribute("aria-label"));
  expect(picker.join("|")).not.toMatch(/Unavailable|Loading/);
});

it("retries a failed settings read on its own and waits no longer than the bound, saying what is missing", async () => {
  vi.useFakeTimers();
  const s = mount(() => Promise.reject(new Error("machine_busy")));
  const store = createRoot(d => { dispose = d; return createWorkspaceStore(s.port); });
  await vi.advanceTimersByTimeAsync(0);
  expect(store.ready()).toBe(true);
  expect(store.loaded()).toBe(false);
  expect(store.machineLoaded()).toBe(false);
  expect(store.machineIssue()).toBe("machine_busy");
  await vi.advanceTimersByTimeAsync(MACHINE_WAIT_MS);
  // Waiting has run out: the interface may open, but the settings are still honestly absent.
  expect(store.loaded()).toBe(true);
  expect(store.machineLoaded()).toBe(false);
  expect(store.machineIssue()).toBe("machine_busy");
});

it("fills the settings in when a retry succeeds, without waiting for another refresh", async () => {
  vi.useFakeTimers();
  const s = mount(call => call === 1 ? Promise.reject(new Error("machine_busy")) : Promise.resolve([facts]));
  const store = createRoot(d => { dispose = d; return createWorkspaceStore(s.port); });
  await vi.advanceTimersByTimeAsync(0);
  expect(store.loaded()).toBe(false);
  await vi.advanceTimersByTimeAsync(3100);
  expect(store.loaded()).toBe(true);
  expect(store.machineLoaded()).toBe(true);
  expect(store.machineIssue()).toBeNull();
  expect(store.state().options.models.map(m => m.label)).toEqual(["Sonnet"]);
});

it("asks for the machine's settings beside the task reads, not after them", async () => {
  const slow = held<unknown>(), s = scriptedPort(), asked: string[] = [];
  s.port.query = (async (op: string) => {
    asked.push(op);
    if (op === "capabilities.get") return { everything: true, operations: [] };
    if (op === "project.snapshot") return slow.promise;
    return op === "machine.facts" ? [facts] : [];
  }) as typeof s.port.query;
  const store = createRoot(d => { dispose = d; return createWorkspaceStore(s.port); });
  await flush();
  // A slow repository read is still pending, and the settings are already in.
  expect(asked).toEqual(["capabilities.get", "machine.facts", "project.snapshot"]);
  expect(store.machineLoaded()).toBe(true);
  expect(store.state().options.models).toHaveLength(1);
  expect(store.loaded()).toBe(false);
  slow.open([]);
  await flush();
  expect(store.loaded()).toBe(true);
});

it("opens at once on a refused settings read, says why and keeps retrying", async () => {
  vi.useFakeTimers();
  const s = mount(call => call === 1 ? Promise.reject(new Error("config_api_unavailable")) : Promise.resolve([facts]));
  const store = createRoot(d => { dispose = d; return createWorkspaceStore(s.port); });
  await vi.advanceTimersByTimeAsync(0);
  // A refusal is not loading: the interface opens without waiting out the bound.
  expect(store.loaded()).toBe(true);
  expect(store.machineLoaded()).toBe(false);
  expect(store.machineIssue()).toBe("config_api_unavailable");
  await vi.advanceTimersByTimeAsync(3100);
  expect(store.machineLoaded()).toBe(true);
  expect(store.machineIssue()).toBeNull();
});

const empty: MachineFacts = { ...facts, options: { ...facts.options!, models: [], efforts: [] } };
const fetching = (running: boolean, failed: { connectionId: string; reason: string }[] = []): MachineFacts[] => [{ ...empty, catalogue: { running, failed } }];

it("shows that the models are loading, keeps the interface back, and opens as soon as the machine has fetched them", async () => {
  vi.useFakeTimers();
  let fetched = false;
  const s = mount(() => Promise.resolve(fetched ? [{ ...facts, catalogue: { running: false, failed: [] } }] : fetching(true)));
  const root = document.createElement("div"); document.body.append(root);
  history.replaceState(null, "", "/");
  dispose = render(() => <Workspace port={s.port} />, root);
  await vi.advanceTimersByTimeAsync(0);
  // The task facts and the settings are in, but the machine is still fetching the model lists.
  expect(root.querySelector(FRAME)).toBeNull();
  expect(root.querySelector(".l-startup")?.textContent).toContain("Loading models…");
  // Past the generic settings bound, the screen keeps waiting for the models: nothing is shown empty.
  await vi.advanceTimersByTimeAsync(MACHINE_WAIT_MS);
  expect(root.querySelector(FRAME)).toBeNull();
  expect(root.querySelector(".l-startup")?.textContent).toContain("Loading models…");
  fetched = true;
  await vi.advanceTimersByTimeAsync(MODELS_POLL_MS * 2);
  expect(root.querySelector(".l-startup")).toBeNull();
  expect(root.querySelector(".titlebar")).toBeTruthy();
  const picker = [...root.querySelectorAll("button.sel")].map(b => b.getAttribute("aria-label")).join("|");
  expect(picker).not.toMatch(/Unavailable|Loading/);
});

it("never calls a not-yet-fetched model list unavailable", async () => {
  vi.useFakeTimers();
  const s = mount(() => Promise.resolve(fetching(true)));
  const store = createRoot(d => { dispose = d; return createWorkspaceStore(s.port); });
  await vi.advanceTimersByTimeAsync(0);
  expect(store.machineLoaded()).toBe(true);
  expect(store.state().options.models).toHaveLength(0);
  // The composer reads this to choose between "Loading…" and "Unavailable".
  expect(store.optionsKnown()).toBe(false);
  expect(store.loaded()).toBe(false);
});

it("gives up on the models after the bound, opens, and says honestly that they are still being fetched", async () => {
  vi.useFakeTimers();
  const s = mount(() => Promise.resolve(fetching(true)));
  const store = createRoot(d => { dispose = d; return createWorkspaceStore(s.port); });
  await vi.advanceTimersByTimeAsync(MODELS_WAIT_MS - 1000);
  expect(store.loaded()).toBe(false);
  await vi.advanceTimersByTimeAsync(2000);
  expect(store.loaded()).toBe(true);
  expect(store.modelsPending()).toBe(false);
  expect(store.modelsIssue()).toBe("slow");
  expect(store.optionsKnown()).toBe(true);
});

it("opens at once when the machine's fetch ended without a list, and says it failed", async () => {
  vi.useFakeTimers();
  const s = mount(() => Promise.resolve(fetching(false, [{ connectionId: "cn_1", reason: "engine_unreachable" }])));
  const store = createRoot(d => { dispose = d; return createWorkspaceStore(s.port); });
  await vi.advanceTimersByTimeAsync(0);
  expect(store.loaded()).toBe(true);
  expect(store.modelsIssue()).toBe("failed");
  expect(store.optionsKnown()).toBe(true);
});

// Live run: an OpenCode connection in no route failed its model list ("engine_unreachable:..."). An engine the owner's models do
// not come from says nothing on screen, and a machine's failure code is never shown as words.
it("says nothing about a failed list of an unused engine while other models are offered, and never shows its code", async () => {
  vi.useFakeTimers();
  const s = mount(() => Promise.resolve([{ ...facts, catalogue: { running: false, failed: [{ connectionId: "cn_8aabf186506d", reason: "engine_unreachable:model_switch_unconfirmed" }] } }]));
  const root = document.createElement("div"); document.body.append(root);
  history.replaceState(null, "", "/");
  dispose = render(() => <Workspace port={s.port} />, root);
  await vi.advanceTimersByTimeAsync(0); await vi.advanceTimersByTimeAsync(0);
  expect(root.querySelector(".titlebar")).toBeTruthy();
  expect(root.querySelector('[data-reason="failed"]')).toBeNull();
  expect(root.textContent).not.toMatch(/engine_unreachable|model_switch|ValueError|could not report their models/);
});

it("opens at once, with no wait and no notice, when saved models are served while a refresh runs behind them", async () => {
  vi.useFakeTimers();
  const s = mount(() => Promise.resolve([{ ...facts, catalogue: { running: true, failed: [] } }]));
  const store = createRoot(d => { dispose = d; return createWorkspaceStore(s.port); });
  await vi.advanceTimersByTimeAsync(0);
  expect(store.loaded()).toBe(true);
  expect(store.modelsIssue()).toBeNull();
  expect(store.state().options.models).toHaveLength(1);
});

describe("the machine's overview", () => {
  const overview: MachineFacts = { machineId: LOCAL_MACHINE_ID, patch: { retentionDays: 12, autoArchiveDays: 4, diskMb: 3, ghAvailable: true } };
  const usage = [{ engine: "codex" as const, connected: true, usedPercent: 40, resetAt: null, observedAt: null, window: "weekly" as const }];
  function recorded(answer: () => Promise<MachineFacts[]>) {
    const s = scriptedPort(), asked: string[] = [], state = withTask(emptyWorkspace(), "A task");
    s.port.query = (async (op: string) => {
      asked.push(op);
      if (op === "capabilities.get") return { everything: true, operations: [] };
      if (op === "project.snapshot") return [{ kind: "projection", scope: "workspace", version: "1", state }];
      if (op === "machine.facts") return [{ ...facts, engineUsage: usage }];
      return op === "machine.overview" ? answer() : [];
    }) as typeof s.port.query;
    return { s, asked };
  }

  it("is never asked for at startup; the first screen opens on the settings alone", async () => {
    const { s, asked } = recorded(() => Promise.resolve([overview]));
    const store = createRoot(d => { dispose = d; return createWorkspaceStore(s.port); });
    await flush(); await flush();
    expect(store.loaded()).toBe(true);
    expect(asked).toContain("machine.facts");
    expect(asked).not.toContain("machine.overview");
    expect(store.overviewLoaded()).toBe(false);
  });

  it("is read once for any number of screens asking, and fills in without clobbering usage or options", async () => {
    const gate = held<MachineFacts[]>(), { s, asked } = recorded(() => gate.promise);
    const store = createRoot(d => { dispose = d; return createWorkspaceStore(s.port); });
    await flush(); await flush();
    // The machine's settings as the poll delivers them, over the workspace projection.
    s.emit({ kind: "machine", facts: { ...facts, engineUsage: usage } });
    expect(store.state().engineUsage).toEqual(usage);
    store.wantOverview(); store.wantOverview(); store.wantOverview();
    expect(asked.filter(op => op === "machine.overview")).toHaveLength(1);
    expect(store.overviewLoaded()).toBe(false);
    gate.open([overview]);
    await flush();
    expect(store.overviewLoaded()).toBe(true);
    expect(store.state().machines[0]).toMatchObject({ retentionDays: 12, autoArchiveDays: 4, diskMb: 3, ghAvailable: true });
    expect(store.state().engineUsage).toEqual(usage);
    expect(store.state().options.models.map(m => m.label)).toEqual(["Sonnet"]);
    store.wantOverview();
    expect(asked.filter(op => op === "machine.overview")).toHaveLength(1);
  });

  it("waits for the service to answer before a screen's early ask is read", async () => {
    const capabilities = held<unknown>(), s = scriptedPort(), asked: string[] = [];
    s.port.query = (async (op: string) => {
      asked.push(op);
      if (op === "capabilities.get") return capabilities.promise;
      return op === "machine.facts" ? [facts] : op === "machine.overview" ? [overview] : [];
    }) as typeof s.port.query;
    const store = createRoot(d => { dispose = d; return createWorkspaceStore(s.port); });
    store.wantOverview();
    await flush();
    expect(asked).toEqual(["capabilities.get"]);
    capabilities.open({ everything: true, operations: [] });
    await flush(); await flush();
    expect(asked.filter(op => op === "machine.overview")).toHaveLength(1);
    expect(store.overviewLoaded()).toBe(true);
  });

  it("is read again on the machine refresh after a command, once a screen wanted it, and not by the models poll", async () => {
    vi.useFakeTimers();
    let retention = 12;
    const { s, asked } = recorded(() => Promise.resolve([{ ...overview, patch: { ...overview.patch, retentionDays: retention } }]));
    const store = createRoot(d => { dispose = d; return createWorkspaceStore(s.port); });
    await vi.advanceTimersByTimeAsync(0);
    store.wantOverview();
    await vi.advanceTimersByTimeAsync(0);
    expect(asked.filter(op => op === "machine.overview")).toHaveLength(1);
    retention = 20;
    const sent = store.dispatch({ type: "save_storage", machineId: LOCAL_MACHINE_ID, retentionDays: 20, autoArchiveDays: 4 });
    s.receipt(true);
    await sent; await vi.advanceTimersByTimeAsync(0);
    expect(asked.filter(op => op === "machine.overview")).toHaveLength(2);
    expect(store.state().machines[0].retentionDays).toBe(20);
  });

  it("never blocks the interface when it fails, says why, and retries on its own", async () => {
    vi.useFakeTimers();
    let calls = 0;
    const { s, asked } = recorded(() => ++calls === 1 ? Promise.reject(new Error("machine_busy")) : Promise.resolve([overview]));
    const store = createRoot(d => { dispose = d; return createWorkspaceStore(s.port); });
    store.wantOverview();
    await vi.advanceTimersByTimeAsync(0);
    expect(store.loaded()).toBe(true);
    expect(store.machineIssue()).toBeNull();
    expect(store.overviewLoaded()).toBe(false);
    expect(store.overviewIssue()).toBe("machine_busy");
    await vi.advanceTimersByTimeAsync(3100);
    expect(asked.filter(op => op === "machine.overview")).toHaveLength(2);
    expect(store.overviewLoaded()).toBe(true);
    expect(store.overviewIssue()).toBeNull();
    expect(store.state().machines[0].retentionDays).toBe(12);
  });
});

describe("model lists that arrive after the first screen", () => {
  const codex = { id: "codex-acp/gpt-6.1-sol", label: "6.1 Sol", engine: "Codex", efforts: ["high"] };
  const fable = { id: "claude-agent-acp/fable", label: "Fable", engine: "Claude", efforts: ["high"] };
  const offering = (models: typeof codex[], catalogue: MachineFacts["catalogue"]): MachineFacts[] => [{ ...facts, options: { ...facts.options!, models }, catalogue }];
  const claudeFailed = { running: false, failed: [{ connectionId: "cn_3aec66405544", reason: "refresh_lock_unavailable" }] };

  it("keeps reading while one engine's list failed, so its models appear once the machine fetched them again", async () => {
    vi.useFakeTimers();
    let fetched = false;
    const s = mount(() => Promise.resolve(fetched ? offering([codex, fable], { running: false, failed: [] }) : offering([codex], claudeFailed)));
    const store = createRoot(d => { dispose = d; return createWorkspaceStore(s.port); });
    await vi.advanceTimersByTimeAsync(0);
    // Codex's models are offered, so nothing waits; Claude's list failed.
    expect(store.loaded()).toBe(true);
    expect(store.state().options.models.map(m => m.label)).toEqual(["6.1 Sol"]);
    fetched = true;
    await vi.advanceTimersByTimeAsync(CATALOGUE_RETRY_MS + 100);
    expect(store.state().options.models.map(m => m.label)).toEqual(["6.1 Sol", "Fable"]);
    expect(store.modelsIssue()).toBeNull();
  });

  it("reads again less and less often while the list keeps failing, never more rarely than the bound", async () => {
    vi.useFakeTimers();
    let reads = 0;
    const s = mount(() => { reads++; return Promise.resolve(offering([codex], claudeFailed)); });
    const store = createRoot(d => { dispose = d; return createWorkspaceStore(s.port); });
    await vi.advanceTimersByTimeAsync(0);
    expect(store.loaded()).toBe(true);
    expect(reads).toBe(1);
    await vi.advanceTimersByTimeAsync(CATALOGUE_RETRY_MS + 10);
    expect(reads).toBe(2);
    await vi.advanceTimersByTimeAsync(CATALOGUE_RETRY_MS * 2 + 10);
    expect(reads).toBe(3);
    await vi.advanceTimersByTimeAsync(CATALOGUE_RETRY_MAX_MS * 4);
    const before = reads;
    await vi.advanceTimersByTimeAsync(CATALOGUE_RETRY_MAX_MS + 10);
    expect(reads).toBe(before + 1);
  });

  it("follows a refresh the port's own machine poll reported, until the new list is in", async () => {
    vi.useFakeTimers();
    let fetched = false;
    const s = mount(call => Promise.resolve(call === 1 ? offering([codex], { running: false, failed: [] }) : fetched ? offering([codex, fable], { running: false, failed: [] }) : offering([codex], { running: true, failed: [] })));
    const store = createRoot(d => { dispose = d; return createWorkspaceStore(s.port); });
    await vi.advanceTimersByTimeAsync(0);
    expect(store.state().options.models.map(m => m.label)).toEqual(["6.1 Sol"]);
    // Minutes later the machine poll's read starts a refresh of the missing list.
    s.emit({ kind: "machine", facts: offering([codex], { running: true, failed: [] })[0] });
    await vi.advanceTimersByTimeAsync(MODELS_POLL_MS + 10);
    expect(store.state().options.models.map(m => m.label)).toEqual(["6.1 Sol"]);
    fetched = true;
    await vi.advanceTimersByTimeAsync(MODELS_POLL_MS + 10);
    expect(store.state().options.models.map(m => m.label)).toEqual(["6.1 Sol", "Fable"]);
  });
});

it("moves a new task's coordinator to the owner's default once that model is offered, and keeps a choice made by hand", async () => {
  const world = withTask(emptyWorkspace(), "Work");
  world.machines[0].connection = "connected";
  const config = world.machines[0].config;
  config.surfaces.user_defaults.coordinator = { engine: "claude-agent-acp", model: "fable", effort: "high", parameters: {} };
  config.setup["codex-acp"].status = "connected";
  config.setup["claude-agent-acp"].status = "connected";
  config.surfaces.routing.engines["codex-acp"].enabled = ["gpt-6-luna"];
  config.surfaces.routing.engines["claude-agent-acp"].enabled = ["fable"];
  config.catalogue["codex-acp"].models = [{ id: "gpt-6-luna", routeId: "codex-acp/gpt-6-luna", label: "6 Luna", efforts: ["high"], observed: true }];
  const refreshed = { ...config, catalogue: { ...config.catalogue,
    "claude-agent-acp": { ...config.catalogue["claude-agent-acp"], models: [{ id: "fable", label: "Fable", efforts: ["high"], observed: true }] },
  } };
  const s = scriptedPort({ capabilities: { everything: true, operations: [] } });
  const refresh = () => s.emit({ kind: "machine", facts: { ...facts, patch: { config: refreshed } } });
  history.replaceState(null, "", `/m/${LOCAL_MACHINE_ID}/new?project=p`);
  const root = document.createElement("div"); document.body.append(root);
  dispose = render(() => <Workspace port={s.port} />, root);
  for (let i = 0; i < 4; i++) await flush();
  s.emit(projection("1", world));
  for (let i = 0; i < 4; i++) await flush();
  const shown = () => root.querySelector('[aria-label^="Coordinator model:"]')?.getAttribute("aria-label");
  // The default's engine has no list yet: an offered model stands in, but is not taken as a choice.
  expect(shown()).toBe("Coordinator model: 6 Luna");
  refresh();
  for (let i = 0; i < 4; i++) await flush();
  expect(shown()).toBe("Coordinator model: Fable");
  // A model the owner picks by hand stays picked when the lists are read again.
  root.querySelector<HTMLButtonElement>('[aria-label="Coordinator model: Fable"]')!.click();
  for (let i = 0; i < 2; i++) await flush();
  [...document.querySelectorAll<HTMLElement>('[role="menuitem"], .mi, button')].find(b => b.textContent?.includes("6 Luna") && b.closest("[popover]"))!.click();
  for (let i = 0; i < 2; i++) await flush();
  expect(shown()).toBe("Coordinator model: 6 Luna");
  refresh();
  for (let i = 0; i < 4; i++) await flush();
  expect(shown()).toBe("Coordinator model: 6 Luna");
});
