import { readableToolText, toolDetailPreview, toolDetailText } from "./tool-details.ts";
import { codeModePresentation } from "../shared/gateway-presentation.ts";
import * as PaseoNative from "@getpaseo/plugin/client/react-native";
import { useRpc, type PluginAgentPanelProps } from "@getpaseo/plugin/client";
import { Icon, ScrollView } from "@getpaseo/plugin/client/react-native";
import { Pressable, Text, View } from "react-native";
import React, { useCallback, useEffect, useRef, useState } from "react";
import { readWorkerHistory, type WorkerHistory } from "../shared/worker-history.js";
import { savedWorkerView, saveWorkerView, selectedWorker, selectedWorkerDetail, subscribeWorkerSelection } from "./worker-selection.js";
import { workerSection } from "./worker-panel-status.js";
import { historyCursor, mergeWorkerHistory, shouldPollWorkerHistory, shouldShowWorkerHistoryLoader, WORKER_HISTORY_POLL_MS } from "./worker-panel-refresh.js";
import { activityPreviewNotice, groupWorkerEvents, withoutShownResult, type WorkerEvent } from "./worker-panel-events.js";
import { presentTool } from "./worker-panel-tool.js";
import { diffBlocks, diffLines, type DiffRow } from "./worker-panel-diff.js";
import { chatToolIcon, groupToolRuns, isToolEvent, nativeRow, toolSemantics, type TranscriptItem } from "./worker-panel-groups.ts";
import { contextUnavailableText, routeSelectionText, workerAssignmentLine, workerDisplayName, workerModelDisplay, workerTerminalState } from "./worker-display.js";
import { workerPopoverStatus } from "./worker-popover-status.js";
import { openWorkspaceFile } from "./open-workspace-file.ts";
import { UsageMeter } from "./usage-meter.tsx";
import { MarkdownText } from "./markdown-text.tsx";
import { RunningCommands } from "./command-stop-panel.tsx";
import { WorkerAttemptResult } from "./worker-result.tsx";
import { attemptResultView } from "./worker-attempt-result.ts";
import { WorkerAttemptPrompt } from "./worker-prompt.tsx";
import { stoppedCommand } from "./command-stop.ts";
import { isWorkerExecutingStatus } from "./worker-card-pulse.ts";
import { WorkingPulse } from "./working-pulse.tsx";

const styles = {
  title: { fontSize: 18, lineHeight: 24, fontWeight: "600" as const },
  muted: { fontSize: 13, lineHeight: 19 },
  body: { fontSize: 15, lineHeight: 23 },
  card: { borderWidth: 1, borderRadius: 9, padding: 12, gap: 5 },
  row: { flexDirection: "row" as const, alignItems: "center" as const, justifyContent: "space-between" as const, gap: 8 },
  button: { paddingVertical: 6, paddingHorizontal: 9, borderRadius: 6, alignSelf: "flex-start" as const },
  mono: { fontFamily: "monospace" as const, fontSize: 12, lineHeight: 18 },
};


export { diffBlocks, diffLines, rawText } from "./worker-panel-diff.js";
type Theme = PluginAgentPanelProps["theme"];

/** The last history read's problem. A rejected request never shows its own
 * error text (it is transport detail, possibly unsafe); an unavailable store
 * shows the server's fixed message. Neither means the worker failed. */
type HistoryReadProblem = { message: string };
const HISTORY_READ_FAILED = "Worker history could not be read.";

/** A failed or unavailable read: with retained content it marks that content
 * stale; without any it replaces a false empty state. Retry re-reads the
 * same selection. */
function HistoryReadNotice({ problem, stale, onRetry, theme }: { problem: HistoryReadProblem; stale: boolean; onRetry: () => void; theme: Theme }) {
  const text = stale ? `${problem.message} Showing the last retained history; it may be out of date.` : problem.message;
  return <View accessibilityLiveRegion="polite" style={[styles.card, { borderColor: theme.colors.border, backgroundColor: theme.colors.surface1 }]}>
    <View style={{ flexDirection: "row", alignItems: "flex-start", gap: 6 }}>
      <View style={{ paddingTop: 3 }}><Icon name="CircleAlert" size={12} color={theme.colors.statusWarning} /></View>
      <Text style={[styles.muted, { color: theme.colors.foreground, flexShrink: 1, minWidth: 0 }]}>{text}</Text>
    </View>
    <Pressable accessibilityRole="button" accessibilityLabel="Retry reading worker history" onPress={onRetry}
      style={[styles.button, { borderWidth: 1, borderColor: theme.colors.border, backgroundColor: theme.colors.surface2 }]}>
      <Text style={[styles.muted, { color: theme.colors.accent }]}>Retry</Text>
    </Pressable>
  </View>;
}

/** Paseo's DiffViewer: no line numbers or file header, a +/-/space prefix,
 * the chat's add/remove/context backgrounds, changed words highlighted, and
 * horizontal scrolling instead of wrapping. */
const DIFF_BACKGROUND = { add: "rgba(46, 160, 67, 0.15)", delete: "rgba(248, 81, 73, 0.1)" } as const;
const DIFF_HIGHLIGHT = { add: "rgba(46, 160, 67, 0.4)", delete: "rgba(248, 81, 73, 0.35)" } as const;
const DIFF_TEXT = { fontFamily: "monospace", fontSize: 12, whiteSpace: "pre" } as const;

function DiffView({ rows, theme }: { rows: DiffRow[]; theme: Theme }) {
  return <ScrollView horizontal nestedScrollEnabled showsHorizontalScrollIndicator>
    <View style={{ alignSelf: "flex-start", padding: 12, minWidth: "100%" }}>
      {rows.map((row, index) => {
        const prefix = row.kind === "add" ? "+" : row.kind === "delete" ? "-" : " ";
        const color = row.kind === "context" ? theme.colors.foregroundMuted : theme.colors.foreground;
        return <View key={`${index}-${row.kind}`} style={{ minWidth: "100%", paddingVertical: 4,
          backgroundColor: row.kind === "context" ? theme.colors.surface1 : DIFF_BACKGROUND[row.kind] }}>
          <Text selectable style={[DIFF_TEXT as object, { color }]}>
            {prefix}
            {row.segments && row.kind !== "context"
              ? row.segments.map((segment, i) => <Text key={i} style={segment.changed ? { backgroundColor: DIFF_HIGHLIGHT[row.kind as "add" | "delete"] } : undefined}>{segment.text}</Text>)
              : row.text}
          </Text>
        </View>;
      })}
    </View>
  </ScrollView>;
}

const PRIVATE_TOOL_BODIES = new Set(["create_draft", "draft_write", "read_draft", "write_report", "read_report", "result_submit", "task_result", "sprint_task_result"]);

function privateToolDetail(event: WorkerEvent): boolean {
  const input = event.raw_input ?? event.input;
  const output = event.raw_output ?? event.output;
  const envelope = input && typeof input === "object" ? input as Record<string, unknown> : {};
  const title = event.title ?? "";
  const codeMode = codeModePresentation(title, input, output, event.tool_kind);
  return [title, event.tool_kind, codeMode?.name, envelope.server === "cortex" ? envelope.tool : null].some(name =>
    typeof name === "string" && PRIVATE_TOOL_BODIES.has(name.trim().replace(/^(?:mcp\.cortex\.|mcp__cortex__|cortex\.)/i, "").toLowerCase()));
}

function DetailSection({ title, value, unavailable, theme }: { title: string; value: unknown; unavailable: string; theme: Theme }) {
  const [readable, setReadable] = useState(false);
  const [expanded, setExpanded] = useState(false);
  const [copyFeedback, setCopyFeedback] = useState<{ text: string; message: string } | null>(null);
  const full = toolDetailText(value);
  const extracted = full === null ? null : readableToolText(value);
  const alternate = extracted === full ? null : extracted;
  const text = (readable ? alternate : null) ?? full;
  const preview = text === null ? null : toolDetailPreview(text, expanded);
  const display = text === "" ? '""' : preview?.text ?? unavailable;
  const actionStyle = [styles.button, { borderWidth: 1, borderColor: theme.colors.border, backgroundColor: theme.colors.surface2 }];
  return <View style={{ paddingHorizontal: 12, paddingTop: 8, paddingBottom: 8, minWidth: 0, gap: 4 }}>
    <Text style={{ color: theme.colors.foregroundMuted, fontSize: 12, fontWeight: "600" }}>{title}</Text>
    <ScrollView horizontal nestedScrollEnabled showsHorizontalScrollIndicator>
      <Text selectable style={[DIFF_TEXT as object, { color: theme.colors.foreground }]}>{display}</Text>
    </ScrollView>
    {preview?.canExpand && !expanded && <Text style={[styles.muted, { color: theme.colors.foregroundMuted }]}>Preview only</Text>}
    <View style={{ flexDirection: "row", flexWrap: "wrap", gap: 4 }}>
      {preview?.canExpand && <Pressable accessibilityRole="button" accessibilityLabel={`${expanded ? "Show less" : "Show all"} ${title}`}
        accessibilityState={{ expanded }} style={actionStyle} onPress={() => setExpanded(!expanded)}>
        <Text style={[styles.muted, { color: theme.colors.foreground }]}>{expanded ? "Show less" : "Show all"}</Text>
      </Pressable>}
      {alternate !== null && <Pressable accessibilityRole="button" accessibilityLabel={`${readable ? "Show full value" : "Show readable text"} for ${title}`}
        style={actionStyle} onPress={() => { setReadable(!readable); setExpanded(false); setCopyFeedback(null); }}>
        <Text style={[styles.muted, { color: theme.colors.foreground }]}>{readable ? "Show full value" : "Show readable text"}</Text>
      </Pressable>}
      {text !== null && typeof PaseoNative.copyText === "function" && <Pressable accessibilityRole="button"
        accessibilityLabel={`Copy ${title}`} style={actionStyle} onPress={async () => {
          try {
            await PaseoNative.copyText(text);
            setCopyFeedback({ text, message: "Copied" });
          } catch {
            setCopyFeedback({ text, message: "Could not copy. Select the text to copy it manually." });
          }
        }}>
        <Text style={[styles.muted, { color: theme.colors.foreground }]}>Copy</Text>
      </Pressable>}
    </View>
    {copyFeedback?.text === text && <Text accessibilityLiveRegion="polite" style={[styles.muted, { color: theme.colors.foreground }]}>{copyFeedback.message}</Text>}
  </View>;
}

function cleanEventText(event: WorkerEvent): string {
  let value = event.text.trim();
  for (const prefix of [event.title, event.status]) {
    if (prefix && value.startsWith(`${prefix} · `)) value = value.slice(prefix.length + 3);
  }
  return value.replace(/^\[final response\]\s*/i, "").trim();
}

/** Paseo's ExpandableBadge, as the main chat draws every tool row and group:
 * 22px icon slot, 14px muted label that brightens on hover/expand, a chevron
 * in place of the icon while toggleable, details attached underneath. */
function Badge({ theme, icon, label, summary, expanded, onToggle, onOpenFile, loading, failed, borderless, children }: {
  theme: Theme; icon: string; label: string; summary?: string; expanded: boolean; onToggle?: () => void;
  onOpenFile?: () => void; loading?: boolean; failed?: boolean; borderless?: boolean; children?: React.ReactNode;
}) {
  const [hovered, setHovered] = useState(false);
  const [openHovered, setOpenHovered] = useState(false);
  const active = hovered || expanded;
  const color = loading || active ? theme.colors.foreground : theme.colors.foregroundMuted;
  const glyph = onToggle && active
    ? <View style={{ marginLeft: -4, transform: [{ scale: 1.3 }, { rotate: expanded ? "90deg" : "0deg" }] }}><Icon name="ChevronRight" size={12} color={theme.colors.foreground} /></View>
    : failed ? <Icon name="CircleAlert" size={12} color={theme.colors.statusDanger} />
    : <Icon name={icon} size={12} color={active ? theme.colors.foreground : theme.colors.foregroundMuted} />;
  return <View style={{ marginBottom: 4 }} onPointerEnter={() => setHovered(true)} onPointerLeave={() => setHovered(false)}>
    <Pressable disabled={!onToggle} onPress={onToggle} accessibilityRole={onToggle ? "button" : undefined}
      accessibilityState={onToggle ? { expanded } : undefined}
      style={{ borderRadius: 8, borderWidth: 1, borderColor: expanded && !borderless ? theme.colors.border : "transparent",
        paddingHorizontal: 8, paddingVertical: 4, overflow: "hidden",
        ...(expanded ? { backgroundColor: theme.colors.surface1 } : {}),
        ...(expanded && !borderless ? { borderBottomLeftRadius: 0, borderBottomRightRadius: 0 } : {}) }}>
      <View style={{ flexDirection: "row", alignItems: "center" }}>
        <View style={{ width: 22, height: 22, borderRadius: 11, alignItems: "center", justifyContent: "center", marginRight: 4 }}>{glyph}</View>
        <View style={{ flex: 1, flexDirection: "row", alignItems: "center", overflow: "hidden" }}>
          <Text numberOfLines={1} style={{ color, fontSize: 14, flexShrink: 0, opacity: loading ? 0.72 : 1 }}>{label}</Text>
          {summary ? <Text numberOfLines={1} style={{ color: active ? theme.colors.foreground : theme.colors.foregroundMuted, fontSize: 14, marginLeft: 8, flexShrink: 1, minWidth: 0 }}>{summary}</Text> : null}
          {onOpenFile && hovered ? <Pressable onPress={onOpenFile} onHoverIn={() => setOpenHovered(true)} onHoverOut={() => setOpenHovered(false)}
            accessibilityRole="button" accessibilityLabel="Open file" hitSlop={6} style={{ marginLeft: 4, padding: 4, borderRadius: 6, flexShrink: 0 }}>
            <Icon name="FileSymlink" size={14} color={openHovered ? theme.colors.foreground : theme.colors.foregroundMuted} />
          </Pressable> : null}
        </View>
      </View>
    </Pressable>
    {expanded && children ? <View style={{ borderBottomLeftRadius: 8, borderBottomRightRadius: 8, borderWidth: borderless ? 0 : 1, borderTopWidth: 0, borderColor: theme.colors.border, overflow: "hidden" }}>{children}</View> : null}
  </View>;
}

export function TranscriptEvent({ event, expanded, onToggle, onOpenFile, theme, active = true }: { event: WorkerEvent; updates: WorkerEvent[]; active?: boolean; expanded: boolean; onToggle: () => void; onOpenFile?: (path: string) => void; theme: Theme }) {
  const tool = isToolEvent(event);
  const failure = event.status === "failed" || (event.status !== "completed" && event.type.toLowerCase().includes("failed"));
  const final = /result|final|complete/.test(event.type.toLowerCase());
  const message = cleanEventText(event);
  const preview = activityPreviewNotice(event);
  if (!tool) return <View style={{ marginVertical: final ? 13 : 9, gap: 6 }}>
    {message && <MarkdownText text={message} color={theme.colors.foreground} mutedColor={theme.colors.foregroundMuted} surfaceColor={theme.colors.surface1} />}
  </View>;
  const presentation = presentTool(event);
  const semantics = toolSemantics(event);
  const row = nativeRow(event);
  const privateDetail = privateToolDetail(event);
  const diffs = privateDetail ? [] : diffBlocks(event.content ?? (event.input && typeof event.input === "object" ? (event.input as Record<string, unknown>).content : null));
  // An ACP content list with no blocks is not a captured command result.
  // Keep explicitly retained output values, including the empty string.
  const content = presentation.kind === "shell" && Array.isArray(event.content) && event.content.length === 0
    ? undefined : event.content;
  const output = event.raw_output ?? event.output ?? content;
  const hasEdit = !privateDetail && presentation.kind === "edit" && (diffs.length > 0 || presentation.newText !== null);
  const showDetails = presentation.kind === "generic" || presentation.kind === "shell";
  const shellCommand = presentation.kind === "shell" ? presentation.command : null;
  const shownError = failure && !privateDetail && event.error !== output ? event.error : null;
  const running = active && (event.status === "in_progress" || event.status === "pending" || event.status === "running");
  const canExpand = semantics.action !== "" && (showDetails ? semantics.expandable : hasEdit);
  const unavailable = privateDetail ? "Private tool detail withheld" : preview ? "Full activity unavailable here" : "Not recorded";
  // A command the user stopped: Cortex's own status/title say so, set
  // only once the server has confirmed a genuine stop for this exact row
  // (see command_control.confirmed_stop). The reason always comes from
  // that trusted title; the row's own raw output/content is consulted
  // only for the attempted command text, since a worker can fabricate
  // that shape in its own tool output (never for the reason itself).
  const stopped = stoppedCommand(event.status, event.title, event.raw_output ?? event.output)
    ?? stoppedCommand(event.status, event.title, event.error);
  const badge = <Badge theme={theme} icon={chatToolIcon(event)} label={stopped ? "Stopped by user" : row.label}
    summary={stopped ? shellCommand ?? stopped.command ?? row.summary : row.summary} expanded={expanded && canExpand}
    onToggle={canExpand ? onToggle : undefined} loading={running} failed={failure}
    onOpenFile={onOpenFile && row.summary && (row.label === "Read" || row.label === "Edit") ? () => onOpenFile(row.summary!) : undefined}>
    {/* As in the chat, an edit's diff fills the detail edge to edge; the
        path is already the row's summary. */}
    {diffs.map((diff, i) => <View key={`${diff.path}-${i}`}>
      {diffs.length > 1 && <Text selectable style={[styles.mono, { paddingHorizontal: 12, paddingTop: 8, color: theme.colors.foregroundMuted }]}>{diff.path}</Text>}
      <DiffView rows={diffLines(diff.oldText, diff.newText)} theme={theme} />
    </View>)}
    {hasEdit && diffs.length === 0 && <DiffView rows={diffLines(presentation.oldText, presentation.newText)} theme={theme} />}
    {showDetails && <>
      <DetailSection title="Input" value={privateDetail ? null : event.raw_input ?? event.input} unavailable={unavailable} theme={theme} />
      <DetailSection title="Output" value={privateDetail ? null : output}
        unavailable={running && !privateDetail && !preview ? "Pending" : unavailable} theme={theme} />
    </>}
    {shownError != null && <DetailSection title="Error" value={shownError} unavailable="Not recorded" theme={theme} />}
  </Badge>;
  const annotatedBadge = preview ? <View>
    {badge}
    <Text selectable style={[styles.muted, { color: theme.colors.foregroundMuted, paddingLeft: 38 }]}>{preview}</Text>
  </View> : badge;
  if (!stopped) return annotatedBadge;
  return <View>
    {annotatedBadge}
    <Text selectable accessibilityLabel={`Stopped by user. Reason: ${stopped.reason}`}
      style={[styles.muted, { color: theme.colors.statusWarning, paddingLeft: 38, paddingRight: 8, marginTop: -2, marginBottom: 6 }]}>
      {`Reason: ${stopped.reason}`}
    </Text>
  </View>;
}

type ScrollOffsets = React.MutableRefObject<Record<string, number>>;

/** Records a scroll area's offset and, after a remount, scrolls back to it
 * once its restored content is long enough. */
function useKeptScroll(offsets: ScrollOffsets, key: string, horizontal = false) {
  const ref = useRef<{ scrollTo(options: { x?: number; y?: number; animated?: boolean }): void } | null>(null);
  const pending = useRef({ ...offsets.current });
  return {
    ref: ref as never,
    scrollEventThrottle: 16,
    onScroll: (event: { nativeEvent: { contentOffset: { x: number; y: number } } }) => {
      offsets.current[key] = horizontal ? event.nativeEvent.contentOffset.x : event.nativeEvent.contentOffset.y;
    },
    onContentSizeChange: (width: number, height: number) => {
      const target = pending.current[key];
      if (!target || (horizontal ? width : height) < target) return;
      delete pending.current[key];
      ref.current?.scrollTo(horizontal ? { x: target, animated: false } : { y: target, animated: false });
    },
  };
}

/** One run of tool rows, collapsed to Paseo's summary badge. */
function ToolGroup({ group, expanded, onToggle, expandedEvent, onToggleEvent, onOpenFile, offsets, theme, active }: {
  group: Extract<TranscriptItem, { kind: "group" }>; active: boolean; expanded: boolean; onToggle: () => void;
  expandedEvent: number | null; onToggleEvent: (id: number) => void; onOpenFile?: (path: string) => void; offsets: ScrollOffsets; theme: Theme;
}) {
  const scroll = useKeptScroll(offsets, `group:${group.id}`);
  return <Badge theme={theme} icon="Wrench" label={group.summary} expanded={expanded} onToggle={onToggle} loading={group.loading} borderless>
    <ScrollView {...scroll} style={{ maxHeight: 400 }} contentContainerStyle={{ paddingTop: 4, paddingHorizontal: 13 }} nestedScrollEnabled>
      {group.entries.map(({ event, updates }) => <TranscriptEvent key={event.id} event={event} updates={updates} active={active}
        expanded={expandedEvent === event.id} onToggle={() => onToggleEvent(event.id)} onOpenFile={onOpenFile} theme={theme} />)}
    </ScrollView>
  </Badge>;
}

export function WorkerPanel({ theme, host, layout, workspaceId, agentId, navigation }: PluginAgentPanelProps) {
  // The deep link exists only in Paseo's browser router (desktop and web).
  const openFile = layout.platform === "web" && host.id ? (path: string) => openWorkspaceFile(host.id, workspaceId, path) : undefined;
  const rpc = useRpc(readWorkerHistory);
  const contextKey = `${workspaceId}\u0000${agentId}`;
  // A remount (Paseo opening a file tab) resumes the exact last view.
  const [saved] = useState(() => savedWorkerView(agentId));
  const [history, setHistory] = useState<WorkerHistory | null>(saved?.history ?? null);
  // The coordinator card records its assignment before opening this panel.
  // Seed both states from that handoff so the first request is scoped to the
  // clicked worker, even when Paseo has just created a new workspace tab.
  const [selected, setSelected] = useState<string | null>(() => saved ? saved.selected : selectedWorker(agentId));
  const [selectedAssignmentRef, setSelectedAssignmentRef] = useState<string | null>(() => saved ? null : selectedWorker(agentId));
  const [nextCursor, setNextCursor] = useState<number | null>(saved?.nextCursor ?? null);
  const [nextAssignmentCursor, setNextAssignmentCursor] = useState<string | null>(saved?.nextAssignmentCursor ?? null);
  const [historyContext, setHistoryContext] = useState<string | null>(saved?.history ? contextKey : null);
  const [expandedEvent, setExpandedEvent] = useState<number | null>(saved?.expandedEvent ?? null);
  const [expandedGroups, setExpandedGroups] = useState<Set<number>>(() => new Set(saved?.expandedGroups));
  const [loading, setLoading] = useState(!saved?.history);
  const [readProblem, setReadProblem] = useState<HistoryReadProblem | null>(null);
  const mountedRef = useRef(true);
  const refreshInFlightRef = useRef<string | null>(null);
  const historyRef = useRef<WorkerHistory | null>(saved?.history ?? null);
  const scrollOffsets = useRef<Record<string, number>>(saved?.scroll ?? {});
  const restoringRef = useRef(saved !== null);
  // A worker opened from its coordinator card (mouse, Enter or Space) moves
  // keyboard focus to the opened worker's heading once it resolves, so the
  // destination is announced and Tab continues from it. A restored view
  // (remount) keeps focus where the host put it.
  const focusDestinationRef = useRef(saved === null && selectedWorker(agentId) !== null);
  const headingRef = useRef<{ focus?: (options?: { preventScroll?: boolean }) => void } | null>(null);
  const pageScroll = useKeptScroll(scrollOffsets, `page:${selected ?? "all"}`);
  const switcherScroll = useKeptScroll(scrollOffsets, `switcher:${selected ?? "all"}`, true);

  const selectionKey = `${contextKey}\u0000${selectedAssignmentRef ?? selected ?? "all"}`;
  const currentRequestRef = useRef(selectionKey);
  currentRequestRef.current = selectionKey;

  useEffect(() => {
    mountedRef.current = true;
    return () => { mountedRef.current = false; };
  }, []);

  useEffect(() => {
    const applySelection = () => {
      const assignmentRef = selectedWorker(agentId);
      focusDestinationRef.current = assignmentRef !== null;
      setSelectedAssignmentRef(assignmentRef);
      // Until the assignment response resolves, `selected` carries the
      // assignment ref. The refresh request recognizes the separate
      // assignmentRef field and resolves it to the exact attempt.
      setSelected(assignmentRef);
    };
    return subscribeWorkerSelection(applySelection);
  }, [agentId]);

  // Keep the whole view across a remount, written once as the panel goes
  // away. An unresolved chat handoff is not a view yet.
  const latestViewRef = useRef<Parameters<typeof saveWorkerView>[1] | null>(null);
  latestViewRef.current = selectedAssignmentRef ? null : { selected, history, historyContext, nextCursor, nextAssignmentCursor,
    expandedEvent, expandedGroups: [...expandedGroups], scroll: scrollOffsets.current };
  useEffect(() => () => {
    if (latestViewRef.current) saveWorkerView(agentId, latestViewRef.current);
  }, [agentId]);

  useEffect(() => {
    // The first run after a restoring remount must not wipe what it restored.
    if (restoringRef.current) { restoringRef.current = false; return; }
    scrollOffsets.current = {};
    setExpandedGroups(new Set());
    setLoading(true);
    setHistory(null);
    historyRef.current = null;
    setSelected(selectedWorker(agentId));
    setSelectedAssignmentRef(selectedWorker(agentId));
    setNextCursor(null);
    setNextAssignmentCursor(null);
    setHistoryContext(null);
    setExpandedEvent(null);
    setReadProblem(null);
  }, [contextKey]);

  const refresh = useCallback(async () => {
    const requestContext = contextKey;
    const requestKey = selectionKey;
    if (!mountedRef.current || refreshInFlightRef.current === requestKey) return;
    refreshInFlightRef.current = requestKey;
    try {
      const priorHistory = historyRef.current;
      setLoading(shouldShowWorkerHistoryLoader(priorHistory));
      // A context change clears historyRef before the first request, so a
      // retained history here is necessarily for this exact panel context.
      const incrementalCursor = selectedAssignmentRef || !selected ? 0 : historyCursor(priorHistory, selected);
      const result = await rpc({
        workspaceId,
        agentId,
        // Timeline rows carry the durable assignment key, not an attempt ID.
        // Resolve it against this task's returned assignment page below.
        attemptId: selectedAssignmentRef ? undefined : selected ?? undefined,
        assignmentRef: selectedAssignmentRef ?? undefined,
        ...(incrementalCursor > 0 ? { cursor: incrementalCursor } : {}),
      });
      if (!mountedRef.current || requestKey !== currentRequestRef.current) return;
      if (!result.available) {
        // Unavailable now: keep whatever was already shown (marked stale)
        // rather than merging an empty unavailable page over it.
        setReadProblem({ message: result.message?.trim() || HISTORY_READ_FAILED });
        if (priorHistory) return;
      } else {
        setReadProblem(null);
      }
      const displayed = priorHistory ? mergeWorkerHistory(priorHistory, result) : result;
      historyRef.current = displayed;
      setHistory(displayed);
      setHistoryContext(requestContext);
      setNextCursor(displayed.nextCursor);
      setNextAssignmentCursor(result.nextAssignmentCursor);
      if (selectedAssignmentRef) {
        const assignment = displayed.assignments.find((candidate) => candidate.id === selectedAssignmentRef || candidate.assignmentRef === selectedAssignmentRef);
        const attempt = assignment?.attempts[assignment.attempts.length - 1];
        if (attempt) {
          setSelectedAssignmentRef(null);
          setSelected(attempt.id);
        }
      }
    } catch {
      // The selection and any retained history stay as they are; the
      // notice offers a retry (a terminal worker no longer polls).
      if (mountedRef.current && requestKey === currentRequestRef.current) setReadProblem({ message: HISTORY_READ_FAILED });
    }
    finally {
      if (refreshInFlightRef.current === requestKey) refreshInFlightRef.current = null;
      if (mountedRef.current && requestKey === currentRequestRef.current) setLoading(false);
    }
  }, [agentId, contextKey, selectionKey, rpc, selected, selectedAssignmentRef, workspaceId]);

  const loadMore = useCallback(async () => {
    if (!nextCursor || !selected) return;
    const requestContext = contextKey;
    const requestKey = selectionKey;
    let result: WorkerHistory;
    try {
      result = await rpc({ workspaceId, agentId, attemptId: selected, cursor: nextCursor });
    } catch {
      if (mountedRef.current && requestKey === currentRequestRef.current) setReadProblem({ message: HISTORY_READ_FAILED });
      return;
    }
    if (requestKey !== currentRequestRef.current) return;
    if (!result.available) { setReadProblem({ message: result.message?.trim() || HISTORY_READ_FAILED }); return; }
    const displayed = historyRef.current ? mergeWorkerHistory(historyRef.current, result) : result;
    historyRef.current = displayed;
    setHistory(displayed);
    setNextCursor(result.nextCursor);
  }, [agentId, contextKey, selectionKey, nextCursor, rpc, selected, workspaceId]);

  const loadMoreWorkers = useCallback(async () => {
    if (!nextAssignmentCursor) return;
    const requestContext = contextKey;
    const requestKey = selectionKey;
    let result: WorkerHistory;
    try {
      result = await rpc({ workspaceId, agentId, assignmentCursor: nextAssignmentCursor });
    } catch {
      if (mountedRef.current && requestKey === currentRequestRef.current) setReadProblem({ message: HISTORY_READ_FAILED });
      return;
    }
    if (requestKey !== currentRequestRef.current) return;
    if (!result.available) { setReadProblem({ message: result.message?.trim() || HISTORY_READ_FAILED }); return; }
    const displayed = historyRef.current ? mergeWorkerHistory(historyRef.current, result) : result;
    historyRef.current = displayed;
    setHistory(displayed);
    setNextAssignmentCursor(result.nextAssignmentCursor);
  }, [agentId, contextKey, selectionKey, nextAssignmentCursor, rpc, workspaceId]);
  useEffect(() => { void refresh(); }, [refresh]);
  useEffect(() => {
    if (!shouldPollWorkerHistory(history)) {
      return;
    }
    const timer = setInterval(() => {
      void refresh();
    }, WORKER_HISTORY_POLL_MS);
    return () => clearInterval(timer);
  }, [history, refresh]);

  const currentHistory = historyContext === contextKey ? history : null;
  const selectedAttempt = currentHistory?.assignments.flatMap((a) => a.attempts).find((a) => a.id === selected);
  const selectedAssignment = currentHistory?.assignments.find((a) => a.attempts.some((attempt) => attempt.id === selected));
  const visibleAssignments = currentHistory?.assignments.filter((assignment) => {
    const attempt = assignment.attempts[assignment.attempts.length - 1];
    return assignment.id === selectedAssignmentRef || assignment.assignmentRef === selectedAssignmentRef || workerSection(attempt?.status ?? assignment.status, attempt?.events.length ?? 0) !== null;
  }) ?? [];
  const activeAssignments = visibleAssignments.filter((assignment) => {
    const attempt = assignment.attempts[assignment.attempts.length - 1];
    return workerSection(attempt?.status ?? assignment.status, attempt?.events.length ?? 0) === "active";
  });
  const completedAssignments = visibleAssignments.filter((assignment) => {
    const attempt = assignment.attempts[assignment.attempts.length - 1];
    return workerSection(attempt?.status ?? assignment.status, attempt?.events.length ?? 0) === "completed";
  });
  // Each card repeats the worker's chat row: its task, then the grey
  // `profile · provider · model · effort · status` line.
  const workerCard = (assignment: WorkerHistory["assignments"][number]) => {
    const attempt = assignment.attempts[assignment.attempts.length - 1];
    const line = workerAssignmentLine(assignment, workerPopoverStatus(assignment));
    return <Pressable key={assignment.id} accessibilityRole="button" accessibilityLabel={`Open worker ${line.workerName}`} onPress={() => attempt && setSelected(attempt.id)} style={[styles.card, { borderColor: theme.colors.border, backgroundColor: theme.colors.surface1 }]}>
      <Text numberOfLines={2} ellipsizeMode="tail" style={{ color: theme.colors.foreground, fontSize: 14, fontWeight: "600" }}>{line.workerName}</Text>
      <Text numberOfLines={1} ellipsizeMode="tail" style={{ color: theme.colors.foregroundMuted, fontSize: 12 }}>{line.subtitle}</Text>
      <UsageMeter context={attempt?.context} tokens={attempt?.tokens} theme={theme} unavailableText={contextUnavailableText(attempt?.route?.provider ?? assignment.route?.provider)} />
      {attempt && assignment.attempts.length > 1 && <Text style={[styles.muted, { color: theme.colors.foregroundMuted }]}>Attempt {attempt.attemptNo} · {assignment.attempts.length} attempts</Text>}
    </Pressable>;
  };
  // The Result section already shows this attempt's summary; its identical
  // worker_result row is not repeated in the activity below.
  const shownSummary = attemptResultView(selectedAttempt)?.summary ?? null;
  const transcript = selectedAttempt ? groupWorkerEvents(withoutShownResult(selectedAttempt.events, shownSummary)) : [];
  const route = selectedAttempt?.route ?? selectedAssignment?.route;
  const selectedTerminal = workerTerminalState(selectedAssignment ? workerPopoverStatus(selectedAssignment) : selectedAttempt?.status, selectedAttempt?.checks);
  // A worker opened from its card whose durable history has no attempt yet
  // (e.g. it could not be started) still shows why, from the card's own
  // host-provided reason, once the read has settled.
  const pendingAssignment = selectedAssignmentRef
    ? currentHistory?.assignments.find((assignment) => assignment.id === selectedAssignmentRef || assignment.assignmentRef === selectedAssignmentRef)
    : undefined;
  const handoffReason = selectedAssignmentRef && !loading && !(pendingAssignment && pendingAssignment.attempts.length > 0)
    ? selectedWorkerDetail(agentId, selectedAssignmentRef)?.failureReason ?? null : null;
  const readNotice = readProblem
    ? <HistoryReadNotice problem={readProblem} stale={Boolean(currentHistory?.available)} onRetry={() => void refresh()} theme={theme} />
    : null;
  const selectedAttemptId = selectedAttempt?.id ?? null;
  useEffect(() => {
    if (!selectedAttemptId || !focusDestinationRef.current) return;
    focusDestinationRef.current = false;
    headingRef.current?.focus?.({ preventScroll: true });
  }, [selectedAttemptId]);
  const selectedStatusColor = selectedTerminal.tone === "success" ? theme.colors.statusSuccess : selectedTerminal.tone === "danger" ? theme.colors.statusDanger : selectedTerminal.tone === "muted" ? theme.colors.foregroundMuted : selectedTerminal.tone === "warning" ? theme.colors.statusWarning : theme.colors.accent;
  return (
    <ScrollView {...pageScroll} style={{ flex: 1, backgroundColor: theme.colors.surface0 }} contentContainerStyle={{ paddingHorizontal: layout.compact ? 12 : 16, paddingTop: 15, paddingBottom: 28, gap: 12 }}>
      {!selectedAttempt ? <>
        <Text style={[styles.title, { color: theme.colors.foreground }]}>Workers</Text>
        {loading && <Text style={[styles.muted, { color: theme.colors.foregroundMuted }]}>Loading worker history…</Text>}
        {!loading && readNotice}
        {!loading && !readProblem && currentHistory && !currentHistory.available && <Text style={[styles.muted, { color: theme.colors.statusWarning }]}>{currentHistory.message}</Text>}
        {handoffReason && <View style={[styles.card, { borderColor: theme.colors.border, backgroundColor: theme.colors.surface1 }]}>
          <Text style={[styles.muted, { color: theme.colors.foregroundMuted, fontWeight: "600" }]}>Selected worker has no attempt history</Text>
          <Text selectable style={[styles.body, { color: theme.colors.statusDanger }]}>{handoffReason}</Text>
        </View>}
        {!loading && !readProblem && currentHistory?.available && currentHistory.assignments.length === 0 && <Text style={[styles.muted, { color: theme.colors.foregroundMuted }]}>No worker assignments recorded yet.</Text>}
        {currentHistory?.truncated && <Text style={[styles.muted, { color: theme.colors.statusWarning }]}>History is bounded; open a specific attempt for its retained events.</Text>}
        {activeAssignments.length > 0 && <Text style={[styles.muted, { color: theme.colors.foregroundMuted, fontWeight: "600" }]}>Active</Text>}
        {activeAssignments.map(workerCard)}
        {completedAssignments.length > 0 && <Text style={[styles.muted, { color: theme.colors.foregroundMuted, fontWeight: "600" }]}>Completed</Text>}
        {completedAssignments.map(workerCard)}
        {nextAssignmentCursor && <Pressable accessibilityRole="button" onPress={() => void loadMoreWorkers()} style={[styles.button, { backgroundColor: theme.colors.surface2 }]}><Text style={{ color: theme.colors.accent }}>Load more workers</Text></Pressable>}
      </> : <>
        <Pressable accessibilityRole="button" accessibilityLabel="Back to all workers" onPress={() => setSelected(null)} style={{ alignSelf: "flex-start", paddingVertical: 2 }}><Text style={[styles.muted, { color: theme.colors.accent }]}>‹  All workers</Text></Pressable>
        <View style={{ gap: 8, paddingBottom: 14, borderBottomWidth: 1, borderColor: theme.colors.border }}>
          <View ref={headingRef as never} tabIndex={-1} accessibilityRole="header">
            <Text style={[styles.title, { color: theme.colors.foreground }]}>{selectedAssignment?.title?.trim() || "Worker"}</Text>
          </View>
          <View style={{ flexDirection: "row", flexWrap: "wrap", gap: 7, alignItems: "center" }}>
            <Text style={[styles.muted, { color: selectedStatusColor }]}>{selectedAssignment ? workerDisplayName(selectedAssignment) : "worker"}</Text>
            {route && <><Text style={[styles.muted, { color: theme.colors.foregroundMuted }]}>·</Text><Text style={[styles.muted, { color: theme.colors.foregroundMuted }]}>{route.provider}</Text><Text style={[styles.muted, { color: theme.colors.foregroundMuted }]}>·</Text><Text style={[styles.muted, { color: theme.colors.foregroundMuted }]}>{workerModelDisplay(route)}</Text>{route.effort && <><Text style={[styles.muted, { color: theme.colors.foregroundMuted }]}>·</Text><Text style={[styles.muted, { color: theme.colors.foregroundMuted }]}>{route.effort} effort</Text></>}</>}
            {/* Keyed by attempt: switching the selection remounts the
                indicator, so a previous attempt's pulse never carries over. */}
            <WorkingPulse key={selectedAttempt.id} active={selectedTerminal.label === "working" && isWorkerExecutingStatus(selectedAttempt.status)}>
              <Icon name={selectedTerminal.icon} size={13} color={selectedStatusColor} />
            </WorkingPulse>
            <Text style={[styles.muted, { color: selectedStatusColor }]}>· {selectedTerminal.label}{selectedTerminal.issueCount > 0 ? ` · ${selectedTerminal.issueCount} issue${selectedTerminal.issueCount === 1 ? "" : "s"}` : ""}</Text>
          </View>
          {routeSelectionText(route?.selection, route?.provider) && <Text style={[styles.muted, { color: theme.colors.foregroundMuted }]}>Route choice: {routeSelectionText(route?.selection, route?.provider)}</Text>}
          <UsageMeter context={selectedAttempt.context} tokens={selectedAttempt.tokens} theme={theme} unavailableText={contextUnavailableText(route?.provider)} />
          {selectedAssignment && selectedAssignment.attempts.length > 1 && <View style={{ flexDirection: "row", flexWrap: "wrap", gap: 6 }}>
            {selectedAssignment.attempts.map((attempt) => <Pressable key={attempt.id} accessibilityRole="button" accessibilityState={{ selected: attempt.id === selectedAttempt.id }} onPress={() => { setExpandedEvent(null); setSelected(attempt.id); }} style={[styles.button, { backgroundColor: attempt.id === selectedAttempt.id ? theme.colors.surface2 : theme.colors.surface1, borderWidth: 1, borderColor: theme.colors.border }]}><Text style={[styles.muted, { color: attempt.id === selectedAttempt.id ? theme.colors.foreground : theme.colors.foregroundMuted }]}>Attempt {attempt.attemptNo}</Text></Pressable>)}
          </View>}
          {currentHistory && currentHistory.assignments.length > 1 && <ScrollView {...switcherScroll} horizontal showsHorizontalScrollIndicator={false} contentContainerStyle={{ gap: 12, alignItems: "center" }}>
            {currentHistory.assignments.filter((assignment) => assignment.id !== selectedAssignment?.id).map((assignment) => {
              const latest = assignment.attempts[assignment.attempts.length - 1];
              return latest ? <Pressable key={assignment.id} accessibilityRole="button" accessibilityLabel={`Switch to ${workerDisplayName(assignment)}`} onPress={() => { setExpandedEvent(null); setSelected(latest.id); }}><Text style={[styles.muted, { color: theme.colors.accent }]}>{workerDisplayName(assignment)}  ›</Text></Pressable> : null;
            })}
          </ScrollView>}
          {selectedAttempt.agentId && navigation?.openAgent && <Pressable accessibilityRole="button" onPress={() => navigation.openAgent({ agentId: selectedAttempt.agentId! })}><Text style={[styles.muted, { color: theme.colors.accent }]}>Open full chat ↗</Text></Pressable>}
        </View>
        {selectedAssignment?.assignmentRef && selectedAssignment.attempts[selectedAssignment.attempts.length - 1]?.id === selectedAttempt.id
          && <RunningCommands theme={theme} workspaceId={workspaceId} agentId={agentId} assignmentRef={selectedAssignment.assignmentRef}
            active={workerSection(selectedAttempt.status, selectedAttempt.events.length) === "active"} />}
        {readNotice}
        {/* The assignment text the runtime sent to this exact attempt. */}
        <WorkerAttemptPrompt key={selectedAttempt.id} prompt={selectedAttempt.prompt} theme={theme} />
        {/* The terminal result and every check of this exact attempt: the
            detail the coordinator card no longer carries. */}
        <WorkerAttemptResult attempt={selectedAttempt} theme={theme} />
        <View style={{ gap: 2 }}>
          {selectedAttempt.failureReason && <Text selectable style={[styles.body, { color: selectedAttempt.status === "interrupted" ? theme.colors.statusWarning : theme.colors.statusDanger }]}>{selectedAttempt.failureReason}</Text>}
          {groupToolRuns(transcript, workerSection(selectedAttempt.status, selectedAttempt.events.length) !== "completed").map((item) => item.kind === "group"
            ? <ToolGroup key={`group-${item.id}`} group={item} theme={theme} active={workerSection(selectedAttempt.status, selectedAttempt.events.length) !== "completed"}
                expanded={expandedGroups.has(item.id)}
                onToggle={() => setExpandedGroups((current) => { const next = new Set(current); next.has(item.id) ? next.delete(item.id) : next.add(item.id); return next; })}
                expandedEvent={expandedEvent} onToggleEvent={(id) => setExpandedEvent(expandedEvent === id ? null : id)} onOpenFile={openFile} offsets={scrollOffsets} />
            : <TranscriptEvent key={item.entry.event.id} event={item.entry.event} updates={item.entry.updates} active={workerSection(selectedAttempt.status, selectedAttempt.events.length) !== "completed"} expanded={expandedEvent === item.entry.event.id} onToggle={() => setExpandedEvent(expandedEvent === item.entry.event.id ? null : item.entry.event.id)} onOpenFile={openFile} theme={theme} />)}
          {selectedAttempt.events.length === 0 && <Text style={[styles.muted, { color: theme.colors.foregroundMuted }]}>No durable events are available for this attempt.</Text>}
          {nextCursor && <Pressable accessibilityRole="button" onPress={() => void loadMore()} style={[styles.button, { marginTop: 12, borderWidth: 1, borderColor: theme.colors.border, backgroundColor: theme.colors.surface1 }]}><Text style={[styles.muted, { color: theme.colors.accent }]}>Load more history</Text></Pressable>}
          {currentHistory?.truncated && !nextCursor && <Text style={[styles.muted, { color: theme.colors.foregroundMuted }]}>Some earlier history is unavailable.</Text>}
        </View>
      </>}
    </ScrollView>
  );
}
