import { createHash } from "node:crypto";
import type { PluginHandlerContext } from "@getpaseo/plugin/server";

/** A call at or above this duration is logged distinctly as slow, so a burst
 * of generic `cortex-acp.invoke` timeouts (the audit's 63 occurrences) can be
 * attributed to one specific RPC method and its actual duration instead of
 * treated as one undifferentiated bucket. */
export const DEFAULT_SLOW_INVOKE_THRESHOLD_MS = 3000;

function safeId(value: string): string {
  // A one-way, truncated digest: enough to correlate repeated slow calls to
  // the same session across a log window without printing the raw
  // workspace/agent identity or any message body into plugin logs.
  return createHash("sha256").update(value).digest("hex").slice(0, 12);
}

type IdentityInput = { workspaceId?: string; agentId?: string };

/** Wrap one RPC handler with duration diagnostics: the exact registered
 * method name, hashed (never raw) workspace/agent identity, and elapsed
 * time. Never logs message bodies or other input/output content. */
// Any object input: an RPC without a workspace/agent identity (for example
// owner configuration) is logged with "-" for both.
export function instrumentHandler<Input extends object, Output>(
  methodName: string,
  handler: (input: Input, context: PluginHandlerContext) => Output | Promise<Output>,
  options: { thresholdMs?: number; log?: (line: string) => void } = {},
): (input: Input, context: PluginHandlerContext) => Promise<Output> {
  const thresholdMs = options.thresholdMs ?? DEFAULT_SLOW_INVOKE_THRESHOLD_MS;
  const log = options.log ?? ((line: string) => console.warn(line));
  return async (input, context) => {
    const startedAt = Date.now();
    try {
      return await handler(input, context);
    } finally {
      const durationMs = Date.now() - startedAt;
      if (durationMs >= thresholdMs) {
        const identity = input as IdentityInput;
        const workspace = typeof identity.workspaceId === "string" && identity.workspaceId ? safeId(identity.workspaceId) : "-";
        const agent = typeof identity.agentId === "string" && identity.agentId ? safeId(identity.agentId) : "-";
        log(`[cortex-acp] slow invoke method=${methodName} workspace=${workspace} agent=${agent} durationMs=${durationMs}`);
      }
    }
  };
}
