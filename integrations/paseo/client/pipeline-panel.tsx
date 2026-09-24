import { useRpc, type PluginAgentPanelProps } from "@getpaseo/plugin/client";
import { Icon, ScrollView } from "@getpaseo/plugin/client/react-native";
import { useEffect, useRef, useState } from "react";
import { Animated, Pressable, Text, View, useWindowDimensions } from "react-native";
import { readPipeline, type Pipeline } from "../shared/pipeline.js";
import { UsageMeter } from "./usage-meter.tsx";
import { WorkerResultDetail } from "./worker-result.tsx";
import { buildPipeline, type PipelineStep, type StepStatus, type Tone } from "./pipeline-model.ts";

type Theme = PluginAgentPanelProps["theme"];
type Props = Pick<PluginAgentPanelProps, "theme" | "workspaceId" | "agentId"> & {
  onOpenWorker(assignmentRef: string): void;
  /** Shown in a composer-pill popover (phones), not a sidebar tab. */
  popover?: boolean;
};

// Live tasks refresh often; settled ones only need to notice a new turn.
const LIVE_POLL_MS = 1500;
const SETTLED_POLL_MS = 8000;
const NODE = 22;

function toneColor(theme: Theme, tone: Tone): string {
  const c = theme.colors;
  return { success: c.statusSuccess, warning: c.statusWarning, danger: c.statusDanger, muted: c.foregroundMuted, accent: c.accent }[tone];
}

const STATUS_TONE: Record<StepStatus, Tone> = {
  done: "success", active: "accent", issues: "warning", failed: "danger", blocked: "warning", cancelled: "muted", pending: "muted",
};
const STATUS_ICON: Partial<Record<StepStatus, string>> = { done: "Check", issues: "CircleAlert", failed: "X", blocked: "Pause", cancelled: "Minus" };
const KIND_ICON: Record<PipelineStep["kind"], string> = { request: "MessageSquare", plan: "ListChecks", worker: "Bot", finish: "Flag" };

/** A soft breathing loop for whatever is happening right now. */
function usePulse(on: boolean) {
  const value = useRef(new Animated.Value(0)).current;
  useEffect(() => {
    if (!on) { value.setValue(0); return; }
    const loop = Animated.loop(Animated.sequence([
      Animated.timing(value, { toValue: 1, duration: 900, useNativeDriver: true }),
      Animated.timing(value, { toValue: 0, duration: 900, useNativeDriver: true }),
    ]));
    loop.start();
    return () => loop.stop();
  }, [on, value]);
  return value;
}

function StepNode({ step, index, theme }: { step: PipelineStep; index: number; theme: Theme }) {
  const color = toneColor(theme, STATUS_TONE[step.status]);
  const pulse = usePulse(step.status === "active");
  const icon = STATUS_ICON[step.status];
  if (step.status === "active") {
    return <View style={{ width: NODE, height: NODE, alignItems: "center", justifyContent: "center" }}>
      <Animated.View style={{ position: "absolute", width: NODE, height: NODE, borderRadius: NODE / 2, backgroundColor: color,
        opacity: pulse.interpolate({ inputRange: [0, 1], outputRange: [0.12, 0.32] }),
        transform: [{ scale: pulse.interpolate({ inputRange: [0, 1], outputRange: [0.85, 1.15] }) }] }} />
      <View style={{ width: 10, height: 10, borderRadius: 5, backgroundColor: color }} />
    </View>;
  }
  const filled = step.status !== "pending";
  return <View style={{ width: NODE, height: NODE, borderRadius: NODE / 2, alignItems: "center", justifyContent: "center",
    borderWidth: filled ? 0 : 1.5, borderColor: theme.colors.border,
    backgroundColor: filled ? color : theme.colors.surface0 }}>
    {icon
      ? <Icon name={icon} size={13} color={theme.colors.surface0} />
      : <Text style={{ color: theme.colors.foregroundMuted, fontSize: 11, fontWeight: "600" }}>{index + 1}</Text>}
  </View>;
}

function Badge({ text, tone, theme }: { text: string; tone: Tone; theme: Theme }) {
  const color = toneColor(theme, tone);
  return <View style={{ borderWidth: 1, borderColor: color, borderRadius: 999, paddingHorizontal: 7, paddingVertical: 1 }}>
    <Text style={{ color, fontSize: 11, fontWeight: "600" }}>{text}</Text>
  </View>;
}

function Step({ step, index, last, nextDone, theme, onOpenWorker }: {
  step: PipelineStep; index: number; last: boolean; nextDone: boolean; theme: Theme; onOpenWorker(ref: string): void;
}) {
  // Pipeline detail is opt-in for every state; the list never opens cards itself.
  const [expanded, setExpanded] = useState(false);
  const muted = step.status === "pending" || step.status === "cancelled";
  const hasDetail = Boolean(step.body || step.items.length || step.followUps?.length);
  const railColor = step.status === "done" && nextDone ? theme.colors.statusSuccess : theme.colors.border;
  return <View style={{ flexDirection: "row", gap: 12 }}>
    <View style={{ alignItems: "center", width: NODE }}>
      <StepNode step={step} index={index} theme={theme} />
      {!last && <View style={{ flex: 1, width: 2, minHeight: 14, marginVertical: 3, borderRadius: 1, backgroundColor: railColor }} />}
    </View>
    <View style={{ flex: 1, minWidth: 0, paddingBottom: last ? 4 : 18 }}>
      <Pressable
        accessibilityRole="button"
        accessibilityLabel={`${step.title}, ${step.status}`}
        disabled={!hasDetail}
        onPress={() => setExpanded((value) => !value)}
        style={{ minHeight: NODE, justifyContent: "center", gap: 2 }}
      >
        <View style={{ flexDirection: "row", alignItems: "center", gap: 6 }}>
          <Icon name={KIND_ICON[step.kind]} size={13} color={theme.colors.foregroundMuted} />
          <Text numberOfLines={expanded ? undefined : 2} style={{ flex: 1, color: muted ? theme.colors.foregroundMuted : theme.colors.foreground,
            fontSize: 14, fontWeight: step.status === "active" ? "700" : "600", textDecorationLine: step.status === "cancelled" ? "line-through" : "none" }}>
            {step.title}
          </Text>
          {hasDetail && <Icon name={expanded ? "ChevronUp" : "ChevronDown"} size={14} color={theme.colors.foregroundMuted} />}
        </View>
        {step.subtitle ? <Text numberOfLines={1} style={{ color: theme.colors.foregroundMuted, fontSize: 12 }}>{step.subtitle}</Text> : null}
      </Pressable>
      {step.usage && <View style={{ marginTop: 4 }}><UsageMeter context={step.usage.context} tokens={step.usage.tokens} theme={theme} /></View>}
      {step.badges.length > 0 && <View style={{ flexDirection: "row", flexWrap: "wrap", gap: 5, marginTop: 5 }}>
        {step.badges.map((badge) => <Badge key={badge.text} {...badge} theme={theme} />)}
      </View>}
      {expanded && hasDetail && <View style={{ marginTop: 8, gap: 8, borderWidth: 1, borderColor: theme.colors.border, borderRadius: 8,
        backgroundColor: theme.colors.surface1, padding: 10 }}>
        <WorkerResultDetail body={step.body} items={step.items} theme={theme} />
        {step.followUps?.map((text, i) => <View key={i} style={{ borderLeftWidth: 2, borderColor: theme.colors.accent, paddingLeft: 8 }}>
          <Text style={{ color: theme.colors.foregroundMuted, fontSize: 11, fontWeight: "600", marginBottom: 2 }}>Follow-up {i + 1}</Text>
          <Text selectable style={{ color: theme.colors.foreground, fontSize: 12, lineHeight: 18 }}>{text}</Text>
        </View>)}
      </View>}
      {step.assignmentRef && <Pressable accessibilityRole="link" onPress={() => onOpenWorker(step.assignmentRef!)} style={{ alignSelf: "flex-start", marginTop: 6 }}>
        <Text style={{ color: theme.colors.accent, fontSize: 12, fontWeight: "600" }}>Open worker ›</Text>
      </Pressable>}
    </View>
  </View>;
}

export function PipelinePanel({ theme, workspaceId, agentId, onOpenWorker, popover = false }: Props) {
  const rpc = useRpc(readPipeline);
  const { height } = useWindowDimensions();
  const frame = popover
    ? { flexGrow: 0, maxHeight: Math.round(height * 0.7), marginHorizontal: -12, backgroundColor: theme.colors.surface1 }
    : { flex: 1, backgroundColor: theme.colors.surface0 };
  const [pipeline, setPipeline] = useState<Pipeline | null>(null);
  const [failed, setFailed] = useState(false);

  useEffect(() => {
    let active = true;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const refresh = async () => {
      let live = true;
      try {
        const result = await rpc({ workspaceId, agentId });
        if (!active) return;
        setPipeline(result);
        setFailed(false);
        live = result.available && buildPipeline(result).headline.live;
      } catch {
        if (active) setFailed(true);
      }
      if (active) timer = setTimeout(refresh, live ? LIVE_POLL_MS : SETTLED_POLL_MS);
    };
    void refresh();
    return () => { active = false; clearTimeout(timer); };
  }, [agentId, rpc, workspaceId]);

  const c = theme.colors;
  if (!pipeline || !pipeline.available) {
    const text = failed ? "The pipeline could not be read." : pipeline?.message ?? "Loading pipeline…";
    return <View style={[frame, { padding: 16 }]}>
      <Text style={{ color: pipeline || failed ? c.statusWarning : c.foregroundMuted, fontSize: 13 }}>{text}</Text>
    </View>;
  }

  const view = buildPipeline(pipeline);
  const headColor = toneColor(theme, view.headline.tone);
  const progress = view.total ? view.done / view.total : 0;
  return <ScrollView style={frame} contentContainerStyle={{ padding: 14, gap: 16 }}>
    <View style={{ gap: 8 }}>
      <View style={{ flexDirection: "row", alignItems: "center", gap: 8 }}>
        <View style={{ width: 8, height: 8, borderRadius: 4, backgroundColor: headColor }} />
        <Text style={{ flex: 1, color: c.foreground, fontSize: 15, fontWeight: "700" }}>{view.headline.text}</Text>
        <Text style={{ color: c.foregroundMuted, fontSize: 12 }}>{view.done} of {view.total}</Text>
      </View>
      <View style={{ height: 4, borderRadius: 2, backgroundColor: c.surface2, overflow: "hidden" }}>
        <View style={{ width: `${Math.round(progress * 100)}%`, height: 4, borderRadius: 2, backgroundColor: headColor }} />
      </View>
      {pipeline.tokens.total && <View style={{ gap: 2 }}>
        <View style={{ flexDirection: "row", alignItems: "center", gap: 6 }}>
          <Text style={{ color: c.foregroundMuted, fontSize: 11, fontWeight: "600" }}>Task tokens</Text>
          <UsageMeter tokens={pipeline.tokens.total} theme={theme} />
        </View>
        {pipeline.tokens.coordinator && <View style={{ flexDirection: "row", alignItems: "center", gap: 6 }}>
          <Text style={{ color: c.foregroundMuted, fontSize: 11, fontWeight: "600" }}>Coordinator</Text>
          <UsageMeter tokens={pipeline.tokens.coordinator} theme={theme} />
        </View>}
      </View>}
    </View>
    {failed && <Text style={{ color: c.statusWarning, fontSize: 12 }}>Showing the last read; refreshing failed.</Text>}
    <View>
      {view.steps.map((step, index) => <Step
        key={step.key}
        step={step}
        index={index}
        last={index === view.steps.length - 1}
        nextDone={view.steps[index + 1]?.status === "done"}
        theme={theme}
        onOpenWorker={onOpenWorker}
      />)}
    </View>
  </ScrollView>;
}
