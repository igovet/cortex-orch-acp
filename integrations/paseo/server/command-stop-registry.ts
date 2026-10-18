import { STOP_COMMAND_CONFIG_ID } from "../shared/command-stop.ts";

/** The exact three-property value Cortex's `cortex_stop_command` accepts. */
export type CommandStopPayload = { toolCallRef: string; reason: string; requestKey: string };
export type CommandStopSend = (payload: CommandStopPayload) => Promise<void>;

/** How long the provider waits for Cortex to acknowledge a stop request. */
export const COMMAND_STOP_ACK_TIMEOUT_MS = 15000;

/** A send that got no acknowledgement in time: the request may still have
 * been recorded, so the same request key can be retried safely. */
export function unconfirmedStopError(): Error {
  return Object.assign(new Error("Cortex did not acknowledge the stop request in time"), { unconfirmed: true });
}

export function isUnconfirmedStop(error: unknown): boolean {
  return error instanceof Error && (error as Error & { unconfirmed?: boolean }).unconfirmed === true;
}

/** The ACP config setting carrying one stop request. */
export function commandStopSetting(payload: CommandStopPayload): Record<string, string> {
  return {
    [STOP_COMMAND_CONFIG_ID]: JSON.stringify({
      toolCallRef: payload.toolCallRef, reason: payload.reason, requestKey: payload.requestKey,
    }),
  };
}

// The plugin server's RPC handlers and the provider connections live in the
// same daemon process. Each open Cortex session registers how to reach its
// own ACP config channel, keyed by the native ACP session id Paseo also
// stores in the agent's runtimeInfo. Only the connection that registered a
// session can remove it.
const sessions = new Map<string, { owner: object; send: CommandStopSend }>();

export function registerCommandStopSession(nativeSessionId: string, owner: object, send: CommandStopSend): void {
  sessions.set(nativeSessionId, { owner, send });
}

export function unregisterCommandStopSessions(owner: object, nativeSessionId?: string): void {
  for (const [id, entry] of sessions) {
    if (entry.owner === owner && (nativeSessionId === undefined || id === nativeSessionId)) sessions.delete(id);
  }
}

export function commandStopSender(nativeSessionId: string): CommandStopSend | undefined {
  return sessions.get(nativeSessionId)?.send;
}
