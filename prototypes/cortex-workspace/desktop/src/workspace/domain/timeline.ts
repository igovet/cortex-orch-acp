import type { Activity, NewEntry, Task, TaskSettings, Worker, WorkspaceState } from "./types";
import { CATALOGUE_MODELS } from "./config-preset";
import { ACCESS_MODES, emptySession, LANGUAGES } from "@cortex/workspace-domain";
import { nextId, requireFact } from "./core";

export { isOpen } from "@cortex/workspace-domain";
export const MOCK_MODES = ACCESS_MODES, MOCK_LANGUAGES = LANGUAGES;

/** What this machine offers. Unavailable choices stay listed, with their reason. */
export const MOCK_MODELS = [
  { id: "opus[1m]", label: "Opus", engine: "claude-agent-acp" },
  { id: "claude-sonnet-5", label: "Sonnet", engine: "claude-agent-acp" },
  { id: "gpt-6.1-sol", label: "GPT-6.1 Sol", engine: "codex-acp" },
  { id: "gpt-6.1-terra", label: "GPT-6.1 Terra", engine: "codex-acp" },
  { id: "haiku", label: "Haiku", engine: "claude-agent-acp" },
  { id: "local", label: "Local model", engine: "", unavailable: "Unavailable: no engine on this machine" },
] as const;
export const MOCK_EFFORTS = [
  { id: "low", label: "Low" }, { id: "medium", label: "Medium" }, { id: "high", label: "High" }, { id: "xhigh", label: "Extra high" },
  { id: "max", label: "Max", unavailable: "Unavailable on this route" },
] as const;
export const DEFAULT_SETTINGS: TaskSettings = { model: "opus[1m]", effort: "high", mode: "development", language: "auto" };

export function validateSettings(settings: TaskSettings) {
  const model = MOCK_MODELS.find(m => m.id === settings.model), known = Object.values(CATALOGUE_MODELS).flat().some(m => m.id === settings.model), effort = MOCK_EFFORTS.find(e => e.id === settings.effort);
  requireFact(known || (model && !("unavailable" in model)), "That model is not available on this machine");
  requireFact(effort && !("unavailable" in effort), "That effort is not available on this route");
  requireFact(MOCK_MODES.some(m => m.id === settings.mode), "Unknown access mode");
  requireFact(MOCK_LANGUAGES.some(l => l.id === settings.language), "Unknown language");
  return settings;
}

/** Most fixture tasks have no recorded session; the first line written gives them one. */
export function sessionFor(state: WorkspaceState, task: Task) {
  let session = state.sessions.find(s => s.taskId === task.id);
  if (!session) { session = emptySession(task.id); state.sessions.push(session); }
  return session;
}
export function addEntry(state: WorkspaceState, task: Task, entry: NewEntry) {
  sessionFor(state, task).timeline.push({ ...entry, id: nextId(state, "entry") });
}
export const addLine = (state: WorkspaceState, task: Task, icon: string, text: string, at: number | undefined, rule = false) =>
  addEntry(state, task, { kind: "line", icon, text, at: at ?? null, rule });

export function newWorker(input: Pick<Worker, "id" | "taskId" | "executionId" | "role" | "status"> & Partial<Worker>): Worker {
  return {
    lane: "execution", goal: "", now: null, waitsFor: null, scope: [], route: null, attempt: null,
    elapsedMs: null, runningSince: null, context: null, usage: null, activity: [], result: null, mock: true, ...input,
  };
}
export function pushActivity(worker: Worker, text: string, entry: Partial<Omit<Activity, "sequence" | "text" | "mock">> = {}) {
  const item: Activity = { kind: "note", at: null, ...entry, sequence: worker.activity.length + 1, text, mock: true };
  worker.activity.push(item);
  return item;
}
/** Elapsed time only runs while a worker runs. Without a clock the last known figure stands. */
export function stopClock(worker: Worker, now: number | undefined) {
  if (worker.runningSince !== null && now !== undefined) worker.elapsedMs = (worker.elapsedMs ?? 0) + Math.max(0, now - worker.runningSince);
  worker.runningSince = null;
}
export function startClock(worker: Worker, now: number | undefined) { worker.runningSince = now ?? null; }
/** A stop reaches a running command too; it can never stay shown as running. */
export function cutCommands(worker: Worker, reason: string) {
  for (const item of worker.activity) if (item.command && ["running", "stopping"].includes(item.command.state)) {
    item.command.state = "stopped"; item.command.reason = reason;
  }
}
