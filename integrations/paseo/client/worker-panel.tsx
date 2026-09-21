import { useRpc, type PluginAgentPanelProps } from "@getpaseo/plugin/client";
import { Icon, ScrollView } from "@getpaseo/plugin/client/react-native";
import { Pressable, Text, View } from "react-native";
import { useCallback, useEffect, useRef, useState } from "react";
import { readWorkerHistory, type WorkerHistory } from "../shared/worker-history.js";
import { selectedWorker, subscribeWorkerSelection } from "./worker-selection.js";
import { workerSection, workerStatus } from "./worker-panel-status.js";
import { historyCursor, mergeWorkerHistory, shouldPollWorkerHistory, shouldShowWorkerHistoryLoader, WORKER_HISTORY_POLL_MS } from "./worker-panel-refresh.js";
import { groupWorkerEvents, type WorkerEvent } from "./worker-panel-events.js";
import { presentTool } from "./worker-panel-tool.js";
import { presentToolSemantics } from "../shared/tool-presentation.js";
import { workerDisplayName } from "./worker-display.js";

const styles = {
  title: { fontSize: 18, lineHeight: 24, fontWeight: "600" as const },
  muted: { fontSize: 13, lineHeight: 19 },
  body: { fontSize: 15, lineHeight: 23 },
  card: { borderWidth: 1, borderRadius: 9, padding: 12, gap: 5 },
  row: { flexDirection: "row" as const, alignItems: "center" as const, justifyContent: "space-between" as const, gap: 8 },
  button: { paddingVertical: 6, paddingHorizontal: 9, borderRadius: 6, alignSelf: "flex-start" as const },
  mono: { fontFamily: "monospace" as const, fontSize: 12, lineHeight: 18 },
};

type DiffBlock = { path: string; oldText: string | null; newText: string | null };
type DiffRow = { kind: "context" | "add" | "delete"; text: string };
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

function rawText(value: unknown): string | null {
  if (value == null) return null;
  if (typeof value === "string") return value;
  try { return JSON.stringify(value, null, 2); } catch { return String(value); }
}

function diffBlocks(value: unknown): DiffBlock[] {
  const entries = Array.isArray(value) ? value : [value];
  return entries.flatMap((item) => {
    if (typeof item !== "object" || item === null) return [];
    const block = item as Record<string, unknown>;
    if (block.type !== "diff" || typeof block.path !== "string" || (block.oldText !== null && typeof block.oldText !== "string") || (block.newText !== null && typeof block.newText !== "string")) return [];
    return [{ path: block.path, oldText: block.oldText, newText: block.newText }];
  });
}

export function diffLines(oldText: string | null, newText: string | null): DiffRow[] {
  const split = (text: string | null) => {
    if (text == null || text.length === 0) return [];
    const normalized = text.replace(/\r\n?/g, "\n");
    const lines = normalized.split("\n");
    return lines[lines.length - 1] === "" ? lines.slice(0, -1) : lines;
  };
  const oldLines = split(oldText);
  const newLines = split(newText);
  if (oldLines.length === 0) return newLines.map((text) => ({ kind: "add", text }));
  if (newLines.length === 0) return oldLines.map((text) => ({ kind: "delete", text }));
  // Keep the exact content while avoiding an unbounded quadratic matrix.
  if (oldLines.length * newLines.length > 200_000) {
    return [...oldLines.map((text) => ({ kind: "delete" as const, text })), ...newLines.map((text) => ({ kind: "add" as const, text }))];
  }
  const table = Array.from({ length: oldLines.length + 1 }, () => new Uint32Array(newLines.length + 1));
  for (let i = oldLines.length - 1; i >= 0; i -= 1) for (let j = newLines.length - 1; j >= 0; j -= 1) {
    table[i][j] = oldLines[i] === newLines[j] ? table[i + 1][j + 1] + 1 : Math.max(table[i + 1][j], table[i][j + 1]);
  }
  const rows: DiffRow[] = [];
  let i = 0; let j = 0;
  while (i < oldLines.length && j < newLines.length) {
    if (oldLines[i] === newLines[j]) { rows.push({ kind: "context", text: oldLines[i] }); i += 1; j += 1; }
    else if (table[i + 1][j] >= table[i][j + 1]) { rows.push({ kind: "delete", text: oldLines[i] }); i += 1; }
    else { rows.push({ kind: "add", text: newLines[j] }); j += 1; }
  }
  while (i < oldLines.length) rows.push({ kind: "delete", text: oldLines[i++] });
  while (j < newLines.length) rows.push({ kind: "add", text: newLines[j++] });
  return rows;
}

type Theme = PluginAgentPanelProps["theme"];

function Detail({ label, value, theme }: { label: string; value: string; theme: Theme }) {
  return <View style={{ gap: 4 }}>
    <Text style={[styles.muted, { color: theme.colors.foregroundMuted }]}>{label}</Text>
    <Text selectable style={[styles.mono, { color: theme.colors.foreground, backgroundColor: theme.colors.surface0, borderRadius: 5, padding: 8 }]}>{value}</Text>
  </View>;
}

function tint(color: string, fallback: string): string {
  return /^#[0-9a-fA-F]{6}$/.test(color) ? `${color}1c` : fallback;
}

/**
 * Paseo 0.8 does not export provider logo assets. Keep the provider cue
 * portable by using a small, accessible text mark instead of an unverified
 * host-private Icon name.
 */
export function providerMark(provider: string): { glyph: string; label: string } {
  const normalized = provider.trim().toLowerCase();
  if (normalized.includes("claude")) return { glyph: "✦", label: "Claude" };
  if (normalized.includes("codex") || normalized.includes("openai")) return { glyph: "⌘", label: "Codex" };
  if (normalized.includes("cortex")) return { glyph: "◈", label: "Cortex" };
  return { glyph: "◆", label: provider || "Unknown provider" };
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

function eventHasPayload(event: WorkerEvent): boolean {
  return [event.raw_input, event.input, event.raw_output, event.output, event.content, event.diff, event.locations, event.error].some((value) => value != null);
}

function TranscriptEvent({ event, updates, expanded, onToggle, theme }: { event: WorkerEvent; updates: WorkerEvent[]; expanded: boolean; onToggle: () => void; theme: Theme }) {
  const [rawExpanded, setRawExpanded] = useState(false);
  const nativeToolTitle = /^(read|view|cat|edit|patch|write|replace|modify|shell|bash|terminal|execute|run|skill|governance|decide|search)\b/i.test(event.title ?? "");
  const tool = event.tool_kind != null || event.activity_ref?.startsWith("tool:") || event.activity_ref?.startsWith("tool-error:") || event.type.toLowerCase().includes("tool") || /^mcp(?:__|\.)/.test(event.title ?? "") || nativeToolTitle;
  const failure = event.error != null || event.status === "failed" || event.type.toLowerCase().includes("failed");
  const final = /result|final|complete/.test(event.type.toLowerCase());
  const input = rawText(event.raw_input ?? event.input);
  const output = rawText(event.raw_output ?? event.output);
  const locations = rawText(event.locations);
  const diffs = diffBlocks(event.content);
  const otherContent = event.content != null && diffs.length === 0 ? rawText(event.content) : null;
  const message = cleanEventText(event);
  const hasPayload = eventHasPayload(event);
  const presentation = presentTool(event);
  const semantics = presentToolSemantics({
    name: event.title ?? event.tool_kind,
    title: event.title,
    toolKind: event.tool_kind,
    input: { ...(event.input && typeof event.input === "object" ? event.input as Record<string, unknown> : {}), content: event.content, locations: event.locations },
    rawInput: event.raw_input,
  });
  const detail = <View style={{ marginLeft: 24, marginTop: 6, marginBottom: 10, borderWidth: 1, borderColor: theme.colors.border, borderRadius: 8, backgroundColor: theme.colors.surface1, padding: 11, gap: 12 }}>
    <Text selectable style={[styles.muted, { color: theme.colors.foregroundMuted }]}>{event.title || event.type}{event.status ? ` · ${event.status}` : ""} · {event.createdAt}</Text>
    {input !== null && <Detail label="Input" value={input} theme={theme} />}
    {output !== null && <Detail label="Output" value={output} theme={theme} />}
    {event.diff != null && <Detail label="File changes" value={event.diff} theme={theme} />}
    {diffs.map((diff, i) => <FileDiff key={`${diff.path}-${i}`} path={diff.path} rows={diffLines(diff.oldText, diff.newText)} theme={theme} />)}
    {otherContent !== null && <Detail label="Content" value={otherContent} theme={theme} />}
    {locations !== null && <Detail label="Locations" value={locations} theme={theme} />}
    {event.error != null && <Text selectable style={[styles.body, { color: theme.colors.statusDanger }]}>{event.error}</Text>}
    {updates.length > 1 && <Detail label={`Recorded updates (${updates.length})`} value={rawText(updates) ?? ""} theme={theme} />}
    {!hasPayload && <Text style={[styles.muted, { color: theme.colors.foregroundMuted }]}>No additional retained payload.</Text>}
  </View>;

  if (tool && !semantics.visible) return null;
  if (tool && !semantics.expandable && (semantics.systemOperation || semantics.kind !== "generic")) return <View style={{ flexDirection: "row", alignItems: "center", gap: 9, minHeight: 32, marginVertical: 2, paddingVertical: 5, paddingHorizontal: 1 }}>
    <Icon name={semantics.icon} size={14} color={failure ? theme.colors.statusDanger : theme.colors.foregroundMuted} />
    <Text numberOfLines={1} style={[styles.muted, { color: failure ? theme.colors.statusDanger : theme.colors.foregroundMuted, flex: 1 }]}>{semantics.label}</Text>
  </View>;
  if (tool) return <View style={{ marginVertical: 2, borderWidth: expanded ? 1 : 0, borderColor: theme.colors.border, borderRadius: 7, overflow: "hidden" }}>
    <Pressable accessibilityRole="button" accessibilityLabel={`${expanded ? "Collapse" : "Expand"} ${semantics.label} details`} accessibilityState={{ expanded }} onPress={onToggle} style={{ flexDirection: "row", alignItems: "center", gap: 9, minHeight: 32, paddingVertical: 5, paddingHorizontal: expanded ? 9 : 1, borderBottomWidth: expanded ? 1 : 0, borderColor: theme.colors.border }}>
      <Icon name={expanded ? "ChevronDown" : semantics.icon} size={14} color={failure ? theme.colors.statusDanger : theme.colors.foregroundMuted} />
      <Text numberOfLines={1} style={[styles.muted, { color: failure ? theme.colors.statusDanger : theme.colors.foregroundMuted, flex: 1 }]}>{semantics.label}</Text>
    </Pressable>
    {expanded && <View style={{ paddingHorizontal: 10, paddingVertical: 9, gap: 8 }}>
      {presentation.kind === "shell" && <View style={{ gap: 12 }}>
        {presentation.command !== null && <Text selectable style={[styles.mono, { color: theme.colors.foreground }]}>{`$ ${presentation.command}`}</Text>}
        {presentation.output !== null && <Text selectable style={[styles.mono, { color: theme.colors.foreground }]}>{presentation.output}</Text>}
      </View>}
      {presentation.kind === "edit" && diffs.length > 0 && diffs.map((diff, i) => <FileDiff key={`${diff.path}-${i}`} path={diff.path} rows={diffLines(diff.oldText, diff.newText)} theme={theme} showPath={false} />)}
      {presentation.kind === "edit" && diffs.length === 0 && presentation.oldText !== null && presentation.newText !== null && <FileDiff path={presentation.path ?? "File changes"} rows={diffLines(presentation.oldText, presentation.newText)} theme={theme} showPath={false} />}
      {presentation.kind === "generic" && <>
        {input !== null && <Detail label="Input" value={input} theme={theme} />}
        {output !== null && <Detail label="Output" value={output} theme={theme} />}
        {event.diff != null && <Detail label="File changes" value={event.diff} theme={theme} />}
        {diffs.map((diff, i) => <FileDiff key={`${diff.path}-${i}`} path={diff.path} rows={diffLines(diff.oldText, diff.newText)} theme={theme} />)}
        {otherContent !== null && <Detail label="Content" value={otherContent} theme={theme} />}
      </>}
      {failure && event.error != null && <Text selectable style={[styles.muted, { color: theme.colors.statusDanger }]}>{event.error}</Text>}
      {!hasPayload && <Text style={[styles.muted, { color: theme.colors.foregroundMuted }]}>No retained tool payload.</Text>}
      <Pressable accessibilityRole="button" accessibilityLabel={`${rawExpanded ? "Hide" : "Show"} raw event data`} accessibilityState={{ expanded: rawExpanded }} onPress={() => setRawExpanded(!rawExpanded)} style={{ alignSelf: "flex-start", paddingVertical: 3 }}><Text style={[styles.muted, { color: theme.colors.foregroundMuted }]}>{rawExpanded ? "⌄  Hide raw data" : "›  Raw data"}</Text></Pressable>
      {rawExpanded && <Detail label={`Recorded events (${updates.length})`} value={rawText(updates) ?? ""} theme={theme} />}
    </View>}
  </View>;

  return <View style={{ marginVertical: final ? 13 : 9, gap: 6 }}>
    {message && <Text selectable style={[styles.body, { color: failure ? theme.colors.statusDanger : theme.colors.foreground }]}>{message}</Text>}
    {hasPayload && <Pressable accessibilityRole="button" accessibilityLabel={`${expanded ? "Hide" : "Show"} event details`} accessibilityState={{ expanded }} onPress={onToggle} style={{ alignSelf: "flex-start", paddingVertical: 3 }}><Text style={[styles.muted, { color: theme.colors.foregroundMuted }]}>{expanded ? "⌄  Hide details" : "›  Details"}</Text></Pressable>}
    {expanded && detail}
  </View>;
}

export function WorkerPanel({ theme, layout, workspaceId, agentId, navigation }: PluginAgentPanelProps) {
  const rpc = useRpc(readWorkerHistory);
  const [history, setHistory] = useState<WorkerHistory | null>(null);
  // The coordinator card records its assignment before opening this panel.
  // Seed both states from that handoff so the first request is scoped to the
  // clicked worker, even when Paseo has just created a new workspace tab.
  const [selected, setSelected] = useState<string | null>(() => selectedWorker(agentId));
  const [selectedAssignmentRef, setSelectedAssignmentRef] = useState<string | null>(() => selectedWorker(agentId));
  const [nextCursor, setNextCursor] = useState<number | null>(null);
  const [nextAssignmentCursor, setNextAssignmentCursor] = useState<string | null>(null);
  const [historyContext, setHistoryContext] = useState<string | null>(null);
  const [expandedEvent, setExpandedEvent] = useState<number | null>(null);
  const [loading, setLoading] = useState(true);
  const mountedRef = useRef(true);
  const refreshInFlightRef = useRef(false);
  const historyRef = useRef<WorkerHistory | null>(null);
  const contextKey = `${workspaceId}\u0000${agentId}`;

  useEffect(() => () => {
    mountedRef.current = false;
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
    applySelection();
    return subscribeWorkerSelection(applySelection);
  }, [agentId]);

  useEffect(() => {
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
    if (!mountedRef.current || refreshInFlightRef.current) return;
    refreshInFlightRef.current = true;
    try {
      const priorHistory = historyRef.current;
      setLoading(shouldShowWorkerHistoryLoader(priorHistory));
      // A context change clears historyRef before the first request, so a
      // retained history here is necessarily for this exact panel context.
      const incrementalCursor = priorHistory ? historyCursor(priorHistory) : 0;
      const result = await rpc({
        workspaceId,
        agentId,
        // Timeline rows carry the durable assignment key, not an attempt ID.
        // Resolve it against this task's returned assignment page below.
        attemptId: selectedAssignmentRef ? undefined : selected ?? undefined,
        assignmentRef: selectedAssignmentRef ?? undefined,
        ...(incrementalCursor > 0 ? { cursor: incrementalCursor } : {}),
      });
      if (!mountedRef.current || requestContext !== `${workspaceId}\u0000${agentId}`) return;
      const displayed = incrementalCursor > 0 && priorHistory ? mergeWorkerHistory(priorHistory, result) : result;
      historyRef.current = displayed;
      setHistory(displayed);
      setHistoryContext(requestContext);
      setNextCursor(displayed.nextCursor);
      setNextAssignmentCursor(result.nextAssignmentCursor);
      if (selectedAssignmentRef) {
        const assignment = displayed.assignments.find((candidate) => candidate.id === selectedAssignmentRef || candidate.assignmentRef === selectedAssignmentRef);
        const attempt = assignment?.attempts[assignment.attempts.length - 1];
        setSelectedAssignmentRef(null);
        setSelected(attempt?.id ?? null);
      }
    }
    finally {
      refreshInFlightRef.current = false;
      if (mountedRef.current && requestContext === `${workspaceId}\u0000${agentId}`) setLoading(false);
    }
  }, [agentId, contextKey, rpc, selected, selectedAssignmentRef, workspaceId]);

  const loadMore = useCallback(async () => {
    if (!nextCursor || !selected) return;
    const requestContext = contextKey;
    const result = await rpc({ workspaceId, agentId, attemptId: selected, cursor: nextCursor });
    if (requestContext !== `${workspaceId}\u0000${agentId}`) return;
    setHistory((current) => {
      if (!current) return result;
      return {
        ...result,
        assignments: result.assignments.map((assignment) => ({
          ...assignment,
          attempts: assignment.attempts.map((attempt) => {
            const prior = current.assignments.flatMap((item) => item.attempts).find((item) => item.id === attempt.id);
            return prior ? { ...attempt, events: [...prior.events, ...attempt.events] } : attempt;
          }),
        })),
      };
    });
    setNextCursor(result.nextCursor);
  }, [agentId, contextKey, nextCursor, rpc, selected, workspaceId]);

  const loadMoreWorkers = useCallback(async () => {
    if (!nextAssignmentCursor) return;
    const requestContext = contextKey;
    const result = await rpc({ workspaceId, agentId, assignmentCursor: nextAssignmentCursor });
    if (requestContext !== `${workspaceId}\u0000${agentId}`) return;
    setHistory((current) => current ? { ...result, assignments: [...current.assignments, ...result.assignments] } : result);
    setNextAssignmentCursor(result.nextAssignmentCursor);
  }, [agentId, contextKey, nextAssignmentCursor, rpc, workspaceId]);
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
  const routeLabel = (route: { provider: string; model: string; effort: string | null } | undefined) => {
    if (!route) return null;
    return [route.provider, route.model, route.effort].filter(Boolean).join(" · ");
  };
  const transcript = selectedAttempt ? groupWorkerEvents(selectedAttempt.events) : [];
  const route = selectedAttempt?.route ?? selectedAssignment?.route;
  return (
    <ScrollView style={{ flex: 1, backgroundColor: theme.colors.surface0 }} contentContainerStyle={{ paddingHorizontal: layout.compact ? 12 : 16, paddingTop: 15, paddingBottom: 28, gap: 12 }}>
      {!selectedAttempt ? <>
        <Text style={[styles.title, { color: theme.colors.foreground }]}>Workers</Text>
        {loading && <Text style={[styles.muted, { color: theme.colors.foregroundMuted }]}>Loading worker history…</Text>}
        {!loading && currentHistory && !currentHistory.available && <Text style={[styles.muted, { color: theme.colors.statusWarning }]}>{currentHistory.message}</Text>}
        {!loading && currentHistory?.available && currentHistory.assignments.length === 0 && <Text style={[styles.muted, { color: theme.colors.foregroundMuted }]}>No worker assignments recorded yet.</Text>}
        {currentHistory?.truncated && <Text style={[styles.muted, { color: theme.colors.statusWarning }]}>History is bounded; open a specific attempt for its retained events.</Text>}
        {activeAssignments.length > 0 && <Text style={[styles.muted, { color: theme.colors.foregroundMuted, fontWeight: "600" }]}>Active</Text>}
        {activeAssignments.map((assignment) => {
          const activeAttempt = assignment.attempts[assignment.attempts.length - 1];
          const primaryStatus = workerStatus(activeAttempt?.status ?? assignment.status, activeAttempt?.events.length ?? 0);
          return <Pressable key={assignment.id} accessibilityRole="button" accessibilityLabel={`Open ${workerDisplayName(assignment)}`} onPress={() => activeAttempt && setSelected(activeAttempt.id)} style={[styles.card, { borderColor: theme.colors.border, backgroundColor: theme.colors.surface1 }]}>
            <View style={styles.row}><Text style={{ color: theme.colors.foreground, fontWeight: "600" }}>{workerDisplayName(assignment)}</Text><Text style={{ color: theme.colors.foregroundMuted }}>{primaryStatus}</Text></View>
            {routeLabel(assignment.route) && <Text style={[styles.muted, { color: theme.colors.foregroundMuted }]}>{routeLabel(assignment.route)}</Text>}
            {activeAttempt && <Text style={[styles.muted, { color: theme.colors.foregroundMuted }]}>Attempt {activeAttempt.attemptNo}{assignment.attempts.length > 1 ? ` · ${assignment.attempts.length} attempts` : ""}</Text>}
          </Pressable>;
        })}
        {completedAssignments.length > 0 && <Text style={[styles.muted, { color: theme.colors.foregroundMuted, fontWeight: "600" }]}>Completed</Text>}
        {completedAssignments.map((assignment) => {
          const completedAttempt = assignment.attempts[assignment.attempts.length - 1];
          const primaryStatus = workerStatus(completedAttempt?.status ?? assignment.status, completedAttempt?.events.length ?? 0);
          return <Pressable key={assignment.id} accessibilityRole="button" accessibilityLabel={`Open ${workerDisplayName(assignment)}`} onPress={() => completedAttempt && setSelected(completedAttempt.id)} style={[styles.card, { borderColor: theme.colors.border, backgroundColor: theme.colors.surface1 }]}>
            <View style={styles.row}><Text style={{ color: theme.colors.foreground, fontWeight: "600" }}>{workerDisplayName(assignment)}</Text><Text style={{ color: theme.colors.foregroundMuted }}>{primaryStatus}</Text></View>
            {routeLabel(assignment.route) && <Text style={[styles.muted, { color: theme.colors.foregroundMuted }]}>{routeLabel(assignment.route)}</Text>}
            {completedAttempt && <Text style={[styles.muted, { color: theme.colors.foregroundMuted }]}>Attempt {completedAttempt.attemptNo}</Text>}
          </Pressable>;
        })}
        {nextAssignmentCursor && <Pressable accessibilityRole="button" onPress={() => void loadMoreWorkers()} style={[styles.button, { backgroundColor: theme.colors.surface2 }]}><Text style={{ color: theme.colors.accent }}>Load more workers</Text></Pressable>}
      </> : <>
        <Pressable accessibilityRole="button" accessibilityLabel="Back to all workers" onPress={() => setSelected(null)} style={{ alignSelf: "flex-start", paddingVertical: 2 }}><Text style={[styles.muted, { color: theme.colors.accent }]}>‹  All workers</Text></Pressable>
        <View style={{ gap: 8, paddingBottom: 14, borderBottomWidth: 1, borderColor: theme.colors.border }}>
          <Text style={[styles.title, { color: theme.colors.foreground }]}>{selectedAssignment ? workerDisplayName(selectedAssignment) : "Worker"}</Text>
          <View style={{ flexDirection: "row", flexWrap: "wrap", gap: 7, alignItems: "center" }}>
            {route && <Text accessibilityLabel={`${providerMark(route.provider).label} provider`} style={[styles.title, { fontSize: 17, lineHeight: 20, color: theme.colors.accent }]}>{providerMark(route.provider).glyph}</Text>}
            <Text style={[styles.muted, { color: selectedAttempt.status === "failed" ? theme.colors.statusDanger : theme.colors.foregroundMuted }]}>{selectedAssignment ? workerDisplayName(selectedAssignment) : "worker"}</Text>
            {route && <><Text style={[styles.muted, { color: theme.colors.foregroundMuted }]}>·</Text><Text style={[styles.muted, { color: theme.colors.foregroundMuted }]}>{route.provider}</Text><Text style={[styles.muted, { color: theme.colors.foregroundMuted }]}>·</Text><Text style={[styles.muted, { color: theme.colors.foregroundMuted }]}>{route.model}</Text>{route.effort && <><Text style={[styles.muted, { color: theme.colors.foregroundMuted }]}>·</Text><Text style={[styles.muted, { color: theme.colors.foregroundMuted }]}>{route.effort} effort</Text></>}</>}
            <Text style={[styles.muted, { color: selectedAttempt.status === "failed" ? theme.colors.statusDanger : theme.colors.foregroundMuted }]}>· {workerStatus(selectedAttempt.status, selectedAttempt.events.length)}</Text>
          </View>
          {selectedAssignment && selectedAssignment.attempts.length > 1 && <View style={{ flexDirection: "row", flexWrap: "wrap", gap: 6 }}>
            {selectedAssignment.attempts.map((attempt) => <Pressable key={attempt.id} accessibilityRole="button" accessibilityState={{ selected: attempt.id === selectedAttempt.id }} onPress={() => { setExpandedEvent(null); setSelected(attempt.id); }} style={[styles.button, { backgroundColor: attempt.id === selectedAttempt.id ? theme.colors.surface2 : theme.colors.surface1, borderWidth: 1, borderColor: theme.colors.border }]}><Text style={[styles.muted, { color: attempt.id === selectedAttempt.id ? theme.colors.foreground : theme.colors.foregroundMuted }]}>Attempt {attempt.attemptNo}</Text></Pressable>)}
          </View>}
          {currentHistory && currentHistory.assignments.length > 1 && <ScrollView horizontal showsHorizontalScrollIndicator={false} contentContainerStyle={{ gap: 12, alignItems: "center" }}>
            {currentHistory.assignments.filter((assignment) => assignment.id !== selectedAssignment?.id).map((assignment) => {
              const latest = assignment.attempts[assignment.attempts.length - 1];
              return latest ? <Pressable key={assignment.id} accessibilityRole="button" accessibilityLabel={`Switch to ${workerDisplayName(assignment)}`} onPress={() => { setExpandedEvent(null); setSelected(latest.id); }}><Text style={[styles.muted, { color: theme.colors.accent }]}>{workerDisplayName(assignment)}  ›</Text></Pressable> : null;
            })}
          </ScrollView>}
          {selectedAttempt.agentId && navigation?.openAgent && <Pressable accessibilityRole="button" onPress={() => navigation.openAgent({ agentId: selectedAttempt.agentId! })}><Text style={[styles.muted, { color: theme.colors.accent }]}>Open full chat ↗</Text></Pressable>}
        </View>
        <View style={{ gap: 2 }}>
          {selectedAttempt.failureReason && <Text selectable style={[styles.body, { color: theme.colors.statusDanger }]}>{selectedAttempt.failureReason}</Text>}
          {transcript.map(({ event, updates }) => <TranscriptEvent key={event.id} event={event} updates={updates} expanded={expandedEvent === event.id} onToggle={() => setExpandedEvent(expandedEvent === event.id ? null : event.id)} theme={theme} />)}
          {selectedAttempt.events.length === 0 && <Text style={[styles.muted, { color: theme.colors.foregroundMuted }]}>No durable events are available for this attempt.</Text>}
          {nextCursor && <Pressable accessibilityRole="button" onPress={() => void loadMore()} style={[styles.button, { marginTop: 12, borderWidth: 1, borderColor: theme.colors.border, backgroundColor: theme.colors.surface1 }]}><Text style={[styles.muted, { color: theme.colors.accent }]}>Load more history</Text></Pressable>}
          {currentHistory?.truncated && !nextCursor && <Text style={[styles.muted, { color: theme.colors.foregroundMuted }]}>Some earlier history is unavailable.</Text>}
        </View>
      </>}
    </ScrollView>
  );
}
