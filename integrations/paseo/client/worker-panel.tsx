import { readableToolText } from "./tool-details.ts";
import { useRpc, type PluginAgentPanelProps } from "@getpaseo/plugin/client";
import { Icon, ScrollView } from "@getpaseo/plugin/client/react-native";
import { Pressable, Text, View } from "react-native";
import React, { useCallback, useEffect, useRef, useState } from "react";
import { readWorkerHistory, type WorkerHistory } from "../shared/worker-history.js";
import { savedWorkerView, saveWorkerView, selectedWorker, subscribeWorkerSelection } from "./worker-selection.js";
import { workerSection, workerStatus } from "./worker-panel-status.js";
import { historyCursor, mergeWorkerHistory, shouldPollWorkerHistory, shouldShowWorkerHistoryLoader, WORKER_HISTORY_POLL_MS } from "./worker-panel-refresh.js";
import { groupWorkerEvents, type WorkerEvent } from "./worker-panel-events.js";
import { presentTool } from "./worker-panel-tool.js";
import { diffBlocks, diffLines, rawText, type DiffBlock, type DiffRow } from "./worker-panel-diff.js";
import { chatToolIcon, groupToolRuns, isToolEvent, nativeRow, toolSemantics, type TranscriptItem } from "./worker-panel-groups.ts";
import { workerAssignmentLine, workerDisplayName } from "./worker-display.js";
import { workerPopoverStatus } from "./worker-popover-status.js";
import { openWorkspaceFile } from "./open-workspace-file.ts";

const styles = {
  title: { fontSize: 18, lineHeight: 24, fontWeight: "600" as const },
  muted: { fontSize: 13, lineHeight: 19 },
  body: { fontSize: 15, lineHeight: 23 },
  card: { borderWidth: 1, borderRadius: 9, padding: 12, gap: 5 },
  row: { flexDirection: "row" as const, alignItems: "center" as const, justifyContent: "space-between" as const, gap: 8 },
  button: { paddingVertical: 6, paddingHorizontal: 9, borderRadius: 6, alignSelf: "flex-start" as const },
  mono: { fontFamily: "monospace" as const, fontSize: 12, lineHeight: 18 },
};

type NumberedDiffRow = DiffRow & { oldLine: number | null; newLine: number | null };

function numberDiffRows(rows: DiffRow[]): NumberedDiffRow[] {
  let oldLine = 1;
  let newLine = 1;
  return rows.map((row) => {
    if (row.kind === "context") return { ...row, oldLine: oldLine++, newLine: newLine++ };
    if (row.kind === "delete") return { ...row, oldLine: oldLine++, newLine: null };
    return { ...row, oldLine: null, newLine: newLine++ };
  });
}

export { diffBlocks, diffLines, rawText } from "./worker-panel-diff.js";

type Theme = PluginAgentPanelProps["theme"];

function tint(color: string, fallback: string): string {
  return /^#[0-9a-fA-F]{6}$/.test(color) ? `${color}1c` : fallback;
}

function FileDiff({ path, rows, theme, showPath = true }: { path: string; rows: DiffRow[]; theme: Theme; showPath?: boolean }) {
  const numberedRows = numberDiffRows(rows);
  return <View style={{ borderWidth: 1, borderColor: theme.colors.border, borderRadius: 6, overflow: "hidden", marginTop: 5 }}>
    {showPath && <Text selectable style={[styles.mono, { paddingHorizontal: 10, paddingVertical: 7, color: theme.colors.foregroundMuted, backgroundColor: theme.colors.surface2 }]}>{path}</Text>}
    <View style={{ paddingVertical: 5, backgroundColor: theme.colors.surface0 }}>
      {numberedRows.map((row, index) => <Text key={`${row.kind}-${index}`} selectable accessibilityLabel={`${row.kind} line`} style={[styles.mono, {
        paddingHorizontal: 10,
        color: row.kind === "context" ? theme.colors.foregroundMuted : theme.colors.foreground,
        backgroundColor: row.kind === "add" ? tint(theme.colors.statusSuccess, theme.colors.surface1) : row.kind === "delete" ? tint(theme.colors.statusDanger, theme.colors.surface1) : theme.colors.surface0,
      }]}>{`${row.oldLine ?? ""}`.padStart(4)} ${`${row.newLine ?? ""}`.padStart(4)} ${row.kind === "delete" ? "−" : row.kind === "add" ? "+" : " "} {row.text || " "}</Text>)}
    </View>
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

export function TranscriptEvent({ event, expanded, onToggle, onOpenFile, theme }: { event: WorkerEvent; updates: WorkerEvent[]; expanded: boolean; onToggle: () => void; onOpenFile?: (path: string) => void; theme: Theme }) {
  const tool = isToolEvent(event);
  const failure = event.status === "failed" || (event.status !== "completed" && event.type.toLowerCase().includes("failed"));
  const final = /result|final|complete/.test(event.type.toLowerCase());
  const message = cleanEventText(event);
  if (!tool) return <View style={{ marginVertical: final ? 13 : 9, gap: 6 }}>
    {message && <Text selectable style={[styles.body, { color: theme.colors.foreground }]}>{message}</Text>}
  </View>;
  const presentation = presentTool(event);
  const semantics = toolSemantics(event);
  const row = nativeRow(event);
  const diffs = diffBlocks(event.content ?? (event.input && typeof event.input === "object" ? (event.input as Record<string, unknown>).content : null));
  const output = readableToolText(presentation.kind === "shell" ? presentation.output : event.raw_output ?? event.output ?? event.content);
  const error = failure ? readableToolText(event.error) : null;
  const hasEdit = presentation.kind === "edit" && (diffs.length > 0 || presentation.newText !== null);
  const canExpand = semantics.expandable && Boolean(output || error || hasEdit || diffs.length);
  const running = event.status === "in_progress" || event.status === "pending" || event.status === "running";
  return <Badge theme={theme} icon={chatToolIcon(event)} label={row.label} summary={row.summary} expanded={expanded && canExpand}
    onToggle={canExpand ? onToggle : undefined} loading={running} failed={failure}
    onOpenFile={onOpenFile && row.summary && (row.label === "Read" || row.label === "Edit") ? () => onOpenFile(row.summary!) : undefined}>
    <View style={{ paddingHorizontal: 10, paddingVertical: 9, gap: 8 }}>
      {presentation.kind === "shell" && presentation.command && <Text selectable style={[styles.mono, { color: theme.colors.foreground }]}>{`$ ${presentation.command}`}</Text>}
      {output && !hasEdit && <Text selectable style={[styles.mono, { color: theme.colors.foreground }]}>{output}</Text>}
      {diffs.map((diff, i) => <FileDiff key={`${diff.path}-${i}`} path={diff.path} rows={diffLines(diff.oldText, diff.newText)} theme={theme} showPath />)}
      {hasEdit && diffs.length === 0 && <FileDiff path={presentation.path ?? "File changes"} rows={diffLines(presentation.oldText, presentation.newText)} theme={theme} showPath />}
      {error && error !== output && <Text selectable style={[styles.muted, { color: theme.colors.statusDanger }]}>{error}</Text>}
    </View>
  </Badge>;
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
function ToolGroup({ group, expanded, onToggle, expandedEvent, onToggleEvent, onOpenFile, offsets, theme }: {
  group: Extract<TranscriptItem, { kind: "group" }>; expanded: boolean; onToggle: () => void;
  expandedEvent: number | null; onToggleEvent: (id: number) => void; onOpenFile?: (path: string) => void; offsets: ScrollOffsets; theme: Theme;
}) {
  const scroll = useKeptScroll(offsets, `group:${group.id}`);
  return <Badge theme={theme} icon="Wrench" label={group.summary} expanded={expanded} onToggle={onToggle} loading={group.loading} borderless>
    <ScrollView {...scroll} style={{ maxHeight: 400 }} contentContainerStyle={{ paddingTop: 4, paddingHorizontal: 13 }} nestedScrollEnabled>
      {group.entries.map(({ event, updates }) => <TranscriptEvent key={event.id} event={event} updates={updates}
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
  const mountedRef = useRef(true);
  const refreshInFlightRef = useRef<string | null>(null);
  const historyRef = useRef<WorkerHistory | null>(saved?.history ?? null);
  const scrollOffsets = useRef<Record<string, number>>(saved?.scroll ?? {});
  const restoringRef = useRef(saved !== null);
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
    const result = await rpc({ workspaceId, agentId, attemptId: selected, cursor: nextCursor });
    if (requestKey !== currentRequestRef.current) return;
    const displayed = historyRef.current ? mergeWorkerHistory(historyRef.current, result) : result;
    historyRef.current = displayed;
    setHistory(displayed);
    setNextCursor(result.nextCursor);
  }, [agentId, contextKey, selectionKey, nextCursor, rpc, selected, workspaceId]);

  const loadMoreWorkers = useCallback(async () => {
    if (!nextAssignmentCursor) return;
    const requestContext = contextKey;
    const requestKey = selectionKey;
    const result = await rpc({ workspaceId, agentId, assignmentCursor: nextAssignmentCursor });
    if (requestKey !== currentRequestRef.current) return;
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
      {attempt && assignment.attempts.length > 1 && <Text style={[styles.muted, { color: theme.colors.foregroundMuted }]}>Attempt {attempt.attemptNo} · {assignment.attempts.length} attempts</Text>}
    </Pressable>;
  };
  const transcript = selectedAttempt ? groupWorkerEvents(selectedAttempt.events) : [];
  const route = selectedAttempt?.route ?? selectedAssignment?.route;
  return (
    <ScrollView {...pageScroll} style={{ flex: 1, backgroundColor: theme.colors.surface0 }} contentContainerStyle={{ paddingHorizontal: layout.compact ? 12 : 16, paddingTop: 15, paddingBottom: 28, gap: 12 }}>
      {!selectedAttempt ? <>
        <Text style={[styles.title, { color: theme.colors.foreground }]}>Workers</Text>
        {loading && <Text style={[styles.muted, { color: theme.colors.foregroundMuted }]}>Loading worker history…</Text>}
        {!loading && currentHistory && !currentHistory.available && <Text style={[styles.muted, { color: theme.colors.statusWarning }]}>{currentHistory.message}</Text>}
        {!loading && currentHistory?.available && currentHistory.assignments.length === 0 && <Text style={[styles.muted, { color: theme.colors.foregroundMuted }]}>No worker assignments recorded yet.</Text>}
        {currentHistory?.truncated && <Text style={[styles.muted, { color: theme.colors.statusWarning }]}>History is bounded; open a specific attempt for its retained events.</Text>}
        {activeAssignments.length > 0 && <Text style={[styles.muted, { color: theme.colors.foregroundMuted, fontWeight: "600" }]}>Active</Text>}
        {activeAssignments.map(workerCard)}
        {completedAssignments.length > 0 && <Text style={[styles.muted, { color: theme.colors.foregroundMuted, fontWeight: "600" }]}>Completed</Text>}
        {completedAssignments.map(workerCard)}
        {nextAssignmentCursor && <Pressable accessibilityRole="button" onPress={() => void loadMoreWorkers()} style={[styles.button, { backgroundColor: theme.colors.surface2 }]}><Text style={{ color: theme.colors.accent }}>Load more workers</Text></Pressable>}
      </> : <>
        <Pressable accessibilityRole="button" accessibilityLabel="Back to all workers" onPress={() => setSelected(null)} style={{ alignSelf: "flex-start", paddingVertical: 2 }}><Text style={[styles.muted, { color: theme.colors.accent }]}>‹  All workers</Text></Pressable>
        <View style={{ gap: 8, paddingBottom: 14, borderBottomWidth: 1, borderColor: theme.colors.border }}>
          <Text style={[styles.title, { color: theme.colors.foreground }]}>{selectedAssignment?.title?.trim() || "Worker"}</Text>
          <View style={{ flexDirection: "row", flexWrap: "wrap", gap: 7, alignItems: "center" }}>
            <Text style={[styles.muted, { color: selectedAttempt.status === "failed" ? theme.colors.statusDanger : theme.colors.foregroundMuted }]}>{selectedAssignment ? workerDisplayName(selectedAssignment) : "worker"}</Text>
            {route && <><Text style={[styles.muted, { color: theme.colors.foregroundMuted }]}>·</Text><Text style={[styles.muted, { color: theme.colors.foregroundMuted }]}>{route.provider}</Text><Text style={[styles.muted, { color: theme.colors.foregroundMuted }]}>·</Text><Text style={[styles.muted, { color: theme.colors.foregroundMuted }]}>{route.model}</Text>{route.effort && <><Text style={[styles.muted, { color: theme.colors.foregroundMuted }]}>·</Text><Text style={[styles.muted, { color: theme.colors.foregroundMuted }]}>{route.effort} effort</Text></>}</>}
            <Text style={[styles.muted, { color: selectedAttempt.status === "failed" ? theme.colors.statusDanger : theme.colors.foregroundMuted }]}>· {workerStatus(selectedAttempt.status, selectedAttempt.events.length)}</Text>
          </View>
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
        <View style={{ gap: 2 }}>
          {selectedAttempt.failureReason && <Text selectable style={[styles.body, { color: theme.colors.statusDanger }]}>{selectedAttempt.failureReason}</Text>}
          {groupToolRuns(transcript).map((item) => item.kind === "group"
            ? <ToolGroup key={`group-${item.id}`} group={item} theme={theme}
                expanded={expandedGroups.has(item.id)}
                onToggle={() => setExpandedGroups((current) => { const next = new Set(current); next.has(item.id) ? next.delete(item.id) : next.add(item.id); return next; })}
                expandedEvent={expandedEvent} onToggleEvent={(id) => setExpandedEvent(expandedEvent === id ? null : id)} onOpenFile={openFile} offsets={scrollOffsets} />
            : <TranscriptEvent key={item.entry.event.id} event={item.entry.event} updates={item.entry.updates} expanded={expandedEvent === item.entry.event.id} onToggle={() => setExpandedEvent(expandedEvent === item.entry.event.id ? null : item.entry.event.id)} onOpenFile={openFile} theme={theme} />)}
          {selectedAttempt.events.length === 0 && <Text style={[styles.muted, { color: theme.colors.foregroundMuted }]}>No durable events are available for this attempt.</Text>}
          {nextCursor && <Pressable accessibilityRole="button" onPress={() => void loadMore()} style={[styles.button, { marginTop: 12, borderWidth: 1, borderColor: theme.colors.border, backgroundColor: theme.colors.surface1 }]}><Text style={[styles.muted, { color: theme.colors.accent }]}>Load more history</Text></Pressable>}
          {currentHistory?.truncated && !nextCursor && <Text style={[styles.muted, { color: theme.colors.foregroundMuted }]}>Some earlier history is unavailable.</Text>}
        </View>
      </>}
    </ScrollView>
  );
}
