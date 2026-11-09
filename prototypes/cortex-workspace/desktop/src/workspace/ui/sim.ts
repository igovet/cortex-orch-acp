import { createSignal } from "solid-js";
import { mockPlan, PLAN_OPENING, PLAN_QUESTION } from "../adapter/plan-fixture";
import { WORKSPACE_SCRIPT } from "../adapter/session-fixture";
import type { Sprint } from "../domain";
import type { DemoBackend } from "./demo-backend";

export type MicFault = "denied" | "no_provider" | null;
export type PairFault = "expired" | "connector" | "fingerprint" | null;
export type SaveFault = "conflict" | "timeout" | null;
/** Owner controls for the simulation. They change what the mock does next and never reach an engine. */
export function createMockControls(autoplayByDefault: boolean) {
  const [autoplay, setAutoplay] = createSignal(autoplayByDefault);
  const [unconfirmedStop, setUnconfirmedStop] = createSignal(false);
  const [titleFails, setTitleFails] = createSignal(false);
  const [micFault, setMicFault] = createSignal<MicFault>(null);
  const [resolveFails, setResolveFails] = createSignal(false);
  const [pairFault, setPairFault] = createSignal<PairFault>(null);
  const [saveFault, setSaveFault] = createSignal<SaveFault>(null);
  return { autoplay, setAutoplay, unconfirmedStop, setUnconfirmedStop, titleFails, setTitleFails, micFault, setMicFault, resolveFails, setResolveFails, pairFault, setPairFault, saveFault, setSaveFault };
}
export type MockControls = ReturnType<typeof createMockControls>;

const MOCK_CHECK = { name: "Mock check", detail: "Simulation, no command ran" };
/** One simulated step at a time: a message moves on, a stop settles, or the running worker reports something. */
export function createSimulator(store: DemoBackend, mock: MockControls) {
  const played = new Map<string, number>();
  const step = (taskId: string): boolean => {
    const state = store.state(), now = store.clock();
    const task = state.tasks.find(t => t.id === taskId);
    const pending = task?.result?.checks.findIndex(c => c.running) ?? -1;
    if (task?.result && pending >= 0 && ["review", "ready_to_merge"].includes(task.status) && task.conflict === "none") {
      return store.dispatch({ type: "finish_check", taskId, digest: task.result.digest, checkIndex: pending, outcome: "passed", detail: "Simulated rerun passed; no command ran" }, { quiet: true }).ok;
    }
    // A conflict handed to Cortex runs its own short script, and it ends in Ready to merge or a failure, never in a merge.
    if (task?.conflict === "resolving") {
      if ((task.merge?.step ?? 2) < 2) return store.dispatch({ type: "advance_resolution", taskId, now }, { quiet: true }).ok;
      const failed = mock.resolveFails();
      mock.setResolveFails(false);
      return store.dispatch({ type: "finish_resolution", taskId, failed, now }, { quiet: true }).ok;
    }
    const running = state.workers.filter(w => w.taskId === taskId && w.status === "working");
    const live = new Set(running.map(w => w.id));
    const direct = state.messages.filter(m => m.taskId === taskId && m.workerId && live.has(m.workerId));
    const sending = direct.find(m => m.delivery === "sending");
    if (sending) return store.dispatch({ type: "simulate_delivery", messageId: sending.id, delivery: "recorded" }, { quiet: true }).ok;
    const waiting = direct.find(m => m.delivery === "recorded");
    if (waiting) return store.dispatch({ type: "simulate_delivery", messageId: waiting.id, delivery: "delivered", now }, { quiet: true }).ok;
    for (const worker of running) {
      const stopping = worker.activity.find(a => a.command?.state === "stopping");
      if (stopping) return store.dispatch({ type: "settle_command", workerId: worker.id, sequence: stopping.sequence, state: "stopped", exit: null }, { quiet: true }).ok;
      const script = WORKSPACE_SCRIPT[worker.id] ?? [], at = played.get(worker.id) ?? 0;
      if (at < script.length) {
        played.set(worker.id, at + 1);
        return store.dispatch({ type: "worker_activity", workerId: worker.id, text: script[at].text, entry: script[at].entry, now }, { quiet: true }).ok;
      }
    }
    return false;
  };
  /** The planner: an opening note, one question, then a plan. Asking for changes earns a new version. */
  const planStep = (sprint: Sprint): boolean => {
    if (sprint.planning !== "planning") return false;
    const now = store.clock(), said = (kind: string) => sprint.conversation.some(e => e.kind === kind);
    if (!sprint.plan && !said("note")) return store.dispatch({ type: "plan_note", sprintId: sprint.id, text: PLAN_OPENING, now }, { quiet: true }).ok;
    if (!sprint.plan && !said("question")) return store.dispatch({ type: "plan_question", sprintId: sprint.id, question: PLAN_QUESTION.text, options: PLAN_QUESTION.options, now }, { quiet: true }).ok;
    const items = sprint.plan?.items ?? mockPlan(sprint);
    const done = store.dispatch({ type: "publish_plan", sprintId: sprint.id, items, now }, { quiet: true }).ok;
    if (done) store.dispatch({ type: "plan_note", sprintId: sprint.id, text: `Plan v${(sprint.plan?.version ?? 0) + 1} is ready. Check the tasks and their order, then create them.`, now }, { quiet: true });
    return done;
  };
  /** The active task of a sprint finishes with a passing mock result. Automatic mode moves it to Ready to merge, never merging it. */
  const sprintStep = (sprintId: string): boolean => {
    const state = store.state(), sprint = state.sprints.find(s => s.id === sprintId);
    if (!sprint) return false;
    if (planStep(sprint)) return true;
    const task = sprint.taskIds.map(id => state.tasks.find(t => t.id === id)).find(t => t?.status === "working");
    if (!task) return false;
    return store.dispatch({ type: "publish_result", taskId: task.id, digest: `mock-run-${state.nextId}`, checks: [{ ...MOCK_CHECK, outcome: "passed" }], summary: "Mock result; no command ran" }, { quiet: true }).ok;
  };
  /** What plays while the owner is elsewhere: planners answer, and a running queue moves on every few ticks. */
  const ticks = new Map<string, number>();
  const background = () => {
    for (const task of store.state().tasks) if (task.result?.checks.some(c => c.running)) step(task.id);
    for (const sprint of store.state().sprints) {
      if (planStep(sprint)) continue;
      if (sprint.queueState !== "running") continue;
      const n = (ticks.get(sprint.id) ?? 0) + 1;
      ticks.set(sprint.id, n);
      if (n % 3 === 0) sprintStep(sprint.id);
    }
  };
  return { step, planStep, sprintStep, background, reset: () => { played.clear(); ticks.clear(); } };
}
export type Simulator = ReturnType<typeof createSimulator>;
