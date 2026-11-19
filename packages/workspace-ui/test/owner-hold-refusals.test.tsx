import { render } from "solid-js/web";
import { afterEach, beforeEach, expect, it } from "vitest";
import { emptyWorkspace, LOCAL_MACHINE_ID, type Sprint, type Task, type WorkspaceState } from "@cortex/workspace-domain";
import { Workspace } from "../src";
import { flush, projection, scriptedPort, withTask } from "./support";

/** Every refusal of a pinned owner decision is shown where it was pressed, and a refused action is not offered again for the same hold. */
let dispose: (() => void) | undefined;
beforeEach(() => { localStorage.clear(); sessionStorage.clear(); });
afterEach(() => { dispose?.(); document.body.innerHTML = ""; });
const settle = async () => { for (let i = 0; i < 8; i++) await flush(); };
const controls = () => [...document.querySelectorAll<HTMLElement>("button, a.btn, a.lnk")];
const control = (text: string) => controls().find(b => b.textContent?.trim() === text);
const card = () => document.querySelector<HTMLElement>(".chat-review-hold");
const taskPath = `/m/${LOCAL_MACHINE_ID}/t/t`;
const UNKNOWN = "A dispatched action still has an unknown outcome. Inspect the task activity before handing its folder to the next task.";
const sprint = (patch: Partial<Sprint> = {}): Sprint => ({ id: "sp", projectId: "p", description: "Coffee landing", title: "Coffee landing", runMode: "automatic", targetBranch: null, planning: "created", conversation: [], plan: null, materializedVersion: 1, taskIds: ["t"], queueState: "paused", pauseReason: null, ...patch });
const held = (): WorkspaceState => {
  const s = withTask(emptyWorkspace(), "Work");
  s.machines[0].connection = "connected";
  Object.assign(s.tasks[0], {
    status: "review", process: "stopped", workspaceDigest: "d2", sprintId: "sp",
    result: { version: 1, digest: "d1", sourceVersion: "1", summary: "Work", checks: [{ name: "Tests", detail: "", outcome: "passed" }], automaticReady: true },
    reviewActions: { accept: false, reason: "The workspace changed.", unblockAction: "request_changes" },
  } satisfies Partial<Task>);
  return { ...s, sprints: [sprint({ automaticQueue: {
    kind: "settlement_blocked", taskId: "t", code: "stale_result", reason: "The workspace changed after this result.", needsOwner: true, holdId: "h1",
    nextActor: "owner", ownerActions: ["allow_repair", "close_as_is"], asks: ["Let the agent fix this and publish a fresh result."],
  } as Sprint["automaticQueue"] })] };
};
async function mount(state: WorkspaceState) {
  const s = scriptedPort({ capabilities: { everything: true, operations: [] } });
  history.replaceState(null, "", taskPath);
  const root = document.createElement("div"); document.body.append(root);
  dispose = render(() => <Workspace port={s.port} />, root);
  await settle(); s.emit(projection("1", state)); await settle();
  return s;
}

it("a refused Let the agent fix it says why in the card and is not offered again for the same hold", async () => {
  const port = await mount(held());
  control("Let the agent fix it")!.click(); await settle();
  expect(port.commands.map(c => c.operation)).toEqual(["allow_queue_repair"]);
  port.receipt(false, undefined, "native_outside_outcome_unknown", UNKNOWN); await settle();
  expect(card()?.querySelector('[role="alert"]')?.textContent).toContain(UNKNOWN);
  // A click elsewhere (which dismisses a passing notice) never hides the card's own refusal.
  document.body.dispatchEvent(new PointerEvent("pointerdown", { bubbles: true })); await settle();
  expect(card()?.textContent).toContain(UNKNOWN);
  const again = control("Let the agent fix it")!;
  expect(again.getAttribute("aria-disabled")).toBe("true");
  again.click(); await settle();
  expect(port.commands).toHaveLength(1);
});

it("a refused Close as is says why in the dialog and in the card, and the card stops offering it for the same hold", async () => {
  const port = await mount(held());
  control("Close as is…")!.click(); await settle();
  control("Close as is and continue")!.click(); await settle();
  expect(port.commands.map(c => c.operation)).toEqual(["close_task"]);
  port.receipt(false, undefined, "native_outside_outcome_unknown", UNKNOWN); await settle();
  const dialog = document.querySelector("dialog[open]");
  expect(dialog?.textContent).toContain(UNKNOWN);
  document.body.dispatchEvent(new PointerEvent("pointerdown", { bubbles: true })); await settle();
  expect(document.querySelector("dialog[open]")?.textContent).toContain(UNKNOWN);
  control("Keep working")!.click(); await settle();
  expect(document.querySelector("dialog[open]")).toBeNull();
  expect(card()?.querySelector('[role="alert"]')?.textContent).toContain(UNKNOWN);
  const close = control("Close as is…")!;
  expect(close.getAttribute("aria-disabled")).toBe("true");
  close.click(); await settle();
  expect(document.querySelector("dialog[open]")).toBeNull();
  expect(port.commands).toHaveLength(1);
});

it("a new hold offers its actions again after a refusal", async () => {
  const state = held();
  const port = await mount(state);
  control("Let the agent fix it")!.click(); await settle();
  port.receipt(false, undefined, "native_outside_outcome_unknown", UNKNOWN); await settle();
  const next = structuredClone(state);
  next.sprints[0].automaticQueue = { ...next.sprints[0].automaticQueue!, holdId: "h2" } as Sprint["automaticQueue"];
  port.emit(projection("2", next)); await settle();
  expect(control("Let the agent fix it")?.getAttribute("aria-disabled")).toBeNull();
  expect(card()?.querySelector('[role="alert"]')).toBeNull();
});
