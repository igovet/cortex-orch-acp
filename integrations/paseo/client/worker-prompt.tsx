import { Icon, ScrollView } from "@getpaseo/plugin/client/react-native";
import { Pressable, Text, View } from "react-native";
import { useState } from "react";
import { MarkdownText } from "./markdown-text.tsx";
import { promptView, type AttemptPrompt, type PromptSection } from "./worker-prompt-model.ts";

type Theme = { colors: {
  foreground: string;
  foregroundMuted: string;
  statusWarning: string;
  surface1: string;
  border: string;
  accent: string;
} };

function Section({ section, theme }: { section: PromptSection; theme: Theme }) {
  const heading = { color: theme.colors.foregroundMuted, fontSize: 12, lineHeight: 18, fontWeight: "600" as const };
  return <View style={{ gap: 4 }}>
    <Text accessibilityRole="header" style={heading}>{section.heading}</Text>
    {section.kind === "markdown" && <MarkdownText text={section.text} color={theme.colors.foreground} mutedColor={theme.colors.foregroundMuted} surfaceColor={theme.colors.surface1} />}
    {section.kind === "list" && section.items.map((item, index) => <Text key={index} selectable style={{ color: theme.colors.foreground, fontFamily: "monospace", fontSize: 12, lineHeight: 18 }}>{item}</Text>)}
    {section.kind === "code" && <ScrollView horizontal nestedScrollEnabled showsHorizontalScrollIndicator>
      <Text selectable style={{ color: theme.colors.foreground, fontFamily: "monospace", fontSize: 12, lineHeight: 18 }}>{section.text}</Text>
    </ScrollView>}
  </View>;
}

/** The selected attempt's assignment prompt: the task text the runtime sent
 * to this exact attempt, collapsed to its state and a one-line preview until
 * the reader opens it. Nothing when the read carried no prompt data. The
 * parent keys it by attempt, so switching attempts starts collapsed. */
export function WorkerAttemptPrompt({ prompt, theme }: { prompt: AttemptPrompt | undefined; theme: Theme }) {
  const [expanded, setExpanded] = useState(false);
  const view = promptView(prompt);
  if (!view) return null;
  const note = { color: view.tone === "warning" ? theme.colors.statusWarning : theme.colors.foregroundMuted, fontSize: 13, lineHeight: 19 };
  const canExpand = view.sections !== null && view.sections.length > 0;
  return <View style={{ gap: 8, paddingBottom: 14, borderBottomWidth: 1, borderColor: theme.colors.border }}>
    <View style={{ flexDirection: "row", flexWrap: "wrap", alignItems: "center", justifyContent: "space-between", columnGap: 8, rowGap: 4 }}>
      <Text accessibilityRole="header" style={{ color: theme.colors.foregroundMuted, fontSize: 12, lineHeight: 18, fontWeight: "600", letterSpacing: 0.3 }}>Assignment prompt</Text>
      {canExpand && <Pressable accessibilityRole="button" accessibilityLabel={expanded ? "Hide assignment prompt" : "Show assignment prompt"}
        accessibilityState={{ expanded }} onPress={() => setExpanded((value) => !value)}
        style={{ flexDirection: "row", alignItems: "center", gap: 2, paddingVertical: 2 }}>
        <Text style={{ color: theme.colors.accent, fontSize: 12, lineHeight: 18, fontWeight: "600" }}>{expanded ? "Hide" : "Show"}</Text>
        <View style={{ transform: [{ rotate: expanded ? "270deg" : "90deg" }] }}><Icon name="ChevronRight" size={12} color={theme.colors.accent} /></View>
      </Pressable>}
    </View>
    <Text style={note}>{view.status}</Text>
    {canExpand && !expanded && view.preview && <Text numberOfLines={2} ellipsizeMode="tail" style={{ color: theme.colors.foreground, fontSize: 14, lineHeight: 20 }}>{view.preview}</Text>}
    {canExpand && expanded && <View style={{ gap: 12 }}>
      {view.sections!.map((section, index) => <Section key={`${index}-${section.heading}`} section={section} theme={theme} />)}
      {view.truncatedNote && <Text style={{ color: theme.colors.foregroundMuted, fontSize: 13, lineHeight: 19 }}>{view.truncatedNote}</Text>}
    </View>}
  </View>;
}
