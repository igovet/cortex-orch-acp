import { render } from "solid-js/web";
import { afterEach, beforeEach, expect, it } from "vitest";
import { emptyWorkspace, LOCAL_MACHINE_ID, type Check, type Sprint, type WorkspaceState } from "@cortex/workspace-domain";
import { Workspace } from "../src";
import { checksOf, type SessionModel } from "../src/session-model";
import { sentence } from "../src/review-model";
import { flush, projection, scriptedPort, withTask } from "./support";

/** Paid live re-test of a dev build (2026-11-17): what the review gate and its surroundings said and offered. */
let dispose: (() => void) | undefined;
beforeEach(() => { localStorage.clear(); sessionStorage.clear(); });
afterEach(() => { dispose?.(); document.body.innerHTML = ""; });
const settle = async () => { for (let i = 0; i < 8; i++) await flush(); };
const text = () => document.body.textContent ?? "";
const buttons = (label: string) => [...document.querySelectorAll<HTMLButtonElement>("button")].filter(b => b.textContent?.trim() === label);
const world = () => { const s = withTask(emptyWorkspace(), "Work"); s.machines[0].connection = "connected"; return s; };
async function mount(state: WorkspaceState, path: string) {
  const s = scriptedPort({ capabilities: { everything: true, operations: [] } });
  history.replaceState(null, "", path);
  const root = document.createElement("div"); document.body.append(root);
  dispose = render(() => <Workspace port={s.port} />, root);
  await settle(); s.emit(projection("1", state)); await settle();
  return s;
}
const taskPath = `/m/${LOCAL_MACHINE_ID}/t/t`;
const passed: Check = { name: "node test/counter.test.js", detail: "", outcome: "passed", source: "runtime", command: ["node", "test/counter.test.js"], rerunnable: true };
const withResult = (status: WorkspaceState["tasks"][number]["status"]) => {
  const s = world(), t = s.tasks[0];
  t.status = status; t.workspaceDigest = "digest";
  t.result = { version: 1, digest: "digest", summary: "Wrote ARCHITECTURE.md", checks: [passed] };
  return s;
};

it("a blocked Accept's reason ends in exactly one full stop, whether or not the runtime's reason has its own", async () => {
  expect(sentence("Resolve the conflict first")).toBe("Resolve the conflict first.");
  expect(sentence("Only recorded technical blockers may be acknowledged.")).toBe("Only recorded technical blockers may be acknowledged.");
  const s = withResult("review");
  s.tasks[0].reviewActions = { accept: false, reason: "Run the unpassed checks first, or request changes. Only recorded technical blockers may be acknowledged.", unblockAction: "rerun_checks" };
  await mount(s, taskPath + "/changes");
  const strip = document.querySelector(".rv-why");
  expect(strip?.textContent).toContain("Only recorded technical blockers may be acknowledged.");
  expect(strip?.textContent).not.toContain("acknowledged..");
});

it("a closed task offers no Request changes on its result", async () => {
  await mount(withResult("done"), taskPath + "/changes");
  expect(text()).toContain("This task is closed.");
  expect(buttons("Request changes…")).toHaveLength(0);
  dispose?.(); document.body.innerHTML = "";
  // In review it still does.
  await mount(withResult("review"), taskPath + "/changes");
  expect(buttons("Request changes…").length).toBeGreaterThan(0);
});

it("while a reopened task works, the Inspector counts only the current version's worker checks", () => {
  const unverified: Check = { name: "node --check src/cli.js", detail: "", outcome: "unverified", source: "worker" };
  const fresh: Check = { name: "unit tests", detail: "", outcome: "passed", source: "worker" };
  const model = { view: { task: { result: null } }, workers: [{ superseded: true, result: { summary: "v1", checks: [unverified] } }, { result: { summary: "v2", checks: [fresh] } }] } as unknown as SessionModel;
  expect(checksOf(model)).toEqual([fresh]);
});

it("a new project-folder task says it stops at Ready to finish, a worktree task at Ready to merge", async () => {
  await mount(world(), `/m/${LOCAL_MACHINE_ID}/new`);
  const hint = () => document.querySelector(".fld.ws-inline .hint")?.textContent ?? "";
  const folder = [...document.querySelectorAll<HTMLLabelElement>("label.rc")].find(l => l.textContent?.includes("Project folder"))!.querySelector("input")!;
  const worktree = [...document.querySelectorAll<HTMLLabelElement>("label.rc")].find(l => l.textContent?.includes("Separate worktree"))!.querySelector("input")!;
  worktree.click(); await settle();
  expect(hint()).toBe("Git default branch. The task stops at Ready to merge for your confirmation.");
  folder.click(); await settle();
  expect(hint()).toBe("Git default branch. The task stops at Ready to finish for your confirmation.");
});

it("a complete sprint queue says its results wait for confirmation only while one of its tasks is still open", async () => {
  const sprint: Sprint = { id: "sp", projectId: "p", title: "Release", description: "Release", runMode: "automatic", targetBranch: null, planning: "created", conversation: [], plan: null, materializedVersion: 1, taskIds: ["t", "u"], queueState: "complete", pauseReason: null };
  const state = (statuses: ("done" | "ready_to_merge")[]) => {
    const s = world();
    s.tasks = ["t", "u"].map((id, i) => ({ ...s.tasks[0], id, title: `Work ${id}`, status: statuses[i], sprintId: "sp" }));
    s.sprints = [sprint];
    return s;
  };
  const path = `/m/${LOCAL_MACHINE_ID}/p/p/s/sp/board`;
  await mount(state(["done", "ready_to_merge"]), path);
  expect(text()).toContain("All queued tasks have run.");
  dispose?.(); document.body.innerHTML = "";
  await mount(state(["done", "done"]), path);
  expect(text()).toContain("Work u");
  expect(text()).not.toContain("All queued tasks have run.");
});
