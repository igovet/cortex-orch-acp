import type { Decision as WireDecision, ProjectSnapshot, Task as WireTask } from "@cortex/workspace-contract";
import { LIMITS, PROTOCOL, SCHEMA_FINGERPRINT } from "@cortex/workspace-contract";
import { applyProjection, emptyWorkspace, type CommandEnvelope, type CommandReceipt, type PortCapabilities, type PortUpdate, type ProjectionUpdate, type QueryOperation, type QueryParameters, type QueryResults, type Task, type WorkspacePort, type WorkspaceState } from "@cortex/workspace-domain";
import type { RealTransport, TransportEvent } from "../src/port/real";

/** A port driven by hand, so a test decides exactly when a receipt or a projection arrives. */
export function scriptedPort(options: { mode?: "real" | "demo"; capabilities?: PortCapabilities; failQuery?: Error } = {}) {
  const listeners = new Set<(update: PortUpdate) => void>();
  const commands: CommandEnvelope[] = [];
  const receipts: ((receipt: CommandReceipt) => void)[] = [];
  const port: WorkspacePort = {
    mode: options.mode ?? "real",
    query<O extends QueryOperation>(operation: O, _parameters: QueryParameters[O]): Promise<QueryResults[O]> {
      if (options.failQuery) return Promise.reject(options.failQuery);
      const answer: QueryResults = { "capabilities.get": options.capabilities ?? { everything: false, operations: ["tasks.create", "tasks.start", "decisions.answer"] }, "project.snapshot": [], "project.facts": [], "machine.facts": [] };
      return Promise.resolve(answer[operation]);
    },
    command(envelope) {
      commands.push(envelope);
      return new Promise(resolve => receipts.push(resolve));
    },
    subscribe(listener) { listeners.add(listener); return () => listeners.delete(listener); },
  };
  return {
    port, commands,
    emit(update: PortUpdate) { for (const listener of listeners) listener(update); },
    receipt(ok = true, entityId?: string) { const r = receipts.shift()!; r({ requestId: commands.at(-1)!.requestId, ok, outcome: ok ? "recorded" : "unsupported", message: ok ? "Recorded" : "Refused", entityId }); },
    listeners,
  };
}

export function draftTask(): Task {
  return {
    id: "t", projectId: "p", sprintId: null, description: "Fix the race", title: "Fix the race", titleState: "generated", checkout: { kind: "main", branch: "main" }, targetBranch: null, dependencies: [],
    status: "draft", resumeStatus: "working", issue: null, executionId: null, result: null, workspaceDigest: null, acceptance: null, conflict: "none", merge: null, resolvedBy: null, integration: null,
    closedAs: null, notVerifiedByCortex: false, doneAt: null, archive: null, archiveQuestion: false, archiveAskAfter: null, work: "unmerged", process: "stopped", unexpectedChanges: false, purgeBlocker: null,
    settings: { model: "", effort: "", mode: "guarded", language: "auto" },
  };
}
export const withTask = (state: WorkspaceState, title: string): WorkspaceState => ({ ...state, projects: [{ id: "p", machineId: state.machines[0].id, name: "Project", repository: "/p", openBranch: "main", defaultBranch: "main" }], tasks: [{ ...draftTask(), title }] });
export const projection = (version: string, state: WorkspaceState): ProjectionUpdate => ({ kind: "projection", version, scope: "workspace", state });
export const initial = () => emptyWorkspace();
export { applyProjection };

export const capabilitiesWire = (operations: string[]) => ({
  protocol: PROTOCOL, buildIdentity: "1.0.6+test", schemaFingerprint: SCHEMA_FINGERPRINT, mappingVersion: "1", operations,
  limits: LIMITS,
  generation: "1", acpCapabilities: {},
});
export const wireTask = (patch: Partial<WireTask> = {}): WireTask => ({
  taskId: "task-1", description: "Fix the login race", title: null, titleAvailability: "not_connected", version: "1", executionId: null,
  phase: "not_started", runtimeState: null, executionState: "not_started", waitReason: null, lastConfirmedAt: null, warnings: [], ...patch,
});
export const wireDecision = (patch: Partial<WireDecision> = {}): WireDecision => ({
  decisionId: "d", kind: "question", questionId: "q", batchId: null, executionId: "e", acpSessionId: "s", scope: "worker", prompt: "Which?",
  options: [{ optionId: "o", label: "A" }], version: "2", ownerEpoch: "3", requirementRevision: "2", status: "pending", allowedOperations: ["decisions.answer"], ...patch,
});
export const snapshot = (version: string, tasks: WireTask[], projectId = "p"): ProjectSnapshot => ({ projectId, version, highWatermark: version, tasks: { items: tasks, nextCursor: null, highWatermark: version } });

/** The service, as the real port sees it. Every request is recorded; the test supplies each answer. */
export function scriptedTransport(answers: Partial<Record<string, (message: unknown) => unknown>> = {}) {
  const calls: { operation: string; message: unknown }[] = [];
  const listeners = new Set<(event: TransportEvent) => void>();
  const transport: RealTransport = {
    async request(operation, message) {
      calls.push({ operation, message });
      const answer = answers[operation];
      if (!answer) throw new Error(`No answer scripted for ${operation}`);
      return answer(message);
    },
    subscribe(listener) { listeners.add(listener); return () => listeners.delete(listener); },
  };
  return { transport, calls, answers, send(event: TransportEvent) { for (const listener of listeners) listener(event); } };
}
export const flush = () => new Promise(resolve => setTimeout(resolve, 0));
