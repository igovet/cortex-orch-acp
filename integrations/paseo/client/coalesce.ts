/**
 * Run an async refresh without ever dropping a request.
 *
 * Paseo's workspace/agent subscriptions fire several times within the
 * window one refresh takes. Returning early while one is in flight lost
 * the notification that carried a just-created agent, so its composer
 * pills (Workers, Pipeline) never appeared for that session at all.
 * A request that arrives during a run is remembered and runs exactly once
 * afterwards, however many arrived.
 */
export function coalesced(run: () => Promise<void>): () => Promise<void> {
  let inFlight = false;
  let queued = false;
  const call = async (): Promise<void> => {
    if (inFlight) {
      queued = true;
      return;
    }
    inFlight = true;
    try {
      await run();
    } finally {
      inFlight = false;
    }
    if (queued) {
      queued = false;
      await call();
    }
  };
  return call;
}
