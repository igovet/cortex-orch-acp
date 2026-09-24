import type { Pipeline } from "../shared/pipeline.js";
import { workerDisplayName, workerModelDisplay } from "./worker-display.ts";
import { workerPopoverStatus } from "./worker-popover-status.ts";
import { workerResultItems } from "./worker-result-model.ts";

export type StepStatus = "done" | "active" | "issues" | "failed" | "blocked" | "cancelled" | "pending";
export type Tone = "success" | "warning" | "danger" | "muted" | "accent";

export interface StepItem { text: string; status: "done" | "failed" | "open"; blocker?: string }

export interface PipelineStep {
  key: string;
  kind: "request" | "plan" | "worker" | "finish";
  status: StepStatus;
  title: string;
  subtitle?: string;
  badges: { text: string; tone: Tone }[];
  body?: string;
  items: StepItem[];
  followUps?: string[];
  assignmentRef?: string;
  usage?: Pick<Pipeline["workers"][number]["attempts"][number], "context" | "tokens">;
}

export interface PipelineView {
  steps: PipelineStep[];
  done: number;
  total: number;
  headline: { text: string; tone: Tone; live: boolean };
}

const TASK_STATE: Record<string, { text: string; tone: Tone; live: boolean }> = {
  running: { text: "In progress", tone: "accent", live: true },
  recovering: { text: "Recovering", tone: "warning", live: true },
  accepted: { text: "Completed", tone: "success", live: false },
  blocked: { text: "Blocked", tone: "danger", live: false },
  idle_awaiting_user: { text: "Waiting for you", tone: "warning", live: false },
  interrupted: { text: "Interrupted", tone: "warning", live: false },
  stopped: { text: "Stopped", tone: "muted", live: false },
};

function workerStep(worker: Pipeline["workers"][number], pipeline: Pipeline): PipelineStep {
  const ref = worker.assignmentRef ?? worker.id;
  const delegation = pipeline.delegations[ref];
  const status = workerPopoverStatus(worker);
  const route = worker.route ?? worker.attempts[worker.attempts.length - 1]?.route;
  const title = worker.title?.trim() && !/^delegate:[0-9a-f-]+$/i.test(worker.title) ? worker.title.trim() : workerDisplayName(worker);
  const badges: PipelineStep["badges"] = [];
  const attemptNo = worker.attempts[worker.attempts.length - 1]?.attemptNo ?? 1;
  if (attemptNo > 1) badges.push({ text: `Attempt ${attemptNo}`, tone: "muted" });
  const decision = delegation?.decision?.decision;
  if (decision === "accept") badges.push({ text: "Accepted", tone: "success" });
  else if (decision === "revise") badges.push({ text: "Sent back", tone: "warning" });
  else if (decision) badges.push({ text: decision, tone: "muted" });
  const failure = worker.attempts[worker.attempts.length - 1]?.failureReason;
  const checks = delegation?.result?.checks ?? [];
  const issues = checks.filter((check) => check.outcome !== "passed");
  // A durable worker_result payload is the strongest terminal signal there
  // is: the graph/runtime status string is a coarser projection that has
  // more values than this popover recognizes (e.g. a work-graph attempt
  // left in "needs_revision", or a runtime_nodes join that could not bind a
  // single row). A reported worker never regresses to the unstarted
  // numbered placeholder just because its status word is unrecognized.
  const reported = delegation?.result != null;
  // A "completed" status word is not itself a success signal: a reported
  // result with any non-passed check must classify as "issues" before this
  // falls through to "done", or the check gets hidden behind a green icon.
  const mappedStatus: StepStatus = status === "completed" ? (issues.length > 0 ? "issues" : "done")
    : status === "completed_with_issues" ? "issues" : status === "running" ? "active"
    : ["failed", "error"].includes(status) ? "failed" : ["cancelled", "canceled"].includes(status) ? "cancelled"
    : reported ? (issues.length > 0 ? "issues" : "done") : "pending";
  if (mappedStatus === "issues" && issues.length > 0) {
    badges.push({ text: `${issues.length} issue${issues.length === 1 ? "" : "s"}`, tone: "warning" });
  }
  return {
    key: `worker:${worker.id}`,
    kind: "worker",
    status: mappedStatus,
    title,
    subtitle: [workerDisplayName(worker), workerModelDisplay(route), route?.effort].filter(Boolean).join(" · "),
    badges,
    body: delegation?.result?.summary ?? (status === "failed" || status === "error" ? failure ?? undefined : undefined),
    items: workerResultItems(checks),
    assignmentRef: ref,
    usage: worker.attempts[worker.attempts.length - 1],
  };
}

const NODE_STATUS: Record<string, StepStatus> = {
  completed: "done", running: "active", pending: "pending", cancelled: "cancelled", failed: "failed",
};

/** A planned runtime node no worker has started for (yet, or ever). */
function nodeStep(node: NonNullable<Pipeline["plan"]>["nodes"][number]): PipelineStep {
  const firstLine = node.objective.split(/(?<=[.!?])\s|\n/, 1)[0].trim();
  const role = node.role.replace(/^worker-/i, "");
  const checks = node.result?.checks ?? [];
  const issues = checks.filter((check) => check.outcome !== "passed");
  // A retained result is this node's own terminal signal, independent of
  // whatever raw runtime status string reached the client: an unrecognized
  // status must never hide a reported result behind the unstarted number.
  const status: StepStatus = node.status === "failed" && node.result ? "issues"
    : node.status === "completed" ? (issues.length > 0 ? "issues" : "done")
    : NODE_STATUS[node.status] ?? (node.result ? (issues.length > 0 ? "issues" : "done") : "pending");
  return {
    key: `node:${node.id}`,
    kind: "worker",
    status,
    title: (firstLine.length > 120 ? `${firstLine.slice(0, 119)}…` : firstLine) || node.id,
    subtitle: [role, node.stage === "review" ? "review" : null, node.waiting ?? (node.status === "running" ? "Starting" : null)]
      .filter(Boolean).join(" · "),
    badges: status === "issues" && issues.length > 0 ? [{ text: `${issues.length} issue${issues.length === 1 ? "" : "s"}`, tone: "warning" }] : [],
    body: node.result?.summary ?? (node.objective || undefined),
    items: workerResultItems(checks),
    assignmentRef: node.assignmentRef ?? undefined,
  };
}

/** The whole task as one top-to-bottom list: request, plan, every delegated
 * worker in delegation order, then the coordinator's acceptance. */
export function buildPipeline(pipeline: Pipeline): PipelineView {
  const state = pipeline.task?.state ?? "running";
  const accepted = state === "accepted";
  const [request, ...followUps] = pipeline.messages;
  const order = (worker: Pipeline["workers"][number]) => pipeline.delegations[worker.assignmentRef ?? worker.id]?.order ?? Number.MAX_SAFE_INTEGER;
  const started = new Set(pipeline.workers.map((worker) => worker.assignmentRef ?? worker.id));
  const plannedNodes = (pipeline.plan?.nodes ?? []).filter((node) => !node.assignmentRef || !started.has(node.assignmentRef));
  const workers = [
    ...[...pipeline.workers].sort((a, b) => order(a) - order(b)).map((worker) => workerStep(worker, pipeline)),
    ...plannedNodes.map(nodeStep),
  ];
  const live = state === "running" || state === "recovering";

  const nodeCount = pipeline.plan?.nodes.length ?? 0;
  const planned = pipeline.requirements.length > 0 || workers.length > 0;
  // With a runtime plan, a step pulses only while it really runs: the
  // coordinator works in PLANNING and RECONCILING, the runtime's own checks
  // in VERIFYING; anything else is workers, or waiting for one.
  const runtime = pipeline.plan?.state ?? null;
  const planStatus: StepStatus = runtime
    ? (runtime === "PLANNING" && live ? "active" : planned ? "done" : "pending")
    : planned ? "done" : live ? "active" : "pending";
  const plan: PipelineStep = {
    key: "plan",
    kind: "plan",
    status: planStatus,
    title: "Plan",
    subtitle: pipeline.requirements.length > 0
      ? `${pipeline.requirements.length} requirement${pipeline.requirements.length === 1 ? "" : "s"}`
      : nodeCount > 0 ? `${nodeCount} planned step${nodeCount === 1 ? "" : "s"}`
      : planned ? "Delegated without recorded requirements" : "The coordinator is reading the request",
    badges: [],
    body: pipeline.requirements.map((requirement) => requirement.statement).join("\n\n") || undefined,
    items: pipeline.requirements.flatMap((requirement) => requirement.criteria.map((text) => ({ text, status: accepted ? "done" as const : "open" as const }))),
  };

  const workerActive = workers.some((step) => step.status === "active");
  const finishing = runtime ? live && (runtime === "RECONCILING" || runtime === "VERIFYING") : planned && !workerActive;
  const finish: PipelineStep = {
    key: "finish",
    kind: "finish",
    status: accepted ? "done"
      : state === "stopped" ? "cancelled"
      : !live ? "blocked"
      : finishing ? "active" : "pending",
    title: accepted ? "Accepted"
      : !live ? TASK_STATE[state]?.text ?? state
      : !finishing ? "Review and acceptance"
      : runtime === "VERIFYING" ? "Running verification" : "Coordinator is reviewing",
    subtitle: accepted ? "The coordinator accepted the verified result" : undefined,
    badges: [],
    body: accepted ? undefined : pipeline.task?.waitDetail ?? (!live ? pipeline.plan?.blocker ?? undefined : undefined),
    items: [],
  };

  const steps: PipelineStep[] = [
    {
      key: "request",
      kind: "request",
      status: "done",
      title: "Request",
      subtitle: followUps.length ? `${followUps.length} follow-up${followUps.length === 1 ? "" : "s"}` : undefined,
      badges: [],
      body: request?.text ?? "The request text is not recorded.",
      items: [],
      followUps: followUps.map((message) => message.text),
    },
    plan,
    ...workers,
    finish,
  ];
  return {
    steps,
    done: steps.filter((step) => step.status === "done").length,
    total: steps.length,
    headline: TASK_STATE[state] ?? { text: state, tone: "muted", live },
  };
}
