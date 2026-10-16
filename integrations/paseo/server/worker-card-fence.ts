/**
 * A Cortex worker card is one ACP tool call (`worker:<assignment>`) that
 * outlives the coordinator turn which created it: steering ends that turn
 * (`session/cancel`, then a new `session/prompt`) while the worker keeps
 * running, and a later turn reports its real outcome on the same call.
 *
 * The installed ACP bridge ends every prompt by settling each tool call it
 * still holds as pending/in_progress (`terminalizeTransientItems`): a
 * cancelled turn makes its timeline item `canceled` (bridge 0.11.x; `failed`
 * in 0.9.x), a failed one `failed`, a completed one `completed`. It does this
 * itself, without any update from the agent and without running provider
 * transformers, and keeps that synthetic status in its snapshot, so later
 * agent updates that carry no status are merged into it. For a worker card
 * this showed a still-running worker as Failed ("Tool call failed: ...").
 *
 * This fence keeps worker cards on the status Cortex actually sent. It reads
 * the raw ACP frames (see ReplayController's frame observer), restores that
 * status in the bridge's merged snapshots (bridge `toolCall` transformer), and
 * identifies the bridge's own turn-end terminal item for a worker Cortex still
 * reports active, so the provider can withhold it when the turn only ended
 * (cancelled or completed). A failed prompt (transport or agent error) keeps
 * the bridge's failure: nothing then proves the worker survived.
 */

export const WORKER_CARD_PREFIX = "worker:";

type AcpToolStatus = "pending" | "in_progress" | "completed" | "failed";

const ACP_TOOL_STATUSES: ReadonlySet<string> = new Set(["pending", "in_progress", "completed", "failed"]);
const TERMINAL: ReadonlySet<string> = new Set(["completed", "failed"]);

function isWorkerCard(id: unknown): id is string {
  return typeof id === "string" && id.startsWith(WORKER_CARD_PREFIX);
}

export class WorkerCardFence {
  private statuses = new Map<string, AcpToolStatus>();
  private outcomes = new Map<string, { id: string; status: string }>();

  /** Record the status Cortex itself sent for a worker card. ACP omits the
   * status of an update that does not change it. A terminal status is final:
   * a late, duplicated or out-of-order non-terminal frame never reopens it,
   * and a later terminal frame never replaces the first one (the runtime's
   * own first-writer-wins rule). */
  observe(frame: unknown): void {
    if (!frame || typeof frame !== "object") return;
    const message = frame as { method?: unknown; params?: { update?: Record<string, unknown> } };
    if (message.method !== "session/update") return;
    const update = message.params?.update;
    if (!update || (update.sessionUpdate !== "tool_call" && update.sessionUpdate !== "tool_call_update")) return;
    const id = update.toolCallId;
    const status = update.status;
    if (!isWorkerCard(id) || typeof status !== "string" || !ACP_TOOL_STATUSES.has(status)) return;
    const current = this.statuses.get(id);
    if (current !== undefined && TERMINAL.has(current)) return;
    this.statuses.set(id, status as AcpToolStatus);
  }

  /** The status Cortex last sent for this worker card, if any. */
  status(id: string): AcpToolStatus | undefined {
    return this.statuses.get(id);
  }

  /** Bridge `toolCall` transformer: a worker snapshot whose status disagrees
   * with Cortex's carries the bridge's own turn-end
   * status merged into a later update, or a stale frame after the real
   * outcome. Either way, Cortex's status wins. */
  toolCall<T extends { id: string; status: string }>(snapshot: T): T {
    if (!isWorkerCard(snapshot.id)) return snapshot;
    const authoritative = this.statuses.get(snapshot.id);
    if (authoritative === undefined) return snapshot;
    if (TERMINAL.has(authoritative)) {
      const retained = this.outcomes.get(snapshot.id);
      // Keep the outcome payload too: changing only the status would attach
      // a stale success result to a failure (or a stale error to a success).
      if (snapshot.status !== authoritative && retained) return { ...retained } as T;
      const outcome = { ...snapshot, status: authoritative };
      this.outcomes.set(snapshot.id, outcome);
      return outcome;
    }
    if (!TERMINAL.has(snapshot.status)) return snapshot;
    return { ...snapshot, status: authoritative };
  }

  /** Whether this bridge timeline item settles a worker card Cortex still
   * reports pending/in_progress. Every item from an agent update has passed
   * through `toolCall` above and agrees with Cortex; only the bridge's own
   * turn-end settlement can disagree. */
  isSynthesizedTerminal(item: { type: string; callId?: unknown; status?: unknown }): boolean {
    if (item.type !== "tool_call" || !isWorkerCard(item.callId)) return false;
    if (item.status !== "failed" && item.status !== "completed" && item.status !== "canceled") return false;
    const authoritative = this.statuses.get(item.callId);
    return authoritative === "pending" || authoritative === "in_progress";
  }
}
