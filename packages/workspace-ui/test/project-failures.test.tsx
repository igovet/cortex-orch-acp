import { render } from "solid-js/web";
import { createRoot } from "solid-js";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { Workspace } from "../src";
import { createRealPort } from "../src/port/real";
import { createWorkspaceStore } from "../src/store";
import { capabilitiesWire, flush, scriptedPort, scriptedTransport, snapshot } from "./support";

/** What the owner meets when a folder or a project's Cortex is not usable: the reason, with the machine still connected. */
let dispose: (() => void) | undefined;
beforeEach(() => { localStorage.clear(); sessionStorage.clear(); history.replaceState(null, "", "/"); });
afterEach(() => { dispose?.(); dispose = undefined; document.body.innerHTML = ""; vi.useRealTimers(); });
const all = (selector: string) => [...document.querySelectorAll<HTMLElement>(selector)];
const button = (text: string) => all("button").find(b => b.textContent?.trim().endsWith(text))!;
const settle = async () => { for (let i = 0; i < 8; i++) await flush(); };
const input = (label: string, value: string) => {
  const el = document.querySelector<HTMLInputElement>(`[aria-label="${label}"]`)!;
  el.value = value; el.dispatchEvent(new Event("input", { bubbles: true }));
};
const value = (label: string) => document.querySelector<HTMLInputElement>(`[aria-label="${label}"]`)!.value;
const METHODS = ["projects.list", "projects.register", "projects.activate", "projects.status", "native.pick_folder"];

function machine(answers: Parameters<typeof scriptedTransport>[0] = {}) {
  return scriptedTransport({
    "service.capabilities": () => ({ methods: METHODS }),
    "projects.list": () => [],
    "capabilities.get": () => capabilitiesWire([]),
    ...answers,
  });
}
async function mount(s: ReturnType<typeof machine>) {
  const root = document.createElement("div"); document.body.append(root);
  dispose = render(() => <Workspace port={createRealPort({ transport: s.transport, actorId: "peer", registry: true })} />, root);
  await settle();
}
const connected = () => !document.querySelector(".ws-conn[role=alert]");

describe("a project whose Cortex is away", () => {
  const project = { projectId: "p", displayName: "Cedar", canonicalRoot: "/cedar", activation: { actorId: "uid:1", recordedAt: 1 } };
  const helper = { "machine.call": (call: unknown) => ({ ok: true, result: (call as { op: string }).op === "product.read" ? { tasks: {}, sprints: [] } : null }) };

  it("names the reason on its page, keeps the machine connected and tries again on request", async () => {
    let ready = false, activations = 0;
    const s = machine({
      ...helper,
      "service.capabilities": () => ({ methods: [...METHODS, "machine.call"], machine: true }),
      "projects.list": () => [project],
      "projects.status": () => ready ? { ready: true, state: "ready" } : { ready: false, state: "failed", reason: "project_folder_missing" },
      "projects.activate": () => { activations++; ready = true; return { projectId: "p" }; },
      "project.snapshot": () => snapshot("3", []),
      "capabilities.get": () => capabilitiesWire(ready ? ["project.snapshot", "tasks.create"] : []),
    });
    history.replaceState(null, "", "/m/local/p/p/board");
    const port = createRealPort({ transport: s.transport, actorId: "peer", registry: true });
    const update = await (async () => { await port.query("capabilities.get", {}); return port.query("project.snapshot", {}); })();
    expect(update).toHaveLength(1);
    expect(s.calls.filter(c => c.operation === "project.snapshot")).toEqual([]);
    await mount(s);
    expect(document.querySelector(".ws-conn[role=alert]")?.textContent).toContain("folder is not there");
    expect(document.body.textContent).not.toContain("Not connected");
    expect(all(".st").map(e => e.textContent)).toContain("Connected");
    button("Try again").click(); await settle();
    expect(activations).toBe(1);
    expect(document.body.textContent).not.toContain("folder is not there");
  });

  it("reports the reason in its facts", async () => {
    const s = machine({
      ...helper,
      "service.capabilities": () => ({ methods: [...METHODS, "machine.call"], machine: true }),
      "projects.list": () => [project],
      "projects.status": () => ({ ready: false, state: "failed", reason: "project_folder_missing" }),
    });
    const port = createRealPort({ transport: s.transport, actorId: "peer", registry: true });
    await port.query("capabilities.get", {});
    const [facts] = await port.query("project.facts", {});
    expect(facts.issue).toMatch(/folder is not there/);
  });

  it("rereads everything when the service says a project changed", async () => {
    const s = machine();
    const port = createRealPort({ transport: s.transport, actorId: "peer", registry: true });
    const seen: string[] = [];
    const stop = port.subscribe(update => seen.push(update.kind));
    s.send({ type: "refresh" });
    stop();
    expect(seen).toEqual(["refresh"]);
  });
});

describe("the connection", () => {
  it("comes back by itself after a passing failure, with no one pressing Retry", async () => {
    vi.useFakeTimers();
    const failure = new Error("service_timeout");
    const s = scriptedPort({ failQuery: failure });
    const store = createRoot(d => { dispose = d; return createWorkspaceStore(s.port); });
    await vi.advanceTimersByTimeAsync(10);
    expect(store.connection().state).toBe("unavailable");
    const query = vi.spyOn(s.port, "query").mockImplementation(((operation: string) => Promise.resolve(operation === "capabilities.get" ? { everything: false, operations: [] } : [])) as never);
    await vi.advanceTimersByTimeAsync(3100);
    expect(query).toHaveBeenCalled();
    expect(store.connection().state).toBe("connected");
  });
});
