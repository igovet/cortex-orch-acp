import { describe, expect, it } from "vitest";
import type { Check as WireCheck, Decision as WireDecision, Result as WireResult, Task as WireTask } from "@cortex/workspace-contract";
import { readableCheckReason, mapCheckOutcome, mapDecision, mapProjectSlice, mapResult, mapTask, statusOfRuntime } from "../src/mapping";
import { boardPhase } from "../src/projections";
import { emptyWorkspace, LOCAL_MACHINE_ID } from "../src/empty";

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
