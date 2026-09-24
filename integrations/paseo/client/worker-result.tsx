import { Icon } from "@getpaseo/plugin/client/react-native";
import { Text, View } from "react-native";
import type { StepItem } from "./pipeline-model.ts";

type Theme = { colors: {
  foreground: string;
  foregroundMuted: string;
  statusSuccess: string;
  statusDanger: string;
} };

/** Shared, always-visible worker result body for timeline cards and Pipeline details. */
export function WorkerResultDetail({ body, items, theme }: { body?: string; items: StepItem[]; theme: Theme }) {
  if (!body && items.length === 0) return null;
  return <View style={{ gap: 8 }}>
    {body ? <Text selectable style={{ color: theme.colors.foreground, fontSize: 12, lineHeight: 18 }}>{body}</Text> : null}
    {items.length > 0 && <View style={{ gap: 5, marginTop: 2 }}>
      {items.map((item, index) => {
        const color = item.status === "done" ? theme.colors.statusSuccess : item.status === "failed" ? theme.colors.statusDanger : theme.colors.foregroundMuted;
        return <View key={index} style={{ flexDirection: "row", gap: 7, alignItems: "flex-start" }}>
          <View style={{ marginTop: 2 }}>
            <Icon name={item.status === "done" ? "CircleCheck" : item.status === "failed" ? "CircleX" : "Circle"} size={13} color={color} />
          </View>
          <View style={{ flex: 1, gap: 2 }}>
            <Text style={{ color: item.status === "open" ? theme.colors.foregroundMuted : theme.colors.foreground, fontSize: 12, lineHeight: 17 }}>{item.text}</Text>
            {item.blocker ? <Text style={{ color: theme.colors.foregroundMuted, fontSize: 11, lineHeight: 15 }}>{item.blocker}</Text> : null}
          </View>
        </View>;
      })}
    </View>}
  </View>;
}
