import React from "react";
import { Text, View } from "react-native";
import { parseMarkdown, type MarkdownInline } from "../shared/markdown.ts";

function Spans({ spans, mutedColor }: { spans: MarkdownInline[]; mutedColor: string }) {
  return <>{spans.map((span, i) => <Text key={i} style={[
    span.bold ? { fontWeight: "700" as const } : undefined,
    span.italic ? { fontStyle: "italic" as const } : undefined,
    span.code ? { fontFamily: "monospace" as const, color: mutedColor } : undefined,
  ]}>{span.text}</Text>)}</>;
}

/** Renders worker/assistant prose as markdown using only React Native's
 * built-in Text/View -- the Paseo plugin SDK has no markdown component and
 * none is installed (see worker-panel.tsx TranscriptEvent). */
export function MarkdownText({ text, color, mutedColor, surfaceColor }: {
  text: string; color: string; mutedColor: string; surfaceColor: string;
}) {
  const blocks = parseMarkdown(text);
  return <View style={{ gap: 6 }}>
    {blocks.map((block, i) => {
      if (block.kind === "heading") return <Text key={i} selectable
        style={{ color, fontSize: block.level <= 2 ? 17 : 15, lineHeight: block.level <= 2 ? 24 : 21, fontWeight: "700" }}>
        <Spans spans={block.spans} mutedColor={mutedColor} />
      </Text>;
      if (block.kind === "code") return <View key={i} style={{ backgroundColor: surfaceColor, borderRadius: 6, padding: 8 }}>
        <Text selectable style={{ fontFamily: "monospace", fontSize: 12, lineHeight: 18, color }}>{block.text}</Text>
      </View>;
      if (block.kind === "list") return <View key={i} style={{ gap: 3 }}>
        {block.items.map((spans, j) => <View key={j} style={{ flexDirection: "row", gap: 6 }}>
          <Text style={{ color: mutedColor, fontSize: 15, lineHeight: 23 }}>{block.ordered ? `${j + 1}.` : "•"}</Text>
          <Text selectable style={{ color, fontSize: 15, lineHeight: 23, flex: 1 }}><Spans spans={spans} mutedColor={mutedColor} /></Text>
        </View>)}
      </View>;
      return <Text key={i} selectable style={{ color, fontSize: 15, lineHeight: 23 }}><Spans spans={block.spans} mutedColor={mutedColor} /></Text>;
    })}
  </View>;
}
