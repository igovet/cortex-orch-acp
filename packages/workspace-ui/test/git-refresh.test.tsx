import { render } from "solid-js/web";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { emptyWorkspace, LOCAL_MACHINE_ID, type GitFile, type ProjectRepo } from "@cortex/workspace-domain";
import { Workspace } from "../src";
import { createRealPort, FACTS_POLL_MAX_MS } from "../src/port/real";
import { capabilitiesWire, scriptedTransport } from "./support";

let dispose = () => {};
beforeEach(() => { vi.useFakeTimers(); localStorage.clear(); sessionStorage.clear(); });
afterEach(() => { dispose(); document.body.innerHTML = ""; vi.useRealTimers(); });
const settled = () => vi.advanceTimersByTimeAsync(0);
const HOME = `/m/${LOCAL_MACHINE_ID}`, GIT = `${HOME}/p/p/git`;

it("refreshes cached Git on open, parked-tab return, focus and periodic reads without a manual action", async () => {
  const repo: ProjectRepo & { openBranch: string; defaultBranch: string } = {
    projectId: "p", remote: "", tree: [], sources: {}, commits: [], branches: [], fetchedAt: null,
    openBranch: "main", defaultBranch: "main",
    checkouts: [{ id: "p:main", projectId: "p", kind: "main", branch: "main", path: "/p", taskId: null,
      label: null, base: "main", ahead: 0, behind: 0, behindOf: {}, draft: "", sources: {}, updateConflict: null, files: [] }],
  };
  let files: GitFile[] = [], finish: ((reply: unknown) => void) | undefined, reads = 0, fail = false, failProduct = false;
  const fresh = () => ({ ok: true, result: { ...repo, checkouts: [{ ...repo.checkouts[0], files: [...files] }] } });
  const script = scriptedTransport({
    "service.capabilities": () => ({ methods: ["machine.call"], machine: true }),
    "projects.list": () => [{ projectId: "p", displayName: "Project", canonicalRoot: "/p", activation: null }],
    "capabilities.get": () => capabilitiesWire(["capabilities.get", "project.snapshot"]),
    "machine.call": message => {
      const { op } = message as { op: string };
      if (op === "repo.snapshot") { reads++; if (fail) throw new Error("offline"); return new Promise(resolve => { finish = resolve; }); }
      if (op === "product.read" && failProduct) throw new Error("unavailable");
      if (op === "product.read") return { ok: true, result: { hasGit: true, tasks: {}, sprints: [] } };
      return { ok: true, result: null };
    },
  });
  const port = createRealPort({ transport: script.transport, actorId: "a", registry: true });
  await port.query("capabilities.get", {});
  const first = port.query("project.facts", {});
  await settled(); finish!(fresh()); await first;
  const state = emptyWorkspace();
  state.machines[0].connection = "connected";
  state.projects.push({ id: "p", machineId: LOCAL_MACHINE_ID, name: "Project", repository: "/p",
    openBranch: "main", defaultBranch: "main", hasGit: true, activated: false });
  state.repos.push(repo);
  history.replaceState(null, "", HOME);
  const root = document.createElement("div"); document.body.append(root);
  dispose = render(() => <Workspace port={port} initial={{ state, version: "1", capabilities: { everything: true, operations: [] } }} />, root);
  await settled();
  const open = async (path: string) => { history.pushState(null, "", path); window.dispatchEvent(new PopStateEvent("popstate")); await settled(); };
  const shown = () => document.querySelector(".ws-pane:not([data-parked])")!.textContent!;
  const add = (path: string): GitFile => ({ path, status: "U", stage: "untracked", add: 1, del: 0, hunks: [] });

  files = [add("node_modules/.vite/vitest/hash/results.json")];
  await open(GIT); // The cache is less than three seconds old, but opening Git must still read.
  expect(reads).toBe(2);
  expect(shown()).toContain("No uncommitted changes.");
  finish!(fresh()); await settled();
  expect(shown()).toContain("results.json");
  expect(shown()).not.toContain("No uncommitted changes.");

  document.dispatchEvent(new KeyboardEvent("keydown", { key: "t", code: "KeyT", ctrlKey: true, bubbles: true }));
  await settled();
  files.push(add("external.txt"));
  const tabs = [...document.querySelectorAll<HTMLElement>('[role="tab"].ws-tab-name')];
  tabs[0].click(); await settled();
  expect(reads).toBe(3);
  expect(shown()).toContain("results.json");
  finish!(fresh()); await settled();
  expect(shown()).toContain("external.txt");

  files.push(add("focus.txt"));
  window.dispatchEvent(new Event("focus")); await settled();
  expect(reads).toBe(4);
  finish!(fresh()); await settled();
  expect(shown()).toContain("focus.txt");

  files.push(add("periodic.txt"));
  // Periodic reads use the same single-flight snapshot and publish its eventual reply.
  await vi.advanceTimersByTimeAsync(FACTS_POLL_MAX_MS);
  expect(reads).toBeGreaterThan(4);
  const pendingReads = reads;
  finish!(fresh()); await settled();
  expect(shown()).toContain("periodic.txt");
  expect(reads).toBe(pendingReads);

  fail = true;
  await open(HOME); await open(GIT);
  expect(shown()).toContain("periodic.txt"); // Failed refresh never erases confirmed changes.
  fail = false;
  await open(HOME); await open(GIT);
  finish!(fresh()); await settled();
  expect(shown()).toContain("periodic.txt");

  failProduct = true;
  await open(HOME); await open(GIT);
  expect(document.body.textContent).toContain("Workspace refresh failed.");
  expect(shown()).toContain("periodic.txt");
  failProduct = false;
  await open(HOME); await open(GIT);
  finish!(fresh()); await settled();
  expect(document.body.textContent).not.toContain("Workspace refresh failed.");
});
