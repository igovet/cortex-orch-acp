/**
 * Share one poll per (rpc, key) across every concurrently mounted caller,
 * instead of each caller independently invoking on its own timer.
 *
 * `getPipeline` is independently polled by more than one mounted component
 * for the same `(workspaceId, agentId)` -- `PipelinePanel` is mounted both as
 * the composer popover content and as the agent panel tab at once. Without
 * sharing, each mount adds its own periodic invoke into the same server
 * queue. This
 * mirrors `worker-result-batch.ts`'s "coalesce every caller into one RPC"
 * shape, generalized to polling rather than a one-shot batch window.
 */
type Listener<Output> = (result: Output | null) => void;

type Group<Input, Output> = {
  input: Input;
  listeners: Map<Listener<Output>, number>;
  timer: ReturnType<typeof setTimeout> | null;
  inFlight: boolean;
  pendingRerun: boolean;
};

/** Shared poll handle returned by `createSharedPoller`. */
export type SharedPoller<Input, Output> = {
  /** Subscribe `onResult` to the shared poll for `input`'s key, at no slower
   * than `intervalMs` (the fastest of every current subscriber wins). Fires
   * one immediate refresh for a new group; a group with an in-flight or
   * already-scheduled refresh does not issue a redundant one. Returns an
   * unsubscribe that stops the shared timer once the last listener leaves. */
  subscribe(input: Input, intervalMs: number, onResult: Listener<Output>): () => void;
};

/** A closure, not a `class`: Paseo evaluates plugin client bundles from
 * source on the phone's Hermes, which compiles a class expression to
 * `undefined` (so `new SharedPoller(...)` threw there). */
export function createSharedPoller<Input, Output>(
  rpc: (input: Input) => Promise<Output>, keyOf: (input: Input) => string,
): SharedPoller<Input, Output> {
  const groups = new Map<string, Group<Input, Output>>();

  function rescheduleTimer(key: string): void {
    const group = groups.get(key);
    if (!group || group.listeners.size === 0) return;
    const fastestIntervalMs = Math.min(...group.listeners.values());
    if (group.timer) clearTimeout(group.timer);
    group.timer = setTimeout(() => { void refresh(key); }, fastestIntervalMs);
  }

  async function refresh(key: string): Promise<void> {
    const group = groups.get(key);
    if (!group) return;
    if (group.inFlight) {
      group.pendingRerun = true;
      return;
    }
    group.inFlight = true;
    try {
      const result = await rpc(group.input);
      const current = groups.get(key);
      if (!current) return;
      for (const listener of current.listeners.keys()) listener(result);
    } catch {
      const current = groups.get(key);
      if (current) for (const listener of current.listeners.keys()) listener(null);
    } finally {
      const current = groups.get(key);
      if (current) {
        current.inFlight = false;
        const rerun = current.pendingRerun;
        current.pendingRerun = false;
        if (current.listeners.size > 0) rescheduleTimer(key);
        if (rerun) void refresh(key);
      }
    }
  }

  return {
    subscribe(input, intervalMs, onResult) {
      const key = keyOf(input);
      let group = groups.get(key);
      const isNewGroup = !group;
      if (!group) {
        group = { input, listeners: new Map(), timer: null, inFlight: false, pendingRerun: false };
        groups.set(key, group);
      }
      group.input = input;
      group.listeners.set(onResult, intervalMs);
      rescheduleTimer(key);
      if (isNewGroup) void refresh(key);
      return () => {
        const current = groups.get(key);
        if (!current) return;
        current.listeners.delete(onResult);
        if (current.listeners.size === 0) {
          if (current.timer) clearTimeout(current.timer);
          groups.delete(key);
        } else {
          rescheduleTimer(key);
        }
      };
    },
  };
}
