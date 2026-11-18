import { render } from "solid-js/web";
import { createRoot } from "solid-js";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { emptyConfig, emptyWorkspace, LOCAL_MACHINE_ID, type Check, type ProjectFacts, type WorkspaceState } from "@cortex/workspace-domain";
import { Workspace } from "../src";
import { createRealPort } from "../src/port/real";
import { createWorkspaceStore } from "../src/store";
import { capabilitiesWire, flush, projection, scriptedPort, scriptedTransport, wireTask, withTask } from "./support";

/**
 * Owner report 2026-11-17: a task finished and went to review, and its page showed only the conversation for about half a
 * minute before the result card, its checks and its controls appeared. The card comes from the first facts read; what the
 * machine is still checking against the project is pending in place, with Accept off until it is known.
 */
let dispose: (() => void) | undefined;
beforeEach(() => { localStorage.clear(); sessionStorage.clear(); });
afterEach(() => { dispose?.(); dispose = undefined; document.body.innerHTML = ""; });
const settle = async () => { for (let i = 0; i < 8; i++) await flush(); };
const CHECKING = "Checking the result against the project…";
const passed: Check = { name: "npm test", detail: "", outcome: "passed", source: "runtime", command: ["npm", "test"], rerunnable: true };
const result = { version: 1, digest: null as unknown as string, summary: "Wrote the HTTPS setup.", checks: [passed], acceptanceAvailable: false };

it("a result in review shows its card at once while it is checked, and the same card fills in when the check lands", async () => {
  const s = scriptedPort({ capabilities: { everything: true, operations: [] } });
  history.replaceState(null, "", `/m/${LOCAL_MACHINE_ID}/t/t`);
  const root = document.createElement("div"); document.body.append(root);
  dispose = render(() => <Workspace port={s.port} />, root);
  await settle();
  const state: WorkspaceState = withTask(emptyWorkspace(), "HTTPS");
  state.machines[0].connection = "connected";
  Object.assign(state.tasks[0], { status: "review", executionId: "e", workspaceDigest: null, result, reviewActions: { accept: false, pending: true, reason: CHECKING, unblockAction: null } });
  s.emit(projection("1", state)); await settle();
  const card = document.querySelector(".rcard");
  expect(card?.textContent).toContain("Result v1");
  expect(card?.textContent).toContain("Wrote the HTTPS setup.");
  const accept = () => [...document.querySelectorAll<HTMLButtonElement>(".rcard button")].find(b => b.textContent === "Accept v1");
  expect(accept()?.getAttribute("aria-disabled")).toBe("true");
  const strip = document.querySelector(".rcard .rv-why");
  expect(strip?.textContent).toContain(CHECKING);
  // Nothing is offered while it is checked: no request for a new result, no check to run.
  expect(strip?.querySelector("button")).toBeNull();
  // The settled facts arrive: the same card element, now acceptable.
  const facts: ProjectFacts = { projectId: "p", openBranch: "main", defaultBranch: "main", repo: null, workers: [], reviews: [], sprints: [],
    overlays: { t: { workspaceDigest: "d1", result: { ...result, digest: "d1", acceptanceAvailable: true }, reviewActions: { accept: true, reason: null, unblockAction: null } } as ProjectFacts["overlays"][string] } };
  s.emit({ kind: "facts", facts }); await settle();
  expect(document.querySelector(".rcard")).toBe(card);
  expect(accept()?.getAttribute("aria-disabled")).toBeNull();
  expect(document.querySelector(".rcard .rv-why")).toBeNull();
});

it("a facts read with pending parts is followed by one settling read, and nothing polls after it", async () => {
  let deferred = 0, settled = 0;
  const item = wireTask({ taskId: "t", title: "HTTPS", titleAvailability: "available", version: "2", phase: "result", executionId: "e", runtimeState: "COMPLETED", executionState: "completed" });
  const overlay = (pending: boolean) => ({ status: "review", result: { ...result, digest: pending ? null : "d1", acceptanceAvailable: !pending },
    workspaceDigest: pending ? null : "d1", reviewActions: pending ? { accept: false, pending: true, reason: CHECKING, unblockAction: null } : { accept: true, reason: null, unblockAction: null } });
  const t = scriptedTransport({
    "service.capabilities": () => ({ methods: ["native.query", "native.board", "machine.call"], machine: true }),
    "projects.list": () => [{ projectId: "p", displayName: "P", canonicalRoot: "/p", activation: {} }],
    "capabilities.get": () => capabilitiesWire(["capabilities.get", "project.snapshot", "tasks.create", "tasks.start"]),
    "native.board": () => ({ protocol: { major: 2, mapping: 2 }, projectId: "p", generation: "g", version: "2", highWatermark: "3", data: { type: "board", value: { items: [item], nextCursor: null, highWatermark: "3" } } }),
    "machine.call": raw => {
      const { op, params } = raw as { op: string; params?: { defer?: boolean } };
      if (op === "overview") return { ok: true, result: { name: "This machine" } };
      if (op === "project.config.read") return { ok: true, result: { value: {}, tokens: {} } };
      if (op === "config.read") return { ok: true, result: { config: emptyConfig(), connections: [], tokens: {} } };
      if (op === "repo.snapshot") return { ok: true, result: null };
      // The board read carries the saved record only, never the result.
      if (op === "product.board") return { ok: true, result: { hasGit: true, tasks: { t: { status: "review" } }, sprints: [] } };
      // The scripted transport answers `product.facts` from this product read and the worker rows.
      if (op === "product.read") {
        const pending = params?.defer === true;
        if (pending) deferred++; else settled++;
        return { ok: true, result: { hasGit: true, tasks: { t: overlay(pending) }, sprints: [] } };
      }
      return { ok: true, result: op === "workers" ? [] : { hasGit: true, tasks: {}, sprints: [] } };
    },
  });
  const store = createRoot(d => { dispose = d; return createWorkspaceStore(createRealPort({ transport: t.transport, actorId: "owner", registry: true, streamFactsMs: 60_000 })); });
  store.interest("p", "facts", "t");
  await vi.waitFor(() => expect(store.state().tasks.find(x => x.id === "t")?.reviewActions?.accept).toBe(true));
  // Each read that came pending is followed by at most one settling read, and once settled nothing reads again on its own.
  expect(deferred).toBeGreaterThan(0);
  expect(settled).toBeGreaterThan(0);
  expect(settled).toBeLessThanOrEqual(deferred);
  const quiet = [deferred, settled];
  await new Promise(resolve => setTimeout(resolve, 300));
  expect([deferred, settled]).toEqual(quiet);
  expect(store.state().tasks.find(x => x.id === "t")?.result?.summary).toBe("Wrote the HTTPS setup.");
});
