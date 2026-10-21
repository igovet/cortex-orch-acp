import type { ProviderEvent } from "@getpaseo/plugin/server/provider";
import { CORTEX_PLUGIN_ID, WORKER_ROW_KIND } from "../shared/worker-card-item.ts";

/**
 * Paseo flushes its streaming text row on ANY plugin item, even an update to
 * an existing card. Keep asynchronous worker events after the current logical
 * message, not between its tokens. Text always passes through immediately.
 * Real timeline boundaries and turn/load/error/close completion flush every
 * queued card in arrival order; no punctuation guessing or text rewriting.
 */
export class WorkerCardOrder {
  private messages = new Map<string, string>();
  private pending = new Map<string, ProviderEvent[]>();

  flush(sessionId: string): ProviderEvent[] {
    this.messages.delete(sessionId);
    const pending = this.pending.get(sessionId) ?? [];
    this.pending.delete(sessionId);
    return pending;
  }

  close(): ProviderEvent[] {
    const events = [...this.pending.values()].flat();
    this.pending.clear();
    this.messages.clear();
    return events;
  }

  accept(event: ProviderEvent): ProviderEvent[] {
    if (!("sessionId" in event)) return [event];
    const sessionId = event.sessionId;
    if (event.type === "timeline.item") {
      const item = event.item;
      if (item.type === "assistant_message") {
        const id = item.messageId ?? item.id;
        const pending = this.messages.get(sessionId) === id ? [] : this.flush(sessionId);
        this.messages.set(sessionId, id);
        return [...pending, event];
      }
      if (item.type === "plugin" && item.pluginId === CORTEX_PLUGIN_ID && item.kind === WORKER_ROW_KIND) {
        if (this.messages.has(sessionId)) {
          const pending = this.pending.get(sessionId) ?? [];
          pending.push(event);
          this.pending.set(sessionId, pending);
          return [];
        }
        return [event];
      }
      return [...this.flush(sessionId), event];
    }
    if (event.type === "session.turn" || event.type === "session.ready"
      || event.type === "session.closed" || event.type === "session.runtime_failed"
      || event.type === "session.permission") return [...this.flush(sessionId), event];
    return [event];
  }
}
