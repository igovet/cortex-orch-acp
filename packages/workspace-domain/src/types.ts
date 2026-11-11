export type Connection = "connected" | "disconnected";
export type EngineId = "claude" | "codex" | "opencode";
export type PairedDevice = { id: string; name: string; paired: string; lastSeen: string | null };
export type CleanupRun = { at: number; removed: number; skipped: number; asked: number; freedMb: number; reclamationFailed?: number };
export type DiagnosticsRun = { at: number; status: "ok" | "degraded" | "failed"; checks: number; items?: { name: string; ok: boolean; detail: string; action?: string }[] };
export type EngineKey = "claude-agent-acp" | "codex-acp" | "opencode";
export type Tier = "flagship" | "deep" | "standard" | "light";
/** Read-only facts of a registered connection. Credentials themselves are never here. */
export type ConnectionDetail = { connectionId: string; provider: string; authMode: string; keyKind: string | null; endpoint: string | null; launchCommand: string; credential: string; qualified: boolean; createdAt: string; updatedAt: string };
export type EngineConnection = { id: string; engine: EngineId; label: string; note: string; sessions: number; state: "connected" | "needs_login" | "unavailable"; checkedAt: number | null; detail: ConnectionDetail };
export type EngineSetup = { status: "not_downloaded" | "downloaded_no_login" | "ready_to_connect" | "connected"; automatic: boolean; reason: string | null; autoConnectDisabled: boolean };
export type CatalogueModel = { routeId?: string; id: string; label: string; efforts: string[]; observed: boolean; contextWindow?: number };
export type EngineCatalogue = { observedAt: number | null; failed: boolean; models: CatalogueModel[] };
export type Problem = { path: string; message: string };

/** Per-model policy of routing.json. Null means neutral: the key is not written. */
export type ModelPolicy = { effortFloor: string | null; maxEffort: string | null; effortLadder: string[] | null; alsoStandardFor: string[] | null; preferredFor: string[] | null; preferredWhen: string | null; requestNames: string[] | null };
/** `models: null` is shared by the engine, a list is exact, an empty list excludes every model. */
export type QuotaWindow = { window: string; models: string[] | null };
export type EngineRules = { enabled: string[]; auto: string[]; tiers: Record<string, Tier[]>; policy: Record<string, ModelPolicy>; quota: QuotaWindow[] };
export type RoutePin = { engine: EngineKey; model: string; effort: string | null };
/** The `routing` surface: routing.json. Models and Routing edit this one value. */
export type RoutingRules = {
  version: number; deny: string[]; providers: EngineKey[]; engines: Record<EngineKey, EngineRules>;
  pins: Record<string, RoutePin>; affinity: Record<string, EngineKey | null>; autoAllow: string[]; legacyTiers: string[];
};
export type StoredLimits = { permissionLevel: string | null; calls: number | null; tokens: number | null; concurrency: number | null; spendLimit: number | null; spendEnforceable: boolean };
/** Null is unrestricted, an empty list allows nothing. */
export type Ceiling = {
  allowedRoutes: string[] | null; allowedConnections: string[] | null; allowedRecipients: string[] | null; attempts: number | null;
  failureClasses: string[] | null; targetRoutes: string[] | null; budget: number | null; consent: boolean; stored: StoredLimits;
};
export type CoordinatorChoice = { engine: EngineKey; model: string; effort: string; parameters: Record<string, string> };
export type RouteDefault = { routeId: string; connectionId: string; model: string; parameters: Record<string, string> };
/** The `user_defaults` surface: defaults.json, plus the two values Workspace owns. Coordinator and Limits edit this one value. */
export type UserDefaults = {
  coordinator: CoordinatorChoice | null; routeDefault: RouteDefault | null; language: TaskSettings["language"]; mode: TaskSettings["mode"]; checkout: "worktree" | "main";
  ceiling: Ceiling; preserved: string[];
};
export type McpEnv = { name: string; stored: boolean; action: SecretAction; value: string };
export type McpTool = { name: string; description: string; hint: "read" | "destructive" | null };
/** What one tool listing returned. It is an observation, not configuration, so a check never makes a draft. */
export type McpObservation = { at: number; name: string; version: string; tools: McpTool[] };
/** One entry of mcp-servers.json. The tool lists always nest: coordinator within read-only within allowed. */
export type McpServer = {
  name: string; command: string; args: string[]; env: McpEnv[]; enabled: boolean; roles: ("coordinator" | "worker")[] | null; engines: EngineKey[] | null;
  allowedTools: string[]; readOnlyTools: string[]; coordinatorTools: string[] | null; alwaysLoadTools: string[] | null;
  readPaths: string[]; writePaths: string[]; network: boolean; runtimeDirEnv: string[];
  handTools: string[]; unsupportedKeys: string[]; problem: string | null;
};
export type SecretAction = "keep" | "replace" | "remove";
export type RoleAccess = "writing" | "read_only" | "consultant";
export type Surfaces = { routing: RoutingRules; user_defaults: UserDefaults; mcp_servers: McpServer[] };
export type SurfaceKey = keyof Surfaces | "project_ceiling";
export type McpPolicy = Omit<McpServer, "command" | "args" | "env" | "handTools" | "unsupportedKeys" | "problem">;
/** A profile never holds project settings, MCP commands, arguments, environment or credentials. */
export type ProfileSections = { routing: RoutingRules; defaults: UserDefaults; mcp: McpPolicy[]; sessions: Record<string, number> };
export type ProfileSection = keyof ProfileSections;
export type Profile = { id: string; name: string; description: string; updatedAt: number | null; sections: Partial<ProfileSections> };
export type ConfigFault = "env_routing" | "invalid_routing" | "migration" | "interrupted" | "unavailable" | "catalogue" | "engine_missing" | "preset_missing";
export type ConfigFiles = { envRouting: string | null; invalidRouting: boolean; migration: boolean; missing: string[]; backups: number; preservedRouting: string[]; preservedDefaults: string[]; askpass: boolean; env: { engineUpdate: string | null; engineFollow: string | null; opencodeBinary: string | null } };
export type MachineConfig = {
  revisions: Record<SurfaceKey, number>; surfaces: Surfaces; projects: Record<string, { routing?: RoutingRules; ceiling?: Ceiling; revisions?: Partial<Record<SurfaceKey,number>> }>;
  active: string | null; interrupted: boolean; unavailable: string | null; profiles: Profile[];
  catalogue: Record<EngineKey, EngineCatalogue>; quotaObserved: Record<EngineKey, { window: string; percent: number }[]>;
  mcpObserved: Record<string, McpObservation>; setup: Record<EngineKey, EngineSetup>; files: ConfigFiles;
};
/** A running Workspace process. Projects live on a machine; a project is never a machine. */
export type Machine = {
  ghAvailable?: boolean; reclamationFailures?: number;
  /** Absolute default resolved by the service in its channel-private data directory. */
  defaultWorktreeRoot?: string;
  id: string; name: string; kind: "local" | "remote"; connection: Connection;
  lastConfirmedAt: string | null; retentionDays: number; autoArchiveDays: number; worktreeRoot: string;
  version: string | null; latencyMs: number | null; historyGap: boolean; diskMb: number | null;
  remoteAccess: { allowed: boolean; devices: PairedDevice[] };
  cleanup: CleanupRun | null; diagnostics: DiagnosticsRun | null; storage: { label: string; mb: number }[];
  connections: EngineConnection[]; config: MachineConfig; mock?: true;
};
export type Project = {
  id: string; machineId: string; name: string; repository: string;
  openBranch: string; defaultBranch: string; activated?: boolean; confirmedAt?: number; mock?: true;
  /** Why Cortex is not working for this project right now, in the owner's words. The machine itself can be fine. */
  issue?: string | null;
  /** Whether the project folder is a Git repository, once the machine has said. Unknown until then. */
  hasGit?: boolean;
};
export type CheckoutChoice = { kind: "main" } | { kind: "worktree"; branch?: string; path?: string };
export type Checkout = { kind: "main"; branch: string } | { kind: "worktree"; branch: string; path: string };
export type CheckOutcome = "passed" | "failed" | "not_run" | "unverified";
export type Check = { name: string; outcome: CheckOutcome; detail: string; source?: "runtime" | "worker"; technicalReason?: string; kind?: "automated" | "manual" | "policy"; command?: string[]; cwd?: string; rerunnable?: boolean; output?: string; exitCode?: number; running?: boolean };
export type Result = { kind?: "answer" | "work" | "unknown"; acceptanceAvailable?: boolean; sourceVersion?: string; evidence?: {reference: string; digest: string}[]; warnings?: string[]; version: number; digest: string; checks: Check[]; summary: string; at?: number | null; automaticReady?: boolean; mock?: true };
/** `failed` and `cancelled` are runtime ends. Neither is Done: Done is an owner decision. */
export type TaskStatus = "draft" | "working" | "needs_you" | "stopping" | "paused" | "review" | "ready_to_merge" | "done" | "failed" | "cancelled";
/** Exactly what the owner accepted. `open` keeps the checks that had not passed, so an override can never read as a clean pass. */
export type Acceptance = { version: number; digest: string; at?: number | null; open?: { failed: number; not_run: number; unverified: number } | null };
export type Integration = { method: "squash" | "pr" | "commit"; targetBranch: string; digest: string; mock?: true; message?: string; title?: string; draft?: boolean };
/** One side of a conflict block as the three-way view shows it. `hit` lists the lines that differ. */
export type CodeSide = { ref: string; start: number; lines: string[]; hit: number[] };
export type BlockChoice = "ours" | "theirs" | "both" | "cortex";
export type ConflictBlock = {
  id: string; title: string; base: CodeSide; ours: CodeSide; theirs: CodeSide;
  results: Record<BlockChoice, Omit<CodeSide, "ref">>; cortexNote: string; choice: BlockChoice | null;
};
export type ConflictFile = { path: string; blocks: ConflictBlock[] };
export type MergeSeed = { files: ConflictFile[]; clean: { path: string; status: GitStatus }[]; checks: Check[] };
/** A merge of the target into the task branch. It keeps what came before, so Abort merge restores it exactly. */
export type MergeState = MergeSeed & {
  source: string; into: string; startedAt: number | null; handedAt: number | null; finishedAt: number | null;
  guidance: string | null; step: number; failure: string | null; finished: boolean;
  before: { status: TaskStatus; acceptance: Acceptance | null; result: Result | null; digest: string | null }; mock?: true;
};
export type Task = {
  id: string; projectId: string; sprintId: string | null; description: string;
  sourceVersion?: string;
  title: string; titleState: "pending" | "generated" | "user" | "failed";
  checkout: Checkout; targetBranch: string | null; dependencies: string[];
  status: TaskStatus; resumeStatus: "working" | "needs_you"; issue: string | null;
  executionId: string | null; result: Result | null; workspaceDigest: string | null;
  acceptance: Acceptance | null;
  reviewActions?: { accept: boolean; reason: string | null; unblockAction: "wait" | "request_changes" | "rerun_checks" | "refresh" | null };
  conflict: "none" | "conflict" | "resolving" | "failed";
  merge: MergeState | null; resolvedBy: "cortex" | "you" | null;
  integration: Integration | null;
  integrationFacts?: { targetBranch: string; alreadyInTarget: boolean; sameBranch?: boolean; commitAvailable?: boolean; pendingPaths?: string[]; otherUncommitted?: boolean; reason?: string | null; pushStatus: "pushed" | "not_pushed" | "unknown" | null };
  closedAs: "as_is" | "cancel" | "external" | null; notVerifiedByCortex: boolean;
  doneAt: number | null; archive: { at: number; previousStatus: "done"; sizeMb?: number | null } | null;
  archiveQuestion: boolean; archiveAskAfter: number | null;
  /** What unmerged work there is, when the worktree was read. Absent facts stay absent. */
  workFacts?: { commits: number; files: number; sizeMb: number } | null;
  work: "unmerged" | "merged" | "discard_confirmed"; process: "live" | "stopped" | "unknown";
  unexpectedChanges: boolean; purgeBlocker: string | null; settings: TaskSettings; mock?: true;
  /** When the machine last confirmed this record. Absent on a record the machine has not reported. */
  lastConfirmedAt?: number | null;
  observedPhase?: string; warnings?: string[];
  /** Confirmed runtime route, separate from editable task settings. */
  coordinatorRoute?: { model: string; effort: string } | null;
};
/** The route and access of one task. Machine defaults fill a new task; changing them here never touches other tasks. */
export type TaskSettings = { model: string; effort: string; mode: "development" | "guarded" | "auto"; language: "auto" | "en" | "ru" };
export type PlanItem = { key: string; description: string; dependsOn: string[]; checkout?: CheckoutChoice; title?: string; acceptance?: string };
export type PlanVersion = { version: number; items: PlanItem[] };
/** One line of the planning conversation. `event` lines record what the owner did, not what anyone said. */
export type SprintEntry = { attachments?: AttachmentInput[]; kind: "question" | "answer" | "note" | "message" | "event"; text: string; options?: string[]; at?: number | null; mock?: true };
export type Sprint = {
  id: string; projectId: string; description: string; attachments?: AttachmentInput[]; title?: string | null; runMode: "manual" | "automatic";
  /** Chosen once for all new sprint tasks. Absent on older sprints. */
  checkout?: { kind: CheckoutChoice["kind"] };
  archivedAt?: number;
  planningFailure?: string; planningTaskId?: string; targetBranch: string | null; planning: "planning" | "question" | "ready" | "created" | "cancelled";
  conversation: SprintEntry[]; settings?: { model: string; effort: string; mode?: TaskSettings["mode"]; language?: TaskSettings["language"] };
  plan: PlanVersion | null; history?: PlanVersion[]; materializedVersion: number | null;
  taskIds: string[]; queueState: "idle" | "running" | "paused" | "complete"; pauseReason: string | null; mock?: true;
};
export type WorkerStatus = "working" | "waiting_you" | "pending" | "not_started" | "paused" | "completed" | "failed" | "cancelled" | "unknown";
export type ActivityKind = "note" | "assignment" | "divider" | "tools" | "tool" | "edit" | "command" | "message";
export type CommandState = "running" | "stopping" | "stopped" | "passed" | "failed" | "native";
/** One entry of a worker's history. Unknown details stay absent, never invented. */
export type Activity = {
  sequence: number; text: string; error?: boolean; kind: ActivityKind; at: number | null; mock?: true;
  code?: string; note?: string; toolCallRef?: string;
  rows?: { icon: "file" | "search"; code: string; note: string }[];
  /** The call's input and output as the machine recorded them, shown when the owner opens the step. */
  args?: string;
  output?: string;
  diff?: { add: number; del: number; lines: string[] };
  command?: { state: CommandState; exit: number | null; took: string | null; tail: string; reason: string | null };
  messageId?: string;
};
export type WorkerResult = {
  summary: string; checks: Check[]; reportId?: string; mock?: true;
  edits?: { path: string; add: number; del: number }[];
  attempts?: { n: number; ok: boolean; detail: string; at: number | null }[];
};
export type Worker = {
  id: string; taskId: string; executionId: string; role: string; status: WorkerStatus;
  lane: "execution" | "review"; goal: string;
  /** What it is doing now, one line. Null when nothing has been reported. */
  now: string | null; waitsFor: string | null; scope: string[];
  route: { model: string; effort: string } | null; attempt: { n: number; of: number } | null;
  /** Each engine fallback within this work attempt, oldest first; unconfirmed routes stay null. */
  routeAttempts?: { n: number; route: { model: string; effort: string } | null }[];
  elapsedMs: number | null; runningSince: number | null;
  context: { used: number; window: number } | null; usage: number | null;
  activity: Activity[]; result: WorkerResult | null; messages?: Message[]; activityPartial?: boolean; activityCursor?: string | null; mock?: true;
};
export type Delivery = "queued" | "sending" | "recorded" | "delivered" | "failed" | "unverified" | "withdrawn";
export type Attachment = { name: string; mimeType: string; data: string };
/** Name-only fixtures belong to demo mode; real submissions require bytes. */
export type AttachmentInput = Attachment | string;
export type Message = {
  id: string; taskId: string; workerId: string | null; text: string;
  mode: "send_now" | "queue"; delivery: Delivery;
  at: number | null; deliveredAt: number | null; failure: string | null;
  /** File names only. Nothing here reads a file. */
  attachments?: AttachmentInput[]; mock?: true;
};
export type PlanNode = { id: string; title: string; who: string; workerId: string | null };
export type Decision = {
  id: string; kind: "question" | "permission"; from: string; workerId: string | null; at: number | null;
  text: string; unavailable?: string; options: { label: string; sub?: string; optionId?: string }[];
  effect?: string; scope?: string; ifDenied?: string;
  answer: { text: string; at: number | null } | null; withdrawn: boolean; suspended?: boolean;
  /** The answer waits in the app for the rest of its batch: it has not reached the machine. */
  held?: true;
  /** Questions asked together share this. The owner reads them as tabs of one card. */
  batchId?: string | null;
};
export type TimelineEntry =
  | { id: string; kind: "cortex"; text: string }
  /** What the owner said, as the runtime recorded it. */
  | { id: string; kind: "said"; text: string }
  | { id: string; kind: "fold"; text: string; steps: string[] }
  | { id: string; kind: "plan" }
  | { id: string; kind: "workers"; title: string; workerIds: string[] }
  | { id: string; kind: "message"; messageId: string }
  | { id: string; kind: "decision"; decisionId: string }
  | { id: string; kind: "line"; icon: string; text: string; at: number | null; rule: boolean };
export type NewEntry = TimelineEntry extends infer E ? E extends TimelineEntry ? Omit<E, "id"> : never : never;
/** The conversation and facts of one task. Tasks with no recorded session start from an empty one. */
export type TaskSession = {
  taskId: string; createdAt: number | null; base: { branch: string; commit: string } | null;
  plan: { revision: number; note: string; nodes: PlanNode[] } | null;
  timeline: TimelineEntry[]; decisions: Decision[];
  context: { used: number; window: number } | null; tokens: number | null;
  planUsage: { percent: number; observedAt: number } | null;
  coordinator: { note: string; elapsedMs: number | null; usage: number | null };
  verification: string[]; changes: { files: number; add: number; del: number } | null; mock?: true;
};
export type GitStatus = "M" | "A" | "D" | "U";
export type DiffLine = { kind: "ctx" | "add" | "del"; old: number | null; new: number | null; text: string };
export type DiffHunk = { header: string; lines: DiffLine[]; staged: boolean };
/** `add` and `del` count the whole file. A record may keep only some hunks, and the screen says when it does. */
export type FileDiff = { path: string; status: GitStatus; add: number; del: number; hunks: DiffHunk[]; filter?: string; changedBy?: string };
export type GitFile = FileDiff & { stage: "staged" | "unstaged" | "untracked" };
export type GraphMark = { paths: { cls: "g1" | "g2" | "g3"; d: string }[]; dot: { cls: "d1" | "d2" | "d3"; x: number } };
export type Commit = { sha: string; message: string; author: string; when: string; branch: string; taskId: string | null; tags: string[]; files: number; add: number; del: number; pushed: boolean; graph: GraphMark };
export type Branch = { name: string; where: string; task: string | null; ahead: number; behind: number };
/** The main checkout or one worktree. A worktree with no task is orphaned. Counts are against `base`. */
export type RepoCheckout = {
  id: string; projectId: string; kind: "main" | "worktree"; branch: string; path: string; taskId: string | null; label: string | null;
  base: string; upstream?: string; ahead: number; behind: number; behindOf: Record<string, number>; files: GitFile[]; draft: string;
  sources: Record<string, string>; updateConflict: MergeSeed | null;
};
/** The Git facts of one project. `remote` is the credential-redacted display URL, not a Git remote name. */
export type ProjectRepo = { projectId: string; hasGit?: boolean; remote: string; tree: string[]; sources: Record<string, string>; commits: Commit[]; branches: Branch[]; checkouts: RepoCheckout[]; fetchedAt: number | null; mock?: true };
export type ChangeScope = { id: string; label: string; sub?: string; paths: string[] };
export type ReviewComment = { attachments?: AttachmentInput[]; id: string; path: string; line: number; text: string; sent: boolean; mock?: true };
/** What one task changed, as the Changes tab reviews it. */
export type TaskChanges = { taskId: string; sha: string; unavailableReason?: string; files: FileDiff[]; scopes: ChangeScope[]; comments: ReviewComment[]; mock?: true };
export type EngineUsageInput = {
  engine: string; connected: boolean; usedPercent: number | null;
  resetAt: string | null; observedAt: string | null;
};
export type EngineWeeklyUsage = Omit<EngineUsageInput, "engine"> & { engine: "codex" | "claude"; window: "weekly"; mock?: true };
/** What the running session offers. A list stays empty until it is reported, and an empty list turns its control off. */
export type SessionChoice = { id: string; label: string; sub?: string; unavailable?: string };
export type SessionOptions = { models: (SessionChoice & { engine: string; efforts?: string[] })[]; efforts: SessionChoice[]; modes: SessionChoice[]; languages: SessionChoice[] };
export type WorkspaceState = {
  mock?: true; options: SessionOptions; machines: Machine[]; projects: Project[]; sprints: Sprint[];
  tasks: Task[]; workers: Worker[]; messages: Message[]; sessions: TaskSession[]; engineUsage: EngineWeeklyUsage[];
  events: { id: string; taskId: string; projectId: string; sequence: number; text: string; occurredAt?: string; mock?: true }[];
  purged: { taskId: string; at: number; mock?: true }[];
  repos: ProjectRepo[]; reviews: TaskChanges[];
};
export type NewTask = { projectId: string; description: string; attachments?: AttachmentInput[]; checkout: CheckoutChoice; sprintId?: string; settings?: TaskSettings };
export type WorkspaceCommand =
  | ({ type: "add_project"; createFolder?: boolean; newFolder?: boolean } & Omit<Project, "mock">)
  | { type: "activate_project"; projectId: string }
  | { type: "rename_project"; projectId: string; name: string }
  | { type: "remove_project"; projectId: string; confirmed: boolean }
  | { type: "open_editor"; projectId: string; checkoutId?: string; file?: string; line?: number }
  | { type: "pair_machine"; id: string; name: string; code: string; fingerprintsMatch: boolean; now?: number }
  | { type: "rename_machine"; machineId: string; name: string }
  | { type: "remove_machine"; machineId: string; confirmed: boolean }
  | { type: "set_retention"; machineId: string; days: number }
  | { type: "save_storage"; machineId: string; retentionDays: number; autoArchiveDays: number }
  | { type: "set_worktree_root"; machineId: string; path: string }
  | { type: "save_surface"; machineId: string; projectId?: string | null; surface: SurfaceKey; value: unknown; revision: number }
  | { type: "activate_profile"; machineId: string; profileId: string; keep: "save" | "discard" }
  | { type: "save_profile"; machineId: string }
  | { type: "revert_profile"; machineId: string }
  | { type: "deactivate_profile"; machineId: string }
  | { type: "create_profile"; machineId: string; name: string; description: string }
  | { type: "rename_profile"; machineId: string; profileId: string; name: string }
  | { type: "describe_profile"; machineId: string; profileId: string; description: string }
  | { type: "delete_profile"; machineId: string; profileId: string }
  | { type: "apply_preset"; machineId: string }
  | { type: "reset_preset"; machineId: string; confirmed: boolean }
  | { type: "migrate_policy"; machineId: string; confirmed: boolean }
  | { type: "record_mcp_tools"; machineId: string; server: string; observation: McpObservation | null }
  | { type: "rename_connection"; machineId: string; connectionId: string; name: string }
  | { type: "download_engine"; machineId: string; engine: EngineKey }
  | { type: "enable_opencode"; machineId: string; name: string }
  | { type: "check_connection"; machineId: string; connectionId: string; now: number }
  | { type: "refresh_models"; machineId: string; now: number }
  | { type: "add_connection"; machineId: string; label: string; engine: EngineId }
  | { type: "remove_connection"; machineId: string; connectionId: string }
  | { type: "set_sessions"; machineId: string; connectionId: string; sessions: number }
  | { type: "open_logs"; machineId: string }
  | { type: "run_diagnostics"; machineId: string; now: number }
  | { type: "set_remote_access"; machineId: string; allowed: boolean }
  | { type: "revoke_device"; machineId: string; deviceId: string | null; confirmed: boolean }
  | ({ type: "create_task" } & NewTask)
  | { type: "rename_task"; taskId: string; title: string }
  | { type: "edit_task"; taskId: string; expectedVersion: string; title: string; description: string; attachments: AttachmentInput[]; settings: TaskSettings; checkout: { kind: CheckoutChoice["kind"] } }
  | { type: "generate_title"; taskId: string; fail?: boolean }
  | { type: "start_task" | "continue_task" | "confirm_stop" | "clear_issue" | "restore_task" | "flush_queue" | "stop_resolution" | "abort_merge" | "finish_merge" | "update_from_target"; taskId: string; now?: number }
  | { type: "retry_delivery"; messageId: string }
  | { type: "resolve_with_cortex"; taskId: string; guidance?: string; attachments?: AttachmentInput[]; now?: number }
  | { type: "resolve_block"; taskId: string; path: string; blockId: string; choice: Exclude<BlockChoice, "cortex"> }
  | { type: "stop_task"; taskId: string; confirmed: boolean; now?: number }
  | { type: "set_task_settings"; taskId: string; settings: Partial<TaskSettings> }
  | { type: "close_task"; taskId: string; choice: "as_is" | "cancel" | "external" | "already_in_target"; confirmed: boolean; now: number }
  | { type: "rerun_check"; taskId: string; digest: string; checkIndex: number }
  | { type: "accept_result"; taskId: string; digest: string; openChecks?: boolean; now?: number }
  | { type: "integrate"; taskId: string; method: "squash" | "pr" | "commit"; confirmed: boolean; digest: string; targetBranch: string; now: number; message?: string; title?: string; draft?: boolean; deleteWorktree?: boolean }
  | { type: "request_changes"; taskId: string; text: string; attachments?: AttachmentInput[]; now?: number }
  | { type: "add_comment"; taskId: string; path: string; line: number; text: string; attachments?: AttachmentInput[] }
  | { type: "edit_comment"; taskId: string; commentId: string; text: string; attachments?: AttachmentInput[] }
  | { type: "remove_comment"; taskId: string; commentId: string }
  | { type: "send_comments"; taskId: string; now?: number }
  | { type: "git_stage"; checkoutId: string; path: string; staged: boolean; hunk?: number }
  | { type: "git_revert"; checkoutId: string; path: string; hunk?: number; confirmed: boolean; now?: number }
  | { type: "git_draft"; checkoutId: string; message: string }
  | { type: "git_commit"; checkoutId: string; message: string; amend?: boolean; confirmed?: boolean; now?: number }
  | { type: "git_fetch"; projectId: string; now: number }
  | { type: "git_init"; projectId: string; confirmed: boolean }
  | { type: "git_pull"; checkoutId: string; now?: number }
  | { type: "git_push"; checkoutId: string; remote: string; branch: string; confirmed: boolean }
  | { type: "create_branch"; projectId: string; name: string; at?: string }
  | { type: "revert_commit"; checkoutId: string; sha: string; confirmed: boolean; now?: number }
  | { type: "delete_worktree"; checkoutId: string; confirmed: boolean; discard: boolean; now?: number }
  | { type: "attach_worktree"; checkoutId: string }
  | { type: "create_sprint"; projectId: string; description: string; attachments?: AttachmentInput[]; runMode: Sprint["runMode"]; checkout?: Sprint["checkout"]; settings?: TaskSettings; targetBranch?: string | null }
  | { type: "answer_planning"; sprintId: string; answer: string; attachments?: AttachmentInput[]; now?: number; decisionId?: string; optionId?: string | null }
  | { type: "plan_message"; sprintId: string; text: string; attachments?: AttachmentInput[]; now?: number }
  | { type: "edit_plan"; sprintId: string; items: PlanItem[]; now?: number }
  | { type: "materialize_plan"; sprintId: string; version: number; now?: number }
  | { type: "cancel_planning" | "resume_planning"; sprintId: string }
  | { type: "rename_sprint"; sprintId: string; title: string }
  | { type: "close_sprint"; sprintId: string; confirmed: boolean }
  | { type: "set_sprint_settings"; sprintId: string; settings: Partial<TaskSettings> }
  | { type: "run_sprint" | "pause_sprint"; sprintId: string; now?: number }
  | { type: "set_run_mode"; sprintId: string; runMode: Sprint["runMode"] }
  | ({ type: "set_target"; branch: string | null } & ({ taskId: string; sprintId?: never } | { sprintId: string; taskId?: never }))
  | { type: "archive_task"; taskId: string; now: number }
  | { type: "answer_archive"; taskId: string; discard: boolean; now: number }
  | { type: "purge_task"; taskId: string; confirmed: boolean; now: number }
  | { type: "purge_many"; taskIds: string[]; confirmed: boolean; now: number }
  | { type: "run_cleanup"; machineId: string; now: number }
  | { type: "clear_purge_block"; taskId: string; confirmed: boolean }
  | { type: "clear_old"; taskIds?: string[]; machineId: string; cutoff: number; force: boolean; confirmed: boolean; now: number }
  | { type: "send_message"; taskId: string; workerId?: string; text: string; mode: Message["mode"]; resume?: boolean; now?: number; attachments?: AttachmentInput[] }
  | { type: "edit_queued"; messageId: string; text: string; attachments?: AttachmentInput[] }
  | { type: "remove_queued"; messageId: string }
  | { type: "move_queued"; messageId: string; before: string | null }
  | { type: "send_queued"; messageId: string; resume?: boolean; now?: number }
  | { type: "withdraw_message"; messageId: string }
  | { type: "redirect_message"; messageId: string; now?: number }
  | { type: "answer_decision"; taskId: string; decisionId: string; answer: string; attachments?: AttachmentInput[]; optionId?: string | null; now?: number }
  | { type: "stop_worker"; workerId: string; reason?: string; now?: number }
  | { type: "stop_command"; workerId: string; sequence: number; reason: string }
