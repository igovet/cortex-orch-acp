import { render } from "solid-js/web";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { emptySession, emptyWorkspace, mapProjectSlice, type AutomaticContinuation, type AutomaticQueue, type PortCapabilities, type QueryResults, type Worker, type WorkspaceState } from "@cortex/workspace-domain";
import { Workspace } from "../src";
import { attemptsSpentOn, buildIndex, refusalLine, refusalReason, reviewDeadEnd, sideWord, stopReason } from "../src/selectors";
import { attemptsByNode, sessionModel, stateChip, workerState } from "../src/session-model";
import { GENERIC_STATUS, inTask, plainReason } from "../src/status-text";
import { flush, projection, scriptedPort, wireDecision, wireTask, withTask } from "./support";

// What the owner reads while orchestration misbehaves, from the 2026-11-20 audit of real stores: a replan waiting for a
// cancelled worker, refused worker results, a review that cannot be completed, unknown reason codes, failed attempts and the
// header's way to the pinned decision.
let dispose: (() => void) | undefined;
beforeEach(() => { localStorage.clear(); sessionStorage.clear(); });
afterEach(() => { dispose?.(); document.body.innerHTML = ""; });
const settle = async () => { for (let i = 0; i < 8; i++) await flush(); };

const worker = (patch: Partial<Worker> = {}): Worker => ({ id: "w", taskId: "t", executionId: "e", role: "qa-engineer", status: "completed", lane: "execution", goal: "Check it",
  now: null, waitsFor: null, scope: [], route: null, attempt: null, elapsedMs: null, runningSince: null, context: null, usage: null, activity: [], result: null, ...patch });
const sprint = (automaticQueue?: AutomaticQueue) => ({ id: "sp", projectId: "p", title: "Release", description: "Release", runMode: "automatic", targetBranch: null, planning: "created",
  conversation: [], plan: null, materializedVersion: 1, taskIds: ["t"], queueState: automaticQueue ? "paused" : "running", pauseReason: null, ...(automaticQueue ? { automaticQueue } : {}) }) as never;
type Wire = Parameters<typeof wireTask>[0];
function world(wire: Wire, options: { automatic?: boolean; workers?: Worker[]; hold?: AutomaticQueue; decision?: boolean } = {}): WorkspaceState {
  const s = withTask(emptyWorkspace(), "Release fix");
  s.machines[0].connection = "connected";
  const slice = mapProjectSlice(s.projects[0], [wireTask({ taskId: "t", description: "Release fix", title: "Release fix", titleAvailability: "generated", executionId: "e", ...wire })],
    options.decision ? [wireDecision({ executionId: "e", prompt: "Which branch?" })] : []);
  const out: WorkspaceState = { ...s, tasks: slice.tasks, sessions: slice.sessions, workers: options.workers ?? [] };
  if (options.automatic || options.hold) {
    out.sprints = [sprint(options.hold)];
    out.tasks = out.tasks.map(t => ({ ...t, sprintId: "sp" }));
  }
  return out;
}
const view = (s: WorkspaceState) => buildIndex(s, 0).views[0];
const model = (s: WorkspaceState) => sessionModel(buildIndex(s, 0), "t")!;
async function mount(state: WorkspaceState, path = "/m/local/t/t") {
  const capabilities: PortCapabilities = { everything: true, operations: [] };
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
const button = (scope: ParentNode, text: string) => [...scope.querySelectorAll<HTMLElement>("button, a")].find(b => b.textContent?.trim() === text);

const replan: Wire = { phase: "recovery", runtimeState: "RECOVERING", executionState: "recovering", waitReason: "cancelled_worker_stopping:qa-engineer",
  warnings: ["process_exit_pending", "cancelled_worker_stopping"] };

describe("a replan waiting for a cancelled worker to stop", () => {
  it("names the worker and the reason, never Working, and is not a recovery that waits for the owner", () => {
    const s = world(replan, { automatic: true, workers: [worker({ status: "cancelled", stopping: true, endedReason: "cancelled_by_coordinator" })] });
    const v = view(s);
    expect(v.task.status).not.toBe("needs_you");
    expect(v.unknown).toBe(true);
    expect(v.fact.text).toBe("Waiting for qa-engineer to stop");
    expect(sideWord(v)).toBe("Changing plan");
    expect(stateChip(model(s)).text).toBe("Changing the plan");
    expect(workerState(model(s).workers[0]).label).toBe("Stopping…");
    expect(stopReason("cancelled_worker_stopping:qa-engineer,backend-dev")).toBe("Waiting for qa-engineer and backend-dev to stop. The coordinator cancelled them to change the plan");
  });
  it("another recovery without a reason in an Automatic sprint reads as not confirmed, not as Working", () => {
    const v = view(world({ ...replan, waitReason: null, warnings: ["process_exit_pending"] }, { automatic: true }));
    expect(sideWord(v)).toBe("Not confirmed");
  });
  it("the conversation says who it waits for and why", async () => {
    await mount(world(replan, { workers: [worker({ status: "cancelled", stopping: true })] }));
    expect(document.querySelector(".ws-hold-quiet")?.textContent).toContain("Waiting for qa-engineer to stop. The coordinator cancelled it to change the plan.");
    expect(document.body.textContent).not.toContain("Working…");
  });
});

describe("refused worker results", () => {
  it.each([false,true])("separates recorded partial work and current actors from refusal history (historical %s)", async earlier => {
    const refused = worker({id:"w1",status:"failed",superseded:earlier || undefined,
      result:{summary:"Updated the guide",objectiveStatus:"partial", remaining:["Run the integration check"],blockerDetail:"Missing fixture",
        outcome:"failed",checks:[{name:"Lint",outcome:"passed",detail:""},{name:"Integration",outcome:"not_run",detail:"Missing fixture"}]},
      resultRefusals:{count:3,code:"open_failed_check",codes:["check_blocker_required","open_failed_check"],missing:["Exact check receipt"]}});
    await mount(world({phase:"execution",runtimeState:"EXECUTING",executionState:"running"},
      {workers:[refused,worker({id:"w2",role:"implementer",status:"working"})]}),"/m/local/t/t/workers/w1");
    const card = document.querySelector<HTMLElement>(".ws-worker-result")!;
    expect(card.textContent).toContain(earlier ? "Earlier attempt · Partially delivered" : "Partially delivered");
    expect(card.textContent).toContain("Run the integration check");
    expect(card.textContent).toContain("Checks · 1 passed · 1 not run");
    expect(card.textContent).toContain("Working on this execution: implementer");
    const history = card.querySelector("details")!;
    expect(history.open).toBe(false);
    expect(history.querySelector("summary")?.textContent).toContain("3 refusals");
    history.querySelector("summary")!.click();
    expect(history.open).toBe(true);
    expect(history.textContent).toContain("Exact check receipt");
  });
  it("keeps old refusals without a result or a confirmed actor factual", async () => {
    const refused = worker({id:"old",status:"failed",resultRefusals:{count:1,code:"omitted_failed_check"}});
    await mount(world({phase:"execution",runtimeState:"EXECUTING",executionState:"running"}, {workers:[
      refused,worker({id:"old-run",executionId:"old-execution",role:"old actor",status:"working"}),
      worker({id:"stale",role:"stale actor",status:"working",stale:true})]}),"/m/local/t/t/workers/old");
    const card = document.querySelector(".ws-worker-result")!;
    expect(card.textContent).toContain("Attempt ended without an accepted result");
    expect(card.textContent).toContain("No checks reported.");
    expect(card.textContent).toContain("No worker is confirmed working");
    expect(card.textContent).not.toMatch(/Partially delivered|Working on this execution|Waiting for you/);
  });
  it.each([false,true])("shows an observed pause only while connected and lists missing details once (offline %s)", async offline => {
    const w = worker({id:"old",status:"failed",resultRefusals:{count:1,code:"review_file_coverage_required",missing:["settings.ts (current complete read is missing)"]}});
    const s = world({phase:"paused",runtimeState:"PAUSED",executionState:"paused"}, {workers:[w]});
    const port = await mount(s,"/m/local/t/t/workers/old");
    if (offline) { port.emit({kind:"connection",connection:{state:"disconnected",at:null,reason:null}}); await settle(); }
    const card = document.querySelector(".ws-worker-result")!;
    expect(card.textContent).toContain(offline ? "No worker is confirmed working" : "Task paused.");
    expect(card.textContent?.match(/settings\.ts/g)).toHaveLength(1);
  });
  it.each(["decision","planning"])("shows the confirmed task state when there is no current worker (%s)", async phase => {
    const w = worker({id:"old",status:"failed",resultRefusals:{count:1,code:"omitted_failed_check"}});
    const s = world({phase:"planning",runtimeState:"PLANNING",executionState:"running"}, {workers:[w],decision:phase === "decision"});
    if (!s.sessions.length) s.sessions.push(emptySession("t"));
    s.sessions[0].plan = {revision:2,note:"",nodes:[]};
    await mount(s,"/m/local/t/t/workers/old");
    expect(document.querySelector(".ws-worker-result")?.textContent).toContain(phase === "decision" ? "Waiting for your answer." : "The coordinator is revising the plan.");
  });
  it("every refusal code the audit found reads in words, never generically", () => {
    for (const code of ["review_file_coverage_required", "command_not_settled", "review_predecessor_reports_required", "review_project_revision_changed",
      "failed_check_repair_required", "review_project_revision_unstable", "review_coverage_unfinished", "assignment_blocker_required", "report_sections_missing",
      "open_failed_check", "omitted_failed_check", "check_blocker_required"]) expect(refusalReason(code), code).not.toBe("the runtime refused it");
    expect(refusalReason("review_file_coverage_required", ["README.md (current complete read is missing or stale)"])).toBe("it had not read README.md in full");
    expect(refusalReason("report_sections_missing", ["Findings", "Checks"])).toBe("its report lacked the sections Findings and Checks");
  });
  it("a worker accepted after being sent back reads neutrally; one still refused reads as the problem with its earlier reasons", () => {
    const refusals = { count: 7, code: "review_file_coverage_required", codes: ["review_project_revision_changed", "review_file_coverage_required"], missing: ["README.md (current complete read is missing or stale)"] };
    expect(refusalLine(refusals, true)).toEqual({ tone: "muted", text: "Sent back 7 times before it was accepted (most recently: it had not read README.md in full). Nothing is needed from you." });
    expect(refusalLine(refusals, false)).toEqual({ tone: "st-bad", text: "Its result was refused 7 times: it had not read README.md in full. Earlier: the files changed while it reviewed." });
  });
});

describe("a review that cannot be completed", () => {
  const continuation = (patch: Partial<AutomaticContinuation>): AutomaticContinuation => ({ policy: "automatic_continuation", continuationCount: 9, lastContinuationReason: "review_incomplete", pause: null, ...patch });
  it("names the unreadable item and what the owner can do, and the attempts say what they went to", () => {
    const task = { automaticContinuation: continuation({ reviewDeadEnd: { item: "docs/local-task-execution.md", count: 2 } }), status: "working" as const, observedPhase: "review", archive: null };
    expect(reviewDeadEnd(task)).toBe("The reviewer cannot read docs/local-task-execution.md with its tools, so this review cannot be completed. Close as is, or change the file so it can be read.");
    expect(reviewDeadEnd({ ...task, automaticContinuation: continuation({ reviewDeadEnd: { item: "change_set", count: 3 } }) })).toContain("cannot read the full list of changed files");
    expect(reviewDeadEnd({ ...task, observedPhase: "verification" })).toBeNull();
    expect(attemptsSpentOn({ automaticContinuation: continuation({ spentOn: [{ reason: "review_incomplete", count: 7 }, { reason: "native_session_lost", count: 1 }] }) }))
      .toBe("The attempts went to: the review is incomplete (7 times); the engine session was lost (once).");
  });
  it("pins a card with Close as is, which the header brings into view", async () => {
    await mount(world({ phase: "review", runtimeState: "REVIEWING", executionState: "running",
      automaticContinuation: continuation({ reviewDeadEnd: { item: "README.md", count: 2 } }) }, { automatic: true }));
    const card = document.querySelector<HTMLElement>(".chat-review-hold")!;
    expect(card?.textContent).toContain("The reviewer cannot read README.md with its tools");
    expect(button(card, "Close as is…")).toBeTruthy();
    expect(button(document, "See what is needed")).toBeTruthy();
  });
});

describe("reason codes in words", () => {
  it("each code the audit found has its own sentence", () => {
    for (const code of ["native_answer_delivery_unconfirmed", "verification_failed", "verification_review_revision_changed", "project_revision_unavailable",
      "native_guidance_delivery_unknown", "unsafe_storage", "completion_unresolved_worker_check", "completion_dropped_failed_check", "completion_worker_check_unverified"]) {
      expect(stopReason(code), code).not.toBe(GENERIC_STATUS);
      expect(stopReason(code), code).not.toBe("The work could not be confirmed as complete");
    }
    expect(stopReason("lost_worker_action_outcome_unknown")).toBe("Whether an action of a lost worker finished is not known");
    expect(stopReason("coordinator_prompt_failed:RuntimeError")).toBe("The coordinator's engine failed while it was working");
    expect(plainReason("Coordinator_Prompt_Failed:runtimeerror")).toBe("The coordinator's engine failed while it was working");
    expect(plainReason("some_new_code:Detail")).toBe(GENERIC_STATUS);
    expect(inTask(GENERIC_STATUS)).toBe("This step stopped for a reason the app cannot describe");
  });
});

describe("failed attempts", () => {
  it("a named dead end, a replaced attempt and a stopping worker read as what they are", () => {
    expect(workerState(worker({ status: "failed", blockerKind: "other_role_work" })).label).toBe("Found problems");
    expect(workerState(worker({ status: "failed", blockerKind: "tool_unavailable" })).label).toBe("Could not run here");
    expect(workerState(worker({ status: "failed", superseded: true, replacedIn: 4 })).label).toBe("Replaced in plan v4");
    expect(workerState(worker({ status: "failed" })).label).toBe("Failed");
  });
  it("attempts of one plan step are grouped under the newest", () => {
    const groups = attemptsByNode([worker({ id: "c", node: "qa" }), worker({ id: "b", node: "docs" }), worker({ id: "a", node: "qa" }), worker({ id: "x" })]);
    expect(groups.map(g => [g.latest.id, g.earlier.map(w => w.id)])).toEqual([["c", ["a"]], ["b", []], ["x", []]]);
  });
  it("the Workers tab folds earlier attempts of a step under its newest", async () => {
    await mount(world({ phase: "execution", runtimeState: "EXECUTING", executionState: "running" }, {
      workers: [worker({ id: "w1", node: "qa", status: "failed", superseded: true, replacedIn: 2 }), worker({ id: "w2", node: "qa", status: "failed", blockerKind: "tool_unavailable" })] }), "/m/local/t/t/workers");
    const list = document.querySelector<HTMLElement>('[role="tablist"][aria-label="Workers"].wlist')!;
    const toggle = button(list, "Show 1 earlier attempt of this step")!;
    const attempt = (id: string) => list.querySelector<HTMLAnchorElement>(`[role="tab"][href$="/workers/${id}"]`);
    expect(toggle.getAttribute("aria-expanded")).toBe("false");
    expect(attempt("w2")?.textContent).toContain("Could not run here");
    expect(attempt("w1")).toBeNull();
    toggle.click(); await settle();
    expect(toggle.getAttribute("aria-expanded")).toBe("true");
    expect(attempt("w1")?.textContent).toContain("Replaced in plan v2");
    expect(attempt("w2")?.textContent).toContain("Could not run here");
    toggle.click(); await settle();
    expect(attempt("w1")).toBeNull();
    expect(attempt("w2")).toBeTruthy();
  });
});

describe("See what is needed", () => {
  const hold: AutomaticQueue = { kind: "settlement_blocked", taskId: "t", code: "task_failed", reason: "The last run of this task ended as failed.", needsOwner: true,
    nextActor: "owner", ownerActions: ["allow_repair", "close_as_is"], asks: ["Let the agent repair it"], holdId: "h" } as AutomaticQueue;
  it("is offered with the pinned card it scrolls to", async () => {
    await mount(world({ phase: "failed", runtimeState: "FAILED", executionState: "failed" }, { hold }));
    expect(document.querySelector(".chat-review-hold")).toBeTruthy();
    expect(button(document, "See what is needed")).toBeTruthy();
  });
  it("is not offered while an open question hides the card and Answer already leads there", async () => {
    await mount(world({ phase: "waiting", runtimeState: "WAITING_USER", executionState: "waiting_user" }, { hold, decision: true }));
    expect(document.querySelector(".chat-review-hold")).toBeNull();
    expect(button(document, "Answer")).toBeTruthy();
    expect(button(document, "See what is needed")).toBeUndefined();
  });
});
