import { render } from "solid-js/web";
import { afterEach, beforeEach, expect, it } from "vitest";
import { emptyWorkspace, LOCAL_MACHINE_ID, type Sprint, type Task, type WorkspaceState } from "@cortex/workspace-domain";
import { Workspace } from "../src";
import { flush, projection, scriptedPort, withTask } from "./support";

/** No screen shows an internal code: sidebar, board, task header, sprint strip and notifications read as plain language, and titles stay. */
let dispose: (() => void) | undefined;
beforeEach(() => { localStorage.clear(); sessionStorage.clear(); });
afterEach(() => { dispose?.(); document.body.innerHTML = ""; });
const settle = async () => { for (let i = 0; i < 8; i++) await flush(); };
const CODE = /[a-z]+_[a-z_]+/;
const TITLE = "Coffee landing page";
const sprint = (patch: Partial<Sprint> = {}): Sprint => ({ id: "sp", projectId: "p", description: "Coffee landing", title: "Coffee landing", runMode: "automatic", targetBranch: null, planning: "created", conversation: [], plan: null, materializedVersion: 1, taskIds: ["t"], queueState: "paused", pauseReason: null, ...patch } as Sprint);
const world = (task: Partial<Task>, sp: Sprint): WorkspaceState => {
  const s = withTask(emptyWorkspace(), TITLE);
  s.machines[0].connection = "connected";
  Object.assign(s.tasks[0], { title: TITLE, sprintId: "sp" }, task);
  return { ...s, sprints: [sp] };
};
async function show(state: WorkspaceState, path: string) {
  const s = scriptedPort({ capabilities: { everything: true, operations: [] } });
  history.replaceState(null, "", path);
  const root = document.createElement("div"); document.body.append(root);
  dispose = render(() => <Workspace port={s.port} />, root);
  await settle(); s.emit(projection("1", state)); await settle();
  return s;
}
const rows = () => [...document.querySelectorAll<HTMLElement>(".task-row")];
const continuing = (reason: string): Partial<Task> => ({ status: "working", automaticContinuation: { policy: "automatic_continuation", continuationCount: 1, lastContinuationReason: reason, pause: null } });

it("the sidebar keeps the task title and a separate plain status for every raw continuation code", async () => {
  for (const code of ["review_incomplete", "review_cycle_limit", "outcome_unknown", "a_brand_new_code"]) {
    await show(world(continuing(code), sprint({ queueState: "running" })), `/m/${LOCAL_MACHINE_ID}/t/t`);
    const row = rows().find(r => r.textContent?.includes(TITLE));
    expect(row, code).toBeTruthy();
    expect(row!.querySelector(".trunc")?.textContent).toBe(TITLE);
    expect(row!.querySelector(".sig")?.textContent?.trim().length).toBeGreaterThan(0);
    expect(row!.textContent).not.toMatch(CODE);
    expect(row!.getAttribute("aria-label")).not.toMatch(CODE);
    expect(document.body.textContent).not.toMatch(CODE);
    dispose?.(); document.body.innerHTML = "";
  }
});

it("a queue hold, a pause and a stale-result code never reach the board, task header or sprint strip raw", async () => {
  const hold = (kind: "settlement_blocked" | "continuation" | "owner_integration" | "start_refused", code: string, reason: string) =>
    sprint({ automaticQueue: { kind, taskId: "t", code, reason, needsOwner: kind !== "continuation" } });
  const cases: [Partial<Task>, Sprint][] = [
    [{ status: "review" }, hold("settlement_blocked", "settlement_blocked", "settlement_blocked")],
    [{ status: "paused" }, hold("continuation", "continuation_cap", "continuation_cap")],
    [{ status: "needs_you", issue: "external_blocker" }, hold("continuation", "external_blocker", "external_blocker")],
    [{ status: "review", workspaceDigest: "d0", result: { version: 1, digest: "d1", sourceVersion: "1", summary: "Done", checks: [{ name: "Tests", detail: "", outcome: "not_run" }], warnings: ["workspace_drift", "task_files_changed"] } }, hold("settlement_blocked", "stale_result", "stale_result")],
    [{ status: "ready_to_merge" }, hold("owner_integration", "owner_accepted", "owner_integration")],
    [{ status: "paused", automaticContinuation: { policy: "automatic_continuation", continuationCount: 2, lastContinuationReason: null, pause: { eventId: "e", kind: "continuation_cap", reason: "queue_blocked" } } }, sprint({ pauseReason: "queue_blocked" })],
  ];
  for (const [task, sp] of cases) for (const path of [`/m/${LOCAL_MACHINE_ID}/t/t`, `/m/${LOCAL_MACHINE_ID}/p/p`, `/m/${LOCAL_MACHINE_ID}/p/p/s/sp/summary`]) {
    await show(world(task, sp), path);
    const label = JSON.stringify([task.status, sp.automaticQueue?.code, path]);
    expect(document.body.textContent, label).not.toMatch(CODE);
    expect(document.body.textContent, label).toContain(TITLE);
    dispose?.(); document.body.innerHTML = "";
  }
});
