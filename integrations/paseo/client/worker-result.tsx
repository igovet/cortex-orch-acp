import { Icon } from "@getpaseo/plugin/client/react-native";
import { Text, View } from "react-native";
import type { StepItem } from "./pipeline-model.ts";

type Theme = { colors: {
  foreground: string;
  foregroundMuted: string;
  statusSuccess: string;
  statusDanger: string;
  statusWarning: string;
} };

/** Shared, always-visible worker result body for timeline cards and Pipeline details. */
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
