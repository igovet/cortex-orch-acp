import { render } from "solid-js/web";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { emptyWorkspace, LOCAL_MACHINE_ID, mapTask, overlayTask, type WorkspaceState } from "@cortex/workspace-domain";
import type { Task as WireTask } from "@cortex/workspace-contract";
import { Workspace } from "../src";
import { buildIndex } from "../src/selectors";
import { resumeState, sessionModel } from "../src/session-model";
import { flush, projection, scriptedPort, wireTask, withTask } from "./support";

/**
 * Every paused, recovering, held or ended task state reads the same everywhere and offers the one existing action that applies
 * (Resume for a pause, Continue for a held interruption), or says exactly why none does. Terminal and unknown states never
 * offer Resume or Continue. The states are built from the service's wire projection, as the real port maps it.
 */
let dispose: (() => void) | undefined;
beforeEach(() => { localStorage.clear(); sessionStorage.clear(); });
afterEach(() => { dispose?.(); document.body.innerHTML = ""; });
const settle = async () => { for (let i = 0; i < 8; i++) await flush(); };

const continuation = { policy: "automatic_continuation", continuationCount: 1, lastContinuationReason: null, pause: null };
const WIRE: Record<string, Partial<WireTask>> = {
  paused: { phase: "paused", runtimeState: "PAUSED", executionState: "paused" },
  interrupted: { phase: "recovery", runtimeState: "RECOVERING", executionState: "recovering", waitReason: "turn_interrupted" },
  recovering: { phase: "recovery", runtimeState: "RECOVERING", executionState: "recovering", waitReason: "automatic_continuation_pending", automaticContinuation: continuation },
  exitUnconfirmed: { phase: "recovery", runtimeState: "RECOVERING", executionState: "recovering", waitReason: null, warnings: ["process_exit_unconfirmed"] },
  exitPending: { phase: "recovery", runtimeState: "RECOVERING", executionState: "recovering", waitReason: null, warnings: ["process_exit_pending"] },
  // A recorded interruption whose process exit is not confirmed (or still being confirmed): it is live, but waits for evidence.
  interruptedExitUnconfirmed: { phase: "recovery", runtimeState: "RECOVERING", executionState: "recovering", waitReason: "turn_interrupted", warnings: ["process_exit_unconfirmed"] },
  interruptedExitPending: { phase: "recovery", runtimeState: "RECOVERING", executionState: "recovering", waitReason: "turn_interrupted", warnings: ["process_exit_pending"] },
  pausedExitUnconfirmed: { phase: "paused", runtimeState: "PAUSED", executionState: "paused", warnings: ["process_exit_unconfirmed"] },
  cancelled: { phase: "stopped", runtimeState: "CANCELLED", executionState: "stopped" },
  failed: { phase: "failed", runtimeState: "FAILED", executionState: "failed" },
  unknown: { phase: "recovery", runtimeState: "RECOVERING", executionState: "unknown", warnings: ["contradictory_evidence"] },
  running: { phase: "execution", runtimeState: "EXECUTING", executionState: "running" },
};
type Shape = keyof typeof WIRE;
function world(shape: Shape): WorkspaceState {
  const s = withTask(emptyWorkspace(), "Work");
  s.machines[0].connection = "connected";
  const task = mapTask(wireTask({ taskId: "t", title: "Work", titleAvailability: "generated", description: "Work", executionId: "e", ...WIRE[shape] }), { projectId: "p" });
  return { ...s, tasks: [task] };
}
const state = (s: WorkspaceState) => resumeState(buildIndex(s, 0).byId.get("t")!);

async function mount(s: WorkspaceState, path = `/m/${LOCAL_MACHINE_ID}/t/t`) {
  const p = scriptedPort({ capabilities: { everything: true, operations: [] } });
  history.replaceState(null, "", path);
  const root = document.createElement("div"); document.body.append(root);
  dispose = render(() => <Workspace port={p.port} />, root);
  await settle(); p.emit(projection("1", s)); await settle();
  return p;
}
const $ = <T extends Element = HTMLElement>(q: string) => document.querySelector<T>(q);
const buttons = (root: ParentNode = document) => [...root.querySelectorAll<HTMLElement>("button, a.btn")];
const band = () => buttons($(".tb-actions") ?? document.createElement("div")).map(b => b.textContent?.trim() ?? "");
const strip = () => $(".l-task .l-strip")?.textContent ?? "";
const composerAction = () => [...$('[aria-label="Task composer"] .c-bar')!.querySelectorAll<HTMLButtonElement>("button")].at(-1)!;
const menuItems = async () => {
  buttons().find(b => b.getAttribute("aria-label") === "Task actions")!.click(); await settle();
  return [...document.querySelectorAll<HTMLElement>('[role="menuitem"]')];
};
const menuItem = (items: HTMLElement[], label: string) => items.find(e => e.textContent?.trim().startsWith(label));
const inspectorState = () => [...document.querySelectorAll(".ins-sec")].find(s => s.querySelector(".ins-h")?.textContent === "State")?.textContent ?? "";
/** Every place a recovery's guidance is written: the status strip, the inspector's State and the title band. */
const guidance = () => `${strip()} ${inspectorState()} ${$(".tb-actions")?.textContent ?? ""}`;

describe("the one state-to-action rule", () => {
  it("maps each confirmed paused, recovery, wait and ending state to its kind and its single action", () => {
    expect(state(world("paused"))).toMatchObject({ kind: "paused", action: "continue_task", text: "Paused by you" });
    expect(state(world("interrupted"))).toMatchObject({ kind: "held", action: "retry_task", text: "Interrupted" });
    expect(state(world("recovering"))).toMatchObject({ kind: "automatic", action: null, text: "Recovering automatically" });
    expect(state(world("recovering"))!.why).toMatch(/by itself/);
    expect(state(world("exitUnconfirmed"))).toMatchObject({ kind: "evidence", action: null, text: "Waiting for exit evidence" });
    expect(state(world("exitPending"))).toMatchObject({ kind: "evidence", action: null });
    // The runtime refuses Resume while shutdown is unconfirmed, so a paused task with that warning does not offer it.
    expect(state(world("pausedExitUnconfirmed"))).toMatchObject({ kind: "evidence", action: null });
    // A recorded interruption whose process exit is not confirmed waits for that evidence, not for the owner.
    expect(state(world("interruptedExitUnconfirmed"))).toMatchObject({ kind: "evidence", action: null, text: "Waiting for exit evidence" });
    expect(state(world("interruptedExitPending"))).toMatchObject({ kind: "evidence", action: null, text: "Waiting for processes to stop" });
    expect(state(world("running"))).toBeNull();
  });

  it("never offers Resume or Continue on a terminal or unknown state, and says why", () => {
    for (const shape of ["cancelled", "failed", "unknown"] as const) {
      const r = state(world(shape))!;
      expect(r.action, shape).toBeNull();
      expect(r.why, shape).not.toBe("");
    }
    expect(state(world("cancelled"))).toMatchObject({ kind: "final", text: "Run cancelled" });
    expect(state(world("cancelled"))!.why).toMatch(/final and cannot be resumed/);
    expect(state(world("failed"))).toMatchObject({ kind: "final", text: "Run failed" });
    expect(state(world("unknown"))).toMatchObject({ kind: "unknown" });
    const offline = world("paused"); offline.machines[0].connection = "disconnected";
    expect(state(offline)).toMatchObject({ kind: "unknown", action: null });
    const closed = world("cancelled"); Object.assign(closed.tasks[0], { status: "done", closedAs: "as_is" });
    expect(state(closed)).toMatchObject({ kind: "final", action: null });
  });
});

describe("the file-wait display", () => {
  const running = { ...WIRE.running, executionId: "e" };
  const releaseTarget = "src/lib.rs, test/a.js and 2 more";
  const message = "Waiting for the task “Old description” to release " + releaseTarget;
  const waiting = () => mapTask(wireTask({ ...running, taskId: "t", title: "Work", waitReason: message,
    filesWait: { holderTaskId: "holder", releaseTarget }, warnings: ["waiting_for_files"] }), { projectId: "p" });
  const holder = () => mapTask(wireTask({ ...running, executionId: "h", taskId: "holder",
    description: "old description. More prose." }), { projectId: "p" });

  it("uses the actual board title, including generated and owner titles and the normal no-title label", () => {
    const s = world("running");
    const card = () => buildIndex(s, 0).byId.get("t")!;
    for (const overlay of [undefined, { title: "Generated title", titleState: "generated" }, { title: "Название владельца", titleState: "user" }] as const) {
      s.tasks = [waiting(), overlayTask(holder(), overlay, "")];
      const index = buildIndex(s, 0);
      expect(index.byId.get("holder")!.task.title).toBe(overlay?.title ?? "Old description");
      expect(card().fact).toMatchObject({
        text: `Waiting for the task “${s.tasks[1].title}” to release ${releaseTarget}`, icon: "hourglass",
      });
      expect(card().column).toBe("work");
    }
    s.tasks = [waiting()];
    expect(card().fact.text).toBe(message);
    // A finished overlay can arrive before the next runtime read clears the old warning.
    s.tasks = [{ ...waiting(), status: "done", process: "stopped" }];
    expect(card().filesWait).toBeUndefined();
  });

  it("shows an unclamped linked wait in the opened task, then removes it on the next confirmed read", async () => {
    const s = world("running");
    const title = "Название владельца";
    s.tasks = [waiting(), overlayTask(holder(), { title, titleState: "user" }, "")];
    const port = await mount(s);
    const notice = () => [...document.querySelectorAll(".l-task .l-strip")].find(el => el.textContent?.includes("to release"));
    expect(notice()?.textContent).toBe(`Waiting for the task “${title}” to release ${releaseTarget}`);
    expect(notice()?.classList.contains("l-strip-readable")).toBe(true);
    const link = notice()?.querySelector("a");
    expect(link?.textContent).toBe(title);
    expect(link?.getAttribute("href")).toContain("/t/holder");

    const resumed = { ...s, tasks: [world("running").tasks[0], s.tasks[1]] };
    port.emit(projection("2", resumed)); await settle();
    expect(notice()).toBeUndefined();
  });

  it("shows a Code edit wait as plain text without a task link", async () => {
    const s = world("running");
    const reason = "Waiting for you to finish editing src/lib.rs in Code";
    s.tasks = [mapTask(wireTask({ ...running, taskId: "t", waitReason: reason,
      warnings: ["waiting_for_files"] }), { projectId: "p" })];
    await mount(s);
    const line = [...document.querySelectorAll(".l-task .l-strip")].find(el => el.textContent === reason);
    expect(line).toBeDefined();
    expect(line?.querySelector("a")).toBeNull();
  });
});

describe("the task page", () => {
  it("a pause offers Resume in the composer, and Resume in the menu, never Continue", async () => {
    const p = await mount(world("paused"));
    expect(composerAction().getAttribute("aria-label")).toBe("Resume");
    expect(band()).not.toContain("Continue");
    const items = await menuItems();
    expect(menuItem(items, "Resume")!.getAttribute("aria-disabled")).toBeNull();
    expect(menuItem(items, "Continue")).toBeUndefined();
    composerAction().click(); await settle();
    expect(p.commands.map(c => c.payload.type)).toEqual(["continue_task"]);
  });

  it("a held interruption waits for you: Continue in the title band and the menu, the strip names it, no Resume", async () => {
    const p = await mount(world("interrupted"));
    expect(strip()).toContain("waits for you");
    expect(strip()).toContain("or use Continue");
    expect(band()).toContain("Continue");
    expect(composerAction().getAttribute("aria-label")).not.toBe("Resume");
    const items = await menuItems();
    expect(menuItem(items, "Continue")!.getAttribute("aria-disabled")).toBeNull();
    expect(menuItem(items, "Resume")).toBeUndefined();
    // The inspector names the same action the band offers.
    expect(inspectorState()).toContain("Continue lets the coordinator decide the next step");
    buttons($(".tb-actions")!).find(b => b.textContent?.trim() === "Continue")!.click(); await settle();
    expect(p.commands.map(c => c.payload.type)).toEqual(["retry_task"]);
  });

  for (const [shape, why] of [["interruptedExitUnconfirmed", "did not confirm that it stopped"], ["interruptedExitPending", "confirming that this task's processes stopped"]] as const) {
    it(`a recorded interruption still waiting for exit evidence (${shape}) gives the reason and never suggests Continue or a message`, async () => {
      // The task is live (a recorded interruption), which is where the strip used to say it waits for the owner.
      expect(sessionModel(buildIndex(world(shape), 0), "t")!.mode).toBe("live");
      const p = await mount(world(shape));
      expect(strip()).toContain(why);
      expect(inspectorState()).toContain(why);
      // No text anywhere tells the owner the turn waits for them or to use an action that is not offered.
      expect(guidance()).not.toMatch(/waits for you|Waiting for you|Send a message to continue|use Continue|Continue lets/);
      expect(band()).not.toContain("Continue");
      expect(band()).not.toContain("Resume");
      const items = await menuItems();
      expect(menuItem(items, "Continue")).toBeUndefined();
      expect(menuItem(items, "Resume")).toBeUndefined();
      expect(p.commands).toHaveLength(0);
    });
  }

  it("an automatic recovery says it continues by itself and offers no Resume or Continue", async () => {
    const p = await mount(world("recovering"));
    expect(strip()).toContain("Recovering automatically");
    // Neither the strip nor the inspector suggests Continue or a message to continue.
    expect(guidance()).not.toMatch(/waits for you|Waiting for you|Send a message to continue|use Continue|Continue lets/);
    expect(band()).not.toContain("Continue");
    expect(band()).not.toContain("Resume");
    const items = await menuItems();
    expect(menuItem(items, "Continue")).toBeUndefined();
    expect(menuItem(items, "Resume")).toBeUndefined();
    expect(p.commands).toHaveLength(0);
  });

  it("a process whose exit is not confirmed offers no Resume or Continue and explains why", async () => {
    const p = await mount(world("exitUnconfirmed"));
    expect(document.body.textContent).toContain("did not confirm that it stopped");
    expect(guidance()).not.toMatch(/waits for you|Send a message to continue|use Continue|Continue lets/);
    expect(band()).not.toContain("Continue");
    expect(band()).not.toContain("Resume");
    expect(p.commands).toHaveLength(0);
  });

  it("a pause whose process exit is not confirmed keeps Resume off with the reason", async () => {
    const p = await mount(world("pausedExitUnconfirmed"));
    expect(strip()).toContain("Waiting for exit evidence");
    const resume = composerAction();
    expect(resume.getAttribute("aria-label")).toBe("Resume");
    expect(resume.getAttribute("aria-disabled")).toBe("true");
    resume.click(); await settle();
    const items = await menuItems();
    expect(menuItem(items, "Resume")!.getAttribute("aria-disabled")).toBe("true");
    expect(menuItem(items, "Resume")!.textContent).toContain("did not confirm that it stopped");
    expect(p.commands).toHaveLength(0);
  });

  for (const shape of ["cancelled", "failed"] as const) {
    it(`a ${shape} run is final: the strip says it cannot be resumed and nothing offers Resume or Continue`, async () => {
      const p = await mount(world(shape));
      expect(strip()).toContain("cannot be resumed");
      expect(band()).not.toContain("Continue");
      expect(band()).not.toContain("Resume");
      const items = await menuItems();
      expect(menuItem(items, "Continue")).toBeUndefined();
      expect(menuItem(items, "Resume")).toBeUndefined();
      expect(p.commands).toHaveLength(0);
    });
  }
});

describe("returning a closed task for rework", () => {
  const closed = (warning = false) => {
    const s = world("cancelled");
    Object.assign(s.tasks[0], { status: "done", closedAs: "as_is", reworkAvailable: true, reworkVersion: "18",
      reworkExecutionId: "e", reworkOwnerEpoch: "2", reworkWarnings: warning ? ["dependents_started"] : [],
      result: { version: 3, digest: "old", checks: [], summary: "Old result" } });
    return s;
  };
  const field = () => $<HTMLTextAreaElement>('[aria-label="Task composer"] textarea')!;
  const type = (text: string) => { field().value = text; field().dispatchEvent(new Event("input", { bubbles: true })); };
  const titleAction = () => buttons($(".tb-actions")!).find(b => b.textContent?.trim() === "Return for rework");
  it("opens Chat from the title band, requires text, and lets the next result come from the projection", async () => {
    const p = await mount(closed(), `/m/${LOCAL_MACHINE_ID}/t/t/changes`);
    titleAction()!.click(); await settle();
    expect(document.activeElement).toBe(field());
    expect($('[aria-label="Task composer"]')?.textContent).toContain("Sending reopens this task for rework");
    expect(composerAction().getAttribute("aria-label")).toBe("Send and reopen");
    type("   "); composerAction().click(); await settle();
    expect(p.commands).toHaveLength(0);
    type("Correct the race"); composerAction().click(); await settle();
    expect(p.commands.map(c => c.payload)).toEqual([{ type: "rework_task", taskId: "t", expectedVersion: "18", executionId: "e", ownerEpoch: "2", text: "Correct the race", attachments: [] }]);
    p.receipt(); await settle();
    const working = world("running"); p.emit(projection("2", working)); await settle();
    expect(field().value).toBe("");
    expect(titleAction()).toBeUndefined();
    expect(buildIndex(working, 0).byId.get("t")?.column).toBe("work");
    expect(buttons().some(b => b.textContent?.trim() === "Accept")).toBe(false);
    expect(document.body.textContent).not.toContain("Old result");
    const next = world("running");
    Object.assign(next.tasks[0], { status: "review", process: "stopped", result: { version: 4, digest: "new", checks: [], summary: "Corrected the race" } });
    p.emit(projection("3", next)); await settle();
    expect(document.body.textContent).toContain("Corrected the race");
    expect(document.body.textContent).toContain("v4");
  });

  it("warns about started dependents; Cancel keeps the text and the refusal leaves it editable", async () => {
    const p = await mount(closed(true));
    menuItem(await menuItems(), "Return for rework")!.click(); await settle();
    type("Fix the old result");
    composerAction().click(); await settle();
    const dialog = () => $("dialog[open]")!;
    expect(dialog().textContent).toContain("Dependent sprint tasks already started using the old result");
    buttons(dialog()).find(b => b.textContent === "Cancel")!.click(); await settle();
    expect(p.commands).toHaveLength(0);
    expect(field().value).toBe("Fix the old result");
    composerAction().click(); await settle();
    buttons(dialog()).find(b => b.textContent === "Send and reopen")!.click(); await settle();
    expect(p.commands).toHaveLength(1);
    p.receipt(false, undefined, "stale_version", "The task changed. Refresh it before sending."); await settle();
    expect($(".mc-effect[role=alert]")?.textContent).toBe("The task changed. Refresh it before sending.");
    expect(field().value).toBe("Fix the old result");
    expect(field().disabled).toBe(false);
  });

  it.each(["board", "list"])("uses the same composer from the %s menu without dragging out of Done", async area => {
    await mount(closed(), `/m/${LOCAL_MACHINE_ID}/p/p/${area}`);
    if (area === "board") expect($<HTMLElement>('.card[data-task="t"]')?.draggable).toBe(false);
    $('[aria-label="Actions for Work"]')!.click(); await settle();
    menuItem([...document.querySelectorAll<HTMLElement>('[role="menuitem"]')], "Return for rework")!.click(); await settle();
    expect(document.activeElement).toBe(field());
  });

  it("offers rework for a closed-unverified projection, but not after archiving", async () => {
    const s = closed();
    Object.assign(s.tasks[0], { status: "closed_unverified", closedAs: "unconfirmed", process: "unknown" });
    const p = await mount(s);
    expect(titleAction()).toBeDefined();
    p.emit(projection("2", { ...s, tasks: [{ ...s.tasks[0], status: "done", closedAs: "as_is", process: "stopped", archive: { at: 1 } as NonNullable<WorkspaceState["tasks"][number]["archive"]> }] }));
    await settle();
    expect(titleAction()).toBeUndefined();
    expect(menuItem(await menuItems(), "Return for rework")).toBeUndefined();
    expect(field().placeholder).toContain("Restore the task first.");
  });
});

describe("the board card", () => {
  const card = () => $(`.card[data-task="t"]`)!;
  const primary = () => [...card().querySelectorAll<HTMLButtonElement>("button")].map(b => b.textContent?.trim() ?? "");
  const board = `/m/${LOCAL_MACHINE_ID}/p/p/board`;

  it("offers Resume for a pause and Continue for a held interruption, each sending its existing command", async () => {
    let p = await mount(world("paused"), board);
    expect(primary()).toContain("Resume");
    [...card().querySelectorAll<HTMLButtonElement>("button")].find(b => b.textContent?.trim() === "Resume")!.click(); await settle();
    expect(p.commands.map(c => c.payload.type)).toEqual(["continue_task"]);
    dispose?.(); document.body.innerHTML = "";
    p = await mount(world("interrupted"), board);
    expect(primary()).toContain("Continue");
    expect(primary()).not.toContain("Answer");
    [...card().querySelectorAll<HTMLButtonElement>("button")].find(b => b.textContent?.trim() === "Continue")!.click(); await settle();
    expect(p.commands.map(c => c.payload.type)).toEqual(["retry_task"]);
  });

  for (const shape of ["cancelled", "failed", "unknown", "recovering", "exitUnconfirmed", "interruptedExitUnconfirmed", "interruptedExitPending"] as const) {
    it(`offers no Resume or Continue on a ${shape} task`, async () => {
      await mount(world(shape), board);
      expect(primary()).not.toContain("Resume");
      expect(primary()).not.toContain("Continue");
    });
  }
});
