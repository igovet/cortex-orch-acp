import type { WorkerHistory } from "../shared/worker-history.js";

type WorkerAssignment = WorkerHistory["assignments"][number];

/**
 * The coordinator timeline calls a worker by its resolved role (for example
 * `explorer`). Assignment refs are durable lookup keys and must never become
 * user-facing worker names.
 */
export function workerDisplayName(assignment: Pick<WorkerAssignment, "role" | "title" | "nodeKey">): string {
  const role = assignment.role?.trim();
  if (role) return role.replace(/^worker-/i, "");

  const title = assignment.title?.trim();
  // Older history rows occasionally used the internal delegate key as the
  // title. Prefer the graph node key in that case, which is the same role
  // identity used by the coordinator card when no resolved role is present.
  if (title && !/^delegate:[0-9a-f-]+$/i.test(title)) return title;
  const nodeKey = assignment.nodeKey.trim().replace(/^worker-/i, "");
  return nodeKey || "worker";
}

/** The status word both the chat row and the Workers popover show. */
export function workerStatusLabel(status: string | null | undefined): string {
  const value = (status ?? "").toLowerCase();
  if (["running", "pending", "in_progress", "in-progress", "provisioning", "ready", "started"].includes(value)) return "working";
  if (["failed", "error"].includes(value)) return "failed";
  if (["canceled", "cancelled"].includes(value)) return "cancelled";
  if (["completed", "reported", "succeeded", "success"].includes(value)) return "completed";
  return value || "working";
}

export type WorkerStatusTone = "active" | "success" | "danger" | "muted";

/**
 * The at-a-glance treatment for a worker card: the same status word as
 * `workerStatusLabel`, plus a tone (mapped to a theme status colour by the
 * caller) and an icon, so status is never carried by colour alone.
 */
export function workerStatusPresentation(status: string | null | undefined): { label: string; tone: WorkerStatusTone; icon: string } {
  const label = workerStatusLabel(status);
  if (label === "completed") return { label, tone: "success", icon: "CircleCheck" };
  if (label === "failed") return { label, tone: "danger", icon: "CircleAlert" };
  if (label === "cancelled") return { label, tone: "muted", icon: "CircleSlash" };
  return { label, tone: "active", icon: "Loader2" };
}

/** One worker line: its task as the name, then `profile · provider · model · effort · status`. */
export function workerLine(name: string, profile: string, route: readonly (string | null | undefined)[], status: string | null | undefined) {
  return {
    workerName: name.trim() || profile || "assignment",
    subtitle: [profile || "worker", ...route.filter((part): part is string => Boolean(part && part.trim())), workerStatusLabel(status)].join(" · "),
  };
}

/**
 * Worker cards use the ACP tool title as their portable metadata channel:
 * `Worker: profile · provider · model · effort — assignment title`.
 * Keep parsing tolerant for older cards that omitted route metadata.
 */
export function workerCardMetadata(title: string, status: string) {
  const body = title.trim().replace(/^Worker:\s*/i, "");
  const [route, ...nameParts] = body.split(/\s+—\s+/, 2);
  const routeParts = route.split(/\s+·\s+/).map((part) => part.trim()).filter(Boolean);
  return workerLine(nameParts.join(" — "), routeParts[0] || "worker", routeParts.slice(1), status);
}

/**
 * The route's real, engine-observed model display text: its resolved name
 * when the engine gave evidence for one, otherwise its routing id/alias
 * explicitly marked as an alias -- never a guessed version. `undefined`
 * when there is no route model at all.
 */
export function workerModelDisplay(route: { model?: string | null; modelName?: string | null } | null | undefined): string | undefined {
  if (!route?.model) return undefined;
  return route.modelName || `${route.model} (alias)`;
}

/** The popover's line for a history assignment, identical to its chat row. */
export function workerAssignmentLine(assignment: WorkerAssignment, status: string) {
  const route = assignment.route ?? assignment.attempts[assignment.attempts.length - 1]?.route;
  const title = assignment.title?.trim() ?? "";
  return workerLine(/^delegate:[0-9a-f-]+$/i.test(title) ? "" : title, workerDisplayName(assignment),
    [route?.provider, workerModelDisplay(route), route?.effort], status);
}

/** Claude Code's compact token count: 950, 84k, 1.2M. */
export function formatTokens(n: number): string {
  if (n >= 1_000_000) return `${+(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1000) return `${Math.round(n / 1000)}k`;
  return String(n);
}

/** The latest attempt's context fill, as the chat's context indicator shows it. */
export function contextSummary(context: { used: number; size: number } | null | undefined) {
  if (!context || context.size <= 0) return null;
  const percent = Math.min(100, Math.round((context.used / context.size) * 100));
  return {
    percent,
    detail: `${formatTokens(context.used)} / ${formatTokens(context.size)}`,
    level: percent >= 90 ? "danger" as const : percent >= 70 ? "warning" as const : "normal" as const,
  };
}
