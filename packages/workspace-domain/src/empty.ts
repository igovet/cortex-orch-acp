import type { Machine, MachineConfig, RoutingRules, UserDefaults, WorkspaceState } from "./types";
import { openCeiling } from "./catalog";

/** Demonstration-only path. Real defaults are absolute paths reported by the channel service. */
export const DEFAULT_WORKTREE_ROOT = "~/.cortex-workspace/worktrees";
/** The machine this app runs on. It is the one machine known before any service has answered. */
export const LOCAL_MACHINE_ID = "local";

export const emptyFiles = (): MachineConfig["files"] => ({ envRouting: null, invalidRouting: false, migration: false, missing: [], backups: 0, preservedRouting: [], preservedDefaults: [], askpass: false, env: { engineUpdate: null, engineFollow: null, opencodeBinary: null } });
const emptyRules = (): RoutingRules => {
  const engine = () => ({ enabled: [], auto: [], tiers: {}, policy: {}, quota: [] });
  return { version: 2, deny: [], providers: [], engines: { "claude-agent-acp": engine(), "codex-acp": engine(), opencode: engine() }, pins: {}, affinity: {}, autoAllow: [], legacyTiers: [] };
};
const emptyDefaults = (): UserDefaults => ({ coordinator: null, routeDefault: null, language: "auto", mode: "guarded", checkout: "worktree", ceiling: openCeiling(), preserved: [] });
/** Nothing is known yet. Every list is empty, and an unknown stays unknown rather than borrowing a preset. */
export function emptyConfig(): MachineConfig {
  const unread = { observedAt: null, failed: false, models: [] };
  const unset = { status: "not_downloaded" as const, automatic: false, reason: null, autoConnectDisabled: false };
  return {
    revisions: { routing: 0, user_defaults: 0, project_ceiling: 0, mcp_servers: 0 }, surfaces: { routing: emptyRules(), user_defaults: emptyDefaults(), mcp_servers: [] }, projects: {}, active: null, interrupted: false, unavailable: null, profiles: [],
    catalogue: { "claude-agent-acp": { ...unread }, "codex-acp": { ...unread }, opencode: { ...unread } }, quotaObserved: { "claude-agent-acp": [], "codex-acp": [], opencode: [] }, mcpObserved: {}, files: emptyFiles(),
    setup: { "claude-agent-acp": { ...unset }, "codex-acp": { ...unset }, opencode: { ...unset } },
  };
}
export function emptyMachine(input: Pick<Machine, "id" | "name" | "kind" | "connection">): Machine {
  return {
    lastConfirmedAt: null, retentionDays: 30, autoArchiveDays: 7, worktreeRoot: "", version: null, latencyMs: null, historyGap: false, diskMb: null,
    remoteAccess: { allowed: false, devices: [] }, cleanup: null, diagnostics: null, storage: [], connections: [], config: emptyConfig(), ...input,
  };
}
/** What the real path shows before a service answers: this machine, offline, and nothing on it. */
export function emptyWorkspace(): WorkspaceState {
  return {
    options: { models: [], efforts: [], modes: [], languages: [] }, machines: [emptyMachine({ id: LOCAL_MACHINE_ID, name: "This machine", kind: "local", connection: "disconnected" })],
    projects: [], tasks: [], sprints: [], workers: [], messages: [], sessions: [], events: [], engineUsage: [], purged: [], repos: [], reviews: [],
  };
}
