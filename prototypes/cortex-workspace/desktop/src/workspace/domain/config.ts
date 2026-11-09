import { activeProfile, capture, ceilingProblems, defaultsProblems, emptyFiles, mcpProblems, routingProblems, sessionsOf } from "@cortex/workspace-domain";
import type { Ceiling, EngineKey, Machine, MachineConfig, McpServer, Problem, Profile, RoutingRules, SurfaceKey, UserDefaults, WorkspaceCommand, WorkspaceState } from "./types";
import { entity, nonempty, requireFact } from "./core";
import { newCatalogue, presetDefaults, presetRouting, presetServers } from "./config-preset";

const SETUP = (status: MachineConfig["setup"][EngineKey]["status"]) => ({ status, automatic: status !== "not_downloaded", reason: null, autoConnectDisabled: false });
export function newConfig(sessions: Record<string, number>): MachineConfig {
  const surfaces = { routing: presetRouting(), user_defaults: presetDefaults(), mcp_servers: presetServers() };
  const profile: Profile = { id: "default", name: "default", description: "", updatedAt: null, sections: capture(surfaces, sessions) };
  return {
    revisions: { routing: 1, user_defaults: 1, project_ceiling: 1, mcp_servers: 1 }, surfaces, projects: {}, active: profile.id, interrupted: false, unavailable: null, profiles: [profile],
    catalogue: newCatalogue(), quotaObserved: { "claude-agent-acp": [], "codex-acp": [], opencode: [] }, mcpObserved: {}, files: emptyFiles(),
    setup: { "claude-agent-acp": SETUP("connected"), "codex-acp": SETUP("connected"), opencode: { ...SETUP("not_downloaded"), automatic: false, reason: "OpenCode is never set up automatically" } },
  };
}
function online(machine: Machine) { requireFact(machine.connection === "connected", `${machine.name} is offline, so its settings are read-only`); return machine; }
const profileOf = (c: MachineConfig, id: string) => { const p = c.profiles.find(x => x.id === id); requireFact(p, "Unknown profile"); return p; };
const bumpAll = (c: MachineConfig) => { for (const k of Object.keys(c.revisions) as SurfaceKey[]) c.revisions[k]++; };
const NAME = /^[a-z0-9][a-z0-9-]{0,39}$/;
const checkName = (c: MachineConfig, name: string, except?: string) => {
  requireFact(NAME.test(name), "Use lowercase letters, digits and dashes, up to 40");
  requireFact(!c.profiles.some(p => p.name === name && p.id !== except), "A profile with this name exists");
};
const checkDescription = (text: string) => requireFact(text.length <= 256, "Keep the description within 256 characters");
function refuseProblems(problems: Problem[]) { requireFact(problems.length === 0, problems[0]?.message.replace(/\.$/, "") ?? ""); }
/** A snapshot only touches what it stores. Commands, arguments and environment stay as they are on the machine. */
function applyProfile(machine: Machine, p: Profile) {
  const c = machine.config, s = p.sections;
  if (s.routing) c.surfaces.routing = structuredClone(s.routing);
  if (s.defaults) c.surfaces.user_defaults = structuredClone(s.defaults);
  if (s.mcp) c.surfaces.mcp_servers = c.surfaces.mcp_servers.map(server => { const stored = s.mcp!.find(x => x.name === server.name); return stored ? { ...server, ...structuredClone(stored) } : server; });
  if (s.sessions) for (const conn of machine.connections) if (s.sessions[conn.id] !== undefined) conn.sessions = s.sessions[conn.id];
  bumpAll(c);
}
const saveInto = (machine: Machine, p: Profile, now: number | null = null) => { p.sections = capture(machine.config.surfaces, sessionsOf(machine)); p.updatedAt = now; };
/** A simulated other writer, so a conflict always has a real key to show. */
function otherWriter(c: MachineConfig, surface: SurfaceKey) {
  if (surface === "routing") { const token = "claude-agent-acp/claude-sonnet-4-5"; c.surfaces.routing.deny = c.surfaces.routing.deny.includes(token) ? c.surfaces.routing.deny.filter(t => t !== token) : [...c.surfaces.routing.deny, token]; }
  if (surface === "user_defaults") c.surfaces.user_defaults.language = c.surfaces.user_defaults.language === "en" ? "ru" : "en";
  if (surface === "mcp_servers") { const s = c.surfaces.mcp_servers[0]; if (s) s.network = !s.network; }
  c.revisions[surface]++;
}
const settleEnv = (s: McpServer): McpServer => ({ ...s, env: s.env.filter(v => v.action !== "remove").map(v => ({ ...v, stored: true, action: "keep", value: "" })) });

/** Saving a machine's rules. Everything here is simulated: no remote write is made and no process is told. */
export function handleConfig(state: WorkspaceState, command: WorkspaceCommand): boolean {
  switch (command.type) {
    case "save_surface": {
      const machine = online(entity(state.machines, command.machineId)), c = machine.config, { surface, projectId } = command;
      const revisions = projectId ? ((c.projects[projectId] ??= {}).revisions ??= {}) : c.revisions;
      requireFact(command.revision === (revisions[surface] ?? 0), "Changed on the machine since you opened it. Your draft is kept");
      if (projectId) requireFact(entity(state.projects, projectId).machineId === machine.id, "That project is on another machine");
      const value = structuredClone(command.value);
      if (surface === "routing") {
        requireFact(!!projectId || !c.files.envRouting, "Routing comes from $CORTEX_ROUTING here, which is read-only");
        refuseProblems(routingProblems(value as RoutingRules, c.catalogue));
        if (projectId) (c.projects[projectId] ??= {}).routing = value as RoutingRules; else c.surfaces.routing = value as RoutingRules;
      } else if (surface === "user_defaults") {
        refuseProblems(defaultsProblems(value as UserDefaults));
        c.surfaces.user_defaults = value as UserDefaults;
      } else if (surface === "project_ceiling") {
        requireFact(projectId, "A project ceiling needs a project");
        if (value === null) delete (c.projects[projectId] ??= {}).ceiling;
        else {
          refuseProblems(ceilingProblems(value as Ceiling, c.surfaces.user_defaults.ceiling));
          (c.projects[projectId] ??= {}).ceiling = value as Ceiling;
        }
      } else {
        const list = value as McpServer[];
        for (const s of list) refuseProblems(mcpProblems(s, list.filter(x => x !== s), !c.surfaces.mcp_servers.some(o => o.name === s.name)));
        c.surfaces.mcp_servers = list.map(settleEnv);
        for (const name of Object.keys(c.mcpObserved)) if (!list.some(s => s.name === name)) delete c.mcpObserved[name];
      }
      revisions[surface] = (revisions[surface] ?? 0) + 1;
      return true;
    }
    case "config_changed": { const c = entity(state.machines, command.machineId).config; if (command.surface) otherWriter(c, command.surface); else bumpAll(c); return true; }
    case "set_config_fault": {
      const c = entity(state.machines, command.machineId).config, { fault, on } = command;
      if (fault === "env_routing") c.files.envRouting = on ? "/etc/cortex/routing.json" : null;
      if (fault === "invalid_routing") c.files.invalidRouting = on;
      if (fault === "migration") c.files.migration = on;
      if (fault === "interrupted") c.interrupted = on;
      if (fault === "unavailable") c.unavailable = on ? "The profiles file could not be read." : null;
      if (fault === "catalogue") c.catalogue["codex-acp"].failed = on;
      if (fault === "engine_missing") c.setup["codex-acp"].status = on ? "not_downloaded" : "connected";
      if (fault === "preset_missing") c.files.missing = on ? ["routing.json"] : [];
      return true;
    }
    case "activate_profile": {
      const machine = online(entity(state.machines, command.machineId)), c = machine.config, target = profileOf(c, command.profileId), from = activeProfile(c);
      if (command.keep === "save" && from && from.id !== target.id) saveInto(machine, from);
      applyProfile(machine, target);
      c.active = target.id; c.interrupted = false; c.unavailable = null;
      return true;
    }
    case "save_profile": {
      const machine = online(entity(state.machines, command.machineId)), p = activeProfile(machine.config);
      requireFact(p, "No profile is active");
      saveInto(machine, p); machine.config.interrupted = false;
      return true;
    }
    case "revert_profile": {
      const machine = online(entity(state.machines, command.machineId)), p = activeProfile(machine.config);
      requireFact(p, "No profile is active");
      applyProfile(machine, p); machine.config.interrupted = false;
      return true;
    }
    case "deactivate_profile": { const c = online(entity(state.machines, command.machineId)).config; c.active = null; c.interrupted = false; return true; }
    case "create_profile": {
      const machine = online(entity(state.machines, command.machineId)), c = machine.config;
      checkName(c, command.name); checkDescription(command.description);
      requireFact(c.profiles.length < 32, "Up to 32 profiles");
      const p: Profile = { id: `profile-${command.name}`, name: command.name, description: command.description, updatedAt: null, sections: {} };
      saveInto(machine, p);
      c.profiles.push(p); c.active = p.id;
      return true;
    }
    case "rename_profile": { const c = online(entity(state.machines, command.machineId)).config; checkName(c, command.name, command.profileId); profileOf(c, command.profileId).name = command.name; return true; }
    case "describe_profile": { const c = online(entity(state.machines, command.machineId)).config; checkDescription(command.description); profileOf(c, command.profileId).description = command.description; return true; }
    case "delete_profile": {
      const c = online(entity(state.machines, command.machineId)).config;
      requireFact(c.active !== command.profileId, "Activate another profile first");
      profileOf(c, command.profileId);
      c.profiles = c.profiles.filter(p => p.id !== command.profileId);
      return true;
    }
    case "apply_preset": {
      const c = online(entity(state.machines, command.machineId)).config;
      requireFact(c.files.missing.length > 0, "Every file exists, so there is nothing to seed");
      c.files.missing = [];
      return true;
    }
    case "reset_preset": {
      const c = online(entity(state.machines, command.machineId)).config;
      requireFact(command.confirmed, "Owner confirmation required");
      c.surfaces.routing = presetRouting(); c.surfaces.user_defaults = presetDefaults(); c.files.backups += 2;
      c.revisions.routing++; c.revisions.user_defaults++;
      return true;
    }
    case "migrate_policy": {
      const c = online(entity(state.machines, command.machineId)).config;
      requireFact(command.confirmed, "Owner confirmation required");
      c.files.migration = false; c.surfaces.routing.version = 2; c.surfaces.routing.legacyTiers = []; c.revisions.routing++;
      return true;
    }
    case "record_mcp_tools": {
      const c = online(entity(state.machines, command.machineId)).config;
      if (command.observation) c.mcpObserved[command.server] = structuredClone(command.observation); else delete c.mcpObserved[command.server];
      return true;
    }
    case "rename_connection": {
      const name = nonempty(command.name, "Connection name");
      requireFact(name.length <= 120, "Keep the name within 120 characters");
      entity(online(entity(state.machines, command.machineId)).connections, command.connectionId).label = name;
      return true;
    }
    case "download_engine": {
      const c = online(entity(state.machines, command.machineId)).config;
      requireFact(command.engine !== "opencode", "OpenCode is never downloaded");
      c.setup[command.engine].status = "downloaded_no_login";
      return true;
    }
    case "enable_opencode": {
      const machine = online(entity(state.machines, command.machineId)), name = nonempty(command.name, "Connection name");
      requireFact(name.length <= 120, "Keep the name within 120 characters");
      machine.config.setup.opencode.status = "connected";
      const conn = machine.connections.find(c => c.engine === "opencode");
      if (conn) conn.label = name;
      return true;
    }
    default: return false;
  }
}
