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
export type TaskOverlay = Partial<Pick<Task, "title" | "titleState" | "acceptance" | "reviewActions" | "integrationFacts" | "closedAs" | "doneAt" | "archive" | "targetBranch" | "sprintId" | "dependencies" | "integration" | "notVerifiedByCortex" | "purgeBlocker" | "work" | "checkout" | "settings" | "result" | "conflict" | "merge" | "resolvedBy" | "writeDenials" | "spend">> & {
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
  issue?: string | null;
  /** When the port numbers its reads: this read's place in the order the reads began. Facts that began earlier never
   *  replace facts that began later. Absent, facts arriving during a coherent read cause another read instead. */
  readOrder?: number };
/** The machine's own background fetch of the engines' model lists: still running, and the connections whose fetch ended without a list. */
export type CatalogueState = { running: boolean; failed: { connectionId: string; reason: string }[] };
/**
 * What the machine reports about itself. `machine.facts` carries the settings the first screen needs (config, connections,
 * engine usage, the composer's options); `machine.overview` carries only the storage, retention, worktree, version, GitHub CLI
 * and maintenance fields, and omits `engineUsage` and `options`, which an update then leaves as they were.
 */
export type MachineFacts = { machineId: string; patch: Partial<Machine>; engineUsage?: EngineWeeklyUsage[]; options?: SessionOptions; catalogue?: CatalogueState };
export type PortUpdate = { kind: "preview"; slice: ProjectSlice; facts: ProjectFacts } | { kind: "enrichment"; update: ProjectionUpdate } | { kind: "project_state"; project: Project; issue: string | null; cause?: string | null } | { kind: "delta"; update: ProjectionUpdate } | { kind: "registry"; projectIds: string[] } | ProjectionUpdate | { kind: "connection"; connection: PortConnection } | { kind: "facts"; facts: ProjectFacts } | { kind: "machine"; facts: MachineFacts }
  /** What the machine can do changed, for example a project's Cortex started. Read everything again. */
  | { kind: "refresh" };

export type CommandEnvelope = {
  requestId: string; operation: WorkspaceCommand["type"];
  /** The version the owner was looking at when they acted. */
  expectedVersion: string | null;
  payload: WorkspaceCommand;
};
/** That a command was recorded. It is not the new state: the screen changes only when a projection confirms it. */
/**
 * What a screen on view shows of a project: its facts, a Git screen (Git, Worktrees, Code, Changes, the merge dialog: Git is
 * also reread on the visible refresh), or a Git status line such as the title band's Git button (Git is read with the facts,
 * on events, on return to the window and after the owner's own action, never on a timer).
 */
export type GitInterest = "facts" | "git" | "status";
export type CommandReceipt = { requestId: string; ok: boolean; outcome: string; message: string; entityId?: string; entityVersion?: string; deletedTaskIds?: string[]; keptPaths?: string[] };

/** One folder the machine found. `git` says a `.git` is there. */
export type DirectoryEntry = { path: string; git: boolean };
/** What a machine found for the text typed. `complete` is false when a limit ended the walk, and `cancelled` when a newer search took over. */
export type DirectorySearch = { query: string; home: string; entries: DirectoryEntry[]; complete: boolean; cancelled?: boolean };

/**
 * `fresh` bypasses repository cache age; `full` rereads all projects even after event invalidations were consumed.
 * `machine.overview` is read on demand, when a screen that shows its fields opens; startup never asks for it.
 */
export type QueryParameters = { "capabilities.get": Record<string, never>; "project.snapshot": { projectId?: string; full?: boolean }; "project.facts": { projectId?: string; fresh?: boolean; git?: boolean }; "machine.facts": Record<string, never>; "machine.overview": Record<string, never> };
export type QueryResults = { "capabilities.get": PortCapabilities; "project.snapshot": ProjectionUpdate[]; "project.facts": ProjectFacts[]; "machine.facts": MachineFacts[]; "machine.overview": MachineFacts[] };
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
  /** A saved task's files, fetched on demand rather than duplicated in every board snapshot. */
  readTaskAttachments?(taskId: string): Promise<import("./types").AttachmentInput[]>;
  /** Uploads files the owner just attached, once; every later command carries the references it answers. */
  uploadAttachments?(files: import("./types").Attachment[]): Promise<import("./types").AttachmentRef[]>;
  /** One saved file's bytes, read only to show or save it. */
  readAttachment?(file: import("./types").AttachmentRef): Promise<import("./types").Attachment>;
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
  /** Reads one task's retained conversation again now, after a read that did not complete. The result arrives as an update. */
  retryHistory?(taskId: string): void;
  /** Reads the next older part of a task's conversation, when only its latest part is held (`historyEarlier`). The rows
   *  arrive as an update and are put before the ones held; a failed read says so on `historyEarlier`. The optional
   *  tool view tells the reader whether a fold of only runtime steps will be shown. */
  loadEarlierHistory?(taskId: string, showAllTools?: () => boolean): void;
  /**
   * A screen on view shows one project's facts (`facts`: workers, reviews, queued messages) or its Git data (`git`: the
   * repository snapshot, task diffs, branches). The port reads only what some screen holds interest in; the returned
   * function releases it. `taskId` names the task the screen shows: while a project has interest, its facts carry worker
   * rows only for such tasks. A port that reads nothing in the background has no such registry.
   */
  interest?(projectId: string, kind: GitInterest, taskId?: string): () => void;
  /** A screen on view shows the machine overview (storage, retention, version); it is reread only while one does. */
  watchOverview?(): () => void;
  /**
   * Settles once every board or facts read that `query` stopped waiting for has delivered its late result as an update
   * (or failed). A port whose queries always carry their whole result has none.
   */
  presented?(): Promise<void>;
  /**
   * The local machine's remote access (`remote.*`), when its service advertises it (`FEATURES.remoteAccess`). Each call
   * answers the whole new status. `watch` tells when the service says the status changed.
   */
  remoteAccess?: {
    call<O extends import("./remote").RemoteOperation>(operation: O, message: import("./remote").RemoteRequests[O]): Promise<import("./remote").RemoteStatus>;
    watch(listener: () => void): () => void;
  };
  /**
   * The machine's open sudo password prompts (`askpass.*`), when its service advertises them. `list` answers null from a
   * service without them. The password goes to the machine once and is not kept by the port.
   */
  sudoPrompts?: {
    list(): Promise<import("./sudo").SudoPrompts | null>;
    answer(promptId: string, password: string): Promise<void>;
    cancel(promptId: string): Promise<void>;
  };
  /**
   * Access the machine's tasks lack and only the owner can give (`owner_actions.*`), when its service advertises them. `list`
   * answers null from a service without them. A token goes to the machine once and is not kept by the port.
   */
  ownerActions?: {
    list(): Promise<import("./owner-actions").OwnerActions | null>;
    act(projectId: string, actionId: string, action: import("./owner-actions").OwnerActionVerb): Promise<import("./owner-actions").OwnerActionReply>;
    token(projectId: string, actionId: string, token: string): Promise<import("./owner-actions").OwnerActionReply>;
  };
  readFile?(checkoutId: string, file: string): Promise<FileRead>;
  /** A commit message or pull request text the machine wrote from the task's real changes. Null when none could be had; the caller keeps its own text. */
  draftIntegrationText?(request: IntegrationTextRequest): Promise<IntegrationText | null>;
  /** Read-only draft from staged changes, or all changes when nothing is staged. */
  draftCheckoutCommit?(request: { checkoutId: string }): Promise<IntegrationText | null>;
  /** Read-only draft from local commits absent from the selected destination's tracking information. */
  draftPushSummary?(request: { checkoutId: string; remote: string; branch: string }): Promise<IntegrationText | null>;
  /** One picture, video or sound of a checkout as bytes, for viewing only. Its type comes from its name. */
  readMedia?(checkoutId: string, file: string): Promise<FileRead>;
  /**
   * The project's own skills (`.agents/skills`, `.claude/skills`, `.codex/skills`): names, project-relative paths and short
   * descriptions, never a body. Read when a composer's `/` list first opens for that project.
   */
  listProjectSkills?(projectId: string): Promise<ProjectSkills>;
  /**
   * One level of each named checkout folder for the Code tree (`""` is the checkout itself), and with `overview` what the
   * repository snapshot's tree leaves out (ignored files, top-most ignored and untracked folders), never walked into.
   * Read only when Code asks: on open, on its refresh and when the owner expands a folder.
   */
  listFolders?(checkoutId: string, request: FolderListRequest): Promise<FolderListing>;
  /** Writes the owner's text only over the revision they last saw. A port with no writes has no editor. */
  saveFile?(checkoutId: string, file: string, text: string, revision: string | null): Promise<FileSaveResult>;
}

/** One read of a checkout file. `revision` is the hash of its bytes; `readOnly` and `writer` say why saving it is not possible now. */
/** `pathOnly`: a bundled skill has the same name, so only the path names this file. `omitted` counts skills past the bound. */
export type ProjectSkill = { name: string; path: string; description: string | null; pathOnly: boolean };
export type ProjectSkills = { skills: ProjectSkill[]; omitted: number };
export type FileRead = { text: string | null; reason?: string; revision?: string | null; size?: number; readOnly?: string | null; writer?: string | null; media?: FileMedia };
export type FolderListRequest = { dirs: string[]; overview?: boolean };
/** `ignored` is Git's verdict (in a folder without Git: one of the folders its tree does not walk); null when Git could not say. */
export type FolderEntry = { name: string; dir: boolean; link?: boolean; ignored: boolean | null };
/** At most a bounded number of entries; `more` counts the rest, a lower bound when `exact` is false. */
export type FolderRead = { dir: string; entries: FolderEntry[]; more: number; exact: boolean } | { dir: string; code: string; message: string };
export type FolderOverview = { truncated: boolean; entries: { path: string; dir: boolean; ignored: boolean }[] };
export type FolderListing = { overview: FolderOverview | null; folders: FolderRead[] };
export type IntegrationTextRequest = { taskId: string; method: "commit" | "squash" | "pr"; targetBranch: string; paths?: string[]; summary?: string };
/** Draft origin; a saved Push body is marked separately from its suggested PR title. */
export type IntegrationText = { source: "model" | "template" | "fallback"; message?: string; title?: string; body?: string; model?: string; reason?: string; bodySource?: "saved_push_summary" };
/** Base64 bytes and the MIME type the machine chose from the file name. */
export type FileMedia = { mime: string; data: string };
/** `unknown` means the reply was lost, which says nothing about the write. */
export type FileSaveResult = { outcome: "saved"; revision: string; size: number } | { outcome: "conflict"; revision: string | null } | { outcome: "writer_active"; writer: string } | { outcome: "refused"; message: string } | { outcome: "unknown" };

/** Native read snapshots and patches use a separate generation/sequence domain from mutation receipts. */
export type NativeTaskSnapshot = { projectId: string; generation: string; highWatermark: string; version: string; items: import("@cortex/workspace-contract").Task[] };
export type NativeTaskDelta = { projectId: string; batch: { generation: string; streamKey: { scope: "project"; project_id: string }; fromSequence: string; throughSequence: string; changes: NativeTaskChange[] } };
export type NativeTaskChange = { kind: "task" | "project"; id: string; baseVersion: string; newVersion: string; operation: { type: "remove" | "invalidate" } | { type: "upsert"; data: { kind: "task_projection"; data: import("@cortex/workspace-contract").Task } } };
/** Throw on a gap or a conflicting base; callers keep confirmed state and resnapshot, never guess. */
export function applyNativeTaskDelta(snapshot: NativeTaskSnapshot, delta: NativeTaskDelta): NativeTaskSnapshot {
  const { batch } = delta;
  if (delta.projectId !== snapshot.projectId || batch.streamKey.project_id !== snapshot.projectId || batch.generation !== snapshot.generation) throw new Error("Native stream changed");
  if (compareVersions(batch.throughSequence, snapshot.highWatermark) <= 0) return snapshot;
  if (BigInt(batch.fromSequence) !== BigInt(snapshot.highWatermark) + 1n || compareVersions(batch.throughSequence, batch.fromSequence) < 0) throw new Error("Native stream gap");
  const items = new Map(snapshot.items.map(t => [t.taskId, t]));
  for (const change of batch.changes) {
    if (change.kind === "project") throw new Error("Native project requires resnapshot");
    const prior = items.get(change.id);
    if ((prior?.version ?? "0") !== change.baseVersion || compareVersions(change.newVersion, change.baseVersion) <= 0) throw new Error("Native entity base changed");
    if (change.operation.type === "remove") items.delete(change.id);
    else if (change.operation.type === "upsert") {
      const task = change.operation.data.data;
      if (task.taskId !== change.id || task.version !== change.newVersion) throw new Error("Native entity identity mismatch");
      items.set(change.id, task);
    } else throw new Error("Native entity requires resnapshot");
  }
  return { ...snapshot, highWatermark: batch.throughSequence, items: [...items.values()] };
}

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
  // A recorded automatic finish is settled even if the native read is still catching up.
  if (status === "done" && next.integration?.automatic === true) {
    next.process = "stopped";
    next.issue = null;
  }
  if (task.closedAs === "unconfirmed") {
    // Older saved presentation must not replace the durable close or imply
    // that an earlier result was accepted for the disputed execution.
    return { ...next, status: "closed_unverified", process: "unknown", observedPhase: task.observedPhase,
      closedAs: "unconfirmed", doneAt: task.doneAt, notVerifiedByCortex: true, archive: null,
      acceptance: null, integration: null };
  }
  if (automaticReady && next.result) next.result = { ...next.result, automaticReady: true };
  return next;
}

const FACT_ENTRY = "facts:";
/** The timeline holds the runtime's record, then what only the machine knows: the workers, and what the owner said here. Each read replaces these. */
function withFactEntries(session: TaskSession, facts: ProjectFacts): TaskSession {
  const workers = facts.workers.filter(w => w.taskId === session.taskId).map(w => w.id);
  // A message the runtime already recorded is that record, so it reads once, in its place.
  const recorded = new Set(session.timeline.flatMap(e => e.kind === "said" ? [e.text.trim()] : []));
  // With only the latest part of the conversation read, a message sent before that part has its record among the rows not
  // read yet: it is not added at the end as if it had just been said.
  const since = session.historyEarlier?.since ?? null;
  const said = (facts.messages ?? []).filter(m => m.taskId === session.taskId && m.workerId === null && m.delivery !== "queued" && !recorded.has(m.text.trim())
    && !(session.historyEarlier && since !== null && m.at !== null && m.at < since));
  const entries: TimelineEntry[] = [
    ...(workers.length ? [{ id: `${FACT_ENTRY}workers`, kind: "workers" as const, title: "Workers", workerIds: workers }] : []),
    ...(facts.overlays[session.taskId]?.acceptance?.automatic ? [{
      id: `${FACT_ENTRY}automatic-acceptance`,
      kind: "line" as const, icon: "check-circle",
      text: `Automatically accepted v${facts.overlays[session.taskId]!.acceptance!.version}`,
      at: facts.overlays[session.taskId]!.acceptance!.at ?? null, rule: false,
    }] : []),
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
  return { ...state, machines: state.machines.map(m => m.id === facts.machineId ? { ...m, ...facts.patch } : m), engineUsage: facts.engineUsage ?? state.engineUsage, options: facts.options ?? state.options };
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

/** Apply only a service-confirmed deletion, including all secondary task surfaces. */
export function applySprintDeletion(state: WorkspaceState, sprintId: string, taskIds: ReadonlySet<string>): WorkspaceState {
  return {...state,sprints:state.sprints.filter(s => s.id !== sprintId),
    tasks:state.tasks.filter(t => !taskIds.has(t.id)),sessions:state.sessions.filter(s => !taskIds.has(s.taskId)),
    workers:state.workers.filter(w => !taskIds.has(w.taskId)),messages:state.messages.filter(m => !taskIds.has(m.taskId)),
    events:state.events.filter(e => !taskIds.has(e.taskId)),reviews:state.reviews.filter(r => !taskIds.has(r.taskId))};
}
