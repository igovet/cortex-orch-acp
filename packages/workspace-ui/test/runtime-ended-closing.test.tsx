import { render } from "solid-js/web";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { emptyWorkspace, LOCAL_MACHINE_ID, mapTask, overlayTask, type Sprint, type Task, type WorkspaceState } from "@cortex/workspace-domain";
import { Workspace } from "../src";
import { archiveBlocked } from "../src/git-actions-model";
import { buildIndex, inboxItems } from "../src/selectors";
import { sprintSummary } from "../src/sprint-model";
import { flush, projection, scriptedPort, wireTask, withTask } from "./support";

/** Every way a task can end without the owner's acceptance has a closing action that works, and archiving never asks. */
let dispose: (() => void) | undefined;
beforeEach(() => { localStorage.clear(); sessionStorage.clear(); });
afterEach(() => { dispose?.(); document.body.innerHTML = ""; });
const settle = async () => { for (let i = 0; i < 8; i++) await flush(); };
const band = () => document.querySelector<HTMLElement>(".tb-main")!;
const buttons = (root: ParentNode = document) => [...root.querySelectorAll<HTMLElement>("button, a.btn")];
const button = (label: string, root: ParentNode = document) => buttons(root).find(b => b.textContent?.trim() === label);
const text = () => document.body.textContent ?? "";
const taskPath = `/m/${LOCAL_MACHINE_ID}/t/t`;
const world = (task: Partial<Task> = {}, project: { hasGit?: boolean } = {}, sprints: Sprint[] = []): WorkspaceState => {
  const s = withTask(emptyWorkspace(), "Work");
  s.machines[0].connection = "connected";
  Object.assign(s.tasks[0], { executionId: "e", process: "stopped", ...task }); Object.assign(s.projects[0], project);
  return { ...s, sprints };
};
async function mount(state: WorkspaceState, path = taskPath) {
  const s = scriptedPort({ capabilities: { everything: true, operations: [] } });
  history.replaceState(null, "", path);
  const root = document.createElement("div"); document.body.append(root);
  dispose = render(() => <Workspace port={s.port} />, root);
  await settle(); s.emit(projection("1", state)); await settle();
  return s;
}
const ownerResolved = () => mapTask(wireTask({ taskId: "t", executionId: "e", phase: "stopped", runtimeState: "CANCELLED", executionState: "stopped", warnings: ["owner_resolved_without_certification"] }), { projectId: "p" });

describe("a run that ended by itself", () => {
  for (const status of ["failed", "cancelled"] as const) {
    it(`a ${status} run offers Close task… at the end of the conversation, and the dialog closes it as is without offering Stop`, async () => {
      const p = await mount(world({ status }));
      const close = button("Close task…", document.querySelector(".conversation-actions")!);
      expect(close).toBeTruthy();
      expect(close!.getAttribute("aria-disabled")).toBeNull();
      close!.click(); await settle();
      const dialog = document.querySelector<HTMLElement>("dialog[open]")!;
      expect(dialog.textContent).not.toContain("Only want to pause?");
      expect(button("Keep it open", dialog)).toBeTruthy();
      button("Close task", dialog)!.click(); await settle();
      expect(p.commands.at(-1)!.payload).toMatchObject({ type: "close_task", taskId: "t", choice: "as_is", confirmed: true });
    });
  }
  it("its composer says how it is closed", async () => {
    await mount(world({ status: "failed" }));
    expect(text()).toContain("Close task moves it to Done");
  });
});

describe("a closed task", () => {
  it("offers Archive in the title band as one command, with no question or dialog", async () => {
    const p = await mount(world({ status: "done", closedAs: "as_is", notVerifiedByCortex: true, doneAt: 1000 }));
    const archive = button("Archive", band())!;
    expect(archive.getAttribute("aria-disabled")).toBeNull();
    archive.click(); await settle();
    expect(p.commands).toHaveLength(1);
    expect(p.commands[0].payload).toMatchObject({ type: "archive_task", taskId: "t" });
    expect(document.querySelector("dialog[open]")).toBeNull();
  });
  it("in a folder without Git, a closed task still offers Archive in the title band", async () => {
    const p = await mount(world({ status: "done", closedAs: "as_is", doneAt: 1000 }, { hasGit: false }));
    button("Archive", band())!.click(); await settle();
    expect(p.commands.map(c => c.payload)).toEqual([expect.objectContaining({ type: "archive_task", taskId: "t" })]);
  });
});

describe("Finish chosen on the runtime's stop question", () => {
  it("reads as closed as is and archives once the machine presents its close, which it now always does", () => {
    const runtimeOnly = ownerResolved();
    // A missing saved close no longer prevents a non-destructive archive.
    expect(archiveBlocked(runtimeOnly)).toBeUndefined();
    // The machine presents the owner's runtime close as a saved close as is (`context::product`).
    const presented = overlayTask(runtimeOnly, { status: "done", closedAs: "as_is", notVerifiedByCortex: true, doneAt: 5 }, "main");
    expect(presented.observedPhase).toBeUndefined();
    expect(archiveBlocked(presented)).toBeUndefined();
    const s = world(presented);
    const index = buildIndex(s, Date.now());
    expect(index.byId.get("t")!.column).toBe("done");
    expect(index.byId.get("t")!.fact.text).toMatch(/^Closed as is/);
    expect(inboxItems(index)).toEqual([]);
  });
  it("a sprint member closed this way counts as closed and unblocks the next task", () => {
    const presented = overlayTask(ownerResolved(), { status: "done", closedAs: "as_is", notVerifiedByCortex: true, doneAt: 5 }, "main");
    const sp: Sprint = { id: "sp", projectId: "p", description: "S", title: "S", runMode: "automatic", targetBranch: null, planning: "created", conversation: [], plan: null, materializedVersion: 1, taskIds: ["t", "u"], queueState: "running", pauseReason: null };
    const s = world({ ...presented, sprintId: "sp" }, {}, [sp]);
    s.tasks.push({ ...s.tasks[0], id: "u", title: "Next", status: "draft", executionId: null, closedAs: null, doneAt: null, notVerifiedByCortex: false, observedPhase: undefined, dependencies: ["t"], warnings: [] });
    const summary = sprintSummary(buildIndex(s, Date.now()), sp);
    expect(summary.closed).toBe(1);
    // No current order from the service: the unblocked draft is queued, not called Next.
    expect(summary.rows.find(r => r.view.id === "u")!.word.text).toBe("Queued");
  });
});

describe("a runtime result that can never be accepted", () => {
  it("offers Close as is… beside the reason Accept is off", async () => {
    const result = { kind: "work" as const, version: 1, digest: "d", checks: [], summary: "From an earlier version", acceptanceAvailable: false, sourceVersion: "1" };
    const p = await mount(world({ status: "review", result, workspaceDigest: "d", reviewActions: { accept: false, reason: "This result is from an earlier version of the app", unblockAction: null } }));
    const close = buttons().find(b => b.textContent?.trim() === "Close as is…")!;
    expect(close).toBeTruthy();
    close.click(); await settle();
    const dialog = document.querySelector<HTMLElement>("dialog[open]")!;
    button("Close task", dialog)!.click(); await settle();
    expect(p.commands.at(-1)!.payload).toMatchObject({ type: "close_task", taskId: "t", choice: "as_is" });
  });
});
