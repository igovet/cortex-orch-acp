import { workerResult, type Pipeline } from "./pipeline.ts";

export type WorkerResult = Pipeline["delegations"][string]["result"];

/** The runtime's embedded `{summary, checks}` projection, validated the
 * same way whether it arrived as a worker card's raw ACP output (server
 * side) or as `item.metadata.cortexWorkerResult` (client side). Anything
 * that does not match is treated as absent, never guessed at. */
export function parseWorkerResult(value: unknown): WorkerResult | undefined {
  const parsed = workerResult.safeParse(value);
  return parsed.success ? parsed.data : undefined;
}
