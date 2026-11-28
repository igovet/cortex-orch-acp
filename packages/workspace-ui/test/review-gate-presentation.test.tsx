import { render } from "solid-js/web";
import { afterEach, beforeEach, expect, it } from "vitest";
import { emptyWorkspace, LOCAL_MACHINE_ID, type Check, type Sprint, type WorkspaceState } from "@cortex/workspace-domain";
import { Workspace } from "../src";
import { checksOf, type SessionModel } from "../src/session-model";
import { sentence, stale } from "../src/review-model";
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

it.each([true, false])("acceptance presentation identifies automatic=%s without offering Accept again", async automatic => {
  const s = withResult("ready_to_merge");
  s.tasks[0].result!.automaticReady = true;
  s.tasks[0].acceptance = { version: 1, digest: "digest", automatic, at: 1 };
  await mount(s, taskPath);
  expect(text()).toContain(automatic ? "accepted automatically" : "accepted by you");
  expect(text()).not.toContain(automatic ? "accepted by you" : "accepted automatically");
  expect(buttons("Accept v1")).toHaveLength(0);
});
it("keeps the merged result after confirmed cleanup and still shows observed drift or an incomplete merge", async () => {
  const s = withResult("done"), task = s.tasks[0];
  task.checkout = { kind: "worktree", path: "/linked", branch: "linked", unavailable: true };
  task.work = "merged";
  task.acceptance = { version: 1, digest: "digest" };
  task.integration = { method: "squash", digest: "digest", targetBranch: "main", worktreeCleanup: "removed" };
  task.workspaceDigest = null;
  const port = await mount(s, taskPath);
  const card = () => document.querySelector(".rcard")!.textContent!;
  expect(card()).toContain("accepted by you");
  expect(card()).not.toContain("Stale result");
  for (const [index, patch] of [{ workspaceDigest: "different" }, { status: "ready_to_merge" as const },
    { integration: { ...task.integration, worktreeCleanup: "retained" } },
    { result: { ...task.result!, warnings: ["workspace_drift"] } }].entries()) {
    const state = structuredClone(s);
    Object.assign(state.tasks[0], patch);
    port.emit(projection(String(index + 2), state)); await settle();
    expect(card()).toContain("Stale result");
  }
});
const REPAIR_ASK = "Let the agent fix this and publish a fresh result. Automatic checks it again and finishes the task when it passes.";
const repairHold = (code: string, reason: string) => ({ kind: "settlement_blocked" as const, taskId: "t", code, reason, needsOwner: true,
  nextActor: "owner" as const, ownerActions: ["allow_repair" as const, "close_as_is" as const], asks: [REPAIR_ASK], holdId: `hold-${code}` });
it("a stale automatic acceptance pins the service's decision in the conversation, once", async () => {
  const s = withResult("ready_to_merge");
  s.tasks[0].sprintId = "sp";
  s.sprints = [{ id: "sp", projectId: "p", title: "Release", description: "Release", runMode: "automatic", targetBranch: null, planning: "created", conversation: [], plan: null, materializedVersion: 1, taskIds: ["t"], queueState: "paused", pauseReason: "stale", automaticQueue: repairHold("automatic_acceptance_blocked", "The accepted result is stale") }];
  s.tasks[0].acceptance = { version: 1, digest: "digest", automatic: true, at: 1 };
  s.tasks[0].workspaceDigest = "changed-after-acceptance";
  s.tasks[0].result!.warnings = ["workspace_drift"];
  expect(stale(s.tasks[0])).toBe(true);
  const port = await mount(s, taskPath);
  const card = document.querySelector<HTMLElement>(".chat-review-hold")!;
  expect(card.textContent).toContain("The accepted result is stale");
  expect(card.textContent).toContain(REPAIR_ASK);
  expect(buttons("Let the agent fix it")).toHaveLength(1);
  expect(buttons("Close as is…")).toHaveLength(1);
  expect(buttons("Ask agent to fix")).toHaveLength(0);
  expect([...document.querySelectorAll("a.btn")].filter(a => a.textContent?.trim() === "Finish task")).toHaveLength(0);
  expect(buttons("Accept v1")).toHaveLength(0);
  const allow = buttons("Let the agent fix it")[0];
  allow.click(); allow.click();
  await settle();
  const allowed = port.commands.filter(c => c.payload.type === "allow_queue_repair");
  expect(allowed).toHaveLength(1);
  expect(allowed[0].payload).toMatchObject({ sprintId: "sp", taskId: "t", code: "automatic_acceptance_blocked", holdId: "hold-automatic_acceptance_blocked" });
  expect(document.querySelector("dialog[open]")).toBeNull();
  port.receipt(false);
  await settle();
  expect(document.querySelector(".chat-review-hold")?.textContent).toContain("Refused");
});
it.each(["task_content_unknown", "task_files_changed"])("shows %s in the open chat as soon as the hold arrives, before result details", async code => {
  const state = withResult("ready_to_merge");
  state.tasks[0].sprintId = "sp";
  state.tasks[0].acceptance = {version: 2, digest: "digest", automatic: true, at: 1};
  state.tasks[0].result = null;
  state.sprints = [{id:"sp", projectId:"p", title:"Release", description:"Release", runMode:"automatic", targetBranch:null, planning:"created", conversation:[], plan:null, materializedVersion:1, taskIds:["t"], queueState:"running", pauseReason:null}];
  const port = await mount(state, taskPath);
  expect(document.querySelector(".chat-review-hold")).toBeNull();
  const held = structuredClone(state);
  const reason = "The recorded task files changed after review.";
  Object.assign(held.sprints[0], {queueState:"paused", pauseReason:reason, automaticQueue:repairHold(code, reason)});
  port.emit(projection("2", held)); await settle();
  const panel = document.querySelector<HTMLElement>(".chat-review-hold");
  expect(panel?.textContent).toContain(reason);
  expect(panel?.closest(".timeline > .tl-in > .conversation-actions")).not.toBeNull();
  expect(buttons("Let the agent fix it")).toHaveLength(1);
  buttons("Let the agent fix it")[0].click();
  await settle();
  expect(port.commands.at(-1)?.payload).toMatchObject({type:"allow_queue_repair", sprintId:"sp", taskId:"t", code});
  expect(document.querySelector("dialog[open]")).toBeNull();
});
it("a used card action is offered again when a new hold arrives", async () => {
  const state = withResult("review");
  state.tasks[0].sprintId = "sp";
  const reason = "The first repair did not pass.";
  state.sprints = [{id:"sp", projectId:"p", title:"Release", description:"Release", runMode:"automatic", targetBranch:null, planning:"created", conversation:[], plan:null, materializedVersion:1, taskIds:["t"], queueState:"paused", pauseReason:reason, automaticQueue:repairHold("task_failed", reason)}];
  const port = await mount(state, taskPath);
  buttons("Let the agent fix it")[0].click();
  await settle();
  port.receipt(true);
  await settle();
  expect(buttons("Let the agent fix it")[0].getAttribute("aria-disabled")).toBe("true");
  const again = structuredClone(state);
  const second = "The second repair did not pass either.";
  Object.assign(again.sprints[0], {pauseReason:second, automaticQueue:repairHold("task_failed", second)});
  port.emit(projection("2", again)); await settle();
  expect(buttons("Let the agent fix it")[0].getAttribute("aria-disabled")).toBeNull();
});
it("sends Try again on the first click of each missing-branch hold, despite a late receipt for the old hold", async () => {
  const state = withResult("ready_to_merge");
  state.tasks[0].sprintId = "sp";
  state.tasks[0].acceptance = { version: 1, digest: "digest", automatic: false, at: 1 };
  const reason = "The target branch must exist locally.";
  state.sprints = [{
    id: "sp", projectId: "p", title: "Release", description: "Release", runMode: "automatic", targetBranch: "later",
    planning: "created", conversation: [], plan: null, materializedVersion: 1, taskIds: ["t"], queueState: "paused", pauseReason: reason,
    automaticQueue: {
      kind: "settlement_blocked", taskId: "t", code: "unknown_branch", reason, needsOwner: true, nextActor: "owner",
      ownerActions: ["retry", "close_as_is"], asks: ["Fix the cause named above, then let Automatic try again."], holdId: "first",
    },
  }];
  const port = await mount(state, taskPath);
  const retry = buttons("Try again")[0];
  expect(retry.closest(".chat-review-hold")?.textContent).toContain(reason);
  retry.dispatchEvent(new MouseEvent("pointerdown", { bubbles: true }));
  port.emit(projection("2", structuredClone(state))); await settle();
  expect(buttons("Try again")[0]).toBe(retry);
  retry.click(); retry.click();
  expect(port.commands.map(c => c.payload)).toEqual([expect.objectContaining({ type: "run_sprint", sprintId: "sp" })]);

  // A fast recheck can publish a new hold before the queue-patch reply arrives.
  const again = structuredClone(state);
  again.sprints[0].automaticQueue!.holdId = "second";
  port.emit(projection("3", again)); await settle();
  expect(buttons("Try again")).toHaveLength(1);
  port.receipt(true); await settle();
  expect(buttons("Try again")).toHaveLength(1);
  expect(buttons("Try again")[0].getAttribute("aria-disabled")).toBeNull();
  buttons("Try again")[0].click();
  expect(port.commands.map(c => c.payload)).toEqual([
    expect.objectContaining({ type: "run_sprint", sprintId: "sp" }),
    expect.objectContaining({ type: "run_sprint", sprintId: "sp" }),
  ]);
  port.receipt(true); await settle();
});
it("an agent or runtime hold asks nothing of the owner", async () => {
  const s = withResult("review");
  s.tasks[0].sprintId = "sp";
  s.sprints = [{ id: "sp", projectId: "p", title: "Release", description: "Release", runMode: "automatic", targetBranch: null, planning: "created", conversation: [], plan: null, materializedVersion: 1, taskIds: ["t"], queueState: "paused", pauseReason: "Waiting for the engine",
    automaticQueue: { kind: "continuation", taskId: "t", code: "external_blocker", reason: "The engine usage limit is reached. Automatic continues by itself when the engine is available again.", needsOwner: false, nextActor: "runtime", ownerActions: [], asks: [] } }];
  await mount(s, taskPath);
  expect(document.querySelector(".chat-review-hold")).toBeNull();
  expect(document.querySelector(".ws-hold-quiet")?.textContent).toContain("This continues by itself");
  expect(buttons("Close as is…")).toHaveLength(0);
});
it("does not ask the owner to repair while Automatic has taken the stale result", async () => {
  const s = withResult("ready_to_merge");
  s.tasks[0].sprintId = "sp";
  s.sprints = [{ id: "sp", projectId: "p", title: "Release", description: "Release", runMode: "automatic", targetBranch: null, planning: "created", conversation: [], plan: null, materializedVersion: 1, taskIds: ["t"], queueState: "running", pauseReason: null, automaticQueue: { kind: "settlement_blocked", taskId: "t", code: "automatic_acceptance_blocked", reason: "Repairing the current result", needsOwner: false } }];
  s.tasks[0].acceptance = { version: 1, digest: "digest", automatic: true, at: 1 };
  s.tasks[0].workspaceDigest = "changed-after-acceptance";
  s.tasks[0].result!.warnings = ["workspace_drift"];
  expect(stale(s.tasks[0])).toBe(true);
  await mount(s, taskPath + "/changes");
  expect(text()).toContain("Repairing the current result");
  expect(buttons("Ask agent to fix")).toHaveLength(0);
  expect(buttons("Close as is…")).toHaveLength(0);
});
it("puts optional skipped evidence under a collapsed informational section after automatic acceptance", async () => {
  const s = withResult("ready_to_merge");
  s.tasks[0].result!.automaticReady = true;
  s.tasks[0].acceptance = { version: 1, digest: "digest", automatic: true, at: 1 };
  s.tasks[0].result!.checks.push({ name: "Paid live check", outcome: "not_run", kind: "policy", technicalReason: "Paid live check not requested by the owner", detail: "", source: "worker" });
  await mount(s, taskPath);
  expect(document.querySelector('[aria-label="Not passed"]')).toBeNull();
  expect(text()).toContain("Informational evidence");
  expect(text()).toContain("Paid live check not requested by the owner");
  // One head row: the chevron, the heading and its count share the summary's single section head, with no bare marker beside it.
  const fold = document.querySelector<HTMLDetailsElement>("details.rv-advisory")!;
  const summary = fold.querySelector<HTMLElement>(":scope > summary")!;
  expect(fold.open).toBe(false);
  expect(summary.children).toHaveLength(1);
  const head = summary.firstElementChild!;
  expect(head.classList.contains("l-sect")).toBe(true);
  expect(head.querySelector(".l-sect-lead .i-chevron-right[aria-hidden='true']")).toBeTruthy();
  expect(head.querySelector("h3")?.textContent).toBe("Informational evidence");
  expect(head.querySelector(".n")?.textContent).toBe("1");
  expect(summary.textContent?.trim()).toBe("Informational evidence1");
  // The rows sit after the head, inside the fold, and the head toggles them.
  const row = [...fold.querySelectorAll<HTMLElement>(":scope > .l-row")];
  expect(row).toHaveLength(1);
  expect(row[0].textContent).toContain("Paid live check");
  summary.click();
  expect(fold.open).toBe(true);
  summary.click();
  expect(fold.open).toBe(false);
});
it("explains an Automatic owner hold in the conversation before offering its actions", async () => {
  const s = withResult("ready_to_merge");
  s.tasks[0].sprintId = "sp";
  s.tasks[0].acceptance = { version: 1, digest: "digest", automatic: true, at: 1 };
  s.tasks[0].workspaceDigest = "changed";
  s.tasks[0].result!.warnings = ["workspace_drift"];
  s.sprints = [{ id: "sp", projectId: "p", title: "Release", description: "Release", runMode: "automatic", targetBranch: null, planning: "created", conversation: [], plan: null, materializedVersion: 1, taskIds: ["t"], queueState: "paused", pauseReason: "The accepted result is stale", automaticQueue: repairHold("stale_result", "The accepted result is stale") }];
  await mount(s, taskPath);
  const card = document.querySelector<HTMLElement>('[aria-label="Your decision is needed"]')!;
  expect(card).toBeTruthy();
  expect(card.textContent).toContain("Automatic needs your decision");
  expect(card.textContent).toContain("What you are asked to allow:");
  expect(card.textContent).toContain("finishes the task when it passes");
  // The primary action grants the request; Close as is is separate and last.
  const labels = [...card.querySelectorAll("button")].map(b => b.textContent?.trim());
  expect(labels).toEqual(["Let the agent fix it", "Close as is…"]);
  expect(card.querySelector("button")!.className).toContain("btn-primary");
  expect(card.textContent).not.toMatch(/_/);
  // The task header points to the same decision.
  expect(buttons("See what is needed")).toHaveLength(1);
});
function awaitingFinish(accepted: boolean) {
  const state = withResult(accepted ? "ready_to_merge" : "review");
  const task = state.tasks[0];
  task.sprintId = "sp";
  task.result!.sourceVersion = "47";
  task.reviewActions = { accept: true, reason: null, unblockAction: null, acknowledgeOpenChecks: true };
  task.result!.checks.push({ name: "Staging check", kind: "policy", outcome: "not_run", detail: "Requires access to staging." });
  if (accepted) task.acceptance = { version: 1, digest: "digest", at: 1 };
  const reason = "The staging check requires owner acknowledgement.";
  state.sprints = [{ id: "sp", projectId: "p", title: "Release", description: "Release", runMode: "automatic", targetBranch: "main", planning: "created", conversation: [], plan: null, materializedVersion: 1, taskIds: ["t"], queueState: "paused", pauseReason: reason, automaticQueue: { kind: accepted ? "owner_integration" : "settlement_blocked", taskId: "t", code: accepted ? "owner_accepted" : "automatic_acceptance_blocked", reason, needsOwner: true,
    nextActor: "owner", ownerActions: ["allow_finish", "close_as_is"], asks: ["Finish without the restricted check “Staging check”. Agents are not allowed to run it."] } }];
  return state;
}

it.each([false, true])("authorizes Automatic to finish the displayed result from chat, previously accepted=%s", async accepted => {
  const port = await mount(awaitingFinish(accepted), taskPath);
  expect(buttons("Allow Automatic to finish…")).toHaveLength(1);
  buttons("Allow Automatic to finish…")[0].click(); await settle();
  const dialog = document.querySelector("dialog[open]")!;
  expect(dialog.textContent).toContain("Staging check");
  expect(dialog.textContent).toContain("main");
  dialog.querySelector<HTMLInputElement>("input[type=checkbox]")!.click();
  const confirm = buttons("Allow Automatic to finish")[0];
  confirm.click(); confirm.click(); await settle();
  expect(port.commands.map(c => c.payload)).toEqual([expect.objectContaining({ type: "accept_result", taskId: "t", digest: "digest", sourceVersion: "47", automaticFinish: { sprintId: "sp", targetBranch: "main" }, openChecks: true })]);
});
it("expires an open finish approval when a newer source version has the same digest and display number", async () => {
  const state = awaitingFinish(false);
  const port = await mount(state, taskPath);
  buttons("Allow Automatic to finish…")[0].click(); await settle();
  document.querySelector<HTMLInputElement>("dialog[open] input[type=checkbox]")!.click();
  const next = structuredClone(state); next.tasks[0].result!.sourceVersion = "48";
  port.emit(projection("2", next)); await settle();
  const confirm = buttons("Allow Automatic to finish")[0];
  expect(confirm.getAttribute("aria-disabled")).toBe("true");
  confirm.click(); await settle();
  expect(port.commands).toHaveLength(0);
});
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
  const sprint: Sprint = { id: "sp", projectId: "p", title: "Release", description: "Release", runMode: "hybrid", targetBranch: null, planning: "created", conversation: [], plan: null, materializedVersion: 1, taskIds: ["t", "u"], queueState: "complete", pauseReason: null };
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
