import { useRpc, type PluginButtonContentProps } from "@getpaseo/plugin/client";
import { Pressable, Text, View } from "react-native";
import { useEffect, useState } from "react";
import { readWorkerHistory, type WorkerHistory } from "../shared/worker-history.js";
import { workerPopoverStatus, orderPopoverWorkers } from "./worker-popover-status.js";
import { workerDisplayName } from "./worker-display.js";

type Props = PluginButtonContentProps & {
  workspaceId: string;
  agentId: string;
  onOpenWorker(assignmentRef: string): void;
};

/** The composer pill's supported popover content. Keep it bounded and read-only. */
export function WorkerPopover({ theme, layout, workspaceId, agentId, close, onOpenWorker }: Props) {
  const rpc = useRpc(readWorkerHistory);
  const [history, setHistory] = useState<WorkerHistory | null>(null);

  useEffect(() => {
    let active = true;
    const refresh = () => rpc({ workspaceId, agentId, limit: 25 }).then((result) => {
      if (active) setHistory(result);
    }).catch(() => {
      if (active) setHistory(null);
    });
    void refresh();
    const timer = setInterval(() => { void refresh(); }, 750);
    return () => { active = false; clearInterval(timer); };
  }, [agentId, rpc, workspaceId]);

  const assignments = orderPopoverWorkers(history?.assignments ?? []);
  return (
    <View accessibilityRole="menu" style={{ width: layout.compact ? undefined : 340, alignSelf: layout.compact ? "stretch" : "auto", marginHorizontal: -12, backgroundColor: theme.colors.surface1 }}>
      {!history && <Text style={{ color: theme.colors.foregroundMuted, fontSize: 13, padding: 14 }}>Loading workers…</Text>}
      {history && !history.available && <Text style={{ color: theme.colors.statusWarning, fontSize: 13, padding: 14 }}>{history.message ?? "Workers are unavailable for this session."}</Text>}
      {history?.available && assignments.length === 0 && <Text style={{ color: theme.colors.foregroundMuted, fontSize: 13, padding: 14 }}>No workers in this session.</Text>}
      {history?.available && assignments.length > 0 && <View>
        {assignments.map((assignment, index) => {
          const attempt = assignment.attempts[assignment.attempts.length - 1];
          const ref = assignment.assignmentRef ?? assignment.id;
          const status = workerPopoverStatus(assignment);
          const route = assignment.route ?? attempt?.route;
          return <Pressable
            key={assignment.id}
            accessibilityRole="menuitem"
            accessibilityLabel={`Open ${workerDisplayName(assignment)}`}
            onPress={() => { close(); onOpenWorker(ref); }}
            style={{ flexDirection: "row", alignItems: "center", gap: 9, paddingHorizontal: 12, paddingVertical: layout.compact ? 8 : 5, borderBottomWidth: index === assignments.length - 1 ? 0 : 1, borderColor: theme.colors.border, backgroundColor: theme.colors.surface1 }}
          >
            <View style={{ flex: 1, flexDirection: "column", alignItems: "stretch", gap: 2, minWidth: 0 }}>
              <Text numberOfLines={1} style={{ flexShrink: 1, color: theme.colors.foreground, fontSize: 16, fontWeight: "400" }}>{workerDisplayName(assignment)}</Text>
              <Text numberOfLines={1} style={{ flexShrink: 1, color: theme.colors.foregroundMuted, fontSize: 13 }}>{[route?.provider, route?.model, route?.effort, status].filter(Boolean).join(" · ")}</Text>
            </View>
          </Pressable>;
        })}
      </View>}
    </View>
  );
}
