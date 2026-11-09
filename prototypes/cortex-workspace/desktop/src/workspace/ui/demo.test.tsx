import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { render } from "solid-js/web";
import type { PortUpdate, ProjectionUpdate } from "@cortex/workspace-domain";
import { loadFixtures } from "../../fixtures";
import { WORKSPACE_CASES } from "../adapter/fixtures";
import { createMockWorkspace } from "../adapter/mock";
import { createDemoBackend } from "./demo-backend";
import Workspace from "./index";

const LOCAL = "mock-machine-00000", BANNER = "Demo — simulated data, no real actions";
const tick = () => new Promise(r => setTimeout(r, 0));
let dispose: (() => void) | undefined;
beforeEach(() => { localStorage.clear(); sessionStorage.clear(); });
afterEach(() => { dispose?.(); dispose = undefined; document.body.innerHTML = ""; });
async function mount(path: string) {
  history.replaceState(null, "", path);
  const root = document.createElement("div");
  document.body.append(root);
  dispose = render(() => <Workspace autoplay={false} />, root);
  await tick();
}
const backendOnFixtures = () => createDemoBackend(() => createMockWorkspace(loadFixtures().workspace), 0);

describe("demo entry", () => {
  it.each(["/", `/m/${LOCAL}/inbox`, `/m/${LOCAL}/t/${WORKSPACE_CASES.session}`, "/settings/app/appearance", "/no/such/page"])("always shows its banner at %s", async path => {
    await mount(path);
    const banner = document.querySelector(".ws-demo-banner")!;
    expect(banner.textContent).toBe(BANNER);
  });

  it("keeps the banner out of the frame, in a row of its own", async () => {
    await mount("/");
    expect(document.querySelector(".ws-app .ws-demo-banner")).toBeNull();
    const { readFileSync } = await import(/* @vite-ignore */ `node:${"fs"}`) as { readFileSync(path: URL, encoding: "utf8"): string };
    const file = (name: string) => readFileSync(new URL(name, import.meta.url), "utf8");
    expect(file("./demo.css")).toContain("grid-template-rows: auto var(--titlebar-h)");
  });

  it("is a demo port, and says every control is simulated", async () => {
    const backend = backendOnFixtures();
    expect(backend.port.mode).toBe("demo");
    expect(await backend.port.query("capabilities.get", {})).toMatchObject({ everything: true });
  });

  it("publishes a newer confirmed projection for each change, and none for a refusal", async () => {
    const backend = backendOnFixtures(), updates: PortUpdate[] = [];
    backend.port.subscribe(u => updates.push(u));
    const start = backend.initial().version;
    const refused = await backend.port.command({ requestId: "r1", operation: "start_task", expectedVersion: null, payload: { type: "start_task", taskId: "missing" } });
    expect(refused.ok).toBe(false);
    expect(updates).toEqual([]);
    const project = backend.state().projects.find(p => p.machineId === LOCAL)!.id;
    const created = await backend.port.command({ requestId: "r2", operation: "create_task", expectedVersion: null, payload: { type: "create_task", projectId: project, description: "New", checkout: { kind: "main" } } });
    expect(created).toMatchObject({ ok: true, requestId: "r2" });
    expect(updates).toHaveLength(1);
    expect(BigInt((updates[0] as ProjectionUpdate).version)).toBeGreaterThan(BigInt(start));
  });

  it("restores the fixture on reset, as a new version", () => {
    const backend = backendOnFixtures(), updates: PortUpdate[] = [];
    backend.port.subscribe(u => updates.push(u));
    const before = backend.initial().version;
    backend.reset();
    expect(updates).toHaveLength(1);
    expect((updates[0] as ProjectionUpdate).version).not.toBe(before);
  });
});
