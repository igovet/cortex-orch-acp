import type { RealTransport, TransportEvent } from "../../../packages/workspace-ui/src/port/real";

export type NativeDependencies = {
  invoke(body: { operation: string; message: unknown }): Promise<unknown>;
  listen(listener: (event: unknown) => void): Promise<() => void>;
};
const lost: TransportEvent = { type: "connection", state: "disconnected", reason: "Machine service disconnected" };
const connected: TransportEvent = { type: "connection", state: "connected" };
/**
 * The one request that says whether the machine is there. Everything else can be refused, be slow or lose its own reply
 * while the service is fine, so what it reports belongs to that action, not to the connection.
 */
const LIVENESS = "service.status";
function eventOf(raw: unknown): TransportEvent | null {
  if (!raw || typeof raw !== "object") return null;
  const e = raw as Record<string, unknown>;
  if (e.type === "invalidated" && typeof e.projectId === "string") return { type: "invalidated", projectId: e.projectId };
  if (e.type === "refresh") return { type: "refresh" };
  if (e.type === "connection" && ["connected", "disconnected", "unavailable"].includes(String(e.state)))
    return { type: "connection", state: e.state as "connected" | "disconnected" | "unavailable",
      ...(typeof e.reason === "string" ? { reason: e.reason } : {}) };
  return null;
}

/** A real-only adapter: no endpoint, token, path picker, engine launch or replay in JavaScript. */
export function createDesktopTransport(native: NativeDependencies) {
  const listeners = new Set<(event: TransportEvent) => void>();
  const pending = new Set<Promise<unknown>>();
  let closing = false;
  let disposed = false, unlisten: (() => void) | undefined;
  let connection: TransportEvent | undefined;
  const publish = (event: TransportEvent) => {
    if (disposed) return;
    if (event.type === "connection") connection = event;
    for (const listener of listeners) listener(event);
  };
  native.listen(raw => { const event = eventOf(raw); if (event) publish(event); })
    .then(stop => { if (disposed) stop(); else unlisten = stop; })
    .catch(() => publish(lost));

  const transport: RealTransport = {
    async request(operation, message) {
      if (disposed) throw new Error("Workspace window detached");
      if (closing) throw new Error("Workspace window closing");
      const request = native.invoke({ operation, message });
      pending.add(request);
      try {
        const reply = await request;
        if (operation === LIVENESS && connection?.type === "connection" && connection.state !== "connected") publish(connected);
        return reply;
      } catch (error) {
        const code = typeof error === "string" ? error : error instanceof Error ? error.message : "";
        if (operation === LIVENESS) publish(lost);
        throw error instanceof Error ? error : new Error(code || "The request failed. Nothing was resent.");
      } finally { pending.delete(request); }
    },
    subscribe(listener) {
      listeners.add(listener);
      if (connection) listener(connection);
      return () => { listeners.delete(listener); };
    },
  };
  return { transport,
    async close(finish: () => Promise<void>) {
      // No new write can race the acknowledgement barrier. Refusal leaves the view alive.
      closing = true;
      try { await Promise.all([...pending]); await finish(); }
      finally { closing = false; }
    },
    dispose() { disposed = true; listeners.clear(); unlisten?.(); } };
}
