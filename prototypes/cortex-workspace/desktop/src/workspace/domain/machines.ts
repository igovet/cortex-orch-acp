import type { EngineConnection, EngineId, Machine, WorkspaceCommand, WorkspaceState } from "./types";
import { DEFAULT_WORKTREE_ROOT } from "@cortex/workspace-domain";
import { entity, nonempty, requireFact } from "./core";
import { handleConfig, newConfig } from "./config";
import { MOCK_EFFORTS, MOCK_LANGUAGES, MOCK_MODELS, MOCK_MODES } from "./timeline";

const connection = (id: string, engine: EngineId, n: number, note: string, sessions: number, provider: string, launch: string): EngineConnection => ({
  id, engine, label: id, note, sessions, state: "connected", checkedAt: null,
  detail: { connectionId: `cn_${String(n).repeat(12).slice(0, 12)}`, provider, authMode: "native", keyKind: null, endpoint: null, launchCommand: launch, credential: "native account", qualified: true, createdAt: "28 Dec", updatedAt: "28 Dec" },
});
const CONNECTIONS = () => [
  connection("claude-agent-acp", "claude", 1, "Your account", 4, "anthropic", "claude-agent-acp"),
  connection("codex-acp", "codex", 2, "Your account", 2, "openai", "codex-acp"),
  connection("opencode", "opencode", 3, "Several providers", 2, "opencode", "opencode acp"),
];
export function newMachine(input: Pick<Machine, "id" | "name" | "kind" | "connection"> & Partial<Machine>): Machine {
  return {
    lastConfirmedAt: null, retentionDays: 30, autoArchiveDays: 7, worktreeRoot: DEFAULT_WORKTREE_ROOT, version: "1.0.6", latencyMs: null, historyGap: false, diskMb: null,
    remoteAccess: { allowed: false, devices: [] }, cleanup: null, diagnostics: null, storage: [],
    connections: CONNECTIONS(),
    config: newConfig(Object.fromEntries(CONNECTIONS().map(c => [c.id, c.sessions]))), mock: true, ...input,
  };
}
export function createInitialWorkspace(): WorkspaceState {
  return {
    mock: true, nextId: 20000, options: { models: [...MOCK_MODELS], efforts: [...MOCK_EFFORTS], modes: [...MOCK_MODES], languages: [...MOCK_LANGUAGES] }, machines: [newMachine({ id: "mock-machine-00000", kind: "local", name: "This machine", connection: "connected" })],
    projects: [], tasks: [], sprints: [], workers: [], messages: [], sessions: [], events: [], engineUsage: [], purged: [], repos: [], reviews: [],
  };
}

function online(machine: Machine) { requireFact(machine.connection === "connected", `${machine.name} is offline, so its settings are read-only`); return machine; }
/** Saving a machine's rules. Everything here is simulated: no remote write is made and no process is told. */
export function handleMachine(state: WorkspaceState, command: WorkspaceCommand): boolean {
  switch (command.type) {
    case "pair_machine":
      requireFact(!state.machines.some(m => m.id === command.id), "Duplicate machine identity");
      requireFact(/^[A-Z0-9]{4}-[A-Z0-9]{4}$/.test(command.code.trim()), "Enter the code shown on the other machine");
      requireFact(command.fingerprintsMatch, "The fingerprints differ, so nothing was connected");
      state.machines.push(newMachine({ id: nonempty(command.id, "Machine id"), name: nonempty(command.name, "Machine name"), kind: "remote", connection: "connected", latencyMs: 40, lastConfirmedAt: command.now === undefined ? null : new Date(command.now).toISOString() }));
      return true;
    case "rename_machine": online(entity(state.machines, command.machineId)).name = nonempty(command.name, "Machine name"); return true;
    case "remove_machine": {
      const machine = entity(state.machines, command.machineId);
      requireFact(machine.kind === "remote", "The local machine cannot be removed");
      requireFact(command.confirmed, "Owner confirmation required");
      const projects = new Set(state.projects.filter(p => p.machineId === machine.id).map(p => p.id));
      const tasks = new Set(state.tasks.filter(t => projects.has(t.projectId)).map(t => t.id));
      state.machines = state.machines.filter(m => m.id !== machine.id);
      state.projects = state.projects.filter(p => !projects.has(p.id));
      state.tasks = state.tasks.filter(t => !tasks.has(t.id));
      state.sprints = state.sprints.filter(s => !projects.has(s.projectId));
      state.workers = state.workers.filter(w => !tasks.has(w.taskId));
      state.messages = state.messages.filter(m => !tasks.has(m.taskId));
      state.events = state.events.filter(e => !tasks.has(e.taskId));
      return true;
    }
    case "set_connection": {
      const machine = entity(state.machines, command.machineId);
      if (machine.connection !== command.connection) {
        // What happened while it was away may never have reached this app, so the reconnect says so.
        machine.historyGap = command.connection === "connected";
        if (command.connection === "disconnected" && command.now !== undefined) machine.lastConfirmedAt = new Date(command.now).toISOString();
      }
      machine.connection = command.connection;
      return true;
    }
    case "save_storage": {
      const machine = online(entity(state.machines, command.machineId));
      requireFact(Number.isInteger(command.retentionDays) && command.retentionDays > 0, "Retention must be a positive integer in days");
      requireFact(Number.isInteger(command.autoArchiveDays) && command.autoArchiveDays > 0, "Auto archive must be a positive integer in days");
      machine.retentionDays = command.retentionDays; machine.autoArchiveDays = command.autoArchiveDays;
      return true;
    }
    case "set_worktree_root": {
      const machine = online(entity(state.machines, command.machineId)), path = nonempty(command.path, "Worktree folder");
      requireFact(/^(~\/|\/)/.test(path) && !path.split("/").includes(".."), "Use a full path, starting with ~/ or /");
      machine.worktreeRoot = path.replace(/\/+$/, "");
      return true;
    }
    case "check_connection": {
      const machine = online(entity(state.machines, command.machineId));
      entity(machine.connections, command.connectionId).checkedAt = command.now;
      return true;
    }
    case "refresh_models": {
      const { catalogue } = online(entity(state.machines, command.machineId)).config;
      for (const c of Object.values(catalogue)) { c.observedAt = command.now; c.failed = false; }
      return true;
    }
    case "add_connection": {
      const machine = online(entity(state.machines, command.machineId)), label = nonempty(command.label, "Connection name");
      requireFact(!machine.connections.some(c => c.id === label), "A connection with this name exists");
      machine.connections.push({ ...connection(label, command.engine as EngineId, 4, "Added here, sign in next", 1, command.engine as string, label), state: "needs_login" });
      return true;
    }
    case "remove_connection": {
      const machine = online(entity(state.machines, command.machineId));
      entity(machine.connections, command.connectionId);
      machine.connections = machine.connections.filter(c => c.id !== command.connectionId);
      return true;
    }
    case "set_sessions": {
      requireFact(Number.isInteger(command.sessions) && command.sessions >= 1 && command.sessions <= 64, "Sessions must be between 1 and 64");
      entity(online(entity(state.machines, command.machineId)).connections, command.connectionId).sessions = command.sessions;
      return true;
    }
    case "run_diagnostics": online(entity(state.machines, command.machineId)).diagnostics = { at: command.now, status: "ok", checks: 14 }; return true;
    case "set_remote_access": online(entity(state.machines, command.machineId)).remoteAccess.allowed = command.allowed; return true;
    case "revoke_device": {
      const access = online(entity(state.machines, command.machineId)).remoteAccess;
      requireFact(command.confirmed, "Owner confirmation required");
      access.devices = command.deviceId === null ? [] : access.devices.filter(d => d.id !== command.deviceId);
      return true;
    }
    default: return handleConfig(state, command);
  }
}
