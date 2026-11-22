import { describe, expect, it } from "vitest";
import type { Check as WireCheck, Decision as WireDecision, Result as WireResult, Task as WireTask } from "@cortex/workspace-contract";
import { readableCheckReason, mapCheckOutcome, mapDecision, mapProjectSlice, mapResult, mapTask, statusOfRuntime } from "../src/mapping";
import { overlayTask } from "../src/port";
import { dependencyReady, isAutomaticQueue, runBlockerOf } from "../src/invariants";
import type { Sprint } from "../src/types";
import { boardPhase } from "../src/projections";
import { emptyWorkspace, LOCAL_MACHINE_ID } from "../src/empty";

it("distinguishes recheckable Automatic holds from holds with a task action", () => {
  const state = emptyWorkspace();
  const sprint: Sprint = {id:"s",projectId:"p",description:"Sprint",runMode:"automatic",targetBranch:null,
    planning:"created",conversation:[],plan:null,materializedVersion:null,taskIds:["t"],queueState:"paused",pauseReason:null};
  sprint.automaticQueue = {kind:"settlement_blocked",taskId:"t",code:"dependencies_blocked",reason:"Check dependencies",needsOwner:true};
  expect(isAutomaticQueue(sprint.automaticQueue)).toBe(true);
  expect(runBlockerOf(state, sprint)).toBeNull();
  sprint.automaticQueue = {kind:"continuation",taskId:"t",code:"continuation_cap",reason:"Resume the task",needsOwner:true};
  expect(runBlockerOf(state, sprint)).toEqual({reason:"Resume the task",taskId:"t"});
  sprint.automaticQueue = {kind:"settlement_blocked",taskId:"t",code:"outcome_unknown",reason:"Inspect the outcome",needsOwner:true};
  expect(runBlockerOf(state, sprint)).toEqual({reason:"Inspect the outcome",taskId:"t"});
});

const wireTask = (patch: Partial<WireTask> = {}): WireTask => ({
  taskId: "t", description: "Fix the login race\nSecond line", title: null, titleAvailability: "not_connected", version: "3", executionId: "e",
  phase: "execution", runtimeState: "EXECUTING", executionState: "running", waitReason: null, lastConfirmedAt: "2026-11-05T00:00:00Z", warnings: [], ...patch,
});
const wireResult = (patch: Partial<WireResult> = {}): WireResult => ({
  resultId: "r", version: "47", sequenceNumber:"2", taskId: "t", executionId: "e", kind: "work", text: "Done the work", evidence: [], checks: [], observedWorkspaceDigest: null, ...patch,
});
const wireDecision = (patch: Partial<WireDecision> = {}): WireDecision => ({
  decisionId: "d", kind: "question", questionId: "q", batchId: null, executionId: "e", acpSessionId: "s", scope: "worker", prompt: "Which?",
  options: [{ optionId: "o", label: "A" }], version: "1", ownerEpoch: "1", requirementRevision: "2", status: "pending", allowedOperations: ["decisions.answer"], ...patch,
});
const ctx = { projectId: "p" };

describe("task mapping", () => {
  it("keeps a policy-bound continuation and its recorded pauses out of the owner-wait state", () => {
    const recovery = wireTask({ phase:"recovery", runtimeState:"RECOVERING", executionState:"recovering", waitReason:"automatic_continuation_pending" });
    const automaticContinuation = { policy:"automatic_continuation", continuationCount:2, lastContinuationReason:"Verification did not pass", pause:null };
    expect(mapTask({...recovery, automaticContinuation}, ctx)).toMatchObject({ status:"working", automaticContinuation });
    for (const kind of ["external_blocker", "continuation_cap", "outcome_unknown", "obstacle", "infrastructure"]) {
      const pause = { eventId:"evt", kind, reason:"Recorded pause" };
      expect(mapTask({...recovery, phase:"paused", executionState:"paused", waitReason:pause.reason,
        automaticContinuation:{...automaticContinuation, pause}}, ctx))
        .toMatchObject({ status:"paused", process:"stopped", issue:pause.reason, automaticContinuation:{ pause } });
    }
    expect(mapTask({...recovery, automaticContinuation:{...automaticContinuation, continuationCount:-1}}, ctx).automaticContinuation).toBeUndefined();
    expect(mapTask(recovery, ctx).status).toBe("needs_you");
  });
  it("keeps automatic acceptance separate from a recorded automatic finish", () => {
    const native = mapTask(wireTask({phase:"result", runtimeState:"COMPLETED", executionState:"finished"}), ctx, wireResult({observedWorkspaceDigest:"digest"}));
    const acceptance = { automatic:true, version:47, digest:"digest" };
    const accepted = overlayTask(native, {status:"ready_to_merge", acceptance}, "dev");
    expect(accepted).toMatchObject({status:"ready_to_merge", acceptance:{automatic:true,version:2}});
    expect(dependencyReady(accepted, "automatic")).toBe(false);
    expect(dependencyReady(accepted, "hybrid")).toBe(true);
    const integration = { method:"commit" as const, targetBranch:"dev", digest:"digest", automatic:true, committed:false };
    const done = overlayTask(native, {status:"done", acceptance, integration}, "dev");
    expect(done).toMatchObject({status:"done", process:"stopped", integration});
    expect(dependencyReady(done, "automatic")).toBe(true);
  });
  it("shows an explicit native owner resolution as done without certifying the work", () => {
    const resolved = wireTask({phase:"stopped",runtimeState:"CANCELLED",executionState:"stopped",warnings:["owner_resolved_without_certification"]});
    expect(mapTask(resolved,ctx)).toMatchObject({status:"done",closedAs:"as_is",notVerifiedByCortex:true,acceptance:null,result:null});
    expect(mapTask({...resolved,warnings:[...resolved.warnings,"contradictory_evidence"]},ctx).status).not.toBe("done");
    expect(mapTask({...resolved,warnings:[]},ctx).status).toBe("cancelled");
  });
  it("does not certify an unconfirmed close or let stale owner facts supersede it", () => {
    const native = mapTask(wireTask({phase:"unknown",runtimeState:null,executionState:"unknown",
      warnings:["contradictory_evidence","owner_closed_unconfirmed"]}), ctx);
    const task = overlayTask(native, {status:"ready_to_merge", acceptance:{automatic:true,version:2,digest:"old"}}, "dev");
    expect(task).toMatchObject({status:"closed_unverified",closedAs:"unconfirmed",process:"unknown",notVerifiedByCortex:true,acceptance:null});
    expect(dependencyReady(task, "automatic")).toBe(false);
    expect(dependencyReady(task, "hybrid")).toBe(false);
    expect(boardPhase({...emptyWorkspace(), tasks:[task]}, task.id)).toBe("closed_unverified");
  });
  it("carries the observed coordinator route separately from editable settings", () => {
    const route = { model: "Observed Model", effort: "High" };
    expect(mapTask(wireTask({ coordinatorRoute: route }), ctx).coordinatorRoute).toEqual(route);
    expect(mapTask(wireTask(), ctx).coordinatorRoute).toBeNull();
    expect(mapTask(wireTask({ coordinatorRoute: route }), ctx).settings.model).toBe("");
  });
  it.each([
    ["not_started", null, "draft"], ["planning", "PLANNING", "working"], ["execution", "EXECUTING", "working"], ["review", "REVIEWING", "working"],
    ["verification", "VERIFYING", "working"], ["finalizing", "RECONCILING", "working"], ["waiting", "WAITING_USER", "needs_you"], ["blocked", "BLOCKED", "needs_you"],
    ["failed", "FAILED", "failed"], ["stopped", "CANCELLED", "cancelled"], ["result", "COMPLETED", "review"],
  ] as const)("maps phase %s (%s) to %s", (phase, runtimeState, status) => {
    expect(mapTask(wireTask({ phase, runtimeState, executionId: phase === "not_started" ? null : "e" }), ctx).status).toBe(status);
  });

  it("keeps the wait reason apart from the status", () => {
    const task = mapTask(wireTask({ phase: "waiting", runtimeState: "WAITING_USER", waitReason: "Which schema?" }), ctx);
    expect(task.status).toBe("needs_you");
    expect(task.issue).toBe("Which schema?");
  });

  it("a missing title stays a pending title, never a generated one", () => {
    const task = mapTask(wireTask({ title: null }), ctx);
    expect(task.titleState).toBe("pending");
    expect(task.title).toBe("Fix the login race");
    expect(mapTask(wireTask({ title: "Login", titleAvailability: "available" }), ctx).titleState).toBe("generated");
  });

  it("FAILED and CANCELLED are never done", () => {
    for (const [phase, runtimeState] of [["failed", "FAILED"], ["stopped", "CANCELLED"]] as const) {
      const task = mapTask(wireTask({ phase, runtimeState }), ctx);
      expect(task.status).not.toBe("done");
      expect(task.doneAt).toBeNull();
      expect(task.closedAs).toBeNull();
    }
    expect(statusOfRuntime("FAILED", "connected")).toBe("failed");
    expect(statusOfRuntime("CANCELLED", "connected")).toBe("cancelled");
  });

  it("COMPLETED is a runtime result, never accepted, merged or ready to merge", () => {
    const task = mapTask(wireTask({ phase: "result", runtimeState: "COMPLETED", executionState: "result" }), ctx, wireResult());
    expect(task.status).toBe("review");
    expect(task.acceptance).toBeNull();
    expect(task.integration).toBeNull();
    expect(task.work).toBe("unmerged");
    expect(statusOfRuntime("COMPLETED", "connected")).toBe("review");
  });

  it("a contradiction between phase and runtime state stays unknown, whatever the phase claims", () => {
    for (const [phase, runtimeState] of [["result", "FAILED"], ["execution", "COMPLETED"], ["result", "CANCELLED"]] as const) {
      const task = mapTask(wireTask({ phase, runtimeState }), ctx);
      expect(task.process).toBe("unknown");
      expect(task.status).not.toBe("done");
      expect(task.status).not.toBe("review");
    }
  });

  it("an unrecognised phase is unknown, not a guess", () => {
    const task = mapTask(wireTask({ phase: "somewhere_new", runtimeState: null }), ctx);
    expect(task.process).toBe("unknown");
    const state = emptyWorkspace();
    state.projects.push({ id: "p", machineId: LOCAL_MACHINE_ID, name: "P", repository: "/p", openBranch: "", defaultBranch: "" });
    state.machines[0].connection = "connected";
    state.tasks.push(task);
    expect(boardPhase(state, "t")).toBe("unknown");
  });

  it("an interrupted turn is an owner-facing wait, not unknown process evidence", () => {
    const task = mapTask(wireTask({ phase: "recovery", runtimeState: "RECOVERING", executionState: "recovering", waitReason: "turn_interrupted" }), ctx);
    expect(task).toMatchObject({ status: "needs_you", process: "live", observedPhase: "recovery", issue: "turn_interrupted" });
    // Live means messaging is possible, not proof of physical quiescence.
    expect(task.process).not.toBe("stopped");
  });

  it.each([
    { phase: "unknown", runtimeState: "RECOVERING", executionState: "unknown", warnings: ["contradictory_evidence"] },
    { phase: "recovery", runtimeState: "EXECUTING", executionState: "recovering" },
    { phase: "recovery", runtimeState: "RECOVERING", executionState: "unknown" },
    { phase: "recovery", runtimeState: null, executionState: "recovering" },
    { phase: "recovery", runtimeState: "RECOVERING", executionState: "recovering", executionId: null },
    { phase: "recovery", runtimeState: "RECOVERING", executionState: "recovering", warnings: ["contradictory_evidence"] },
  ])("an interruption reason cannot override missing or contradictory evidence: %j", patch => {
    expect(mapTask(wireTask({ ...patch, waitReason: "turn_interrupted" }), ctx).process).toBe("unknown");
  });

  it("a recovery with a recorded native reason is an owner-facing wait with Stop and Close, not unknown", () => {
    // D7: after a native start failure the task sat in RECOVERING with Stop and Close disabled.
    const task = mapTask(wireTask({ phase: "recovery", runtimeState: "RECOVERING", executionState: "recovering", waitReason: "native_session_outcome_unknown" }), ctx);
    expect(task).toMatchObject({ status: "needs_you", process: "live", observedPhase: "recovery", issue: "native_session_outcome_unknown" });
    expect(mapTask(wireTask({ phase: "recovery", runtimeState: "RECOVERING", executionState: "recovering", waitReason: " " }), ctx).process).toBe("unknown");
  });

  it("an unconfirmed recovery is unknown until the runtime settles it", () => {
    expect(mapTask(wireTask({ phase: "recovery", runtimeState: "RECOVERING", executionState: "recovering" }), ctx).process).toBe("unknown");
  });

  it("without a connection nothing is claimed", () => {
    for (const runtime of ["COMPLETED", "FAILED", "EXECUTING", null]) expect(statusOfRuntime(runtime, "disconnected")).toBe("unknown");
    expect(statusOfRuntime("NO_SUCH_STATE", "connected")).toBe("unknown");
    expect(statusOfRuntime(null, "connected")).toBe("unknown");
  });
});

describe("result mapping", () => {
  it.each([
    ["Could not create worktree: git_configuration_protected", "Git settings"],
    ["command_run receipt completeness could not be established", "full result"],
  ])("makes mixed technical blockers readable: %s", (raw, expected) => {
    expect(readableCheckReason(raw)).toContain(expected);
    expect(readableCheckReason(raw)).not.toMatch(/git_configuration_protected|command_run|completeness/);
  });
  it("keeps its identity and text, and labels the check source as the runtime", () => {
    const result = mapResult(wireResult({ checks: [{ name: "unit", outcome: "passed", evidenceRef: "e1" }] }));
    expect(result.version).toBe(2);
    expect(result.summary).toBe("Done the work");
    expect(result.checks).toEqual([expect.objectContaining({ name: "unit", outcome: "passed", source: "runtime" })]);
  });

  it.each([["not_run", "not_run"], ["unverified", "unverified"], ["failed", "failed"], ["unknown", "unverified"], ["not_applicable", "not_run"], ["something_else", "unverified"]])("reads check outcome %s as %s, never a pass", (wire, view) => {
    expect(mapCheckOutcome(wire as WireCheck["outcome"])).toBe(view);
  });

  it("a result with no observed digest cannot bind to a digest it never saw", () => {
    expect(mapResult(wireResult({ observedWorkspaceDigest: null })).digest).toBe("result:r:47");
    expect(mapResult(wireResult({ observedWorkspaceDigest: "a".repeat(64) })).digest).toBe("a".repeat(64));
  });
});

describe("decision mapping", () => {
  it("carries the prompt and options; an answered decision is no longer open", () => {
    const open = mapDecision(wireDecision());
    expect(open).toMatchObject({ id: "d", kind: "question", text: "Which?", answer: null, withdrawn: false });
    expect(open.options).toEqual([{ label: "A", optionId: "o" }]);
    expect(mapDecision(wireDecision({ status: "superseded" })).withdrawn).toBe(true);
    expect(mapDecision(wireDecision({ status: "expired" })).withdrawn).toBe(true);
    expect(mapDecision(wireDecision({ status: "answered" })).answer).toEqual({ text: "Answered", at: null });
  });
  it("names the worker role that asked, and no one for the coordinator's question", () => {
    expect(mapDecision(wireDecision({ askedBy: "devops-engineer" })).from).toBe("devops-engineer");
    expect(mapDecision(wireDecision()).from).toBe("");
  });
  it("keeps a batch member's topic label, and adds none when the asker gave none", () => {
    expect(mapDecision(wireDecision({ topic: "Paid checks" })).topic).toBe("Paid checks");
    expect("topic" in mapDecision(wireDecision())).toBe(false);
  });
  it("never offers an answer to a decision it cannot place, or one the runtime does not allow", () => {
    expect(mapDecision(wireDecision({ status: "something_new" })).withdrawn).toBe(true);
    const suspended = mapDecision(wireDecision({ allowedOperations: [] }));
    expect(suspended.suspended).toBe(true);
    expect(suspended.withdrawn).toBe(false); // A paused pending question stays visible, never answerable.
  });
});

describe("project slice", () => {
  it("surfaces a worker-local question while the runtime continues executing", () => {
    const slice = mapProjectSlice({id:"p",machineId:LOCAL_MACHINE_ID,name:"P",repository:"/p",openBranch:"",defaultBranch:""}, [wireTask()], [wireDecision()]);
    expect(slice.tasks[0]).toMatchObject({status:"needs_you",issue:"Which?",observedPhase:"execution",process:"live"});
    const answered = mapProjectSlice(slice.project, [wireTask()], [wireDecision({status:"answered"})]);
    expect(answered.tasks[0].status).toBe("working");
    const suspended = mapProjectSlice(slice.project, [wireTask()], [wireDecision({allowedOperations:[]})]);
    expect(suspended.tasks[0].status).toBe("working");
  });

  it("maps a project's tasks, questions and results together", () => {
    const slice = mapProjectSlice({ id: "p", machineId: LOCAL_MACHINE_ID, name: "P", repository: "/p", openBranch: "", defaultBranch: "" }, [wireTask({ phase: "result", runtimeState: "COMPLETED" })], [wireDecision()], [wireResult()]);
    expect(slice.tasks).toHaveLength(1);
    expect(slice.tasks[0].result?.summary).toBe("Done the work");
    expect(slice.sessions[0].decisions).toHaveLength(1);
  });
});

describe("permission decisions", () => {
  it("keeps a native permission request's details and counts its task as needing the owner", () => {
    const permission = { operation: "write", paths: ["/srv/a", 7], reason: "outside_project", reasonText: "it writes outside the project", access: "development", rule: "outside_write" };
    const decision = wireDecision({ kind: "permission", prompt: "Owner decision: worker-backend-dev wants to write /srv/a", permission } as never);
    expect(mapDecision(decision)).toMatchObject({ kind: "permission", permission: { operation: "write", paths: ["/srv/a"], reason: "outside_project", access: "development" } });
    const slice = mapProjectSlice({id:"p",machineId:LOCAL_MACHINE_ID,name:"P",repository:"/p",openBranch:"",defaultBranch:""}, [wireTask()], [decision]);
    expect(slice.tasks[0]).toMatchObject({ status: "needs_you", issue: "Owner decision: worker-backend-dev wants to write /srv/a", process: "live" });
    expect(mapDecision(wireDecision()).kind).toBe("question");
  });
});

describe("check reasons carry no internal codes", () => {
  it.each(["check_preflight_unavailable", "task_files_changed", "workspace_drift", "stale_result", "some_unlisted_code", "Not run: check_preflight_unavailable"])("%s", raw => {
    expect(readableCheckReason(raw)).not.toMatch(/[a-z]+_[a-z]+/);
  });
});
