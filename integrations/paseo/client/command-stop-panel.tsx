import { useRpc, type PluginAgentPanelProps } from "@getpaseo/plugin/client";
import { Icon } from "@getpaseo/plugin/client/react-native";
import { Pressable, Text, TextInput, View } from "react-native";
import { useCallback, useEffect, useRef, useState } from "react";
import { readRunningCommands, requestCommandStop, STOP_REASON_MAX, type RunningCommand } from "../shared/command-stop.ts";
import {
  startStop, stopOriginLabel, stopRequestInput, stopResultState, stopTransportFailure, submitStop, validateStopReason,
  type StopState,
} from "./command-stop.ts";

type Theme = PluginAgentPanelProps["theme"];

/** Polling interval of the selected worker's running commands. */
export const RUNNING_COMMANDS_POLL_MS = 2000;

const mono = { fontFamily: "monospace" as const, fontSize: 12, lineHeight: 18 };
const muted = { fontSize: 13, lineHeight: 19 };

function Button({ label, accessibilityLabel, onPress, disabled, tone, theme }: {
  label: string; accessibilityLabel: string; onPress: () => void; disabled?: boolean; tone: "danger" | "plain"; theme: Theme;
}) {
  const color = disabled ? theme.colors.foregroundMuted : tone === "danger" ? theme.colors.statusDanger : theme.colors.foreground;
  return <Pressable accessibilityRole="button" accessibilityLabel={accessibilityLabel} accessibilityState={{ disabled: Boolean(disabled) }}
    disabled={disabled} onPress={onPress} hitSlop={4}
    style={{ paddingVertical: 5, paddingHorizontal: 10, borderRadius: 6, borderWidth: 1,
      borderColor: tone === "danger" && !disabled ? theme.colors.statusDanger : theme.colors.border, backgroundColor: theme.colors.surface1 }}>
    <Text style={{ color, fontSize: 13, fontWeight: "600" }}>{label}</Text>
  </Pressable>;
}

function CommandRow({ command, state, onChange, onSubmit, onCancel, theme }: {
  command: RunningCommand; state: StopState | undefined; onChange: (next: StopState | undefined) => void;
  onSubmit: () => void; onCancel: () => void; theme: Theme;
}) {
  const summary = command.argvSummary || command.operation;
  const recorded = command.stopRequest;
  const requested = state?.phase === "requested" || (recorded !== null && state?.phase !== "sending");
  const editing = state && (state.phase === "editing" || state.phase === "refused" || state.phase === "unconfirmed");
  const sending = state?.phase === "sending";
  const valid = state ? validateStopReason(state.reason).ok : false;
  const statusColor = state?.phase === "refused" ? theme.colors.statusDanger : theme.colors.statusWarning;
  return <View style={{ borderWidth: 1, borderColor: theme.colors.border, borderRadius: 8, padding: 10, gap: 8, backgroundColor: theme.colors.surface1 }}>
    <View style={{ flexDirection: "row", alignItems: "center", gap: 8 }}>
      <Icon name="SquareTerminal" size={14} color={theme.colors.foregroundMuted} />
      <Text selectable numberOfLines={2} ellipsizeMode="tail" style={[mono, { flex: 1, color: theme.colors.foreground }]}>{`$ ${summary}`}</Text>
      {!state && !requested && <Button label="Stop" accessibilityLabel={`Stop command ${summary}`} tone="danger" theme={theme}
        onPress={() => onChange(startStop())} />}
    </View>
    {requested && <Text accessibilityLiveRegion="polite" style={[muted, { color: theme.colors.statusWarning }]}>
      {`Stop requested${recorded && !state ? ` ${stopOriginLabel(recorded.origin)}`.trimEnd() : ""}: ${state?.submittedReason ?? recorded?.reason ?? ""} — waiting for the command to end.`}
    </Text>}
    {sending && <Text accessibilityLiveRegion="polite" style={[muted, { color: theme.colors.foregroundMuted }]}>Sending the stop request to Cortex…</Text>}
    {(editing || sending) && state && <View style={{ gap: 6 }}>
      <TextInput
        value={state.reason}
        onChangeText={(reason) => onChange({ ...state, reason, message: state.phase === "editing" ? null : state.message })}
        editable={!sending}
        multiline
        maxLength={STOP_REASON_MAX}
        autoFocus
        placeholder="Why stop this command? The worker sees this reason and continues without it."
        placeholderTextColor={theme.colors.foregroundMuted}
        accessibilityLabel={`Reason for stopping ${summary}`}
        accessibilityHint="Required. The worker receives this reason."
        style={{ minHeight: 56, borderWidth: 1, borderRadius: 6, paddingHorizontal: 8, paddingVertical: 6, fontSize: 13,
          borderColor: state.message && state.phase === "editing" ? theme.colors.statusDanger : theme.colors.border,
          color: theme.colors.foreground, backgroundColor: sending ? theme.colors.surface1 : theme.colors.surface0 }}
      />
      {state.message && <Text accessibilityRole="alert" style={[muted, { color: state.phase === "editing" ? theme.colors.statusDanger : statusColor }]}>{state.message}</Text>}
      <View style={{ flexDirection: "row", gap: 8, flexWrap: "wrap" }}>
        <Button label={sending ? "Stopping…" : state.phase === "editing" ? "Stop command" : "Try again"}
          accessibilityLabel={`Confirm stopping ${summary}`} tone="danger" theme={theme}
          disabled={sending || !valid} onPress={onSubmit} />
        <Button label="Cancel" accessibilityLabel={`Keep running ${summary}`} tone="plain" theme={theme} disabled={sending} onPress={onCancel} />
      </View>
    </View>}
  </View>;
}

/** The selected worker's running commands, each stoppable with a reason. */
export function RunningCommands({ theme, workspaceId, agentId, assignmentRef, active }: {
  theme: Theme; workspaceId: string; agentId: string; assignmentRef: string; active: boolean;
}) {
  const list = useRpc(readRunningCommands);
  const stop = useRpc(requestCommandStop);
  const [commands, setCommands] = useState<RunningCommand[]>([]);
  const [states, setStates] = useState<Record<string, StopState>>({});
  const mountedRef = useRef(true);
  const scopeRef = useRef(`${workspaceId}\u0000${agentId}\u0000${assignmentRef}`);
  scopeRef.current = `${workspaceId}\u0000${agentId}\u0000${assignmentRef}`;

  useEffect(() => {
    mountedRef.current = true;
    return () => { mountedRef.current = false; };
  }, []);

  const refresh = useCallback(async () => {
    const scope = `${workspaceId}\u0000${agentId}\u0000${assignmentRef}`;
    try {
      const result = await list({ workspaceId, agentId, assignmentRef });
      if (!mountedRef.current || scope !== scopeRef.current) return;
      // An unavailable read keeps the last list: no stale command is
      // invented, and a transient read failure does not hide a pending stop.
      if (!result.available) return;
      setCommands(result.commands);
      const live = new Set(result.commands.map((command) => command.toolCallRef));
      // A command that is no longer running has its outcome on its own row.
      setStates((current) => Object.fromEntries(Object.entries(current).filter(([ref]) => live.has(ref))));
    } catch {
      // Keep the last list; the next poll retries.
    }
  }, [agentId, assignmentRef, list, workspaceId]);

  useEffect(() => {
    setCommands([]);
    setStates({});
  }, [workspaceId, agentId, assignmentRef]);

  useEffect(() => {
    void refresh();
    if (!active) return;
    const timer = setInterval(() => { void refresh(); }, RUNNING_COMMANDS_POLL_MS);
    return () => clearInterval(timer);
  }, [active, refresh]);

  const update = (ref: string, next: StopState | undefined) => setStates((current) => {
    const copy = { ...current };
    if (next) copy[ref] = next; else delete copy[ref];
    return copy;
  });

  const submit = async (ref: string) => {
    const current = states[ref];
    if (!current) return;
    const sending = submitStop(current);
    update(ref, sending);
    if (sending.phase !== "sending") return;
    let next: StopState;
    try {
      next = stopResultState(sending, await stop(stopRequestInput(workspaceId, agentId, ref, sending)));
    } catch {
      next = stopTransportFailure(sending);
    }
    if (!mountedRef.current) return;
    update(ref, next);
    if (next.phase === "requested") void refresh();
  };

  if (commands.length === 0) return null;
  return <View accessibilityLabel="Running commands" style={{ gap: 8, marginBottom: 10 }}>
    <Text style={[muted, { color: theme.colors.foregroundMuted, fontWeight: "600" }]}>Running commands</Text>
    {commands.map((command) => <CommandRow key={command.toolCallRef} command={command} theme={theme}
      state={states[command.toolCallRef]}
      onChange={(next) => update(command.toolCallRef, next)}
      onSubmit={() => { void submit(command.toolCallRef); }}
      onCancel={() => update(command.toolCallRef, undefined)} />)}
  </View>;
}

