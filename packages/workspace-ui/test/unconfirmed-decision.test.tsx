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
type Shape = "pending" | "unconfirmed" | "records" | "waiting";
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
    warnings: shape === "pending" ? ["process_exit_pending"] : shape === "unconfirmed" ? ["process_exit_unconfirmed"] : shape === "records" ? ["contradictory_evidence"] : [],
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
async function mount(state: WorkspaceState, capabilities: PortCapabilities = { everything: true, operations: [] }) {
  const p = scriptedPort({ capabilities });
  p.port.query = async operation => {
    if (operation === "project.snapshot") return [projection("1", state)] as never;
    return ({ "capabilities.get": capabilities, "project.facts": [], "machine.facts": [] } as Partial<QueryResults>)[operation] as never;
  };
  history.replaceState(null, "", "/m/local/t/t");
  const root = document.createElement("div"); document.body.append(root);
  dispose = render(() => <Workspace port={p.port} />, root);
  await settle();
  return p;
}
const card = () => document.querySelector<HTMLElement>(".chat-review-hold");
const button = (scope: ParentNode, text: string) => [...scope.querySelectorAll<HTMLElement>("button, a")].find(b => b.textContent === text);

it("states which unconfirmed case holds and allows Close only where the runtime can still bind the task", () => {
  for (const [shape, kind, closable] of [["pending", "stopping", true], ["unconfirmed", "exit_unconfirmed", true], ["records", "records", false]] as const) {
    const m = model(world(shape));
    expect(m.mode).toBe("unconfirmed");
    expect(m.view.unknown).toBe(true);
    expect(unconfirmedState(m)).toEqual({ kind, closable });
    expect(closeBlocked(m) === null).toBe(closable);
  }
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

it("records that disagree get the card with the way to inspect them and a Close that says why it is off", async () => {
  const p = await mount(world("records"));
  const hold = card()!;
  expect(hold.textContent).toContain("disagree");
  expect(button(hold, "Open Diagnostics")).toBeTruthy();
  const close = button(hold, "Close as is…")!;
  expect(close.getAttribute("aria-disabled")).toBe("true");
  expect(close.getAttribute("data-why")).toContain("records disagree");
  close.click(); await settle();
  expect(document.querySelector("ul.conseq")).toBeNull();
  expect(p.commands).toHaveLength(0);
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
