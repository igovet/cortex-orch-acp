import { isHiddenOperation } from "./shared/timeline-visibility.ts";
import { useAgent, type PluginClientContext, type PluginTimelineItemProps } from "@getpaseo/plugin/client";
import { Icon } from "@getpaseo/plugin/client/react-native";
import { useEffect, useRef } from "react";
import { Animated, Platform, Pressable, Text, View } from "react-native";
import { z } from "zod";
import { WorkerPanel } from "./client/worker-panel.js";
import { PipelinePanel } from "./client/pipeline-panel.js";
import { selectWorker } from "./client/worker-selection.js";
import { isWorkerTimelineRunning } from "./client/worker-timeline-status.js";
import { resolveDurableWorkerCardStatus, workerCardMetadata, workerTerminalState, type WorkerStatusTone } from "./client/worker-display.ts";
import { coalesced } from "./client/coalesce.ts";
import { UsageMeter, useWorkerUsage } from "./client/usage-meter.tsx";
import { workerResult } from "./shared/pipeline.js";
import { parseWorkerResult } from "./shared/worker-timeline-result.ts";
import { WorkerResultDetail } from "./client/worker-result.tsx";
import { workerResultItems } from "./client/worker-result-model.ts";
import { setWorkerCardPulse } from "./client/worker-card-pulse.ts";
import { useFallbackWorkerResult } from "./client/use-worker-result.ts";

const workerTimelineSchema = z.object({
  callId: z.string(),
  assignmentRef: z.string(),
  title: z.string(),
  // Paseo 0.8 projects ACP tool calls into this host-owned status set
  // before running timeline transformers.  Keep the plugin data schema
  // aligned with that exact projected shape so the renderer is mounted
  // instead of falling back to "Plugin timeline item unavailable.".
  status: z.enum(["running", "completed", "canceled", "failed"]),
  // The runtime's terminal `{summary, checks}` projection, carried on the
  // host-persisted/replayed tool_call item's `metadata` (see this file's
  // `worker-tool-call-row` transformer and `server/provider.ts`'s
  // `presentTimeline`) so a completed card renders it with zero RPC, even
  // the first time a session is opened. `undefined` for a running card or
  // an older item that predates this field -- see `useFallbackWorkerResult`.
  result: workerResult.nullable().optional(),
});

const CORTEX_PROVIDER = "cortex";
// How often the Pipeline pill catalogue is re-read (see its poll).
const CATALOGUE_REFRESH_MS = 5000;

type WorkerTimelineData = z.output<typeof workerTimelineSchema>;

/** Each status tone maps to one of the host's own status colours, never a
 * one-off hex, so the card matches the theme's light/dark contrast. */
function statusToneColor(tone: WorkerStatusTone, colors: PluginTimelineItemProps<never>["theme"]["colors"]): string {
  if (tone === "success") return colors.statusSuccess;
  if (tone === "warning") return colors.statusWarning;
  if (tone === "danger") return colors.statusDanger;
  if (tone === "muted") return colors.foregroundMuted;
  return colors.accent;
}

function WorkerTimelineRow({ theme, layout, agentId, item, workspaceId, onOpen }: PluginTimelineItemProps<z.output<typeof workerTimelineSchema>> & { workspaceId: string | null; onOpen: () => void }) {
  const opacity = useRef(new Animated.Value(1)).current;
  const pulseElement = useRef<HTMLElement | null>(null);
  const hostWorking = isWorkerTimelineRunning(item.data.status);
  // Polled while the host still shows the card running: a plugin reload
  // closes the provider connection and the host re-sends nothing until the
  // next prompt, so the durable attempt status is what settles the card.
  const usage = useWorkerUsage(workspaceId, agentId, item.data.assignmentRef, hostWorking);
  const resolvedStatus = resolveDurableWorkerCardStatus(item.data.title, item.data.status, usage?.status);
  const isWorking = isWorkerTimelineRunning(resolvedStatus);
  // A running card never shows a result yet, so the fallback read only ever
  // triggers once the card is terminal and has no embedded result -- an
  // older item, or a host build that did not persist the field.
  const embedded = item.data.result ?? null;
  const fallback = useFallbackWorkerResult(workspaceId, agentId, item.data.assignmentRef, !isWorking && !embedded);
  const result = embedded ?? fallback;
  const terminal = workerTerminalState(resolvedStatus, result?.checks);
  const metadata = workerCardMetadata(item.data.title, resolvedStatus);
  const { issueCount } = terminal;
  const statusColor = statusToneColor(terminal.tone, theme.colors);

  useEffect(() => {
    if (Platform.OS === "web") {
      const element = pulseElement.current;
      if (!element) return;
      setWorkerCardPulse(element, isWorking);
      return () => setWorkerCardPulse(element, false);
    }
    if (!isWorking) {
      opacity.stopAnimation();
      opacity.setValue(1);
      return;
    }

    // Native surfaces still use Animated; web uses CSS so a hidden chat panel
    // resumes its pulse when the host makes it visible without remounting.
    opacity.setValue(1);
    const activity = Animated.loop(
      Animated.sequence([
        Animated.timing(opacity, { toValue: 0.58, duration: 850, useNativeDriver: false }),
        Animated.timing(opacity, { toValue: 1, duration: 850, useNativeDriver: false }),
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
      style={{ marginVertical: layout.compact ? 6 : 8, borderWidth: 1, borderColor: theme.colors.border, borderLeftWidth: 3,
        borderLeftColor: statusColor, borderRadius: 8,
        backgroundColor: theme.colors.surface1, paddingHorizontal: 12, paddingVertical: 10, gap: 6 }}
    >
      <Animated.View ref={(node) => { pulseElement.current = node as unknown as HTMLElement | null; }} style={{ opacity: Platform.OS === "web" ? 1 : opacity, gap: 6 }}>
        <View style={{ flexDirection: "row", alignItems: "center", gap: 6 }}>
          <Icon name="Bot" size={12} color={theme.colors.foregroundMuted} />
          <Text style={{ flex: 1, color: theme.colors.foregroundMuted, fontSize: 11, fontWeight: "600", letterSpacing: 0.6, textTransform: "uppercase" }}>Worker</Text>
          {/* Colour plus an icon and a text label, so status is never carried by colour alone. */}
          <View style={{ flexDirection: "row", alignItems: "center", gap: 4 }}>
            <Icon name={terminal.icon} size={12} color={statusColor} />
            <Text style={{ color: statusColor, fontSize: 11, fontWeight: "600", textTransform: "capitalize" }}>{terminal.label}</Text>
            {issueCount > 0 && <Text style={{ color: statusColor, fontSize: 11, fontWeight: "600" }}>{issueCount} issue{issueCount === 1 ? "" : "s"}</Text>}
          </View>
        </View>
        <Text numberOfLines={2} ellipsizeMode="tail" style={{ color: theme.colors.foreground, fontSize: 14, fontWeight: "600" }}>{metadata.workerName}</Text>
        <Text numberOfLines={1} ellipsizeMode="tail" style={{ color: theme.colors.foregroundMuted, fontSize: 12 }}>{metadata.subtitle}</Text>
        <UsageMeter context={usage?.context} tokens={usage?.tokens} theme={theme} />
        {!isWorking && result && <View style={{ marginTop: 2, paddingTop: 8, borderTopWidth: 1, borderColor: theme.colors.border }}>
          <WorkerResultDetail body={result.summary ?? undefined} items={workerResultItems(result.checks)} theme={theme} />
        </View>}
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
      // The server's `presentTimeline` embeds the runtime's terminal
      // `{summary, checks}` projection at `item.metadata.cortexWorkerResult`
      // (see `server/provider.ts`). Paseo persists and replays a tool_call
      // item's `metadata` exactly like its other fields, so this is a
      // completed card's result the *first* time a session is opened, not
      // just after a live update -- the result no longer needs its own RPC.
      const result = parseWorkerResult(item.metadata?.cortexWorkerResult);
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
          data: { callId: item.callId, assignmentRef: item.callId.slice("worker:".length), title: item.name, status: item.status,
            ...(result !== undefined ? { result } : {}) },
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
  // The task's whole lifecycle, top to bottom, beside the chat in the
  // explorer (right sidebar). Paseo 0.8 has no "focused agent" hook, so the
  // tab is bound to the chat that opened it, like every agent panel.
  client.addWorkspacePanel({
    id: "pipeline",
    title: "Pipeline",
    icon: "Workflow",
    context: "agent",
    locations: ["explorer"],
    Component: (props) => <PipelinePanel {...props} onOpenWorker={(assignmentRef) => {
      selectWorker(props.agentId, assignmentRef);
      client.openPanel("workers", { workspaceId: props.workspaceId, agentId: props.agentId, location: "workspace" });
    }} />,
  });
  client.addCommandCenterItem({
    id: "open-pipeline",
    title: "Open pipeline",
    icon: "Workflow",
    keywords: ["plan", "progress", "steps", "todo", "cortex"],
    context: "agent",
    onSelect({ agent, openPanel }) {
      if (agent.provider === CORTEX_PROVIDER) openPanel("pipeline", { location: "explorer" });
    },
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
  const candidates = new Map<string, Candidate>();
  const registrations = new Map<string, { remove(): void }>();
  const updateRegistrations = () => {
    for (const [agentId, registration] of registrations) {
      if (!candidates.has(agentId)) {
        registration.remove();
        registrations.delete(agentId);
      }
    }
    // Keep the affordance present for every open Cortex agent. Pipeline
    // history is session-scoped and the popover owns the empty state, so a
    // missing worker must not make the composer pill disappear or change the
    // layout.
    for (const agentId of candidates.keys()) {
      const candidate = candidates.get(agentId);
      if (!candidate || registrations.has(agentId)) continue;
      // Composer pills are agent-scoped in Paseo 0.8. Header buttons are only
      // workspace-scoped, so using one here would leak one session's pipeline
      // into every chat sharing the workspace.
      const pipeline = client.addComposerPill({
        id: `pipeline-${agentId}`,
        workspaceId: candidate.workspace.id,
        agentId,
        button: {
          title: "Pipeline",
          icon: "Workflow",
          label: "Pipeline",
          // Phones have no explorer sidebar to open it in; there it opens in
          // place. The desktop keeps the sidebar tab.
          behavior: Platform.OS === "web" ? {
            kind: "action",
            onPress: () => client.openPanel("pipeline", { workspaceId: candidate.workspace.id, agentId, location: "explorer" }),
          } : {
            kind: "popover",
            Content: (props) => <PipelinePanel theme={props.theme} workspaceId={candidate.workspace.id} agentId={agentId} popover
              onOpenWorker={(assignmentRef) => {
                props.close();
                selectWorker(agentId, assignmentRef);
                client.openPanel("workers", { workspaceId: candidate.workspace.id, agentId, location: "workspace" });
              }} />,
          },
        },
      });
      registrations.set(agentId, { remove() { pipeline.remove(); } });
    }
  };
  // Coalesced, never dropped: creating an agent fires several
  // subscription events inside the window one refresh (a workspace list
  // plus the agent list) takes, and ignoring those lost the only
  // notification carrying the new agent -- its Pipeline composer pill
  // then never appeared for that session at all.
  const refreshCatalogue = coalesced(async () => {
    if (!active) return;
    let next: Map<string, Candidate>;
    try {
      const listed = await client.paseo.workspaces.list();
      const workspaces = new Map(listed.entries.map((workspace) => [workspace.id, workspace] as const));
      next = new Map<string, Candidate>();
      // One listing of every active agent, matched to its own workspace:
      // one request instead of one per workspace, and no dependence on how
      // the daemon resolves `filter.projectKeys` against a `projectId`.
      let cursor: string | undefined;
      for (let page = 0; page < 20; page += 1) {
        const result = await client.paseo.agents.list({
          scope: "active",
          page: { limit: 200, ...(cursor ? { cursor } : {}) },
        });
        for (const entry of result.entries) {
          // An agent without a workspace has no composer to hold a pill.
          const workspace = entry.agent.workspaceId ? workspaces.get(entry.agent.workspaceId) : undefined;
          // Workers and Pipeline read a Cortex task; other providers have none.
          if (!workspace || entry.agent.status === "closed" || entry.agent.provider !== CORTEX_PROVIDER) continue;
          next.set(entry.agent.id, { workspace, agent: entry.agent });
        }
        if (!result.pageInfo.hasMore || !result.pageInfo.nextCursor) break;
        cursor = result.pageInfo.nextCursor;
      }
    } catch {
      // Keep the last known catalogue; a transient host read must not remove
      // a valid Pipeline pill from the current session.
      return;
    }
    candidates.clear();
    for (const [agentId, candidate] of next) candidates.set(agentId, candidate);
    // Deliberately outside the catch above: a pill that fails to register
    // is a plugin bug Paseo should surface, not something to swallow into
    // "the catalogue is empty".
    updateRegistrations();
  });
  void refreshCatalogue();
  const unsubscribe = client.paseo.workspaces.subscribe(() => { void refreshCatalogue(); });
  const unsubscribeAgents = client.paseo.agents.subscribe(() => { void refreshCatalogue(); });
  // The subscriptions above are not enough on their own. This entry can run
  // before the host connection is up, so the first read fails; and
  // `subscribe()` only relays daemon updates that something else already
  // asked the daemon to stream. Observed against the live daemon: loaded
  // before the connection, the first read failed and no update arrived in
  // the following 15s, so the pills never appeared at all. A steady re-read
  // keeps the catalogue current whatever the load order.
  const poll = setInterval(() => { void refreshCatalogue(); }, CATALOGUE_REFRESH_MS);
  return () => {
    active = false;
    clearInterval(poll);
    unsubscribe();
    unsubscribeAgents();
    for (const registration of registrations.values()) registration.remove();
    registrations.clear();
  };
}
