import type { McpServer, ConnectResult } from "./index";
import type { Decision, EngineWeeklyUsage, Machine, Message, Project, ProjectRepo, SessionOptions, Sprint, Task, TaskChanges, TaskSession, TimelineEntry, Worker, WorkspaceCommand, WorkspaceState } from "./types";

/**
 * The one seam between the presentation and any backend. A screen sends intents and reads projections; it never changes
 * state itself. `real` is the machine service. `demo` is the labelled simulation and only ever comes from the demo entry.
 */
export type PortMode = "real" | "demo";
export type PortConnection = { state: "connecting" | "connected" | "disconnected" | "unavailable"; reason: string | null; at: number | null };
/** `everything` is for the demo entry, where every control is simulated. A real port lists what it can do. */
export type PortCapabilities = { everything: boolean; operations: string[] };

/** One project's confirmed facts, as they replace what was shown for that project. */
export type ProjectSlice = { project: Project; tasks: Task[]; sessions: TaskSession[] };
/** A confirmed snapshot. `version` is a decimal string, and a store installs only a newer one. */
export type ProjectionUpdate = { kind: "projection"; version: string } & ({ scope: "workspace"; state: WorkspaceState } | { scope: "project"; slice: ProjectSlice });
/** What the owner decided about a task. The runtime knows nothing of these, so they come from the machine and sit over its facts. */
export type TaskOverlay = Partial<Pick<Task, "title" | "titleState" | "acceptance" | "reviewActions" | "integrationFacts" | "closedAs" | "doneAt" | "archive" | "archiveQuestion" | "archiveAskAfter" | "targetBranch" | "sprintId" | "dependencies" | "integration" | "notVerifiedByCortex" | "purgeBlocker" | "work" | "checkout" | "settings" | "conflict" | "merge" | "resolvedBy">> & {
  status?: "ready_to_merge" | "done"; automaticReady?: boolean; planning?: boolean; purged?: boolean;
};
/**
 * One project's facts from the machine itself: Git, workers and owner decisions. They move on their own, without the
 * runtime's project version moving, so they are not a projection and each read replaces the last.
 */
export type ProjectFacts = { hasGit?: boolean; projectId: string; openBranch: string; defaultBranch: string; repo: ProjectRepo | null; workers: Worker[]; reviews: TaskChanges[]; overlays: Record<string, TaskOverlay>; sprints: Sprint[];
  /** What the owner said or queued in this app. The runtime keeps no owner messages a screen can read back. */
  messages?: Message[];
  /** Answers kept for a batch of questions, which the machine takes only as a whole. By decision. */
  heldAnswers?: Record<string, string>;
  /** Why Cortex is not working for this project now, when it is not. */
  issue?: string | null };
/** What the machine reports about itself. */
export type MachineFacts = { machineId: string; patch: Partial<Machine>; engineUsage: EngineWeeklyUsage[]; options?: SessionOptions };
export type PortUpdate = { kind: "registry"; projectIds: string[] } | ProjectionUpdate | { kind: "connection"; connection: PortConnection } | { kind: "facts"; facts: ProjectFacts } | { kind: "machine"; facts: MachineFacts }
  /** What the machine can do changed, for example a project's Cortex started. Read everything again. */
  | { kind: "refresh" };

export type CommandEnvelope = {
  requestId: string; operation: WorkspaceCommand["type"];
  /** The version the owner was looking at when they acted. */
  expectedVersion: string | null;
  payload: WorkspaceCommand;
};
/** That a command was recorded. It is not the new state: the screen changes only when a projection confirms it. */
export type CommandReceipt = { requestId: string; ok: boolean; outcome: string; message: string; entityId?: string; entityVersion?: string };

/** One folder the machine found. `git` says a `.git` is there. */
export type DirectoryEntry = { path: string; git: boolean };
/** What a machine found for the text typed. `complete` is false when a limit ended the walk, and `cancelled` when a newer search took over. */
export type DirectorySearch = { query: string; home: string; entries: DirectoryEntry[]; complete: boolean; cancelled?: boolean };

export type QueryParameters = { "capabilities.get": Record<string, never>; "project.snapshot": { projectId?: string }; "project.facts": { projectId?: string }; "machine.facts": Record<string, never> };
export type QueryResults = { "capabilities.get": PortCapabilities; "project.snapshot": ProjectionUpdate[]; "project.facts": ProjectFacts[]; "machine.facts": MachineFacts[] };
export type QueryOperation = keyof QueryResults;

export interface WorkspacePort {
  readonly mode: PortMode;
  /** Private owner maintenance input is never retained in command history or receipts. */
  machineAction?(op: "workflow.describe" | "maintenance" | "engine.connection" | "native.engine_login", params: Record<string, unknown>): Promise<Record<string, unknown>>;
  query<O extends QueryOperation>(operation: O, parameters: QueryParameters[O]): Promise<QueryResults[O]>;
  command(envelope: CommandEnvelope): Promise<CommandReceipt>;
  subscribe(listener: (update: PortUpdate) => void): () => void;
  /** One file of a checkout, read when it is opened. A port with no files has none. */
  discoverMcp?(server: McpServer): Promise<ConnectResult>;
  takeDroppedFiles?(): Promise<import("./types").Attachment[]>;
  /** Saved creation bytes for one task, fetched on demand rather than duplicated in every board snapshot. */
  readTaskAttachments?(taskId: string): Promise<import("./types").Attachment[]>;
  readClipboardImage?(): Promise<import("./types").Attachment[]>;
  /** Saves one attachment where the owner picks, through the system's save dialog. A path of null means the owner cancelled. */
  saveAttachment?(file: import("./types").Attachment): Promise<{ path: string | null }>;
  /** The system's folder chooser. Null when the owner cancelled. */
  pickFolder?(): Promise<string | null>;
  /** Folders on one machine for the text the owner typed. A newer search in the same session replaces the one before it. */
  searchDirectories?(request: { machineId: string; session: string; query: string; limit?: number }): Promise<DirectorySearch>;
  cancelDirectorySearch?(machineId: string, session: string): Promise<void>;
  readWorkerActivity?(taskId: string, workerId: string, cursor?: string): Promise<Pick<Worker, "activity" | "activityCursor" | "messages">>;
  readReport?(taskId: string, reportId: string, cursor?: string): Promise<import("@cortex/workspace-contract").ReportPage>;
  readFile?(checkoutId: string, file: string): Promise<FileRead>;
  /** Writes the owner's text only over the revision they last saw. A port with no writes has no editor. */
  saveFile?(checkoutId: string, file: string, text: string, revision: string | null): Promise<FileSaveResult>;
}

/** One read of a checkout file. `revision` is the hash of its bytes; `readOnly` and `writer` say why saving it is not possible now. */
export type FileRead = { text: string | null; reason?: string; revision?: string | null; size?: number; readOnly?: string | null; writer?: string | null };
/** `unknown` means the reply was lost, which says nothing about the write. */
export type FileSaveResult = { outcome: "saved"; revision: string; size: number } | { outcome: "conflict"; revision: string | null } | { outcome: "writer_active"; writer: string } | { outcome: "refused"; message: string } | { outcome: "unknown" };

const DECIMAL = /^(0|[1-9][0-9]*)$/;
/** Versions are wire decimals, which can pass the safe integer range. */
export function compareVersions(a: string, b: string): -1 | 0 | 1 {
  if (!DECIMAL.test(a) || !DECIMAL.test(b)) throw new Error(`Not a decimal version: ${a}, ${b}`);
  const x = BigInt(a), y = BigInt(b);
  return x === y ? 0 : x < y ? -1 : 1;
}
/** Each project has its own version line, so a project is only ever compared with itself. */
export const projectionKey = (update: ProjectionUpdate) => update.scope === "workspace" ? "workspace" : `project:${update.slice.project.id}`;

/** A runtime fact stays until the owner's decision replaces it, and a decided task no longer reports a live phase. */
export function overlayTask(task: Task, overlay: TaskOverlay | undefined, branch: string): Task {
  const checkout = task.checkout.kind === "main" ? { kind: "main" as const, branch } : task.checkout;
  if (!overlay) return { ...task, checkout };
  const { status, automaticReady, planning: _planning, purged: _purged, ...facts } = overlay;
  // Runtime activity supersedes acceptance of an older result, even if a
  // machine-state write failed after durable steering capture.
  const reopened = task.process === "live" && status === "ready_to_merge";
  const next: Task = { ...task, ...facts, checkout: facts.checkout ?? checkout, status: reopened ? task.status : status ?? task.status };
  if (reopened) { next.acceptance = null; next.observedPhase = task.observedPhase; return next; }
  // Acceptance stores the wire source version; screens label the result sequence.
  if (next.acceptance && next.result && String(next.acceptance.version) === next.result.sourceVersion && next.acceptance.digest === next.result.digest) {
    next.acceptance = { ...next.acceptance, version: next.result.version };
  }
  if (status) next.observedPhase = undefined;
  if (automaticReady && next.result) next.result = { ...next.result, automaticReady: true };
  return next;
}

const FACT_ENTRY = "facts:";
/** The timeline holds the runtime's record, then what only the machine knows: the workers, and what the owner said here. Each read replaces these. */
function withFactEntries(session: TaskSession, facts: ProjectFacts): TaskSession {
  const workers = facts.workers.filter(w => w.taskId === session.taskId).map(w => w.id);
  // A message the runtime already recorded is that record, so it reads once, in its place.
  const recorded = new Set(session.timeline.flatMap(e => e.kind === "said" ? [e.text] : []));
  const said = (facts.messages ?? []).filter(m => m.taskId === session.taskId && m.workerId === null && m.delivery !== "queued" && !recorded.has(m.text));
  const entries: TimelineEntry[] = [
    ...(workers.length ? [{ id: `${FACT_ENTRY}workers`, kind: "workers" as const, title: "Workers", workerIds: workers }] : []),
    ...said.map(m => ({ id: `${FACT_ENTRY}message:${m.id}`, kind: "message" as const, messageId: m.id })),
  ];
  const kept = session.timeline.filter(e => !e.id.startsWith(FACT_ENTRY)), held = facts.heldAnswers ?? {};
  const decisions = session.decisions.map((d): Decision => {
    if (held[d.id] !== undefined) return d.answer && !d.held ? d : { ...d, answer: { text: held[d.id], at: null }, held: true };
    if (!d.held) return d;
    const { held: _held, ...open } = d;
    return { ...open, answer: null };
  });
  return { ...session, decisions, timeline: [...kept, ...entries] };
}

/** One project's records replaced by what its facts say now. When they are the same records in the same order, each stays
 *  where it stood, so reading unchanged facts again leaves every list as it was and nothing on screen redraws; otherwise the
 *  project's records move to the end, as they arrive. */
function reinstall<T>(list: T[], owned: (item: T) => boolean, next: T[], keyOf: (item: T) => string): T[] {
  const old = list.filter(owned);
  if (old.length === next.length && old.every((item, i) => keyOf(item) === keyOf(next[i]))) { let i = 0; return list.map(item => owned(item) ? next[i++] : item); }
  return [...list.filter(item => !owned(item)), ...next];
}

export function applyFacts(state: WorkspaceState, facts: ProjectFacts): WorkspaceState {
  const own = (t: Task) => t.projectId === facts.projectId;
  const ids = new Set(state.tasks.filter(own).map(t => t.id));
  const gone = (id: string) => { const o = facts.overlays[id]; return !!o && (o.purged === true || o.planning === true); };
  // A planning run has no task on the board, but its questions and held answers are still the owner's to read.
  const planning = new Set(Object.entries(facts.overlays).flatMap(([id, o]) => o.planning === true ? [id] : []));
  return {
    ...state,
    projects: state.projects.map(p => p.id === facts.projectId ? { ...p, openBranch: facts.openBranch, defaultBranch: facts.defaultBranch, issue: facts.issue ?? null, hasGit: facts.hasGit ?? facts.repo?.hasGit ?? (facts.repo ? true : p.hasGit) } : p),
    tasks: state.tasks.filter(t => !own(t) || !gone(t.id)).map(t => own(t) ? overlayTask(t, facts.overlays[t.id], facts.openBranch) : t),
    workers: reinstall(state.workers, w => ids.has(w.taskId), facts.workers, w => w.id),
    repos: reinstall(state.repos, r => r.projectId === facts.projectId, facts.repo ? [facts.repo] : facts.hasGit === false ? [] : state.repos.filter(r => r.projectId === facts.projectId), r => r.projectId),
    reviews: reinstall(state.reviews, r => ids.has(r.taskId), facts.reviews, r => r.taskId),
    sprints: reinstall(state.sprints, s => s.projectId === facts.projectId, facts.sprints, s => s.id),
    messages: reinstall(state.messages, m => ids.has(m.taskId), facts.messages ?? [], m => m.id),
    sessions: state.sessions.map(s => ids.has(s.taskId) || planning.has(s.taskId) ? withFactEntries(s, facts) : s),
  };
}

export function applyMachineFacts(state: WorkspaceState, facts: MachineFacts): WorkspaceState {
  return { ...state, machines: state.machines.map(m => m.id === facts.machineId ? { ...m, ...facts.patch } : m), engineUsage: facts.engineUsage, options: facts.options ?? state.options };
}

export function applyProjection(state: WorkspaceState, update: ProjectionUpdate): WorkspaceState {
  if (update.scope === "workspace") return update.state;
  const { project, tasks, sessions } = update.slice;
  // A planning run's session outlives its hidden task, so what arrives replaces it too.
  const owned = new Set([...state.tasks.filter(t => t.projectId === project.id).map(t => t.id), ...sessions.map(s => s.taskId)]);
  const known = state.projects.some(p => p.id === project.id);
  return {
    ...state,
    projects: known ? state.projects.map(p => p.id === project.id ? project : p) : [...state.projects, project],
    tasks: [...state.tasks.filter(t => t.projectId !== project.id), ...tasks],
    sessions: [...state.sessions.filter(s => !owned.has(s.taskId)), ...sessions],
  };
}
