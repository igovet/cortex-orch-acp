import { render } from "solid-js/web";
import { afterEach, beforeEach, expect, it } from "vitest";
import { capabilityOf, emptyWorkspace, MACHINE_COMMANDS, mapProjectSlice, type PortCapabilities, type QueryResults, type WorkspaceState } from "@cortex/workspace-domain";
import { Workspace } from "../src";
import { buildIndex } from "../src/selectors";
import { closeBlocked, sessionModel, unconfirmedState } from "../src/session-model";
import { flush, projection, scriptedPort, wireTask, withTask } from "./support";

// A task whose state Cortex cannot confirm always ends in a decision: the runtime's own, or the owner's pinned card with a plain
// explanation, a way to inspect it and Close as is… (which replays nothing). A needs-you task with no open question offers
// Continue and Close as is…, never a control the machine does not have.
let dispose: (() => void) | undefined;
beforeEach(() => { localStorage.clear(); sessionStorage.clear(); });
afterEach(() => { dispose?.(); document.body.innerHTML = ""; });
const settle = async () => { for (let i = 0; i < 8; i++) await flush(); };
type Shape = "pending" | "unconfirmed" | "records" | "closed" | "waiting";
function world(shape: Shape, runMode?: "automatic"): WorkspaceState {
  const s = withTask(emptyWorkspace(), "Retained task");
  s.machines[0].connection = "connected";
  const recovery = shape === "pending" || shape === "unconfirmed";
  const slice = mapProjectSlice(s.projects[0], [wireTask({
    taskId: "t", description: "Retained task", title: "Retained task", titleAvailability: "generated", executionId: "e",
    phase: shape === "waiting" ? "waiting" : recovery ? "recovery" : "unknown",
    runtimeState: shape === "waiting" ? "WAITING_USER" : "RECOVERING",
    executionState: shape === "waiting" ? "waiting_user" : recovery ? "recovering" : "unknown",
    waitReason: null,
    warnings: shape === "pending" ? ["process_exit_pending"] : shape === "unconfirmed" ? ["process_exit_unconfirmed"] : shape === "closed" ? ["contradictory_evidence", "owner_closed_unconfirmed"] : shape === "records" ? ["contradictory_evidence"] : [],
  })]);
  const out = { ...s, tasks: slice.tasks, sessions: slice.sessions };
  if (runMode) {
    out.sprints = [{ id: "sp", projectId: "p", title: "Release", description: "Release", runMode, targetBranch: null, planning: "created", conversation: [], plan: null,
      materializedVersion: 1, taskIds: ["t"], queueState: "paused", pauseReason: null } as never];
    out.tasks = out.tasks.map(t => ({ ...t, sprintId: "sp" }));
  }
  return out;
}
const model = (s: WorkspaceState) => sessionModel(buildIndex(s, 0), "t")!;
async function mount(state: WorkspaceState, capabilities: PortCapabilities = { everything: true, operations: [] }, path = "/m/local/t/t") {
  const p = scriptedPort({ capabilities });
  p.port.query = async operation => {
    if (operation === "project.snapshot") return [projection("1", state)] as never;
    return ({ "capabilities.get": capabilities, "project.facts": [], "machine.facts": [] } as Partial<QueryResults>)[operation] as never;
  };
  history.replaceState(null, "", path);
  const root = document.createElement("div"); document.body.append(root);
  dispose = render(() => <Workspace port={p.port} />, root);
  await settle();
  return p;
}
const card = () => document.querySelector<HTMLElement>(".chat-review-hold");
const button = (scope: ParentNode, text: string) => [...scope.querySelectorAll<HTMLElement>("button, a")].find(b => b.textContent === text);

it("distinguishes a process hold from a records-only close", () => {
  for (const [shape, kind, closable] of [["pending", "stopping", true], ["unconfirmed", "exit_unconfirmed", true], ["records", "records", true]] as const) {
    const m = model(world(shape));
    expect(m.mode).toBe("unconfirmed");
    expect(m.view.unknown).toBe(true);
    expect(unconfirmedState(m)).toEqual({ kind, closable });
    expect(closeBlocked(m) === null).toBe(closable);
  }
  const missing = world("records");
  missing.tasks[0].warnings = [];
  expect(unconfirmedState(model(missing))).toEqual({ kind: "records", closable: false });
  expect(closeBlocked(model(missing))).toContain("Unavailable");
  expect(unconfirmedState(model(world("waiting")))).toBeNull();
});

it("an exit Cortex could not confirm pins the owner's card with an explanation, inspection and Close as is", async () => {
  const p = await mount(world("unconfirmed"));
  const hold = card()!;
  expect(hold).toBeTruthy();
  expect(hold.textContent).toContain("did not confirm that it stopped");
  expect(hold.textContent).toContain("Nothing it did is repeated");
  expect(hold.textContent).not.toMatch(/_|process_exit/);
  expect(button(hold, "Re-check state")).toBeTruthy();
  expect(button(hold, "Open workers")?.getAttribute("href")).toContain("/workers");
  expect(button(hold, "Open Diagnostics")).toBeTruthy();
  const close = button(hold, "Close as is…")!;
  expect(close.getAttribute("aria-disabled")).toBeNull();
  // The header brings the card into view.
  expect(button(document, "See what is needed")).toBeTruthy();
  // Sending stays off: a message could not continue the task while the exit is unknown.
  expect(document.querySelector<HTMLTextAreaElement>('textarea[aria-label="Message"]')!.disabled).toBe(true);
  close.click(); await settle();
  const dialog = document.querySelector("ul.conseq")!;
  expect(dialog.textContent).toContain("not confirmed as stopped");
  expect(dialog.textContent).toContain("repeats nothing");
  expect(p.commands).toHaveLength(0);
});

it("the same card names Automatic when the task belongs to an Automatic sprint", async () => {
  await mount(world("unconfirmed", "automatic"));
  expect(card()?.querySelector("h3")?.textContent).toBe("Automatic needs your decision");
});

it("a session still being stopped is the runtime's to decide: one quiet line and no question", async () => {
  await mount(world("pending"));
  expect(card()).toBeNull();
  expect(document.querySelector(".ws-hold-quiet")?.textContent).toContain("continues by itself");
  expect(button(document, "See what is needed")).toBeUndefined();
});

it("records that disagree can be inspected or explicitly closed without touching the run", async () => {
  const state = world("records");
  const p = await mount(state);
  const hold = card()!;
  expect(hold.textContent).toContain("disagree");
  expect(button(hold, "Open Diagnostics")).toBeTruthy();
  const close = button(hold, "Close as is…")!;
  expect(close.getAttribute("aria-disabled")).toBeNull();
  close.click(); await settle();
  const dialog = document.querySelector('dialog[open][data-dialog="Close task"]')!;
  expect(dialog.textContent).toContain("without trying to stop or resume");
  expect(button(dialog, "Cancel task")).toBeUndefined();
  expect(p.commands).toHaveLength(0);
  button(dialog, "Keep it open")!.click(); await settle();
  expect(p.commands).toHaveLength(0);
  close.click(); await settle();
  button(document.querySelector('dialog[open][data-dialog="Close task"]')!, "Close as is")!.click(); await settle();
  expect(p.commands.at(-1)?.payload).toMatchObject({ type: "close_task", taskId: "t", choice: "unconfirmed",
    confirmed: true, executionId: "e", expectedVersion: state.tasks[0].sourceVersion });
});

it("groups a closed unverified task apart from Done and excludes it from active counts", async () => {
  const state = world("closed", "automatic");
  state.tasks.push({ ...world("waiting").tasks[0], id: "done", title: "Finished task", status: "done", process: "stopped" });
  const summary = buildIndex(state, 0).summaries.get("p")!;
  expect(summary.counts).toMatchObject({ done: 1, closed_unverified: 1, work: 0, needs: 0, review: 0, merge: 0 });
  await mount(state, undefined, "/m/local/p/p/board");
  const closed = document.querySelector('section[data-col="closed_unverified"]')!;
  const done = document.querySelector('section[data-col="done"]')!;
  expect(closed.getAttribute("aria-label")).toBe("Closed, not verified");
  expect(closed.querySelector(".card")?.textContent).toContain("Retained task");
  expect(closed.querySelector(".card")?.getAttribute("draggable")).toBe("false");
  expect(done.querySelector(".card")?.textContent).toContain("Finished task");
  expect(done.textContent).not.toContain("Retained task");
  expect(closed.querySelector(".col-h .n")?.textContent).toBe("1");
  expect(done.querySelector(".col-h .n")?.textContent).toBe("1");

  document.querySelector<HTMLAnchorElement>('[role="tab"][href$="/list"]')!.click();
  await settle();
  const list = document.querySelector(".ws-list")!;
  const groups = [...list.querySelectorAll(".ws-list-h")];
  expect(groups.map(h => [h.textContent?.trim(), h.querySelector(".n")?.textContent])).toEqual([
    ["Done 1", "1"], ["Closed, not verified 1", "1"],
  ]);
});

it("a freshly loaded owner close shows the retained warning, not an open decision", async () => {
  const state = world("closed");
  expect(model(state)).toMatchObject({ mode: "closed", view: { unknown: false } });
  expect(state.tasks[0]).toMatchObject({ process: "unknown", notVerifiedByCortex: true });
  await mount(state);
  expect(card()).toBeNull();
  expect(document.body.textContent).toContain("Closed by you · Not verified");
  expect(document.body.textContent).toContain("Its records still disagree");
  expect(button(document, "Open Diagnostics")).toBeTruthy();
});

it("a needs-you task with no open question offers Continue and Close as is, and Continue sends retry", async () => {
  const p = await mount(world("waiting"), { everything: false, operations: [...MACHINE_COMMANDS.map(capabilityOf), "decisions.answer"] });
  const waiting = document.querySelector<HTMLElement>("#decision-issue")!;
  expect(waiting).toBeTruthy();
  expect(waiting.textContent).toContain("Continue lets the agent decide the next step");
  expect(waiting.textContent).not.toContain("Decision details are not available yet");
  const close = button(waiting, "Close as is…")!;
  expect(close.getAttribute("aria-disabled")).toBeNull();
  button(waiting, "Continue")!.click(); await settle();
  expect(p.commands.at(-1)?.payload).toMatchObject({ type: "retry_task", taskId: "t" });
});
