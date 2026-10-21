import { workerCardOutcome } from "./worker-card-outcome.ts";

/**
 * One worker is one coordinator card, however many host records report it.
 *
 * A worker card is one ACP tool call (`worker:<assignment>`) whose lifecycle
 * outlives the coordinator turn that created it: a Stop, a steer or a later
 * continuation reports its terminal state from another turn. The installed
 * Paseo client (0.12.0) merges a `tool_call` timeline item only into a row
 * of the same turn (`turn:<turnId>/<callId>`), so each such later update
 * starts a second tool_call row, and every row runs through the client's
 * timeline transformer: one worker rendered as two identical cards. A
 * provider-emitted `plugin` item, by contrast, is merged by its stable
 * `<pluginId>/<id>` identity across turns and replays, replacing the row's
 * data in place.
 *
 * The server provider therefore projects each worker tool_call into that
 * plugin item (`workerCardPluginItem`). The client transformer keeps the
 * same projection (`workerRowData`) for tool_call items an older plugin
 * already persisted, so replayed history still renders.
 */

/** The plugin manifest id (`paseo-plugin.json`). The host refuses a
 * provider plugin item naming any other plugin. */
export const CORTEX_PLUGIN_ID = "cortex-acp";
export const WORKER_ROW_KIND = "worker-row";
export const WORKER_ROW_VERSION = 1;
export const WORKER_CARD_PREFIX = "worker:";

export type WorkerRowStatus = "running" | "completed" | "canceled" | "failed";

export type WorkerRowData = {
  callId: string;
  assignmentRef: string;
  title: string;
  status: WorkerRowStatus;
  openIssues?: number;
  outcomeStatus?: "completed" | "failed";
  failureReason?: string;
};

type WorkerToolItem = {
  callId: string;
  name: string;
  status: string;
  error?: unknown;
  metadata?: unknown;
};

const ROW_STATUSES: ReadonlySet<string> = new Set(["running", "completed", "canceled", "failed"]);

export function isWorkerCardCallId(callId: unknown): callId is string {
  return typeof callId === "string" && callId.startsWith(WORKER_CARD_PREFIX);
}

/** The logical card identity: one per worker call id, never per host record. */
export function workerRowId(callId: string): string {
  return `${WORKER_ROW_KIND}:${callId}`;
}

/** The compact card data for one worker tool_call item: title, status, the
 * open-issue count and the host's own short failure line -- never a result
 * summary, check list or transcript. */
export function workerRowData(item: WorkerToolItem): WorkerRowData {
  const outcome = workerCardOutcome(item.metadata);
  const status = outcome?.status ?? (ROW_STATUSES.has(item.status) ? item.status : "running") as WorkerRowStatus;
  const failureReason = status === "failed" && typeof item.error === "string" && item.error.trim()
    ? item.error.trim() : undefined;
  return {
    callId: item.callId,
    assignmentRef: item.callId.slice(WORKER_CARD_PREFIX.length),
    title: item.name,
    status,
    ...(outcome !== undefined ? { openIssues: outcome.openIssues } : {}),
    ...(outcome?.status ? { outcomeStatus: outcome.status } : {}),
    ...(failureReason !== undefined ? { failureReason } : {}),
  };
}

/** The provider-emitted plugin timeline item for one worker card. */
export function workerCardPluginItem(item: WorkerToolItem) {
  return {
    type: "plugin" as const,
    id: workerRowId(item.callId),
    pluginId: CORTEX_PLUGIN_ID,
    kind: WORKER_ROW_KIND,
    version: WORKER_ROW_VERSION,
    data: workerRowData(item),
  };
}
