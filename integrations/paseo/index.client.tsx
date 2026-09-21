import { isHiddenOperation } from "./shared/timeline-visibility.ts";
import { useAgent, type PluginClientContext, type PluginTimelineItemProps } from "@getpaseo/plugin/client";
import { Icon } from "@getpaseo/plugin/client/react-native";
import { useEffect, useRef } from "react";
import { Animated, Pressable, Text, View } from "react-native";
import { z } from "zod";
import { WorkerPanel } from "./client/worker-panel.js";
import { selectWorker } from "./client/worker-selection.js";
import { WorkerPopover } from "./client/worker-popover.js";
import { isWorkerTimelineRunning } from "./client/worker-timeline-status.js";
import { workerCardMetadata } from "./client/worker-display.ts";

const workerTimelineSchema = z.object({
  callId: z.string(),
  assignmentRef: z.string(),
  title: z.string(),
  // Paseo 0.8 projects ACP tool calls into this host-owned status set
  // before running timeline transformers.  Keep the plugin data schema
  // aligned with that exact projected shape so the renderer is mounted
  // instead of falling back to "Plugin timeline item unavailable.".
  status: z.enum(["running", "completed", "canceled", "failed"]),
});

type WorkerTimelineData = z.output<typeof workerTimelineSchema>;

function WorkerTimelineRow({ theme, layout, agentId, item, workspaceId, onOpen }: PluginTimelineItemProps<z.output<typeof workerTimelineSchema>> & { workspaceId: string | null; onOpen: () => void }) {
  const metadata = workerCardMetadata(item.data.title, item.data.status);
  const opacity = useRef(new Animated.Value(1)).current;
  const isWorking = isWorkerTimelineRunning(item.data.status);

  useEffect(() => {
    if (!isWorking) {
      opacity.stopAnimation();
      opacity.setValue(1);
      return;
    }

    const activity = Animated.loop(
      Animated.sequence([
        Animated.timing(opacity, { toValue: 0.58, duration: 850, useNativeDriver: true }),
        Animated.timing(opacity, { toValue: 1, duration: 850, useNativeDriver: true }),
      ]),
    );
    activity.start();
    return () => {
      activity.stop();
      opacity.stopAnimation();
      opacity.setValue(1);
    };
  }, [isWorking, opacity]);

  // A worker is its own block in the coordinator's timeline, never a
  // plain row between tool calls: bordered card, labelled, opens its panel.
  return (
    <Pressable
      accessibilityRole="button"
      accessibilityLabel={`Open worker ${metadata.workerName}`}
      onPress={() => workspaceId && (selectWorker(agentId, item.data.assignmentRef), onOpen())}
      style={{ marginVertical: layout.compact ? 6 : 8, borderWidth: 1, borderColor: theme.colors.border, borderRadius: 8,
        backgroundColor: theme.colors.surface1, paddingHorizontal: 12, paddingVertical: 10, gap: 6 }}
    >
      <Animated.View style={{ opacity, gap: 6 }}>
        <View style={{ flexDirection: "row", alignItems: "center", gap: 6 }}>
          <Icon name="Bot" size={12} color={theme.colors.foregroundMuted} />
          <Text style={{ flex: 1, color: theme.colors.foregroundMuted, fontSize: 11, fontWeight: "600", letterSpacing: 0.6, textTransform: "uppercase" }}>Worker</Text>
          <Text style={{ color: theme.colors.foregroundMuted, fontSize: 12 }}>Open ›</Text>
        </View>
        <Text numberOfLines={2} ellipsizeMode="tail" style={{ color: theme.colors.foreground, fontSize: 14, fontWeight: "600" }}>{metadata.workerName}</Text>
        <Text numberOfLines={1} ellipsizeMode="tail" style={{ color: theme.colors.foregroundMuted, fontSize: 12 }}>{metadata.subtitle}</Text>
      </Animated.View>
    </Pressable>
  );
}


export default function contribute(client: PluginClientContext) {
  client.addTimelineTransformer({
    id: "hide-thinking", query: { itemType: "reasoning" },
    transform: () => ({ items: [] }),
  });
  client.addTimelineTransformer({
    id: "worker-tool-call-row",
    query: { itemType: "tool_call" },
    transform({ item }) {
      if (isHiddenOperation(item.name)) return { items: [] };
      // Coordinator calls stay native tool_call items. Paseo's overview
      // mode groups a run of native calls into one expandable row before any
      // transformer runs and hands the transformer only the run's latest
      // call; a plugin row here would replace the whole run with that call.
      // The server projection already gives them readable names and native
      // read/edit/shell detail.
      if (!item.callId.startsWith("worker:")) return undefined;
      return {
        items: [{
          type: "plugin",
          // A worker lifecycle is one stable ACP tool call.  The bridge
          // sends each lifecycle change as a new host timeline record, so
          // the plugin item must carry the stable call ID explicitly.  If it
          // is omitted, Paseo derives an ID from each host record and shows
          // duplicate worker rows after a status update or cancellation.
          id: `worker-row:${item.callId}`,
          kind: "worker-row",
          version: 1,
          data: { callId: item.callId, assignmentRef: item.callId.slice("worker:".length), title: item.name, status: item.status },
        }],
      };
    },
  });
  client.addTimelineRenderer({
    kind: "worker-row",
    version: 1,
    schema: workerTimelineSchema,
    Component: (props) => {
      const workspaceId = useAgent(props.agentId, (agent) => agent.workspaceId);
      return <WorkerTimelineRow {...props} workspaceId={workspaceId} onOpen={() => {
        if (workspaceId) client.openPanel("workers", { workspaceId, agentId: props.agentId, location: "workspace" });
      }} />;
    },
  });
  client.addWorkspacePanel({
    id: "workers",
    title: "Workers",
    icon: "Users",
    context: "agent",
    // Use Paseo's supported workspace location so a worker opens as a normal
    // editor tab, matching the native Claude subagent flow.  Keep the agent
    // context: the panel still resolves the exact worker selected by the
    // coordinator card and retains its read-only transcript.
    locations: ["workspace"],
    Component: WorkerPanel,
  });
  client.addCommandCenterItem({
    id: "open-workers",
    title: "Open workers",
    icon: "Users",
    keywords: ["assignments", "attempts", "history", "cortex"],
    context: "agent",
    onSelect({ openPanel }) {
      openPanel("workers", { location: "workspace" });
    },
  });
  let active = true;
  type Candidate = { workspace: { id: string }; agent: { id: string; status: string } };
  let catalogueInFlight = false;
  const candidates = new Map<string, Candidate>();
  const registrations = new Map<string, { remove(): void }>();
  const updateRegistrations = () => {
    for (const [agentId, registration] of registrations) {
      if (!candidates.has(agentId)) {
        registration.remove();
        registrations.delete(agentId);
      }
    }
    // Keep the affordance present for every open ACP agent. Worker history is
    // session-scoped and the popover owns the empty state, so a missing worker
    // must not make the composer pill disappear or change the composer layout.
    for (const agentId of candidates.keys()) {
      const candidate = candidates.get(agentId);
      if (!candidate || registrations.has(agentId)) continue;
      // Composer pills are agent-scoped in Paseo 0.8. Header buttons are only
      // workspace-scoped, so using one here would leak one session's workers
      // into every chat sharing the workspace.
      registrations.set(agentId, client.addComposerPill({
        id: `workers-${agentId}`,
        workspaceId: candidate.workspace.id,
        agentId,
        button: {
          title: "Workers",
          icon: "Users",
          label: "Workers",
          behavior: {
            kind: "popover",
            Content: (props) => <WorkerPopover {...props} workspaceId={candidate.workspace.id} agentId={agentId} onOpenWorker={(assignmentRef) => {
              selectWorker(agentId, assignmentRef);
              client.openPanel("workers", { workspaceId: candidate.workspace.id, agentId, location: "workspace" });
            }} />,
          },
        },
      }));
    }
  };
  const refreshCatalogue = async () => {
    if (!active || catalogueInFlight) return;
    catalogueInFlight = true;
    try {
      const listed = await client.paseo.workspaces.list();
      const next = new Map<string, Candidate>();
      await Promise.all(listed.entries.map(async (workspace) => {
        const result = await client.paseo.agents.list({ filter: { projectKeys: [workspace.projectId] } });
        for (const entry of result.entries) {
          if (entry.agent.workspaceId === workspace.id && entry.agent.status !== "closed") {
            next.set(entry.agent.id, { workspace, agent: entry.agent });
          }
        }
      }));
      candidates.clear();
      for (const [agentId, candidate] of next) candidates.set(agentId, candidate);
      updateRegistrations();
    } catch {
      // Keep the last known catalogue; a transient host read must not remove
      // a valid Workers pill from the current session.
    } finally {
      catalogueInFlight = false;
    }
  };
  void refreshCatalogue();
  const unsubscribe = client.paseo.workspaces.subscribe(() => { void refreshCatalogue(); });
  const unsubscribeAgents = client.paseo.agents.subscribe(() => { void refreshCatalogue(); });
  return () => {
    active = false;
    unsubscribe();
    unsubscribeAgents();
    for (const registration of registrations.values()) registration.remove();
    registrations.clear();
  };
}
