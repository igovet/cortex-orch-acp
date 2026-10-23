import type { WorkerHistory } from "../shared/worker-history.ts";

type Attempt = WorkerHistory["assignments"][number]["attempts"][number];

/**
 * Everything a coordinator worker card keeps from its compact durable read:
 * status, route, usage and the open-issue count. Never the summary, check list,
 * failure text or transcript -- those belong to the worker tab.
 */
export type CardUsage = {
  status: string;
  route?: Attempt["route"];
  context?: Attempt["context"];
  tokens?: Attempt["tokens"];
  openIssues?: number;
};

/**
 * The card's outer frame for one interaction state. Its boundary is the
 * same neutral 1-unit border in every state: no focus, selected, pressed or
 * status perimeter or outline (and on web none of the browser's own focus
 * ring). Hover, pressed and keyboard focus only change the fill to the hover
 * surface; the owner removed the visible Open worker action that used to
 * carry the focus underline.
 */
export function cardFrameStyle(
  state: { pressed: boolean; hovered?: boolean; focused?: boolean },
  options: { disabled: boolean; web: boolean; compact: boolean; border: string; surface: string; hoverSurface: string },
): Record<string, unknown> {
  return {
    alignSelf: "stretch", marginVertical: options.compact ? 6 : 8,
    borderWidth: 1, borderColor: options.border, borderRadius: 8,
    backgroundColor: !options.disabled && (state.pressed || state.hovered || state.focused) ? options.hoverSurface : options.surface,
    paddingHorizontal: 12, paddingVertical: options.compact ? 8 : 10, gap: 6,
    ...(options.web ? { outlineWidth: 0, outlineStyle: "none" } : {}),
  };
}

export function cardUsageSnapshot(attempt: Pick<Attempt, "status" | "route" | "context" | "tokens" | "openIssues">): CardUsage {
  return {
    status: attempt.status,
    ...(attempt.route ? { route: { ...attempt.route, ...(attempt.route.selection ? { selection: { ...attempt.route.selection } } : {}) } } : {}),
    ...(attempt.context ? { context: { used: attempt.context.used, size: attempt.context.size } } : {}),
    ...(attempt.tokens ? { tokens: { ...attempt.tokens } } : {}),
    ...(attempt.openIssues !== undefined ? { openIssues: attempt.openIssues } : {}),
  };
}

function sameRecord(a: Record<string, unknown> | undefined, b: Record<string, unknown> | undefined): boolean {
  if (a === b) return true;
  if (!a || !b) return false;
  const keys = Object.keys(a);
  return keys.length === Object.keys(b).length && keys.every((key) => Object.is(a[key], b[key]));
}

/** True when a poll returned nothing new: the card keeps its state object,
 * so an unchanged 3-second poll causes no re-render. */
export function sameCardUsage(a: CardUsage | undefined, b: CardUsage | undefined): boolean {
  if (a === b) return true;
  if (!a || !b) return false;
  return a.status === b.status && a.openIssues === b.openIssues
    && sameRecord(a.route && { ...a.route, selection: undefined }, b.route && { ...b.route, selection: undefined })
    && sameRecord(a.route?.selection ?? undefined, b.route?.selection ?? undefined)
    && sameRecord(a.context, b.context) && sameRecord(a.tokens, b.tokens);
}

/** The open-issue count a card may show: for failed or completed-with-issues outcomes,
 * and only when a durable or embedded count is actually known. */
export function cardIssueCount(statusLabel: string, ...counts: Array<number | undefined>): number | null {
  if (statusLabel !== "completed with issues" && statusLabel !== "failed") return null;
  const known = counts.find((count) => count !== undefined);
  return known === undefined ? null : known;
}

/**
 * At most one short attention line: a fixed category and where to look,
 * never any part of the reason itself. A failure or cancellation reason can
 * list check names, blockers, cancellation tokens or prose in any language;
 * all of that is worker detail, shown in the worker tab (the selected
 * attempt's failure reason, or the handed-off reason of a worker that never
 * started an attempt). An interruption names no next step: recovery may
 * write the interrupted attempt off and schedule a new one, so the card never
 * promises that this worker resumes.
 */
export function cardAttention(statusLabel: string, issueCount: number | null, failureReason?: string): string | null {
  if (statusLabel === "failed") return issueCount ? `${issueCount} open issue${issueCount === 1 ? "" : "s"} · details in the worker tab` : "Failure details in the worker tab";
  if (statusLabel === "cancelled") return failureReason?.trim() ? "Cancellation details in the worker tab" : null;
  if (statusLabel === "interrupted") return "Interrupted · details in the worker tab";
  if (statusLabel === "completed with issues") {
    return issueCount ? `${issueCount} open issue${issueCount === 1 ? "" : "s"} · details in the worker tab` : "Issues reported · details in the worker tab";
  }
  return null;
}

export const CARD_ANNOUNCEMENT_TITLE_MAX_CHARS = 100;

/** What a card's polite live region says for one status transition: which
 * worker changed, then its new status (and open-issue count). */
export function cardStatusAnnouncement(input: { title: string; role: string; status: string; issueCount: number | null }): string {
  const trimmed = input.title.trim().replace(/[.\s]+$/, "");
  const title = trimmed.length <= CARD_ANNOUNCEMENT_TITLE_MAX_CHARS ? trimmed : `${trimmed.slice(0, CARD_ANNOUNCEMENT_TITLE_MAX_CHARS - 1)}…`;
  const issues = input.issueCount ? `, ${input.issueCount} open issue${input.issueCount === 1 ? "" : "s"}` : "";
  return `Worker ${input.role}, ${title || "untitled"}: ${input.status}${issues}`;
}

/** One rendered status of a card, as its announcer last saw it. `key` is
 * the status and issue count only: usage polling never changes it. */
export type CardStatusSnapshot = { key: string; loaded: boolean };

export function cardStatusKey(status: string, issueCount: number | null): string {
  return `${status}\u0000${issueCount ?? ""}`;
}

/**
 * Whether a card announces its status: only on a real transition of status
 * or issue count, never on mount or replay (no previous snapshot), never for
 * an unchanged status, and never for the first durable read that merely
 * reconciles a replayed card with its saved state.
 */
export function shouldAnnounceCardStatus(previous: CardStatusSnapshot | null, next: CardStatusSnapshot): boolean {
  if (!previous || previous.key === next.key) return false;
  return !(!previous.loaded && next.loaded);
}

/** The card's one accessible name: full title, role, status, then the action. */
export function cardAccessibilityLabel(input: { title: string; role: string; status: string; route: string | null; selection?: string | null; issueCount: number | null; disabled: boolean }): string {
  const issues = input.issueCount ? `, ${input.issueCount} open issue${input.issueCount === 1 ? "" : "s"}` : "";
  const route = (input.route ? `, ${input.route}` : "") + (input.selection ? `, chosen as ${input.selection}` : "");
  const action = input.disabled ? "Worker unavailable" : "Open worker";
  const title = input.title.replace(/[.\s]+$/, "");
  return `${title}. ${input.role}${route}. Status: ${input.status}${issues}. ${action}`;
}
