import { useRpc, type PluginHostProps } from "@getpaseo/plugin/client";
import { useEffect, useState } from "react";
import { Text, View } from "react-native";
import { readWorkerHistory, type WorkerHistory } from "../shared/worker-history.js";
import { contextSummary, isSettledWorkerStatus, tokensAccessibilityLabel, tokensSummaryLabel } from "./worker-display.ts";
import { cardUsageSnapshot, sameCardUsage, type CardUsage } from "./worker-card-model.ts";

type Attempt = WorkerHistory["assignments"][number]["attempts"][number];
type Theme = PluginHostProps["theme"];

/** Context fill as a short bar plus percent, then Claude Code's `↑ in ↓ out`
 * token counts. Callers can explain missing context independently of tokens. */
export function UsageMeter({ context, tokens, theme, unavailableText, mutedColor }: { context?: Attempt["context"]; tokens?: Attempt["tokens"] | null; theme: Theme; unavailableText?: string;
  /** The caller's own readable secondary text colour (the coordinator card
   * derives one that holds 4.5:1 on its hover background). */
  mutedColor?: string }) {
  const summary = contextSummary(context);
  const mutedText = mutedColor ?? theme.colors.foregroundMuted;
  // Missing context stays explicit even when token totals are known.
  if (!summary && !tokens && !unavailableText) return null;
  const color = summary?.level === "danger" ? theme.colors.statusDanger : summary?.level === "warning" ? theme.colors.statusWarning : theme.colors.accent;
  const muted = { color: mutedText, fontSize: 11 };
  return <View style={{ flexDirection: "row", alignItems: "center", gap: 8, flexWrap: "wrap" }}>
    {!summary && unavailableText && <Text style={{ ...muted, flexShrink: 1 }}>{unavailableText}</Text>}
    {summary && <View accessibilityLabel={`Context ${summary.percent}%, ${summary.detail} tokens`} style={{ flexDirection: "row", alignItems: "center", gap: 5 }}>
      <View style={{ width: 32, height: 4, borderRadius: 2, backgroundColor: theme.colors.surface2, overflow: "hidden" }}>
        <View style={{ width: `${summary.percent}%`, height: 4, backgroundColor: color }} />
      </View>
      <Text style={muted}>Context {summary.percent}% · {summary.detail}</Text>
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
 * once more when it settles. The cursor keeps its transcript out of the read
 * and `compact` keeps its summary and check list out (an open-issue count
 * instead); the card keeps only that snapshot, and an unchanged poll keeps
 * the same state object so it does not re-render the card. `loaded` is set
 * after the first answered read. */
export function useWorkerUsage(workspaceId: string | null, agentId: string, assignmentRef: string, live: boolean): { usage: CardUsage | undefined; loaded: boolean } {
  const rpc = useRpc(readWorkerHistory);
  const [attempt, setAttempt] = useState<CardUsage>();
  const [loaded, setLoaded] = useState(false);
  useEffect(() => {
    if (!workspaceId) return;
    let active = true;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const load = async () => {
      let settled = false;
      try {
        const history = await rpc({ workspaceId, agentId, assignmentRef, cursor: Number.MAX_SAFE_INTEGER, limit: 1, compact: true });
        const assignment = history.assignments[0];
        const latest = assignment && latestAttempt(assignment);
        if (active && latest) {
          const next = cardUsageSnapshot(latest);
          setAttempt((current) => sameCardUsage(current, next) ? current : next);
        }
        if (active && history.available) setLoaded(true);
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
  return { usage: attempt, loaded };
}
