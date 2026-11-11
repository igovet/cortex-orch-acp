import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { render } from "solid-js/web";
import { afterEach, beforeEach, expect, it } from "vitest";
import { emptyWorkspace, LOCAL_MACHINE_ID, type PortCapabilities, type ProjectRepo, type RepoCheckout, type Sprint, type Task, type WorkspaceState } from "@cortex/workspace-domain";
import { Workspace } from "../src";
import { flush, projection, scriptedPort, withTask } from "./support";

/** Every lifecycle stop has one obvious next action, and no completion path depends on Git when the project has none. */
let dispose: (() => void) | undefined;
beforeEach(() => { localStorage.clear(); sessionStorage.clear(); });
afterEach(() => { dispose?.(); document.body.innerHTML = ""; });
const settle = async () => { for (let i = 0; i < 8; i++) await flush(); };
const controls = () => [...document.querySelectorAll<HTMLElement>("button, a.btn, a.lnk")];
const control = (text: string) => controls().find(b => b.textContent?.trim() === text);
const text = () => document.body.textContent ?? "";
const ops = (p: ReturnType<typeof scriptedPort>) => p.commands.map(c => c.operation);
const taskPath = `/m/${LOCAL_MACHINE_ID}/t/t`;
const sprint = (patch: Partial<Sprint> = {}): Sprint => ({ id: "sp", projectId: "p", description: "Coffee landing", title: "Coffee landing", runMode: "manual", targetBranch: null, planning: "created", conversation: [], plan: null, materializedVersion: 1, taskIds: [], queueState: "idle", pauseReason: null, ...patch });
const world = (task: Partial<Task> = {}, project: { hasGit?: boolean } = {}, sprints: Sprint[] = []): WorkspaceState => {
  const s = withTask(emptyWorkspace(), "Work");
  s.machines[0].connection = "connected";
  Object.assign(s.tasks[0], task); Object.assign(s.projects[0], project);
  return { ...s, sprints };
};
const ready = (patch: Partial<Task> = {}): Partial<Task> => ({
  status: "ready_to_merge", checkout: { kind: "main", branch: "task-work" }, workspaceDigest: "d1",
  result: { version: 1, digest: "d1", sourceVersion: "7", summary: "Added the page", checks: [{ name: "Tests", detail: "", outcome: "passed" }], automaticReady: true }, ...patch,
});
async function mount(state: WorkspaceState, path: string, capabilities: PortCapabilities = { everything: true, operations: [] }) {
  const s = scriptedPort({ capabilities });
  history.replaceState(null, "", path);
  const root = document.createElement("div"); document.body.append(root);
  dispose = render(() => <Workspace port={s.port} />, root);
  await settle(); s.emit(projection("1", state)); await settle();
  return s;
}

it("a failed native plan exposes its reason beside Retry, outside the scrollable conversation", async () => {
  const reason = "The model replied with prose instead of the required task-plan JSON. Resume planning to retry.";
  const sp = { ...sprint({ planning: "cancelled", materializedVersion: null }), planningFailure: reason };
  const p = await mount(world({}, {}, [sp]), `/m/${LOCAL_MACHINE_ID}/p/p/s/sp/planning`);
  const alert = document.querySelector('[role="alert"]');
  expect(alert?.textContent).toContain(reason);
  expect(alert?.closest(".sp-body")).toBeNull();
  control("Retry planning")!.click(); await settle();
  expect(ops(p)).toEqual(["resume_planning"]);
});

it("a paused queue names the blocked review and offers its task before Continue queue", async () => {
  const sp = sprint({runMode:"automatic", taskIds:["t"], queueState:"paused", pauseReason:"A result has missing or unsuccessful checks."});
  const state = world({status:"review", sprintId:"sp", reviewActions:{accept:false,reason:"Run the current checks before accepting work.",unblockAction:"rerun_checks"}}, {}, [sp]);
  await mount(state, `/m/local/p/p/s/sp/summary`);
  expect(text()).toContain("Run the current checks before accepting work.");
  expect(text()).not.toContain("Review this task");
  expect(control("Open task")?.getAttribute("href")).toContain("/t/t");
});

it("an automatic Ready task offers one enabled Review and merge that accepts this exact version and then merges", async () => {
  const p = await mount(world(ready()), `${taskPath}/changes`);
  const go = control("Review and merge…")!;
  expect(go).toBeTruthy();
  expect(go.getAttribute("aria-disabled")).toBeNull();
  expect(controls().some(b => b.textContent?.trim() === "Merge…" && b.getAttribute("aria-disabled") === "true")).toBe(false);
  go.click(); await settle();
  const confirm = document.querySelector<HTMLButtonElement>("dialog [data-bar=\"dialog\"] .btn-primary")!;
  expect(confirm.textContent).toContain("Accept v1 and merge");
  confirm.click(); await settle();
  expect(ops(p)).toEqual(["accept_result"]);
  expect(p.commands[0].payload).toMatchObject({ type: "accept_result", taskId: "t", digest: "d1" });
  p.receipt(true); await settle();
  expect(ops(p)).toEqual(["accept_result", "integrate"]);
  expect(p.commands[1].payload).toMatchObject({ type: "integrate", method: "squash", digest: "d1", targetBranch: "main", confirmed: true });
});

it("a refused merge after acceptance keeps the dialog open and never resends anything by itself", async () => {
  const p = await mount(world(ready()), `${taskPath}/changes`);
  control("Review and merge…")!.click(); await settle();
  document.querySelector<HTMLButtonElement>("dialog [data-bar=\"dialog\"] .btn-primary")!.click(); await settle();
  p.receipt(true); await settle();
  p.receipt(false); await settle();
  expect(document.querySelector("dialog[open]")).not.toBeNull();
  expect(ops(p)).toEqual(["accept_result", "integrate"]);
});

it("a Ready task in a project without Git finishes by closing, with no merge, pull request or dead button", async () => {
  await mount(world(ready({ acceptance: { version: 1, digest: "d1", at: 1, open: {} } as Task["acceptance"] }), { hasGit: false }), `${taskPath}/changes`);
  expect(control("Merge…")).toBeUndefined();
  expect(control("Create PR…")).toBeUndefined();
  expect(control("Review and merge…")).toBeUndefined();
  expect(control("Set up Git…")).toBeTruthy();
  const close = control("Close as is…")!;
  expect(close.getAttribute("aria-disabled")).toBeNull();
  close.click(); await settle();
  const dialog = document.querySelector("dialog[open]")!;
  expect(dialog.textContent).not.toMatch(/marked not verified|Not verified warning/);
  expect(dialog.textContent).toContain("stays on record");
});

it("an unaccepted Ready task without Git still has a way to finish", async () => {
  await mount(world(ready(), { hasGit: false }), `${taskPath}/changes`);
  expect(control("Review and merge…")).toBeUndefined();
  expect(control("Close as is…")?.getAttribute("aria-disabled")).toBeNull();
});

it("Commit stays busy for the single helper request, and the keyboard cannot send it twice", async () => {
  const files = [{ path: "a.md", status: "U", stage: "untracked", add: 1, del: 0, hunks: [] }, { path: "b.md", status: "U", stage: "untracked", add: 1, del: 0, hunks: [] }] as never;
  const checkout = { id: "p:main", projectId: "p", kind: "main", branch: "main", path: "/p", base: "main", ahead: 0, behind: 0, behindOf: {}, taskId: null, files, draft: "Add docs", sources: {} } as unknown as RepoCheckout;
  const s = world(); s.repos = [{ projectId: "p", remote: "", tree: [], sources: {}, commits: [], branches: [], checkouts: [checkout], fetchedAt: null } as unknown as ProjectRepo];
  const p = await mount(s, `/m/${LOCAL_MACHINE_ID}/p/p/git`);
  const commit = document.querySelector<HTMLButtonElement>(".commit-box .btn-primary")!;
  commit.click(); await settle();
  expect(commit.hasAttribute("data-busy")).toBe(true);
  expect(ops(p)).toEqual(["git_commit"]);
  const box = document.querySelector<HTMLTextAreaElement>(".commit-box textarea")!;
  box.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", ctrlKey: true, bubbles: true }));
  await settle();
  expect(ops(p)).toEqual(["git_commit"]);
  expect(commit.hasAttribute("data-busy")).toBe(true);
  p.receipt(true); await settle();
  expect(commit.hasAttribute("data-busy")).toBe(false);
  expect(ops(p)).toEqual(["git_commit"]);
});

it("a busy button keeps its label and icon readable, so it never seems to vanish", () => {
  const css = readFileSync(join(dirname(fileURLToPath(import.meta.url)), "../src/workspace.css"), "utf8");
  expect(css).not.toMatch(/ws-busy-mute|ws-busy-hide/);
  expect(css).not.toMatch(/\[data-busy\][^{]*\{[^}]*(color: transparent|visibility: hidden)/);
});

it("a sprint whose tasks could not be loaded offers Retry and an explicit container-only close", async () => {
  await mount(world({}, {}, [sprint({ taskIds: ["gone"] })]), `/m/${LOCAL_MACHINE_ID}/p/p/s/sp/summary`);
  expect(text()).toContain("Task details unavailable");
  expect(text()).not.toContain("No tasks yet");
  expect(control("Retry")).toBeTruthy();
  expect(control("Close sprint…")).toBeTruthy();
  expect(control("Close sprint…")!.getAttribute("aria-disabled")).not.toBe("true");
  control("Close sprint…")!.click(); await settle();
  expect(text()).toContain("No task is cancelled, accepted, merged, archived or deleted");
});

it("an empty sprint board offers New task for that sprint, already selected", async () => {
  await mount(world({}, {}, [sprint()]), `/m/${LOCAL_MACHINE_ID}/p/p/s/sp`);
  const add = [...document.querySelectorAll<HTMLButtonElement>(".note-panel button")].find(b => b.textContent?.trim() === "New task")!;
  expect(add).toBeTruthy();
  add.click(); await settle();
  expect(location.search).toContain("sprint=sp");
  expect(document.querySelector('[popovertarget="m-new-sprint"]')?.textContent).toContain("Coffee landing");
});

it("a new task in a folder without Git works in the folder and offers no branch choices", async () => {
  await mount(world({}, { hasGit: false }), `/m/${LOCAL_MACHINE_ID}/new?project=p`);
  const worktree = document.querySelector<HTMLInputElement>('input[name="checkout"]')!;
  expect(worktree.disabled).toBe(true);
  expect(text()).toContain("A worktree needs Git");
  expect(text()).not.toContain("Merge into");
});

it("Work details and Changes stay reachable when the machine offers no Git operations", async () => {
  await mount(world(ready()), taskPath, { everything: false, operations: ["workspace.accept_result"] });
  const tabs = [...document.querySelectorAll('nav[aria-label="Task details"] a')].map(t => t.textContent?.trim() ?? "");
  expect(tabs.some(t => t.startsWith("Work details"))).toBe(true);
  expect(tabs.some(t => t.startsWith("Changes"))).toBe(true);
});

it("a stalled automatic queue names its reason beside the sprint with a way to the blocking task", async () => {
  await mount(world({ status: "review", sprintId: "sp" }, {}, [sprint({ runMode: "automatic", taskIds: ["t"] })]), `/m/${LOCAL_MACHINE_ID}/p/p/s/sp/planning`);
  const strip = [...document.querySelectorAll(".l-strip")].find(s => s.textContent?.includes("Review Work first"));
  expect(strip).toBeTruthy();
  expect([...strip!.querySelectorAll("a")].some(a => a.textContent?.trim() === "Open task")).toBe(true);
});

const board = `/m/${LOCAL_MACHINE_ID}/p/p/s/sp/board`;
it("a paused board says why once, in the strip that describes Continue queue, and keeps the words out of the button", async () => {
  await mount(world({ status: "paused", sprintId: "sp" }, {}, [sprint({ runMode: "automatic", taskIds: ["t"], queueState: "paused", pauseReason: "A task stopped" })]), board);
  const go = control("Continue queue")!;
  expect(go.getAttribute("aria-disabled")).toBe("true");
  const strip = document.getElementById(go.getAttribute("aria-describedby")!)!;
  expect(strip.textContent).toContain("Queue paused: Work is stopped. Resume it first.");
  expect(strip.textContent).toContain("Open task");
  expect(go.closest("[aria-label='Sprint queue']")?.textContent).not.toContain("Resume it first");
  expect(text()).not.toContain("Review this task");
});

it("keeps the explanation of the automatic queue behind a question mark on the board, not in a strip", async () => {
  await mount(world({ sprintId: "sp" }, {}, [sprint({ runMode: "automatic", taskIds: ["t"] })]), board);
  const hint = document.querySelector<HTMLButtonElement>("[aria-label='Sprint queue'] .ws-hint")!;
  expect(hint.tagName).toBe("BUTTON");
  expect(hint.getAttribute("aria-label")).toBe("About the automatic queue");
  expect(document.getElementById(hint.getAttribute("popovertarget")!)?.textContent).toContain("Passed checks leave each task ready");
  expect([...document.querySelectorAll(".l-strip")].some(s => s.textContent?.includes("Passed checks leave"))).toBe(false);
});

it("offers no explanation for a manual queue, which does not run by itself", async () => {
  await mount(world({ sprintId: "sp" }, {}, [sprint({ taskIds: ["t"] })]), board);
  expect(document.querySelector("[aria-label='Sprint queue'] .ws-hint")).toBeNull();
  expect(control("Start task 1")).toBeTruthy();
});
