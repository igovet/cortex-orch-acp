import type { Pipeline } from "../shared/pipeline.js";
import { workerDisplayName } from "./worker-display.ts";
import { workerPopoverStatus } from "./worker-popover-status.ts";

export type StepStatus = "done" | "active" | "failed" | "blocked" | "cancelled" | "pending";
export type Tone = "success" | "warning" | "danger" | "muted" | "accent";

export interface StepItem { text: string; status: "done" | "failed" | "open" }

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
  return {
    key: `worker:${worker.id}`,
    kind: "worker",
    status: status === "completed" ? "done" : status === "running" ? "active" : ["failed", "error"].includes(status) ? "failed"
      : ["cancelled", "canceled"].includes(status) ? "cancelled" : "pending",
    title,
    subtitle: [workerDisplayName(worker), route?.model, route?.effort].filter(Boolean).join(" · "),
    badges,
    body: delegation?.result?.summary ?? (status === "failed" || status === "error" ? failure ?? undefined : undefined),
    items: (delegation?.result?.checks ?? []).map((check) => ({
      text: check.name,
      status: check.outcome === "passed" ? "done" : check.outcome === "failed" ? "failed" : "open",
    })),
    assignmentRef: ref,
    usage: worker.attempts[worker.attempts.length - 1],
  };
}

/** The whole task as one top-to-bottom list: request, plan, every delegated
 * worker in delegation order, then the coordinator's acceptance. */
export function buildPipeline(pipeline: Pipeline): PipelineView {
  const state = pipeline.task?.state ?? "running";
  const accepted = state === "accepted";
  const [request, ...followUps] = pipeline.messages;
  const order = (worker: Pipeline["workers"][number]) => pipeline.delegations[worker.assignmentRef ?? worker.id]?.order ?? Number.MAX_SAFE_INTEGER;
  const workers = [...pipeline.workers].sort((a, b) => order(a) - order(b)).map((worker) => workerStep(worker, pipeline));
  const live = state === "running" || state === "recovering";

  const planned = pipeline.requirements.length > 0 || workers.length > 0;
  const plan: PipelineStep = {
    key: "plan",
    kind: "plan",
    status: planned ? "done" : live ? "active" : "pending",
    title: "Plan",
    subtitle: pipeline.requirements.length > 0
      ? `${pipeline.requirements.length} requirement${pipeline.requirements.length === 1 ? "" : "s"}`
      : planned ? "Delegated without recorded requirements" : "The coordinator is reading the request",
    badges: [],
    body: pipeline.requirements.map((requirement) => requirement.statement).join("\n\n") || undefined,
    items: pipeline.requirements.flatMap((requirement) => requirement.criteria.map((text) => ({ text, status: accepted ? "done" as const : "open" as const }))),
  };

  const workerActive = workers.some((step) => step.status === "active");
  const finish: PipelineStep = {
    key: "finish",
    kind: "finish",
    status: accepted ? "done"
      : state === "stopped" ? "cancelled"
      : !live ? "blocked"
      : planned && !workerActive ? "active" : "pending",
    title: accepted ? "Accepted" : live ? (planned && !workerActive ? "Coordinator is reviewing" : "Review and acceptance") : TASK_STATE[state]?.text ?? state,
    subtitle: accepted ? "The coordinator accepted the verified result" : undefined,
    badges: [],
    body: accepted ? undefined : pipeline.task?.waitDetail ?? undefined,
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
