import { isHiddenOperation } from "./shared/timeline-visibility.ts";
import { useAgent, type PluginClientContext, type PluginTimelineItemProps } from "@getpaseo/plugin/client";
import { Icon } from "@getpaseo/plugin/client/react-native";
import { Platform, Pressable, Text, View } from "react-native";
import { z } from "zod";
import { WorkerPanel } from "./client/worker-panel.js";
import { PipelinePanel } from "./client/pipeline-panel.js";
import { SettingsScreen } from "./client/settings-panel.js";
import { selectWorker } from "./client/worker-selection.js";
import { isWorkerTimelineRunning } from "./client/worker-timeline-status.js";
import { resolveDurableWorkerCardStatus, workerCardIdentity, workerCardRouteText, workerStatusPresentation, type WorkerStatusTone } from "./client/worker-display.ts";
import { coalesced } from "./client/coalesce.ts";
import { UsageMeter, useWorkerUsage } from "./client/usage-meter.tsx";
import { isWorkerCardCallId, WORKER_ROW_KIND, WORKER_ROW_VERSION, workerRowData, workerRowId } from "./shared/worker-card-item.ts";
import { cardAccessibilityLabel, cardAttention, cardFrameStyle, cardIssueCount, cardStatusAnnouncement, cardStatusKey, shouldAnnounceCardStatus, type CardStatusSnapshot } from "./client/worker-card-model.ts";
import { useEffect, useRef, useState } from "react";
import { PluginErrorBoundary } from "./client/plugin-error-boundary.tsx";
import { readableTextColor } from "./client/card-contrast.ts";
import { createActivationGuard, createSpaceActivation } from "./client/card-keyboard.ts";
import { workerCardPulseActive } from "./client/worker-card-pulse.ts";
import { WorkingPulse } from "./client/working-pulse.tsx";

const workerTimelineSchema = z.object({
  callId: z.string(),
  assignmentRef: z.string(),
  title: z.string(),
  // Paseo 0.8 projects ACP tool calls into this host-owned status set
  // before running timeline transformers.  Keep the plugin data schema
  // aligned with that exact projected shape so the renderer is mounted
  // instead of falling back to "Plugin timeline item unavailable.".
  status: z.enum(["running", "completed", "canceled", "failed"]),
  // The only terminal-result fact a coordinator card carries: the open-issue
  // count (`shared/worker-card-outcome.ts`). The summary, check list,
  // blockers and evidence gaps live in the worker tab only; an older item's
  // embedded `result` is reduced to this count by the transformer, and any
  // other key is stripped by this schema, so replay never brings them back.
  openIssues: z.number().int().nonnegative().optional(),
  outcomeStatus: z.enum(["completed", "failed"]).optional(),
  // The host's own bounded, server-redacted ACP tool-call error (see
  // `_emit_worker_card`'s `raw_output`). The card never shows it, only a
  // category (`cardAttention`); opening the worker hands it to the worker
  // tab, which shows it when no durable attempt explains the failure.
  failureReason: z.string().optional(),
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

type PressableState = { pressed: boolean; hovered?: boolean; focused?: boolean };

function WorkerTimelineRow({ theme, layout, agentId, item, workspaceId, onOpen }: PluginTimelineItemProps<z.output<typeof workerTimelineSchema>> & { workspaceId: string | null; onOpen: () => void }) {
  const hostWorking = isWorkerTimelineRunning(item.data.status);
  // Polled while the host still shows the card running: a plugin reload
  // closes the provider connection and the host re-sends nothing until the
  // next prompt, so the durable attempt status is what settles the card.
  // The read is compact (status, route, usage and an open-issue count only): an
  // unopened card never fetches a summary, check list or transcript.
  const { usage, loaded } = useWorkerUsage(workspaceId, agentId, item.data.assignmentRef, hostWorking);
  const resolvedStatus = resolveDurableWorkerCardStatus(item.data.title, item.data.status, usage?.status, usage?.openIssues ?? item.data.openIssues, item.data.outcomeStatus);
  const presentation = workerStatusPresentation(resolvedStatus);
  // Only the worker marker and status icon breathe, and only while the
  // worker is executing; the card, its text and its border never animate.
  const pulsing = workerCardPulseActive(resolvedStatus, usage?.status);
  const identity = workerCardIdentity(item.data.title, usage?.route);
  const issueCount = cardIssueCount(presentation.label, usage?.openIssues, item.data.openIssues);
  const attention = cardAttention(presentation.label, issueCount, item.data.failureReason);
  // Every coloured text is readable (>= 4.5:1) on every background the card
  // shows: normal/focused (surface1), hovered/pressed (surface2) and the
  // status chip (surface0), in whichever theme the host supplies. The theme's
  // own token is kept when it already passes (card-contrast.ts).
  const surfaces = [theme.colors.surface0, theme.colors.surface1, theme.colors.surface2];
  const muted = readableTextColor(theme.colors.foregroundMuted, theme.colors.foreground, surfaces);
  const statusColor = presentation.tone === "muted" ? muted
    : readableTextColor(statusToneColor(presentation.tone, theme.colors), theme.colors.foreground, surfaces);
  const statusText = presentation.label.charAt(0).toUpperCase() + presentation.label.slice(1);
  const disabled = !workspaceId;
  const open = () => {
    if (!workspaceId) return;
    selectWorker(agentId, item.data.assignmentRef, { failureReason: item.data.failureReason });
    onOpen();
  };
  // Native activation (Enter, click, touch) and the card's own Space handling
  // share one guard, so one gesture never opens the worker twice.
  const openRef = useRef(open);
  openRef.current = open;
  const [activateOnce] = useState(() => createActivationGuard(() => openRef.current()));
  const cardRef = useRef<unknown>(null);
  useEffect(() => {
    if (Platform.OS !== "web" || disabled || typeof window === "undefined") return;
    const space = createSpaceActivation({ isCard: (target) => target !== null && target === cardRef.current, activate: activateOnce });
    const keydown = (event: KeyboardEvent) => space.keydown(event);
    const keyup = (event: KeyboardEvent) => space.keyup(event);
    const blur = () => space.cancel();
    window.addEventListener("keydown", keydown, true);
    window.addEventListener("keyup", keyup, true);
    window.addEventListener("blur", blur);
    return () => {
      window.removeEventListener("keydown", keydown, true);
      window.removeEventListener("keyup", keyup, true);
      window.removeEventListener("blur", blur);
    };
  }, [activateOnce, disabled]);
  // One polite announcement per real status transition, naming the worker.
  // It is a separate live region outside the card's button (whose children
  // are presentational) and is empty until a transition: mounting or
  // replaying a card, its first reconciling read and usage polls say nothing.
  const statusKey = cardStatusKey(statusText, issueCount);
  const lastStatus = useRef<CardStatusSnapshot | null>(null);
  const [announcement, setAnnouncement] = useState("");
  useEffect(() => {
    const next = { key: statusKey, loaded };
    if (shouldAnnounceCardStatus(lastStatus.current, next)) {
      setAnnouncement(cardStatusAnnouncement({ title: identity.title, role: identity.role, status: statusText, issueCount }));
    }
    lastStatus.current = next;
  }, [statusKey, loaded]);

  // A worker is its own compact block in the coordinator's timeline: one
  // whole-card action that opens the worker tab, a fixed neutral 1-unit
  // boundary that never changes colour or width in any state (no status
  // stripe, no focus/selected/pressed perimeter), status as icon + text, no
  // result body; the only motion is the executing indicator's pulse.
  return (
    <View style={{ alignSelf: "stretch" }}>
    <Pressable
      ref={cardRef as never}
      accessibilityRole="button"
      accessibilityLabel={cardAccessibilityLabel({ title: identity.title, role: identity.role, status: statusText, route: identity.route, selection: identity.selection, issueCount, disabled })}
      accessibilityState={{ disabled }}
      disabled={disabled}
      onPress={() => { activateOnce(); }}
      // No focus ring or outline around the card, ours or the browser's:
      // focus shows as the hover fill (the owner removed the card outline
      // and the visible Open worker action).
      style={(state: PressableState) => cardFrameStyle(state, { disabled, web: Platform.OS === "web", compact: layout.compact,
        border: theme.colors.border, surface: theme.colors.surface1, hoverSurface: theme.colors.surface2 }) as never}
    >
      <View style={{ flexDirection: "row", flexWrap: "wrap", alignItems: "center", justifyContent: "space-between", columnGap: 8, rowGap: 4 }}>
        <View style={{ flexDirection: "row", alignItems: "center", gap: 6, flexShrink: 1, minWidth: 0 }}>
          <WorkingPulse active={pulsing}>
            <View style={{ width: 20, height: 20, borderRadius: 6, alignItems: "center", justifyContent: "center", backgroundColor: theme.colors.surface2 }}>
              <Icon name="Bot" size={14} color={muted} />
            </View>
          </WorkingPulse>
          <Text numberOfLines={1} style={{ color: muted, fontSize: 12, lineHeight: 18, flexShrink: 1, minWidth: 0 }}>{identity.role}</Text>
        </View>
        {/* Colour plus an icon and a text label, so status is never carried by colour alone. */}
        <View style={{ flexDirection: "row", alignItems: "center", gap: 4, paddingHorizontal: 8, paddingVertical: 2, borderRadius: 999, borderWidth: 1, borderColor: theme.colors.border, backgroundColor: theme.colors.surface0 }}>
          <WorkingPulse active={pulsing}><Icon name={presentation.icon} size={12} color={statusColor} /></WorkingPulse>
          <Text style={{ color: statusColor, fontSize: 12, lineHeight: 18, fontWeight: "600" }}>{statusText}{issueCount ? ` · ${issueCount}` : ""}</Text>
        </View>
      </View>
      <Text numberOfLines={2} ellipsizeMode="tail" style={{ color: theme.colors.foreground, fontSize: 14, lineHeight: 20, fontWeight: "600" }}>{identity.title}</Text>
      <Text style={{ color: muted, fontSize: 12, lineHeight: 18 }}>{workerCardRouteText(identity.route, resolvedStatus)}</Text>
      {identity.selection && <Text style={{ color: muted, fontSize: 12, lineHeight: 18 }}>{identity.selection}</Text>}
      {attention && <View style={{ flexDirection: "row", alignItems: "center", gap: 6, minWidth: 0 }}>
        <View style={{ flexShrink: 0 }}><Icon name="CircleAlert" size={12} color={statusColor} /></View>
        <Text numberOfLines={1} ellipsizeMode="tail" style={{ color: statusColor, fontSize: 12, lineHeight: 18, flexShrink: 1, minWidth: 0 }}>{attention}</Text>
      </View>}
      <View style={{ flexShrink: 1, minWidth: 0 }}>
        <UsageMeter context={usage?.context} tokens={usage?.tokens} theme={theme} mutedColor={muted} unavailableText={loaded ? "Usage unavailable" : undefined} />
      </View>
      {disabled && <Text style={{ color: muted, fontSize: 12, lineHeight: 18 }}>Worker unavailable</Text>}
    </Pressable>
    {/* Visually hidden but still in the accessibility tree: clipped to one
        pixel, never zero opacity, off-screen or display:none. */}
    <Text accessibilityLiveRegion="polite" style={{ position: "absolute", width: 1, height: 1, overflow: "hidden", color: "transparent" }}>{announcement}</Text>
    </View>
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
      if (!isWorkerCardCallId(item.callId)) return undefined;
      // The current server provider emits each worker card as a plugin item
      // with a stable identity (shared/worker-card-item.ts), which the host
      // merges across turns. This transformer only renders tool_call worker
      // items an older plugin persisted, with the same compact projection:
      // an embedded `cortexWorkerResult` is reduced to its open-issue count,
      // never forwarded into the coordinator card.
      return {
        items: [{
          type: "plugin",
          id: workerRowId(item.callId),
          kind: WORKER_ROW_KIND,
          version: WORKER_ROW_VERSION,
          data: workerRowData(item),
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
      return <PluginErrorBoundary theme={props.theme} label="Worker card">
        <WorkerTimelineRow {...props} workspaceId={workspaceId} onOpen={() => {
          if (workspaceId) client.openPanel("workers", { workspaceId, agentId: props.agentId, location: "workspace" });
        }} />
      </PluginErrorBoundary>;
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
    Component: (props) => <PluginErrorBoundary theme={props.theme} label="Workers panel"><WorkerPanel {...props} /></PluginErrorBoundary>,
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
    Component: (props) => <PluginErrorBoundary theme={props.theme} label="Pipeline panel">
      <PipelinePanel {...props} onOpenWorker={(assignmentRef) => {
        selectWorker(props.agentId, assignmentRef);
        client.openPanel("workers", { workspaceId: props.workspaceId, agentId: props.agentId, location: "workspace" });
      }} />
    </PluginErrorBoundary>,
  });
  // A dedicated settings screen (not a workspace/agent panel: the owner
  // configuration it edits -- user defaults, MCP servers, connections -- is
  // not scoped to one project). Reads and writes the Cortex configuration
  // API from `r_072f338e9c81`/`r_1e6ed08710fe` directly; Paseo's own
  // settings document is never used, so hand-edited Cortex JSON files stay
  // the single source of truth.
  client.addSettingsScreen({
    id: "cortex-settings",
    title: "Cortex",
    icon: "Settings",
    Component: (props) => <PluginErrorBoundary theme={props.theme} label="Cortex settings">
      <SettingsScreen {...props} />
    </PluginErrorBoundary>,
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
  // Discoverable from anywhere (the command palette), not only from a
  // Cortex chat: the settings it edits are owner- and project-scoped, not
  // agent-scoped, so this entry never gates on the active agent's provider.
  client.addCommandCenterItem({
    id: "open-cortex-settings",
    title: "Open Cortex settings",
    icon: "Settings",
    keywords: ["config", "configuration", "defaults", "routing", "mcp", "connections", "opencode", "cortex"],
    context: "global",
    onSelect({ openSettings }) {
      openSettings("cortex-settings");
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
            Content: (props) => <PluginErrorBoundary theme={props.theme} label="Pipeline">
              <PipelinePanel theme={props.theme} workspaceId={candidate.workspace.id} agentId={agentId} popover
                onOpenWorker={(assignmentRef) => {
                  props.close();
                  selectWorker(agentId, assignmentRef);
                  client.openPanel("workers", { workspaceId: candidate.workspace.id, agentId, location: "workspace" });
                }} />
            </PluginErrorBoundary>,
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
