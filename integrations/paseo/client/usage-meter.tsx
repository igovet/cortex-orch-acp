import { useRpc, type PluginHostProps } from "@getpaseo/plugin/client";
import { useEffect, useState } from "react";
import { Text, View } from "react-native";
import { readWorkerHistory, type WorkerHistory } from "../shared/worker-history.js";
import { contextSummary, formatTokens, isSettledWorkerStatus, tokensAccessibilityLabel, tokensSummaryLabel } from "./worker-display.ts";

type Attempt = WorkerHistory["assignments"][number]["attempts"][number];
type Theme = PluginHostProps["theme"];

/** Context fill as a short bar plus percent, then Claude Code's `↑ in ↓ out`
 * token counts. Renders nothing until the engine has reported either. */
export function UsageMeter({ context, tokens, theme }: { context?: Attempt["context"]; tokens?: Attempt["tokens"] | null; theme: Theme }) {
  const summary = contextSummary(context);
  if (!summary && !tokens) return null;
  const color = summary?.level === "danger" ? theme.colors.statusDanger : summary?.level === "warning" ? theme.colors.statusWarning : theme.colors.accent;
  const muted = { color: theme.colors.foregroundMuted, fontSize: 11 };
  return <View style={{ flexDirection: "row", alignItems: "center", gap: 8, flexWrap: "wrap" }}>
    {summary && <View accessibilityLabel={`Context ${summary.percent}%, ${summary.detail} tokens`} style={{ flexDirection: "row", alignItems: "center", gap: 5 }}>
      <View style={{ width: 32, height: 4, borderRadius: 2, backgroundColor: theme.colors.surface2, overflow: "hidden" }}>
        <View style={{ width: `${summary.percent}%`, height: 4, backgroundColor: color }} />
      </View>
      <Text style={muted}>{summary.percent}% · {summary.detail}</Text>
    </View>}
    {tokens && <Text accessibilityLabel={tokensAccessibilityLabel(tokens) ?? undefined} style={muted}>
      {tokensSummaryLabel(tokens)}
    </Text>}
  </View>;
}

/** The latest attempt of a worker, whose usage the views show. */
export function latestAttempt(assignment: WorkerHistory["assignments"][number]): Attempt | undefined {
  return assignment.attempts[assignment.attempts.length - 1];
}

const CARD_USAGE_POLL_MS = 3000;

/** One worker's latest attempt for its chat card: polled while it runs, read
 * once more when it settles. The cursor keeps its transcript out of the read. */
export function useWorkerUsage(workspaceId: string | null, agentId: string, assignmentRef: string, live: boolean) {
  const rpc = useRpc(readWorkerHistory);
  const [attempt, setAttempt] = useState<Attempt>();
  useEffect(() => {
    if (!workspaceId) return;
    let active = true;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const load = async () => {
      let settled = false;
      try {
        const history = await rpc({ workspaceId, agentId, assignmentRef, cursor: Number.MAX_SAFE_INTEGER, limit: 1 });
        const assignment = history.assignments[0];
        const latest = assignment && latestAttempt(assignment);
        if (active && latest) setAttempt(latest);
        // A settled attempt never runs again (a retry is a new card), so a
        // card the host still shows running stops polling once it settles.
        settled = isSettledWorkerStatus(latest?.status);
      } catch {
        // Usage is decoration; a failed read leaves the card as it was.
      }
      if (active && live && !settled) timer = setTimeout(load, CARD_USAGE_POLL_MS);
    };
    void load();
    return () => { active = false; clearTimeout(timer); };
  }, [agentId, assignmentRef, live, rpc, workspaceId]);
  return attempt;
}
