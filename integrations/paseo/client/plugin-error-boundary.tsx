import React from "react";
import { Text, View } from "react-native";
import { boundaryFallbackMessage } from "./plugin-error-boundary-message.ts";

type Theme = { colors: { statusWarning: string; surface1: string; border: string } };

type Props = { theme: Theme; label: string; children: React.ReactNode };
type State = { error: unknown };

const INITIAL_STATE: State = { error: undefined };

type Instance = React.Component<Props, State>;

/**
 * One host-visible surface (a workspace panel, or the phone composer-pill
 * popover) crashing must not take down the whole plugin: Paseo has no
 * per-surface recovery of its own, so an uncaught render error here
 * previously surfaced as the host's generic "Plugin failed" message with no
 * way back short of a reload. This bounds the blast radius to the one
 * surface and lets the host keep everything else (other panels, the
 * composer, the rest of the timeline) working.
 *
 * React only offers error boundaries as class components, but this is
 * deliberately not `class` syntax: Paseo evaluates plugin client bundles
 * from source on the phone's Hermes, which compiles a class expression to
 * `undefined`, so a `class` boundary was itself the "Element type is invalid
 * ... got: undefined" crash on every surface it wrapped. A constructor
 * inheriting `React.Component.prototype` is the same class component to
 * React (it checks `prototype.isReactComponent`).
 */
function ErrorBoundary(this: Instance, props: Props) {
  (React.Component as unknown as (this: Instance, props: Props) => void).call(this, props);
  this.state = INITIAL_STATE;
}

ErrorBoundary.prototype = Object.create(React.Component.prototype, {
  constructor: { value: ErrorBoundary, writable: true, configurable: true },
});

ErrorBoundary.getDerivedStateFromError = (error: unknown): State => ({ error });

ErrorBoundary.prototype.componentDidCatch = function componentDidCatch(this: Instance, error: unknown): void {
  // eslint-disable-next-line no-console -- the host has no plugin-error channel to report to.
  console.error(`[cortex-acp] ${this.props.label} crashed`, error);
};

ErrorBoundary.prototype.render = function render(this: Instance) {
  if (this.state.error !== undefined) {
    const c = this.props.theme.colors;
    return <View style={{ padding: 16, borderWidth: 1, borderColor: c.border, borderRadius: 8, backgroundColor: c.surface1 }}>
      <Text style={{ color: c.statusWarning, fontSize: 13 }}>{boundaryFallbackMessage(this.props.label, this.state.error)}</Text>
    </View>;
  }
  return this.props.children;
};

export const PluginErrorBoundary = ErrorBoundary as unknown as React.ComponentClass<Props, State>;
