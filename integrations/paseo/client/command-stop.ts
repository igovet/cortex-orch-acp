import { STOP_REASON_MAX, type CommandStopResult } from "../shared/command-stop.ts";

/** Local progress of one user stop request in the worker panel. */
export type StopPhase = "editing" | "sending" | "requested" | "refused" | "unconfirmed";
export type StopState = {
  phase: StopPhase;
  /** The reason as typed. */
  reason: string;
  /** Retry identity: kept while the submitted reason is unchanged, so a
   * retry after an unconfirmed or failed delivery cannot record a second,
   * different stop. */
  requestKey: string | null;
  submittedReason: string | null;
  message: string | null;
};

export function startStop(): StopState {
  return { phase: "editing", reason: "", requestKey: null, submittedReason: null, message: null };
}

/** A reason Cortex will accept: required, non-blank, at most 2000 characters. */
export function validateStopReason(text: string): { ok: true; reason: string } | { ok: false; message: string } {
  const reason = text.trim();
  if (!reason) return { ok: false, message: "Enter a reason. The worker sees it and continues without this command." };
  if (text.length > STOP_REASON_MAX) return { ok: false, message: `Keep the reason within ${STOP_REASON_MAX} characters.` };
  return { ok: true, reason };
}

/** A fresh retry identity, without depending on a host crypto API. */
export function makeRequestKey(): string {
  const crypto = (globalThis as { crypto?: { randomUUID?: () => string } }).crypto;
  if (crypto?.randomUUID) return `paseo-stop:${crypto.randomUUID()}`;
  return `paseo-stop:${Date.now().toString(36)}-${Math.random().toString(36).slice(2)}-${Math.random().toString(36).slice(2)}`;
}

/** The state while one stop request is in flight, or a validation error. */
export function submitStop(state: StopState, newKey: () => string = makeRequestKey): StopState {
  const valid = validateStopReason(state.reason);
  if (!valid.ok) return { ...state, phase: state.phase === "sending" ? "editing" : state.phase, message: valid.message };
  const requestKey = state.requestKey && state.submittedReason === valid.reason ? state.requestKey : newKey();
  return { ...state, phase: "sending", requestKey, submittedReason: valid.reason, message: null };
}

/** The exact `cortex.commands.stop` input for a sending state. */
export function stopRequestInput(workspaceId: string, agentId: string, toolCallRef: string, state: StopState) {
  if (state.phase !== "sending" || !state.requestKey || !state.submittedReason) {
    throw new Error("A stop request is sent only from a validated sending state");
  }
  return { workspaceId, agentId, toolCallRef, reason: state.submittedReason, requestKey: state.requestKey };
}

export function stopResultState(state: StopState, result: CommandStopResult): StopState {
  if (result.status === "requested") return { ...state, phase: "requested", message: null };
  return { ...state, phase: result.status, message: result.message ?? "Cortex did not accept the stop request." };
}

/** A transport failure of the plugin call itself: delivery is unknown. */
export function stopTransportFailure(state: StopState): StopState {
  return { ...state, phase: "unconfirmed", message: "The stop request could not be delivered. It may still apply; retry to check." };
}

export type StoppedCommand = { reason: string; toolCallRef: string | null; command: string | null };

const STOPPED_TITLE_PREFIX = "Stopped by user: ";

/** The command this row's own trusted server fields say was stopped by the
 * user, or null.
 *
 * Cortex sets `status === "failed"` and a title of the exact form
 * `"Stopped by user: <reason>"` on a worker tool-call row only once the
 * server itself has confirmed a genuine, durable `COMMAND_STOPPED` record
 * for a `kind === "execute"` call, and has bound that one real stop to
 * only the single row that first observed it (see
 * `orchestrating_runtime._emit_worker_tool_update` and
 * `command_control.confirmed_stop`'s `attribute_to`). The reason shown
 * here always comes from that trusted `title`, never from a worker's own
 * claimed `stop_reason` text -- so this can never be forged by comparing
 * identifier namespaces that do not actually correlate (the ACP engine's
 * own per-call ref vs. the gateway's `tool_call_ref`), which is what made
 * the previous ref-equality check never match a genuine stop. `value`
 * (this row's raw output/content) is consulted only to recover the
 * attempted command text for display: a worker can fabricate that
 * *shape* inside its own tool output (for example by writing and reading
 * back a crafted file), so it is never used for the reason or as proof
 * that a stop happened. */
export function stoppedCommand(status: string | null | undefined, title: string | null | undefined, value: unknown): StoppedCommand | null {
  if (status !== "failed" || typeof title !== "string" || !title.startsWith(STOPPED_TITLE_PREFIX)) return null;
  const reason = title.slice(STOPPED_TITLE_PREFIX.length);
  const shape = stoppedShape(value);
  return { reason, toolCallRef: shape?.toolCallRef ?? null, command: shape?.command ?? null };
}

/** The display-only parts (attempted command, claimed ref) of a "stopped
 * by user" shaped payload, found inside the supported tool-output mirrors
 * (see command_control.stopped_receipt). Never a source of trust by
 * itself -- see `stoppedCommand`. */
function stoppedShape(value: unknown, depth = 0): { toolCallRef: string | null; command: string | null } | null {
  if (depth > 8) return null;
  if (typeof value === "string") {
    try { value = JSON.parse(value); } catch { return null; }
  }
  if (Array.isArray(value)) {
    for (const item of value) {
      const found = stoppedShape(item, depth + 1);
      if (found) return found;
    }
    return null;
  }
  if (!value || typeof value !== "object") return null;
  const record = value as Record<string, unknown>;
  if (record.failure === "stopped_by_user" && typeof record.stop_reason === "string") {
    const argv = record.argv;
    const command = Array.isArray(argv) && argv.every((part) => typeof part === "string")
      ? (argv as string[]).join(" ") : null;
    return { toolCallRef: typeof record.tool_call_ref === "string" ? record.tool_call_ref : null, command };
  }
  for (const key of ["structuredContent", "result", "content", "text"]) {
    if (key in record) {
      const found = stoppedShape(record[key], depth + 1);
      if (found) return found;
    }
  }
  return null;
}

/** Where an already recorded stop came from, in plain words. */
export function stopOriginLabel(origin: string | null): string {
  if (origin === "paseo_config") return "from Paseo";
  if (origin === "acp_command") return "with /cortex-kill";
  if (origin === "owner_cli") return "from the local command";
  return "";
}
