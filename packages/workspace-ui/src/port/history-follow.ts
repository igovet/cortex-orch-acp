import { validate, type DecisionPage, type HistoryPage } from "@cortex/workspace-contract";
import { nativeVersion } from "./native";

export type HistoryFollowRequest = {
  projectId: string; taskId: string; executionId: string;
  generation: string | null; cursor: string | null; limit: number;
};
export type FollowHistory = (request: HistoryFollowRequest, onEvent: (event: unknown) => void, onEnd: () => void) => () => void;
export type FollowPage = {
  generation: string; throughSequence: string; fromCursor: string | null;
  page: HistoryPage & { tailCursor: string; prevCursor?: string | null };
  decisions?: DecisionPage;
};

const record = (value: unknown): value is Record<string, unknown> =>
  value !== null && typeof value === "object" && !Array.isArray(value);

/** Accept only complete pages on this watched identity. Forward backlog returns to the paged reader. */
export function followPage(raw: unknown, request: HistoryFollowRequest): FollowPage {
  if (!record(raw) || raw.type !== "native_history_page" || raw.projectId !== request.projectId
    || raw.taskId !== request.taskId || raw.executionId !== request.executionId
    || typeof raw.generation !== "string" || !raw.generation || (request.generation !== null && raw.generation !== request.generation)
    || !nativeVersion(raw.throughSequence) || !(raw.fromCursor === null || typeof raw.fromCursor === "string"))
    throw Error("Conversation stream identity changed");
  const page = validate("HistoryPage", raw.page);
  if (page.nextCursor !== null || typeof page.tailCursor !== "string" || !page.tailCursor
    || !nativeVersion(page.highWatermark) || !page.decisionRevision || page.items.length > request.limit
    || !(page.prevCursor === undefined || page.prevCursor === null || typeof page.prevCursor === "string")
    || new Set(page.items.map(row => row.historyId)).size !== page.items.length)
    throw Error("Incomplete conversation stream page");
  const decisions = raw.decisions === undefined ? undefined : validate("DecisionPage", raw.decisions);
  if (decisions && (decisions.nextCursor !== null || decisions.decisionRevision !== page.decisionRevision
    || decisions.items.some(d => d.executionId !== request.executionId || d.conversationSequence === undefined || d.conversationTieBreaker === undefined)
    || new Set(decisions.items.map(d => d.decisionId)).size !== decisions.items.length))
    throw Error("Unmatched conversation decision page");
  return { generation: raw.generation, throughSequence: raw.throughSequence, fromCursor: raw.fromCursor,
    page: page as FollowPage["page"], decisions };
}
