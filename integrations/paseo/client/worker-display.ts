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
  // A host disconnect (e.g. a plugin update) paused this worker before it
  // reported a result; recovery reconciles it from saved state on the next
  // message. Never the same pill as a genuine failure.
  if (value === "interrupted") return "interrupted";
  if (["failed", "error"].includes(value)) return "failed";
  if (["canceled", "cancelled"].includes(value)) return "cancelled";
  if (value === "completed_with_issues") return "completed with issues";
  if (["completed", "reported", "succeeded", "success"].includes(value)) return "completed";
  return value || "working";
}

export type WorkerStatusTone = "active" | "success" | "warning" | "danger" | "muted";
export type WorkerCheck = { outcome: string; closedBy?: string };

/**
 * One durable projection for every worker terminal surface. Callers supply
 * the status resolved from the persisted assignment, never Paseo's coarse
 * host status, plus the runtime's retained checks when they are available.
 */
export function workerTerminalState(status: string | null | undefined, checks: readonly WorkerCheck[] = []) {
  const presentation = workerStatusPresentation(status);
  // A failed check an exact later rerun closed is resolved history, not a
  // current open issue.
  const issueCount = checks.filter((check) => check.outcome !== "passed" && !check.closedBy).length;
  const terminal = presentation.label === "completed" && issueCount > 0
    ? workerStatusPresentation("completed_with_issues") : presentation;
  return { ...terminal, issueCount };
}

/**
 * The at-a-glance treatment for a worker card: the same status word as
 * `workerStatusLabel`, plus a tone (mapped to a theme status colour by the
 * caller) and an icon, so status is never carried by colour alone.
 */
export function workerStatusPresentation(status: string | null | undefined): { label: string; tone: WorkerStatusTone; icon: string } {
  const label = workerStatusLabel(status);
  if (label === "completed") return { label, tone: "success", icon: "CircleCheck" };
  if (label === "completed with issues") return { label, tone: "warning", icon: "CircleAlert" };
  if (label === "failed") return { label, tone: "danger", icon: "CircleAlert" };
  if (label === "interrupted") return { label, tone: "warning", icon: "PauseCircle" };
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
const STATUS_SUFFIX = /\s+·\s+(completed with issues|cancelled|interrupted)$/i;

function parseWorkerCardTitle(title: string) {
  let body = title.trim().replace(/^Worker:\s*/i, "");
  // The runtime appends the status to the whole title
  // (`Worker: role · route — name · Interrupted`, see replay_store.py and
  // orchestrating_runtime.py); older cards carried it inside the route.
  const trailing = body.includes(" — ") ? STATUS_SUFFIX.exec(body)?.[1] : undefined;
  if (trailing) body = body.slice(0, body.length - trailing.length).replace(/\s+·\s*$/, "");
  const [route, ...nameParts] = body.split(/\s+—\s+/, 2);
  const routeParts = route.split(/\s+·\s+/).map((part) => part.trim()).filter(Boolean);
  if (trailing) routeParts.push(trailing);
  const suffix = routeParts.at(-1)?.toLowerCase();
  const hasIssues = suffix === "completed with issues";
  // ACP's ToolCallStatus has no "cancelled" value, so a cancelled
  // assignment's card carries this same portable title suffix instead
  // (see orchestrating_runtime.py's `assignment_cancelled` handler).
  const isCancelled = suffix === "cancelled";
  // Same channel for a host-disconnect pause (see replay_store.py's
  // `terminal_worker_card_reconciliation`): ACP has no "interrupted" value
  // either, and unlike `_meta` this suffix survives the installed Paseo
  // ACP bridge, which drops `_meta` from a `tool_call_update` entirely.
  const isInterrupted = suffix === "interrupted";
  if (hasIssues || isCancelled || isInterrupted) routeParts.pop();
  return { name: nameParts.join(" — "), routeParts, hasIssues, isCancelled, isInterrupted };
}

/**
 * The status a reader should see for this card: the title's own
 * "Completed with issues"/"Cancelled"/"Interrupted" suffix (T035's portable
 * channel, above) takes precedence over a coarser host/embedded status --
 * this is what lets a card rendered with zero RPC (embedded ``result``
 * only, no live pipeline projection) still show the right pill.
 */
export function resolveWorkerCardStatus(title: string, status: string): string {
  const { hasIssues, isCancelled, isInterrupted } = parseWorkerCardTitle(title);
  return hasIssues ? "completed_with_issues" : isInterrupted ? "interrupted" : isCancelled ? "cancelled" : status;
}

/** Durable attempt statuses that settle a card whatever the host still shows. */
const SETTLED = new Set(["interrupted", "completed", "completed_with_issues", "failed", "cancelled", "canceled"]);

export function isSettledWorkerStatus(status: string | null | undefined): boolean {
  return SETTLED.has((status ?? "").toLowerCase());
}

/**
 * A settled durable verdict overrides coarse host completion/cancellation
 * guesses and old issue markers. A committed result supersedes stale title
 * markers; without result evidence the portable Interrupted marker survives
 * recovery writing a lost attempt off as failed. Active durable rows never
 * resurrect a terminal card. Counts refine success, not failure/cancellation.
 */
export function resolveDurableWorkerCardStatus(title: string, status: string, durable: string | null | undefined, openIssues?: number, outcomeStatus?: "completed" | "failed"): string {
  // A committed assignment verdict outranks even a terminal host guess or
  // an older title marker. Counts refine success, never failure/cancellation.
  let resolved = outcomeStatus ?? resolveWorkerCardStatus(title, status);
  const value = (durable ?? "").toLowerCase();
  if (isSettledWorkerStatus(value)) {
    // A known result count from the compact durable read distinguishes a
    // committed failure from legacy cancellation/interruption guesses.
    if (value === "failed" && openIssues !== undefined && resolved === "cancelled") resolved = value;
    if (resolved === "running" || resolved === "working" || resolved === "canceled") resolved = value;
    else if (resolved === "failed" && ["interrupted", "cancelled", "canceled"].includes(value)) resolved = value;
    else if (["completed", "completed_with_issues", "failed"].includes(resolved)
      && ["completed", "completed_with_issues", "failed"].includes(value)) resolved = value;
  }
  return (resolved === "completed" && openIssues !== undefined && openIssues > 0)
    ? "completed_with_issues" : resolved;
}

/** Route-selection provenance in a card title's route (`flagship`,
 * `light→standard`, `hint …`, `by …`, `usage 42%`, see
 * routing/decision_display.py). The card shows it on its own line, apart
 * from the engine · model · effort route. */
const ROUTE_PROVENANCE = /^(?:by|usage|hint)\s|^(?:light|standard|deep|flagship)(?:\s*→\s*(?:light|standard|deep|flagship))?$/i;

/** The chosen plan's usage reads `plan usage 42%`, never a bare `usage`
 * that would look like this worker's own context or token usage. */
function planUsageLabel(part: string): string {
  return part.replace(/^usage\s/i, "plan usage ");
}

/** How the runtime chose a route, one line: `deep · by usage · plan usage 42%`
 * (`null` without a recorded decision). */
export function routeSelectionText(selection: {
  tier?: string | null; hint?: string | null; decidedBy?: string | null; planUsedPercent?: number | null;
} | null | undefined): string | null {
  if (!selection) return null;
  const used = selection.planUsedPercent;
  const parts = [selection.tier, selection.hint ? `hint ${selection.hint}` : null,
    selection.decidedBy ? `by ${selection.decidedBy}` : null,
    `plan usage ${typeof used === "number" && Number.isFinite(used) ? `${Math.round(used)}%` : "unknown"}`];
  return parts.filter((part): part is string => Boolean(part)).join(" · ");
}

/** The compact coordinator card's identity: task title, role, the actual
 * engine · model · effort route, and separately how that route was chosen
 * (each `null` when the title carries none). */
export function workerCardIdentity(title: string): { title: string; role: string; route: string | null; selection: string | null } {
  const { name, routeParts } = parseWorkerCardTitle(title);
  const role = (routeParts[0] || "worker").replace(/^worker-/i, "");
  const route = routeParts.slice(1).filter((part) => !ROUTE_PROVENANCE.test(part));
  const selection = routeParts.slice(1).filter((part) => ROUTE_PROVENANCE.test(part)).map(planUsageLabel);
  return { title: name.trim() || role, role, route: route.length ? route.join(" · ") : null,
    selection: selection.length ? selection.join(" · ") : null };
}

export function workerCardMetadata(title: string, status: string) {
  const { name, routeParts, hasIssues, isCancelled, isInterrupted } = parseWorkerCardTitle(title);
  return workerLine(name, routeParts[0] || "worker", routeParts.slice(1),
    hasIssues ? "completed_with_issues" : isInterrupted ? "interrupted" : isCancelled ? "cancelled" : status);
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

type TokenTotals = { input: number; output: number; cachedRead: number; cachedWrite: number; knownCalls: number; unknownCalls: number };

/**
 * The token-count text a worker card shows for one attempt, or `null` to
 * render nothing at all. F1 fix: a `released_after_result` call (the
 * runtime released the worker right after its accepted result_submit,
 * before the engine's own usage for that exact call arrived -- see
 * observability.usage) is excluded entirely by server/worker-history.ts's
 * `sum_tokens`, so an attempt whose only calls were that kind has
 * `knownCalls === 0` and `unknownCalls === 0` here, and renders nothing --
 * never the "Tokens unavailable" / "call unavailable" alarm a genuinely
 * unaccounted-for call shows.
 */
export function tokensSummaryLabel(tokens: TokenTotals | null | undefined): string | null {
  if (!tokens) return null;
  const known = tokens.knownCalls
    ? `\u2191 ${formatTokens(tokens.input)}  \u2193 ${formatTokens(tokens.output)}${tokens.cachedRead ? `  \u00b7 ${formatTokens(tokens.cachedRead)} cache read` : ""}${tokens.cachedWrite ? `  \u00b7 ${formatTokens(tokens.cachedWrite)} cache write` : ""}`
    : "Tokens unavailable";
  const unavailable = tokens.unknownCalls ? `  \u00b7 ${tokens.unknownCalls} call${tokens.unknownCalls === 1 ? "" : "s"} unavailable` : "";
  return known + unavailable;
}

/** The accessibility label paired with {@link tokensSummaryLabel}'s text. */
export function tokensAccessibilityLabel(tokens: TokenTotals | null | undefined): string | null {
  if (!tokens) return null;
  return tokens.knownCalls
    ? `${tokens.input} uncached input, ${tokens.output} output, ${tokens.cachedRead} cache read, ${tokens.cachedWrite} cache write tokens`
    : "Token usage unavailable";
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
