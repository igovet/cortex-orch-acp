import React from "react";
import { Text, View } from "react-native";
import { boundaryFallbackMessage } from "./plugin-error-boundary-message.ts";

type Theme = { colors: { statusWarning: string; surface1: string; border: string } };

type Props = { theme: Theme; label: string; children: React.ReactNode };
type State = { error: unknown };

const INITIAL_STATE: State = { error: undefined };

/**
 * One host-visible surface (a workspace panel, or the phone composer-pill
 * popover) crashing must not take down the whole plugin: Paseo has no
 * per-surface recovery of its own, so an uncaught render error here
 * previously surfaced as the host's generic "Plugin failed" message with no
 * way back short of a reload. This bounds the blast radius to the one
 * surface and lets the host keep everything else (other panels, the
 * composer, the rest of the timeline) working.
 */
export class PluginErrorBoundary extends React.Component<Props, State> {
  state: State = INITIAL_STATE;

  static getDerivedStateFromError(error: unknown): State {
    return { error };
  }

  componentDidCatch(error: unknown): void {
    // eslint-disable-next-line no-console -- the host has no plugin-error channel to report to.
    console.error(`[cortex-acp] ${this.props.label} crashed`, error);
  }

  render() {
    if (this.state.error !== undefined) {
      const c = this.props.theme.colors;
      return <View style={{ padding: 16, borderWidth: 1, borderColor: c.border, borderRadius: 8, backgroundColor: c.surface1 }}>
        <Text style={{ color: c.statusWarning, fontSize: 13 }}>{boundaryFallbackMessage(this.props.label, this.state.error)}</Text>
      </View>;
    }
    return this.props.children;
  }
}
