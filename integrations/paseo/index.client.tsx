import { useAgent, type PluginClientContext, type PluginTimelineItemProps } from "@getpaseo/plugin/client";
import { Icon } from "@getpaseo/plugin/client/react-native";
import { useEffect, useRef } from "react";
import { Animated, Pressable, Text, View } from "react-native";
import { z } from "zod";
import { WorkerPanel } from "./client/worker-panel.js";
import { selectWorker } from "./client/worker-selection.js";
import { projectCoordinatorToolRow } from "./client/coordinator-tool-row.js";
import { WorkerPopover } from "./client/worker-popover.js";
import { isWorkerTimelineRunning } from "./client/worker-timeline-status.js";

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

const compactToolTimelineSchema = z.object({
  callId: z.string(),
  label: z.string(),
  icon: z.string(),
});

type CompactToolTimelineData = z.output<typeof compactToolTimelineSchema>;

function CompactToolRow({ theme, layout, item }: PluginTimelineItemProps<CompactToolTimelineData>) {
  return (
    <View style={{ flexDirection: "row", alignItems: "center", gap: 7, paddingVertical: layout.compact ? 5 : 7 }}>
      <Icon name={item.data.icon} size={14} color={theme.colors.foregroundMuted} />
      <Text style={{ color: theme.colors.foreground }}>
      {item.data.label}
      </Text>
    </View>
  );
}

/**
 * Worker cards use the ACP tool title as their portable metadata channel.
 * Runtime titles are currently shaped as:
 * `Worker: profile · provider · model · effort — assignment title`.
 * Keep parsing tolerant for older cards that omitted route metadata.
 */
function workerCardMetadata(title: string, status: WorkerTimelineData["status"]) {
  const body = title.trim().replace(/^Worker:\s*/i, "");
  const [route, ...nameParts] = body.split(/\s+—\s+/, 2);
  const routeParts = route.split(/\s+·\s+/).map((part) => part.trim()).filter(Boolean);
  const workerName = (nameParts.join(" — ").trim() || routeParts[0] || "assignment");
  const profile = routeParts[0] || "worker";
  const statusLabel = status === "running" ? "working" : status === "failed" ? "failed" : status === "canceled" ? "cancelled" : status;
  return {
    workerName,
    subtitle: [profile, ...routeParts.slice(1), statusLabel].join(" · "),
  };
}

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

  return (
    <Pressable
      accessibilityRole="button"
      accessibilityLabel={`Open worker ${metadata.workerName}`}
      onPress={() => workspaceId && (selectWorker(agentId, item.data.assignmentRef), onOpen())}
      style={{ paddingVertical: layout.compact ? 6 : 8, paddingHorizontal: 0 }}
    >
      <Animated.View style={{ opacity }}>
        <View style={{ flexDirection: "row", alignItems: "center", gap: 7 }}>
          <Icon name="Users" size={14} color={theme.colors.foregroundMuted} />
          <Text style={{ color: theme.colors.accent, fontWeight: "600" }}>Worker: {metadata.workerName}</Text>
        </View>
        <Text style={{ color: theme.colors.foregroundMuted, fontSize: 12, marginLeft: 21 }}>{metadata.subtitle}</Text>
      </Animated.View>
    </Pressable>
  );
}

export default function contribute(client: PluginClientContext) {
  client.addTimelineTransformer({
    id: "cortex-compact-tool-row",
    query: { itemType: "tool_call" },
    transform({ item }) {
      if (item.callId.startsWith("worker:")) return undefined;
      const projection = projectCoordinatorToolRow(item);
      if (!projection) return undefined;
      if (projection.hidden || !projection.presentation) return { items: [] };
      const presentation = projection.presentation;
      return {
        items: [{
          type: "plugin",
          kind: "cortex-compact-tool-row",
          version: 1,
          data: {
            callId: item.callId,
            label: presentation.label,
            icon: presentation.icon,
          },
        }],
      };
    },
  });
  client.addTimelineRenderer({
    kind: "cortex-compact-tool-row",
    version: 1,
    schema: compactToolTimelineSchema,
    Component: CompactToolRow,
  });
  client.addTimelineTransformer({
    id: "worker-tool-call-row",
    query: { itemType: "tool_call" },
    transform({ item }) {
      if (!item.callId.startsWith("worker:")) return undefined;
      return {
        items: [{
          type: "plugin",
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
