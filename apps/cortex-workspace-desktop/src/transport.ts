import { validate, type CodeLspEvent } from "@cortex/workspace-contract";
import { nativeDelta } from "../../../packages/workspace-ui/src/port/native";
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
/** Reads carry no acknowledgement to lose: closing never waits for them. Anything not listed is a write. */
const READS = new Set(["native.query", "native.board", "service.status", "service.capabilities", "projects.list", "projects.status",
  "capabilities.get", "project.snapshot", "tasks.list", "tasks.get", "executions.get", "decisions.list", "decisions.get",
  "results.get", "reports.read", "history.page", "events.page", "requests.get", "directories.search", "terminal.read", "voice.read", "voice.settings", "remote.status", "askpass.list", "owner_actions.list"]);
const MACHINE_READS = new Set(["config.read", "project.config.read", "product.read", "product.facts", "product.board", "workers",
  "repo.snapshot", "repo.file", "repo.list", "repo.locate", "repo.skills", "repo.context", "overview", "logs.folder", "workflow.describe"]);
function isRead(operation: string, message: unknown): boolean {
  const m = (message ?? {}) as { op?: unknown; operation?: unknown };
  if (operation === "machine.call") return typeof m.op === "string" && MACHINE_READS.has(m.op);
  if (operation === "workspace") return typeof m.operation === "string" && READS.has(m.operation);
  return READS.has(operation);
}
function eventOf(raw: unknown): TransportEvent | null {
  if (!raw || typeof raw !== "object") return null;
  const e = raw as Record<string, unknown>;
  if (e.type === "invalidated" && typeof e.projectId === "string") return { type: "invalidated", projectId: e.projectId, ...(e.source === "runtime" ? {source:"runtime" as const} : {}) };
  if (e.type === "native_delta") { const delta = nativeDelta(e); return delta ? { type: "native_delta", ...delta } : typeof e.projectId === "string" ? {type:"native_resync",projectId:e.projectId} : null; }
  if (e.type === "native_resync" && typeof e.projectId === "string") return { type: "native_resync", projectId: e.projectId };
  if (e.type === "code_lsp_resync") return { type: "code_lsp_resync" };
  if (e.type === "code_lsp") {
    try { validate("CodeLspEvent", e); return e as CodeLspEvent; }
    catch { return null; }
  }
  if (e.type === "refresh") return { type: "refresh" };
  if (e.type === "connection" && ["connecting", "connected", "disconnected", "unavailable"].includes(String(e.state)))
    return { type: "connection", state: e.state as "connecting" | "connected" | "disconnected" | "unavailable",
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
  /** Status checks failed in a row. One lost answer while live is not a lost machine: the next poll decides. */
  let missed = 0;
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
      const write = !isRead(operation, message);
      if (write) pending.add(request);
      try {
        const reply = await request;
        if (operation === LIVENESS) missed = 0;
        if (operation === LIVENESS && connection?.type === "connection" && connection.state !== "connected") publish(connected);
        return reply;
      } catch (error) {
        const code = typeof error === "string" ? error : error instanceof Error ? error.message : "";
        const live = connection?.type === "connection" && connection.state === "connected";
        if (operation === LIVENESS && (++missed >= 2 || !live)) publish(lost);
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
