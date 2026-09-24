import type { AccountUsage } from "./account-usage.js";

export function unavailableMessage(engine: "codex-acp" | "claude-agent-acp",
  usage: AccountUsage | null, connectionCount: number): string {
  if (usage === null) return "Loading…";
  if (!usage.available) return "Usage reader failed";
  if (connectionCount === 0) return "No configured connection";
  return engine === "claude-agent-acp"
    ? "No Claude rate-limit event observed"
    : "No current native plan window reported";
}
