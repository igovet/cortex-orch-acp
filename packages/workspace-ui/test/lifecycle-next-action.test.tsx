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
  const sp = { ...sprint({ planning: "cancelled", materializedVersion: null }), planningFailure: { text: reason, step: "start" as const, code: "planning_start_refused", at: 123 } };
  const p = await mount(world({}, {}, [sp]), `/m/${LOCAL_MACHINE_ID}/p/p/s/sp/planning`);
  const alert = document.querySelector('[role="alert"]');
  expect(alert?.textContent).toContain(reason);
  expect(alert?.textContent).toContain("While starting the planning task");
  expect(alert?.textContent).toContain("Change request");
  expect(alert?.closest(".sp-body")).toBeNull();
  control("Retry planning")!.click(); await settle();
  expect(ops(p)).toEqual(["resume_planning"]);
});

const planningPath = `/m/${LOCAL_MACHINE_ID}/p/p/s/sp/planning`;
const terminalSprint = (kind: "limit" | "unknown" | "owner_stopped", code: string, reason: string, recommendedAction = "retry") => ({
  ...sprint({ planning: "cancelled", materializedVersion: null }),
  planningFailure: { text: reason, step: "run" as const, code, at: 123, kind, recommendedAction },
});

it("a classified planning failure names the limit and what Retry changes", async () => {
  const sp = terminalSprint("limit", "plan_part_too_large",
    "The planning task ended after a plan part was rejected for exceeding the size limit. Retry starts a new planning task using smaller parts from the outset.");
  const p = await mount(world({}, {}, [sp]), planningPath);
  const reason = document.querySelector('[role="alert"]')?.textContent;
  expect(reason).toContain("size limit");
  expect(reason).toContain("using smaller parts");
  expect(reason).toContain("saved request, attachments and answers");
  expect(reason).toContain("previously approved proposal");
  expect(text()).not.toContain("Resume to continue");
  control("Retry planning")!.click(); await settle();
  expect(ops(p)).toEqual(["resume_planning"]);
});

it("an unknown planning failure says the cause is unknown", async () => {
  const sp = terminalSprint("unknown", "planning_task_stopped",
    "The planning task ended without a known cause. Retry starts a new planning task.");
  await mount(world({}, {}, [{...sp, planningTaskId: "t"}]), planningPath);
  expect(document.querySelector('[role="alert"]')?.textContent).toContain("without a known cause");
  expect(text()).not.toContain("Resume to continue");
  expect(control("Retry planning")).toBeTruthy();
});

it("a repeated limit failure offers Change request instead of Retry", async () => {
  const sp = terminalSprint("limit", "plan_part_too_large",
    "The planning task ended after a plan part was rejected for exceeding the size limit. The smaller-parts retry met the same limit. Change the request before trying again.",
    "change_request");
  const p = await mount(world({}, {}, [sp]), planningPath);
  const change = control("Change request")!;
  expect(change.classList.contains("btn-primary")).toBe(true);
  expect(control("Retry planning")).toBeUndefined();
  expect(control("Resume planning")).toBeUndefined();
  change.click(); await settle();
  const input = document.querySelector<HTMLTextAreaElement>("#ws-plan-input")!;
  expect(document.activeElement).toBe(input);
  input.value = "Cover phase one only";
  input.dispatchEvent(new Event("input", {bubbles:true}));
  document.querySelector<HTMLButtonElement>('button[aria-label="Send amendment and retry"]')!.click(); await settle();
  expect(p.commands.at(-1)?.payload).toMatchObject({type:"plan_message", text:"Cover phase one only"});
});

it("owner Stop stays neutral and does not promise unsaved parts", async () => {
  const sp = sprint({ planning:"cancelled", materializedVersion:null });
  const p = await mount(world({}, {}, [sp]), planningPath);
  expect(text()).toContain("You stopped planning.");
  expect(text()).toContain("previously approved proposal");
  expect(text()).not.toMatch(/parts|planning failed/i);
  expect(document.querySelector('[role="alert"]')).toBeNull();
  expect(control("Resume planning")).toBeTruthy();
  const terminal = terminalSprint("owner_stopped", "planning_stopped_by_owner", "You stopped the planning task.");
  p.emit(projection("2", world({}, {}, [terminal]))); await settle();
  expect(text()).toContain("You stopped the planning task.");
  expect(text()).toContain("Retry starts a new planning task");
  expect(document.querySelector('[role="alert"]')).toBeNull();
});

it("a paused queue names the blocked review and offers its task before Continue queue", async () => {
  const sp = sprint({runMode:"hybrid", taskIds:["t"], queueState:"paused", pauseReason:"A result has missing or unsuccessful checks."});
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

it("a sprint whose tasks could not be loaded offers Retry and refuses to archive unseen tasks", async () => {
  const state = world({}, {}, [sprint({ taskIds: ["gone"] })]);
  state.projects[0].issue = "The project failed repeated reads.";
  await mount(state, `/m/${LOCAL_MACHINE_ID}/p/p/s/sp/summary`);
  expect(text()).toContain("Task details unavailable");
  expect(text()).not.toContain("No tasks yet");
  expect(control("Retry")).toBeTruthy();
  expect(control("Close sprint…")).toBeTruthy();
  const close = control("Close sprint…")!;
  expect(close.getAttribute("aria-disabled")).toBe("true");
  expect(close.getAttribute("data-why")).toContain("gone: task details not loaded");
  close.click(); await settle();
  expect(text()).not.toContain("Close this sprint?");
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
  expect(text()).toContain("A separate worktree needs a Git repository.");
  expect(text()).not.toContain("Merge into");
});

it("Work details and Changes stay reachable when the machine offers no Git operations", async () => {
  await mount(world(ready()), taskPath, { everything: false, operations: ["workspace.accept_result"] });
  const tabs = [...document.querySelectorAll('nav[aria-label="Task details"] a')].map(t => t.textContent?.trim() ?? "");
  expect(tabs.some(t => t.startsWith("Work details"))).toBe(true);
  expect(tabs.some(t => t.startsWith("Changes"))).toBe(true);
});

it("a stalled automatic queue names its reason beside the sprint with a way to the blocking task", async () => {
  await mount(world({ status: "review", sprintId: "sp" }, {}, [sprint({ runMode: "automatic", taskIds: ["t"], queueState:"paused", automaticQueue:{kind:"settlement_blocked",taskId:"t",code:"checks_not_passed",reason:"Checks did not pass",needsOwner:false} })]), `/m/${LOCAL_MACHINE_ID}/p/p/s/sp/planning`);
  const strip = [...document.querySelectorAll(".l-strip")].find(s => s.textContent?.includes("Checks did not pass"));
  expect(strip).toBeTruthy();
  expect([...strip!.querySelectorAll("a")].some(a => a.textContent?.trim() === "Open task")).toBe(true);
});

it("an Automatic acceptance hold is a Needs you decision with a real task route and owner escape", async () => {
  const reason = "Automatic acceptance needs a decision about the required check.";
  const sp = sprint({ runMode: "automatic", taskIds: ["t"], queueState: "paused", automaticQueue: {
    kind: "settlement_blocked", taskId: "t", code: "automatic_acceptance_blocked", reason, needsOwner: true,
    nextActor: "owner", ownerActions: ["allow_repair", "close_as_is"], asks: ["Let the agent fix this and publish a fresh result."],
  } });
  const state = world({ status: "review", process: "stopped", workspaceDigest: "d1", sprintId: "sp", result: { version: 1, digest: "d1", sourceVersion: "1", summary: "Work", checks: [{ name: "Required check", detail: "", outcome: "not_run", kind: "policy" }], automaticReady: true, acceptanceAvailable: false }, reviewActions: { accept: false, reason, unblockAction: "request_changes" } }, {}, [sp]);
  const port = await mount(state, `/m/${LOCAL_MACHINE_ID}/p/p/s/sp/board`);
  expect(text()).toContain(reason);
  expect(text()).toContain("Needs you");
  const open = control("Open task")!;
  expect(open.getAttribute("href")).toBe(taskPath);
  open.click(); await settle();
  expect(location.pathname).toContain("/t/t");
  // The pinned card renders the service's decision: what is asked, the allow action, and the separate close.
  expect(document.querySelector(".chat-review-hold")?.textContent).toContain("Let the agent fix this and publish a fresh result.");
  expect(control("Let the agent fix it")).toBeTruthy();
  expect(control("Ask agent to fix")).toBeUndefined();
  expect(text()).toContain("Close as is");
  expect(text()).not.toContain("Allow and run");
  control("Close as is…")!.click(); await settle();
  const dialog = document.querySelector("dialog[open]")!;
  expect(dialog.textContent).toContain(reason);
  expect(dialog.textContent).toContain("Required check");
  expect(dialog.textContent).toContain("creates no commit");
  const confirm = control("Close as is and continue")!;
  confirm.click(); confirm.click(); await settle();
  expect(ops(port)).toEqual(["close_task"]);
  expect(port.commands[0].payload).toMatchObject({ choice: "as_is", confirmed: true, taskId: "t" });
  port.receipt(true); await settle();
  expect(document.querySelector("dialog[open]")).toBeNull();
});

it("keeps close reachable when Accept allows acknowledgement and refuses a superseded confirmation", async () => {
  const sp = sprint({ runMode: "automatic", taskIds: ["t"], queueState: "paused", automaticQueue: {
    kind: "settlement_blocked", taskId: "t", code: "automatic_acceptance_blocked", reason: "Deployment was not run.", needsOwner: true,
  } });
  const state = world({ status: "review", sprintId: "sp", workspaceDigest: "d", result: {
    version: 1, sourceVersion: "7", digest: "d", summary: "Review", checks: [{
      name: "Deployment", detail: "Restricted by policy", technicalReason: "Restricted by policy", kind: "policy", outcome: "not_run"
    }], acceptanceAvailable: true }, reviewActions: { accept: true, reason: null, unblockAction: null, acknowledgeOpenChecks: true } }, {}, [sp]);
  const port = await mount(state, taskPath + "/changes");
  expect(control("Accept v1")?.getAttribute("aria-disabled")).toBeNull();
  control("Close as is…")!.click(); await settle();
  expect(control("Close as is and continue")?.getAttribute("aria-disabled")).toBeNull();
  const changed = structuredClone(state);
  changed.tasks[0].result!.sourceVersion = "8";
  port.emit(projection("2", changed)); await settle();
  const confirm = control("Close as is and continue")!;
  expect(confirm.getAttribute("aria-disabled")).toBe("true");
  confirm.click(); await settle();
  expect(ops(port)).toEqual([]);
});

it("a normal Automatic result stays runtime-owned and does not invent owner controls", async () => {
  const state = world({ status: "review", sprintId: "sp", result: { version: 1, digest: "d1", sourceVersion: "1", summary: "Work", checks: [{ name: "Tests", detail: "", outcome: "passed" }], automaticReady: true } }, {}, [sprint({ runMode: "automatic", taskIds: ["t"], queueState: "running" })]);
  await mount(state, `/m/${LOCAL_MACHINE_ID}/p/p/s/sp/board`);
  expect(text()).toContain("awaiting automatic acceptance");
  expect(control("Close as is…")).toBeUndefined();
  expect(control("Request changes…")).toBeUndefined();
});

const board = `/m/${LOCAL_MACHINE_ID}/p/p/s/sp/board`;
it("a paused board says why once, in the strip that describes Continue queue, and keeps the words out of the button", async () => {
  await mount(world({ status: "paused", sprintId: "sp" }, {}, [sprint({ runMode: "hybrid", taskIds: ["t"], queueState: "paused", pauseReason: "A task stopped" })]), board);
  const go = control("Continue queue")!;
  expect(go.getAttribute("aria-disabled")).toBe("true");
  const strip = document.getElementById(go.getAttribute("aria-describedby")!)!;
  expect(strip.textContent).toContain("Queue paused: Work is stopped. Resume it first.");
  expect(strip.textContent).toContain("Open task");
  expect(go.closest("[aria-label='Sprint queue']")?.textContent).not.toContain("Resume it first");
  expect(text()).not.toContain("Review this task");
});

it("a Manual sprint paused by a mode switch never points to a Continue queue button it does not show", async () => {
  await mount(world({ sprintId: "sp" }, {}, [sprint({ runMode: "manual", taskIds: ["t"], queueState: "paused", pauseReason: "This sprint is in manual mode." })]), board);
  expect(control("Continue queue")).toBeUndefined();
  expect(text()).not.toContain("Continue queue when you are ready");
  expect(text()).toContain("Start the next task when you are ready");
});

it("keeps the explanation of the automatic queue behind a question mark on the board, not in a strip", async () => {
  await mount(world({ sprintId: "sp" }, {}, [sprint({ runMode: "automatic", taskIds: ["t"] })]), board);
  const hint = document.querySelector<HTMLButtonElement>("[aria-label='Sprint queue'] .ws-hint")!;
  expect(hint.tagName).toBe("BUTTON");
  expect(hint.getAttribute("aria-label")).toBe("About the automatic queue");
  expect(document.getElementById(hint.getAttribute("popovertarget")!)?.textContent).toContain("Done on their own in the current branch");
  expect([...document.querySelectorAll(".l-strip")].some(s => s.textContent?.includes("Done on their own"))).toBe(false);
});

it("offers no explanation for a manual queue, which does not run by itself", async () => {
  await mount(world({ sprintId: "sp" }, {}, [sprint({ taskIds: ["t"] })]), board);
  expect(document.querySelector("[aria-label='Sprint queue'] .ws-hint")).toBeNull();
  expect(control("Start task 1")).toBeTruthy();
});

it("a failed run outside a queue asks the owner in plain words: try again, or close as is", async () => {
  const reason = "This task failed before it produced a result. Request changes to try again.";
  const ask = "Ask the agent to try again. It continues from the recorded state, and review and verification run again before the task can finish.";
  const state = world({ status: "failed", process: "stopped", reviewActions: { accept: false, reason, unblockAction: "request_changes",
    nextActor: "owner", ownerActions: ["ask_agent", "close_as_is"], asks: [ask] } });
  const port = await mount(state, taskPath);
  const card = document.querySelector<HTMLElement>(".chat-review-hold")!;
  expect(card.textContent).toContain("This task needs your decision");
  expect(card.textContent).toContain(ask);
  expect([...card.querySelectorAll("button")].map(b => b.textContent?.trim())).toEqual(["Ask the agent to try again", "Close as is…"]);
  // The run took no message before; a failed run with known effects is reopened by one.
  expect(text()).not.toContain("This run has ended. Nothing more can be sent to it");
  control("Ask the agent to try again")!.click(); await settle();
  expect(port.commands.at(-1)?.payload).toMatchObject({ type: "request_changes", taskId: "t" });
});

it("a Hybrid queue hold names its task and pins the same decision card", async () => {
  const reason = "This task depends on “Schema”, which was cancelled, so it cannot start by itself.";
  const sp = sprint({ runMode: "hybrid", taskIds: ["t"], queueState: "paused", pauseReason: reason, automaticQueue: {
    kind: "settlement_blocked", taskId: "t", code: "dependencies_blocked", reason, needsOwner: true,
    nextActor: "owner", ownerActions: ["close_as_is"], asks: ["Close this task as is or cancel it, because “Schema” will not finish. The queue then continues."],
  } });
  await mount(world({ status: "draft", sprintId: "sp" }, {}, [sp]), taskPath);
  const card = document.querySelector<HTMLElement>(".chat-review-hold")!;
  expect(card.textContent).toContain("The queue needs your decision");
  expect(card.textContent).toContain(reason);
  expect([...card.querySelectorAll("button")].map(b => b.textContent?.trim())).toEqual(["Close as is…"]);
});
