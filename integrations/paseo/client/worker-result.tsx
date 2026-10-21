import { Icon } from "@getpaseo/plugin/client/react-native";
import { Text, View } from "react-native";
import type { StepItem } from "./pipeline-model.ts";
import { MarkdownText } from "./markdown-text.tsx";
import { attemptResultView, type AttemptResultInput } from "./worker-attempt-result.ts";

type Theme = { colors: {
  foreground: string;
  foregroundMuted: string;
  statusSuccess: string;
  statusDanger: string;
  statusWarning: string;
  surface1: string;
  border: string;
} };

/** The selected attempt's terminal Result and Checks in the worker tab: the
 * complete retained summary and every check with its true disposition,
 * resolved history, weak-evidence warning, blocker and evidence gap. The
 * coordinator card never renders any of this. Nothing for an active attempt
 * or one that ended without a result. */
export function WorkerAttemptResult({ attempt, theme }: { attempt: AttemptResultInput; theme: Theme }) {
  const view = attemptResultView(attempt);
  if (!view) return null;
  const heading = { color: theme.colors.foregroundMuted, fontSize: 12, lineHeight: 18, fontWeight: "600" as const, letterSpacing: 0.3 };
  const note = { color: theme.colors.foregroundMuted, fontSize: 13, lineHeight: 19 };
  const toneColor = (tone: string) => tone === "success" ? theme.colors.statusSuccess
    : tone === "danger" ? theme.colors.statusDanger
    : tone === "warning" ? theme.colors.statusWarning
    : theme.colors.foregroundMuted;
  return <View style={{ gap: 14, paddingBottom: 14, borderBottomWidth: 1, borderColor: theme.colors.border }}>
    <View accessibilityRole="summary" style={{ gap: 6 }}>
      <Text accessibilityRole="header" style={heading}>Result</Text>
      {view.summary
        ? <MarkdownText text={view.summary} color={theme.colors.foreground} mutedColor={theme.colors.foregroundMuted} surfaceColor={theme.colors.surface1} />
        : <Text style={note}>No result summary reported.</Text>}
      {view.summaryTruncated && <Text style={note}>Summary clipped for display; the worker's published report keeps the full text.</Text>}
    </View>
    <View style={{ gap: 8 }}>
      <Text accessibilityRole="header" style={heading}>{view.checks ? `Checks · ${view.checks.length}${view.openCount ? ` · ${view.openCount} open` : ""}` : "Checks"}</Text>
      {!view.checks && <Text style={note}>Check details unavailable.</Text>}
      {view.checks?.length === 0 && <Text style={note}>No checks reported.</Text>}
      {view.checks?.map((check, index) => <View key={`${index}-${check.name}`} accessibilityLabel={check.accessibilityLabel}
        style={{ flexDirection: "row", gap: 8, alignItems: "flex-start" }}>
        <View style={{ marginTop: 3 }}><Icon name={check.icon} size={13} color={toneColor(check.tone)} /></View>
        <View style={{ flex: 1, minWidth: 0, gap: 2 }}>
          <View style={{ flexDirection: "row", flexWrap: "wrap", alignItems: "center", columnGap: 8, rowGap: 2 }}>
            <Text selectable style={{ color: theme.colors.foreground, fontSize: 13, lineHeight: 19, flexShrink: 1 }}>{check.name}</Text>
            <Text style={{ color: toneColor(check.tone), fontSize: 11, lineHeight: 16, fontWeight: "600" }}>{check.label}</Text>
            {check.weakEvidence && <Text style={{ color: theme.colors.statusWarning, fontSize: 11, lineHeight: 16, fontWeight: "600" }}>WEAK EVIDENCE</Text>}
          </View>
          {check.details.map((detail, detailIndex) => <Text key={detailIndex} selectable style={{ color: theme.colors.foregroundMuted, fontSize: 12, lineHeight: 17 }}>{detail}</Text>)}
        </View>
      </View>)}
    </View>
  </View>;
}

/** Shared, always-visible worker result body for Pipeline details. */
export function WorkerResultDetail({ body, items, theme }: { body?: string; items: StepItem[]; theme: Theme }) {
  if (!body && items.length === 0) return null;
  return <View style={{ gap: 8 }}>
    {body ? <Text selectable style={{ color: theme.colors.foreground, fontSize: 12, lineHeight: 18 }}>{body}</Text> : null}
    {items.length > 0 && <View style={{ gap: 5, marginTop: 2 }}>
      {items.map((item, index) => {
        // "resolved": a failed check an exact later rerun closed -- kept
        // visible as history, distinct from an open failure or a pass.
        const color = item.status === "done" ? theme.colors.statusSuccess
          : item.status === "failed" ? theme.colors.statusDanger
          : item.status === "resolved" ? theme.colors.foregroundMuted
          : theme.colors.foregroundMuted;
        const icon = item.status === "done" ? "CircleCheck"
          : item.status === "failed" ? "CircleX"
          : item.status === "resolved" ? "History"
          : "Circle";
        return <View key={index} style={{ flexDirection: "row", gap: 7, alignItems: "flex-start" }}>
          <View style={{ marginTop: 2 }}>
            <Icon name={icon} size={13} color={color} />
          </View>
          <View style={{ flex: 1, gap: 2 }}>
            <View style={{ flexDirection: "row", alignItems: "center", gap: 6 }}>
              <Text style={{ color: item.status === "open" || item.status === "resolved" ? theme.colors.foregroundMuted : theme.colors.foreground, fontSize: 12, lineHeight: 17 }}>{item.text}</Text>
              {item.status === "resolved" && <Text style={{ color: theme.colors.foregroundMuted, fontSize: 10, fontWeight: "600" }}>RESOLVED</Text>}
              {item.weakEvidence && <Text style={{ color: theme.colors.statusWarning, fontSize: 10, fontWeight: "600" }}>WEAK EVIDENCE</Text>}
            </View>
            {item.blocker ? <Text style={{ color: theme.colors.foregroundMuted, fontSize: 11, lineHeight: 15 }}>{item.blocker}</Text> : null}
          </View>
        </View>;
      })}
    </View>}
  </View>;
}
