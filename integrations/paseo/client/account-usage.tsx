import { useRpc, type PluginHostProps } from "@getpaseo/plugin/client";
import { useEffect, useState } from "react";
import { Text, View } from "react-native";
import { readAccountUsage, type AccountUsage } from "../shared/account-usage.js";
import { unavailableMessage } from "../shared/account-usage-display.js";

type Props = { theme: PluginHostProps["theme"] };
const REFRESH_MS = 30_000;
const PROVIDERS = [
  { engine: "codex-acp", label: "Codex" },
  { engine: "claude-agent-acp", label: "Claude" },
] as const;

export function AccountUsagePopover({ theme }: Props) {
  const rpc = useRpc(readAccountUsage);
  const [usage, setUsage] = useState<AccountUsage | null>(null);
  useEffect(() => {
    let active = true;
    const refresh = async () => {
      try {
        const next = await rpc({});
        if (active) setUsage(next);
      } catch {
        if (active) setUsage({ available: false, connections: [] });
      }
    };
    void refresh();
    const timer = setInterval(() => { void refresh(); }, REFRESH_MS);
    return () => { active = false; clearInterval(timer); };
  }, [rpc]);

  const c = theme.colors;
  return <View style={{ minWidth: 240, gap: 12, padding: 8 }}>
    <Text style={{ color: c.foreground, fontSize: 14, fontWeight: "700" }}>Plan usage</Text>
    {PROVIDERS.map(({ engine, label }) => {
      const connections = usage?.connections.filter((item) => item.engine === engine) ?? [];
      const unavailable = unavailableMessage(engine, usage, connections.length);
      return <View key={engine} style={{ gap: 4 }}>
        <Text style={{ color: c.foreground, fontSize: 12, fontWeight: "600" }}>{label}</Text>
        {usage === null
          ? <Text style={{ color: c.foregroundMuted, fontSize: 12 }}>Loading…</Text>
          : !usage.available || connections.length === 0
            ? <Text style={{ color: c.foregroundMuted, fontSize: 12 }}>{unavailable}</Text>
            : connections.map((connection, index) => <View key={index} style={{ gap: 2 }}>
                {connections.length > 1 && <Text style={{ color: c.foregroundMuted, fontSize: 11 }}>{connection.name}</Text>}
                {connection.windows.length === 0
                  ? <Text style={{ color: c.foregroundMuted, fontSize: 12 }}>{unavailable}</Text>
                  : connection.windows.map((window) => <Text key={window.name} style={{ color: c.foreground, fontSize: 12 }}>
                      {window.name.replaceAll("_", " ")}: {Math.round(window.usedPercent)}% used
                    </Text>)}
              </View>)}
      </View>;
    })}
    <Text style={{ color: c.foregroundMuted, fontSize: 11 }}>Reported by native provider sessions. Refreshes every 30 seconds.</Text>
  </View>;
}
