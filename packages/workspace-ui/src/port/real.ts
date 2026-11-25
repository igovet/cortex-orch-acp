import { wireAttachments as checkedAttachments } from "../attachments";
import { capabilities as localCapabilities, decodeResponse, negotiate, PROTOCOL, validate } from "@cortex/workspace-contract";
import type { Answer as WireAnswer, Decision as WireDecision, Receipt as WireReceipt, Result as WireResult, Task as WireTask, HistoryRow, WireTypes } from "@cortex/workspace-contract";
import { ACCESS_MODES, readRemoteStatus, readSudoPrompts, capabilityOf, checksPassed, DomainError, effortLabel, ENGINE_KEYS, engineLabel, FEATURES, LANGUAGES, LOCAL_MACHINE_ID, MACHINE_COMMANDS, mapProjectSlice, orderedEfforts, parseLiveSprintGraph, readableCheckReason, routeIdOf, emptySession, overlayTask, validatePlan, WIRE_OPERATIONS } from "@cortex/workspace-domain";
import { nativeDelta, nativeVersion } from "./native";
import { readOwnerActionReply, readOwnerActions, type OwnerActionRequests } from "@cortex/workspace-domain";
import { applyNativeTaskDelta, weeklyUsage } from "@cortex/workspace-domain";
import type { NativeTaskDelta, NativeTaskSnapshot } from "@cortex/workspace-domain";
import { conversationOf, shownFold } from "./history";
import { FIRST_VIEW_MS, firstWindowEntries, type SessionReads } from "../first-view";
import { CODE_LANGUAGE, CodeRefusal, type CodeEvent, type CodeFile, type LocalCodePort } from "../code-support";
import { same } from "../stable";
import { plainReason } from "../status-text";
import { classifiedPlanningFailure } from "../selectors";
import { readBoards, writeBoards, type BoardStorage, type CachedBoard } from "./board-cache";
import { liveMembers, presentSprint, unreadMembers, type SavedSprint } from "./sprint-members";
import type { Attachment, AttachmentRef, CatalogueState, CommandEnvelope, CommandOutput, CommandReceipt, DirectorySearch, GitInterest, EngineConnection, EngineWeeklyUsage, FileDiff, FileRead, FolderListing, IntegrationText, Machine, MachineConfig, MachineFacts, Message, PlanItem, PortCapabilities, PortUpdate, ProjectFacts, ProjectionUpdate, ProjectRepo, ProjectSkills, ProjectSlice, QueryOperation, QueryParameters, QueryResults, SessionOptions, Sprint, TaskChanges, TaskOverlay, Worker, WorkspaceCommand, WorkspacePort } from "@cortex/workspace-domain";

/** Whether two facts reads say the same thing: their place in the read order is not part of what they say. */
const sameFacts = (a: ProjectFacts | undefined, b: ProjectFacts) => !!a && same({ ...a, readOrder: undefined }, { ...b, readOrder: undefined });

/** How the native bridge reaches the machine service. The bridge owns the socket; this side only ever sees typed messages. */
export type TransportEvent = CodeEvent | { type: "connection"; state: "connecting" | "connected" | "disconnected" | "unavailable"; reason?: string } | { type: "invalidated"; projectId: string; source?: "runtime" } | { type: "refresh" } | ({ type: "native_delta" } & NativeTaskDelta) | { type: "native_resync"; projectId: string };
export type RealTransport = {
  request(operation: string, message: unknown): Promise<unknown>;
  subscribe(listener: (event: TransportEvent) => void): () => void;
};
/** One project the service has registered. Listing them is the service's; here they arrive already resolved. */
export type ProjectLocator = { projectId: string; name: string; repository: string; openBranch: string; defaultBranch: string; activated?: boolean };
export type RealPortOptions = { transport: RealTransport; actorId: string; projects?: ProjectLocator[]; registry?: boolean; boardStorage?: BoardStorage; localCode?: boolean;
  /** Least time between the end of one streamed facts read of a project and the start of the next (default `STREAM_FACTS_MS`). */
  streamFactsMs?: number;
  /** How many conversation entries make about one screen, for the first window of a conversation (default from the window's
   *  height, `firstWindowEntries`). */
  firstWindow?: () => number };
/** What this port adds to a facts read: the tasks whose worker rows it was read for. A screen showing one of them knows its
 *  workers, results and messages are in once a read naming it has arrived; until then an empty list is not "none". */
export type ScopedFacts = ProjectFacts & { workerScope?: string[];
  /** The execution each task of `workerScope` had when this read began: its workers are those of that execution. */
  workerExecutions?: Record<string, string | null> };
/** What the real port offers beyond the shared port contract, for a screen's first reveal (`first-view.ts`). */
export type FirstViewPort = {
  /** Whether facts reads carry `workerScope`, so a screen can wait for its task's workers. */
  scopesFacts(): boolean;
  /** Reads a task's conversation and questions again for its screen, after a first read did not finish. */
  retryFirstView(taskId: string): void;
};
/** While a task streams updates, the facts of a project on screen are reread at most this often. */
export const STREAM_FACTS_MS = 5_000;

/** For a build with no bridge yet. It fails every call, so the screen says so instead of showing anything. */
export function unavailableTransport(): RealTransport {
  return { request: () => Promise.reject(new Error("The machine service is not connected")), subscribe: () => () => {} };
}

/**
 * Every `executions.control` action and `machine.call` operation this port sends. `control` and `machine` below take only
 * these, so a new one is added here on purpose; the desktop bridge policy test (`every_request_the_real_port_sends_is_allowed`
 * in apps/cortex-workspace-desktop/src-tauri/tests/policy.rs) reads both lists and checks that the bridge accepts each one
 * with a representative message, so a request the bridge would refuse fails a test instead of the owner's click.
 */
export const CONTROL_ACTIONS = ["pause", "resume", "retry", "worker_message", "worker_stop", "allow_write", "command_stop", "rerun_check", "task_settings", "close_task", "handoff_merge", "inspect_merge", "request_merge_checks"] as const;
export const MACHINE_OPS = ["overview", "logs.folder", "maintenance", "workflow.describe", "engine.connection", "machine.update", "diagnostics.run", "cleanup.run", "config.read", "config.save", "config.action", "config.mcp.discover",
  "project.config.read", "project.config.save", "repo.snapshot", "repo.file", "repo.file.save", "repo.list", "repo.skills", "git", "workers", "command.output",
  "repo.file.lease.acquire", "repo.file.lease.renew", "repo.file.lease.release",
  "repo.lsp.open", "repo.lsp.change", "repo.lsp.close", "repo.lsp.status", "repo.lsp.definition",
  "product.read", "product.board", "product.facts", "product.task", "product.integrate", "product.sprint", "product.message", "product.comment", "product.attachment"] as const;
type ControlAction = typeof CONTROL_ACTIONS[number];
type MachineOp = typeof MACHINE_OPS[number];

const PAGE_LIMIT = 200;
/** How long one conversation page may take, the first retry delay after a failed conversation read, and the pause between
 *  rereads of a conversation that keeps changing. */
const HISTORY_PAGE_MS = 60_000, HISTORY_RETRY_MS = 3_000, HISTORY_AGAIN_MS = 120;
/** A conversation opens on its latest part, about one screen of it: read from its end backwards, in pages of one screen's
 *  worth of rows (`RealPortOptions.firstWindow`, at most `HISTORY_PAGE_ROWS`), until it holds the entries that fill one
 *  screen or `HISTORY_FIRST_ROWS` rows (so never more than `ceil(HISTORY_FIRST_ROWS / page size)` pages). The rest is read
 *  as the owner scrolls up. Earlier reads check for visible progress after each page.
 *  The service binds every history cursor to the page size it was read with and refuses it with another, so that one page
 *  size is kept on the held conversation and every later read from its cursors, backwards or forwards, asks for exactly it. */
const HISTORY_FIRST_ROWS = 150;
/** The largest page a history read may ask for, and the page size of a conversation read forward from its beginning. */
const HISTORY_PAGE_ROWS = 100;
/** Use the same conversation and decision projection for display and for measuring prepend progress. */
function projectedConversation(rows: HistoryRow[], description: string, decisions: WireDecision[], partial: boolean) {
  const first = partial ? Date.parse(rows.find(r => typeof r.recordedAt === "string")?.recordedAt ?? "") : NaN;
  const since = Number.isFinite(first) ? first : null;
  // Settled questions before the held rows belong to the missing history. An open question always shows, and an
  // answer from before the held rows must not hide a later owner message of the same words.
  const before = (at: unknown) => since !== null && typeof at === "string" && Date.parse(at) < since;
  const dated = [
    ...conversationOf(rows, { description, partial, answers: decisions.flatMap(d => typeof d.answerText === "string" && !before(d.answeredAt ?? d.createdAt) ? [d.answerText.trim()] : []) }),
    ...decisions.filter(d => d.status === "pending" || !before(d.createdAt)).map(d => ({at:typeof d.createdAt === "string" ? d.createdAt : "",entry:{id:`decision:${d.decisionId}`,kind:"decision" as const,decisionId:d.decisionId}})),
  ];
  return { dated, since };
}

/** New visible text, a settled decision, or a visible tool turn. A split call can change a fold's ID without changing
 *  its headline; count like headlines with multiplicity so that alone cannot end a read. */
function addsVisibleHistory(held: HistoryRow[], candidate: HistoryRow[], description: string, decisions: WireDecision[], showAllTools: boolean) {
  const keys = (rows: HistoryRow[]) => projectedConversation(rows, description, decisions, true).dated.flatMap(({entry}) => {
    if (entry.kind === "fold") {
      const title = shownFold(entry, showAllTools).text;
      return title ? [`fold:${title}`] : [];
    }
    // Pending decisions already show independently of their timeline position.
    if (entry.kind === "decision" && decisions.some(d => d.decisionId === entry.decisionId && d.status === "pending")) return [];
    return ["text" in entry ? `${entry.id}:${entry.text}` : entry.id];
  });
  const seen = new Map<string, number>();
  for (const key of keys(held)) seen.set(key, (seen.get(key) ?? 0) + 1);
  return keys(candidate).some(key => {
    const count = seen.get(key) ?? 0;
    if (!count) return true;
    seen.set(key, count - 1);
    return false;
  });
}

const request = (projectId: string, operation: string, extra: Record<string, unknown> = {}) => validate("Query", { protocol: { ...PROTOCOL }, requestId: `read-${operation}-${projectId}`, projectId, operation, ...extra });
const OUTCOME_WORDS: Record<string, string> = {
  recorded: "Recorded", already_resolved: "Already recorded", stale_version: "This changed on the machine. Look again and retry.", stale_epoch: "Another owner took over this task.",
  superseded: "This was replaced by a newer request.", expired: "This has expired.", unauthorized: "This machine refused the action.", unsupported: "This machine does not support that yet.",
  busy: "Another task is still running or waiting to stop. Wait for it to finish before starting this task.", invalid_request: "The machine could not read that request.", payload_conflict: "This request was already sent with different content.",
  unknown_outcome: "The result of this action is not known. It was not sent again.", unknown: "The machine gave a result this app does not know.",
};
const RECORDED = new Set(["recorded", "already_resolved"]);
/** The bridge's mark for the machine's own answer. Anything unmarked may be a reply that never came, so its outcome stays unknown. */
const REFUSED = "refused:";
const FOLDER_MISSING = "project_folder_missing";
/** Why a project has no working Cortex, as the service reports it. */
const PROJECT_ISSUES: Record<string, string> = {
  project_folder_missing: "This project's folder is not there. Work comes back here when the folder returns.",
  project_folder_unreadable: "There is no permission to open this project's folder.",
  project_folder_not_directory: "This project's path is no longer a folder.",
  child_stdout_closed_or_oversized: "This project stopped unexpectedly. It is being started again.",
  child_exited: "This project stopped unexpectedly. It is being started again.",
  acp_version_mismatch: "This runtime does not match the app. Reinstall the package.",
  capability_mismatch: "This runtime does not match the app. Reinstall the package.",
  mapping_mismatch: "This runtime does not match the app. Reinstall the package.",
  trusted_activation_unavailable: "This project refused to activate.",
  acp_request_refused: "This project refused to start.",
  unknown_outcome: "This project did not answer in time. It is being started again.",
};
const PROTOCOL_REFUSALS = new Set(["version_mismatch", "service_protocol_mismatch", "bridge_invalid_capabilities", "unsupported_protocol"]);
const PROTOCOL_REFUSAL = "This app and the machine service are different versions. Reinstall the package so both are the same version.";
const UPDATE_NEEDED = "The computer and this app are different versions. Update the app or the computer so both match.";
const compatibilityRefusal = (text: string) => {
  const code = text.startsWith(REFUSED) ? text.slice(REFUSED.length) : text;
  if (PROTOCOL_REFUSALS.has(code) || code === PROTOCOL_REFUSAL) return PROTOCOL_REFUSAL;
  if (code === "remote_incompatible_version" || code === UPDATE_NEEDED) return UPDATE_NEEDED;
  return undefined;
};
/** Local and remote bridges carry a confirmed service refusal as a string. */
function storageRefusal(error: unknown) {
  const text = error instanceof Error ? error.message : typeof error === "string" ? error : "";
  const match = /^refused:project_store:([a-z][a-z0-9_]*): (.+)$/.exec(text);
  return match ? { reason: match[1], message: match[2] } : undefined;
}
const refusalOf = (requestId: string, error: unknown): CommandReceipt => {
  const storage = storageRefusal(error);
  if (storage) return { requestId, ok: false, outcome: storage.reason, message: storage.message };
  const text = error instanceof Error ? error.message : String(error);
  if (text === FOLDER_MISSING) return { requestId, ok: false, outcome: FOLDER_MISSING, message: "This folder does not exist yet." };
  if (text === "service_request_failed" || text === "bridge_request_refused") return { requestId, ok: false, outcome: "refused", message: "The machine refused this request. Nothing changed." };
  const compatibility = compatibilityRefusal(text);
  if (compatibility) return { requestId, ok: false, outcome: "refused", message: compatibility };
  if (text.startsWith(REFUSED)) return { requestId, ok: false, outcome: "refused", message: text.slice(REFUSED.length) };
  return { requestId, ok: false, outcome: "unknown_outcome", message: OUTCOME_WORDS.unknown_outcome };
};
const unsupported = (requestId: string): CommandReceipt => ({ requestId, ok: false, outcome: "unsupported", message: OUTCOME_WORDS.unsupported });

/**
 * Nothing is read on a blind timer for what is not on screen. A project's facts are read when a screen showing them opens
 * (`interest`), when its events say it changed, and when the window comes back after `FOCUS_STALE_MS`. Its Git data is read
 * only while a screen showing it is on view: on open, after the owner's own Git action, on the manual refresh, on return to
 * the window after `FOCUS_STALE_MS`, and every `GIT_VISIBLE_REFRESH_MS` while that screen is on view and the window focused,
 * because files can change outside the app without any event. A Git status line (`status`, the title band's Git button) is read
 * the same way but never on that timer. `MACHINE_POLL_MS` is the plain machine settings poll.
 */
export const GIT_VISIBLE_REFRESH_MS = 30_000, FOCUS_STALE_MS = 10_000, MACHINE_POLL_MS = 300_000;
/** First retry delay of a failed repository read; doubles per failure up to `REPO_RETRY_MAX_MS`, while a Git screen is on view. */
export const REPO_RETRY_MS = 3_000, REPO_RETRY_MAX_MS = 30_000;
/**
 * The service reads each connection's engine limits at most every five minutes, and only when a settings read asks for it
 * (`usage_refresh.rs`): nothing reads them while no window does. A machine read lands `USAGE_GRACE_MS` after the next
 * snapshot is due and starts that refresh; the read `USAGE_RETRY_MS` later shows it, so what the sidebar shows is never
 * older than about `USAGE_REFRESH_MS + USAGE_GRACE_MS + USAGE_RETRY_MS` while the window is visible. Two unaligned five-minute timers in a row had let it reach twenty minutes.
 */
export const USAGE_REFRESH_MS = 300_000, USAGE_GRACE_MS = 45_000, USAGE_RETRY_MS = 30_000, USAGE_LATE_MS = 120_000;
/**
 * How long until the next machine read, given when each engine's shown limits were observed (ms since the epoch).
 * A late snapshot is asked for again every `USAGE_RETRY_MS` for `USAGE_LATE_MS`; after that the engine is left to the plain
 * `MACHINE_POLL_MS` poll, so one that cannot report never makes reads more frequent. With limits shown, the machine is read
 * about once per snapshot (every five minutes and a little), instead of on the plain poll as well. `catchUp` (the first read, or the first
 * after the window was hidden, when the service's loop starts again) asks once more soon for a snapshot already overdue.
 */
export function machineReadDelay(observed: readonly number[], now: number, catchUp: boolean): number {
  const next = observed.filter(Number.isFinite).map(at => at + USAGE_REFRESH_MS + USAGE_GRACE_MS);
  if (catchUp && next.some(due => due <= now)) return USAGE_RETRY_MS;
  const due = next.filter(at => at > now - USAGE_LATE_MS);
  if (!due.length) return MACHINE_POLL_MS;
  // Never later than one snapshot and its grace, even for a time ahead of this clock.
  return Math.min(USAGE_REFRESH_MS + USAGE_GRACE_MS, Math.max(USAGE_RETRY_MS, Math.min(...due) - now));
}
/** When each engine's newest shown limit was observed, in ms since the epoch. An engine with no time is left out. */
export function usageObserved(rows: readonly EngineWeeklyUsage[]): number[] {
  return rows.flatMap(row => {
    const times = [row.observedAt, ...(row.windows ?? []).map(w => w.observedAt)].flatMap(at => { const t = at ? Date.parse(at) : NaN; return Number.isFinite(t) ? [t] : []; });
    return times.length ? [Math.max(...times)] : [];
  });
}
type Product = { hasGit?: boolean; tasks: Record<string, TaskOverlay>; sprints: SavedSprint[]; drafts?: Record<string, string>; messages?: Message[]; comments?: Record<string, TaskChanges["comments"]> };
type MachineRead = Partial<Machine> & { name: string };
type ConfigRead = { config: MachineConfig; connections: EngineConnection[]; tokens: Record<string, unknown>; usage?: EngineWeeklyUsage[]; catalogueRefresh?: CatalogueState };
class BoardPending extends Error {}
/** Newer stream events made a background details read obsolete. That is neither success nor failure. */
class DetailsSuperseded extends Error {}
class Refusal extends Error { constructor(readonly code: string, message: string) { super(message); } }
/** A read's own error, on one bounded line: the code the service gave and its message, never a payload. */
function readCause(error: unknown) {
  const code = (error as { code?: unknown })?.code, message = error instanceof Error ? error.message : String(error);
  const line = `${typeof code === "string" && code && !message.startsWith(code) ? code + ": " : ""}${message}`.replace(/\s+/g, " ").trim();
  return line.length > 300 ? line.slice(0, 299) + "…" : line;
}
/** A copy for the schema check only: each file reference shown as an empty inline body of the same name and type. */
function referencesAsBodies(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(referencesAsBodies);
  if (!value || typeof value !== "object") return value;
  return Object.fromEntries(Object.entries(value).map(([key, v]) => [key, key === "attachments" && Array.isArray(v)
    ? v.map(f => f && typeof f === "object" && "sha256" in f ? { name: (f as { name: unknown }).name, mimeType: (f as { mimeType: unknown }).mimeType, data: "" } : f)
    : referencesAsBodies(v)]));
}
function wireAttachments(files: Message["attachments"] = []) {
  try { return checkedAttachments(files); }
  catch (error) { throw new Refusal("invalid_attachment", error instanceof Error ? error.message : "Choose the files again."); }
}
/** The plan a planning run ended on, or null when its answer holds none that can run. */
export function planFrom(text: string): PlanItem[] | null {
  const block = /```json\s*([\s\S]*?)```/.exec(text)?.[1];
  if (!block) return null;
  try {
    const tasks = (JSON.parse(block) as { tasks?: unknown }).tasks;
    if (!Array.isArray(tasks)) return null;
    const items: PlanItem[] = tasks.map((t: { key?: unknown; title?: unknown; description?: unknown; dependsOn?: unknown }) => ({ key: String(t.key), title: String(t.title ?? t.description), description: String(t.description ?? t.title), dependsOn: Array.isArray(t.dependsOn) ? t.dependsOn.map(String) : [] }));
    validatePlan(items);
    return items;
  } catch (error) { if (error instanceof SyntaxError || error instanceof DomainError) return null; throw error; }
}
/** The choices the machine's engines report, in the words the session uses. An engine that reported nothing offers nothing. */
export function sessionOptions(config: MachineConfig, rules = config.surfaces?.routing, allowedRoutes: string[] | null = null): SessionOptions {
  const reported = ENGINE_KEYS.flatMap(engine => {
    if (config.setup?.[engine]?.status !== "connected" || config.catalogue?.[engine]?.failed || !rules) return [];
    return (config.catalogue?.[engine]?.models ?? []).filter(m => rules.engines[engine].enabled.includes(m.id)
      && !rules.deny.some(token => token === m.id || token === m.routeId || token === `${engine}/${m.id}` || token === `${engine}/*`)
      && (allowedRoutes === null || allowedRoutes.includes(routeIdOf(engine, m.id)) || (m.routeId !== undefined && allowedRoutes.includes(m.routeId))))
      .map(m => ({ ...m, engine: engineLabel(engine) }));
  });
  const models = reported.map(({ id, routeId, label, engine, efforts }) => ({ id: routeId ?? id, label, engine, efforts: orderedEfforts(efforts) }));
  const efforts = orderedEfforts(reported.flatMap(m => m.efforts)).map(id => ({ id, label: effortLabel(id) }));
  return { models, efforts, modes: [...ACCESS_MODES], languages: [...LANGUAGES] };
}
/** The files a task in the project folder left there, once per path. */
function changesOf(repo: ProjectRepo | null, taskId?: string, overlay?: TaskOverlay): FileDiff[] {
  const binding = overlay?.checkout;
  const checkout = binding?.kind === "worktree"
    ? !binding.unavailable && repo?.checkouts.find(c => c.kind === "worktree" && (binding.id ? c.id === binding.id : c.taskId === taskId))
    : repo?.checkouts.find(c => c.kind === "main");
  const byPath = new Map<string, FileDiff>();
  for (const file of (checkout || null)?.files ?? []) {
    const seen = byPath.get(file.path);
    byPath.set(file.path, seen ? { ...seen, add: seen.add + file.add, del: seen.del + file.del, hunks: [...seen.hunks, ...file.hunks] } : { path: file.path, status: file.status, add: file.add, del: file.del, hunks: file.hunks });
  }
  return [...byPath.values()];
}

/**
 * The production port. It speaks only the public contract, validates every response, and holds the wire identities the
 * view models leave out. A command gets one attempt: a lost response is an unknown outcome and is never sent again.
 */
export function createRealPort(options: RealPortOptions): WorkspacePort & LocalCodePort & FirstViewPort {
  const { transport, actorId } = options, projects = options.projects ?? [];
  const observers = new Set<(update: PortUpdate) => void>();
  const cachedBoards = new Map(readBoards(options.boardStorage,actorId).map(b => [b.project.projectId,b]));
  /** Projects whose tasks the service has confirmed since launch, and the reads waiting for that. */
  const confirmedProjects = new Set<string>(), confirmations = new Set<() => void>();
  const recheckConfirmations = () => { for (const check of [...confirmations]) check(); };
  const tasks = new Map<string, { projectId: string; version: string; executionId?: string | null; runtimeState?: WireTask["runtimeState"] }>(), decisions = new Map<string, { projectId: string; wire: WireDecision }>();
  const nativeSnapshots = new Map<string, NativeTaskSnapshot>();
  const boardTasks = new Map<string, Set<string>>();
  const nativeWork = new Map<string, Promise<void>>();
  const nativeQueued = new Map<string, number>(), nativeEpoch = new Map<string, number>();
  const wireDecisions = new Map<string, WireDecision[]>(), wireResults = new Map<string, WireResult[]>();
  const slices = new Map<string, ProjectSlice>(), localDrafts = new Map<string, string>();
  /** Durable queue and exact-attempt guidance read from the machine. */
  const messages: Message[] = [];
  const workerTasks = new Map<string, string>();
  /** A batch of questions goes to the machine as one message, so its answers wait here until the last one is given. */
  const held = new Map<string, Map<string, { answer: WireAnswer; identity: string }>>();
  // Execution versions advance with unrelated display events while the owner answers.
  // Rebind a draft only when every question, option and permission identity is unchanged.
  const answerIdentity = (d: WireDecision) => JSON.stringify([d.decisionId,d.questionId,d.batchId,d.executionId,d.acpSessionId,d.ownerEpoch,d.requirementRevision,d.prompt,d.options,d.status,d.allowedOperations]);
  const isHeld = (d: WireDecision) => d.batchId !== null && !!held.get(d.batchId)?.has(d.decisionId);
  const projectConfigTokens = new Map<string, Record<string, unknown>>();
  const projectConfigBases = new Map<string, Record<string, unknown>>();
  let attachmentSupport = false, presentationRead = 0;
  // Every facts read is numbered as it begins (`ProjectFacts.readOrder`).
  let factsBegun = 0;
  // The facts each project's screens last received, by the coherent read or the poll. A poll sends only what differs.
  const sent = new Map<string, ProjectFacts>();
  const sprintProjects = new Map<string, string>();
  const rememberSprints = (id: string, product: Product) => {
    for (const [sprintId, projectId] of sprintProjects) if (projectId === id) sprintProjects.delete(sprintId);
    for (const sprint of product.sprints) sprintProjects.set(sprint.id, id);
  };
  /** Projects whose events arrived since the last refresh; the next refresh reads these and leaves the rest. */
  let invalidated = new Set<string>(), readScope = new Set<string>();
  /** What the screens on view show, per project: `facts` (workers, reviews, queued messages) or `git` (the repository too).
   *  Background reads go only to projects some screen holds interest in. */
  const interests = new Map<string, number>();
  const hasInterest = (projectId: string, kind: GitInterest) => (interests.get(`${projectId}|${kind}`) ?? 0) > 0;
  /** A Git screen: its Git is also reread on the visible refresh. */
  const wantsGit = (projectId: string) => hasInterest(projectId, "git");
  /** Git shown at all, a Git screen or a status line: read with the facts on events, on return and after an own action, never on a timer. */
  const showsGit = (projectId: string) => wantsGit(projectId) || hasInterest(projectId, "status");
  const watched = (projectId: string) => hasInterest(projectId, "facts") || showsGit(projectId);
  /** The tasks the screens on view show, per project, counted. A watched project's facts carry worker rows only for these:
   *  a worker row holds its whole recent activity, and the finished workers of every other task are not on screen. */
  const shownTasks = new Map<string, Map<string, number>>();
  /** The tasks whose workers a facts read of this project carries: every started one for a project no screen shows (an
   *  explicit read), only the shown ones for a project on view. */
  const workerScope = (projectId: string, started: string[]) => {
    if (!watched(projectId)) return started;
    const shown = shownTasks.get(projectId);
    return started.filter(id => (shown?.get(id) ?? 0) > 0);
  };
  /** Screens showing the machine overview. It is reread on the machine poll only while one is on view. */
  let overviewWatchers = 0;
  /** A Git screen came on view: its periodic refresh starts (see `subscribe`). */
  const gitShown = new Set<() => void>();
  let offered: string[] = [], serviceMethods: string[] = [], machineOn = false, taskWorktrees = false, configTokens: Record<string, unknown> = {}, configRead: ConfigRead | null = null;

  let codeLanguageOn = false;
  const fileParams = ({ checkoutId, path }: CodeFile) => ({ checkoutId, file: path });
  async function codeCall<K extends keyof Pick<WireTypes, "CodeLeaseAcquireResult" | "CodeLeaseRenewResult" | "CodeLeaseReleaseResult" | "CodeSaveResult" | "CodeLspOpenResult" | "CodeLspSnapshot" | "CodeLspCloseResult" | "CodeLspDefinitionResult">>(shape: K, op: MachineOp, projectId: string, params: unknown) {
    if (!options.localCode) throw new CodeRefusal("not_available", "");
    const result = await machine<unknown>(op, projectId, params);
    return decodeResponse(shape, result).value;
  }

  async function machine<T>(op: MachineOp, projectId?: string, params?: unknown): Promise<T> {
    const reply = await transport.request("machine.call", { op, ...(projectId ? { projectId } : {}), ...(params === undefined ? {} : { params }) }) as { ok?: boolean; result?: T; code?: string; message?: string; holderTaskId?: unknown };
    if (reply?.ok !== true) {
      const code = reply?.code ?? "refused", message = reply?.message ?? OUTCOME_WORDS.unsupported;
      if (op.startsWith("repo.file.lease.") || op === "repo.file.save" || op.startsWith("repo.lsp."))
        throw new CodeRefusal(code, message, typeof reply?.holderTaskId === "string" ? reply.holderTaskId : undefined);
      throw new Refusal(code, message);
    }
    if (projectId && ["git", "git_init", "repo.file.save", "product.task", "product.integrate"].includes(op)) {
      const cached = repoReads.get(projectId);
      // Retain the last confirmed repository while a post-action read catches up.
      // Replacing the entry also prevents an older in-flight read from publishing over it.
      if (cached) repoReads.set(projectId, { ...cached, at: -Infinity, running: false });
    }
    if (op === "product.read" && projectId && !(params as { attachments?: boolean } | undefined)?.attachments) rememberSprints(projectId, reply.result as Product);
    return reply.result as T;
  }
  const projectOfCheckout = (checkoutId: string) => checkoutId.split(":")[0];
  const projectOfTask = (taskId: string) => tasks.get(taskId)?.projectId ?? null;
  /**
   * One short model text on the machine: the job is started, then read until it is written or a minute has passed.
   * It changes no repository, so it does not drop the repository cache, and a lost reply is just no text.
   */
  async function shortText(projectId: string, params: { action: string; taskId?: string } & Record<string, unknown>): Promise<Record<string, unknown> | null> {
    const call = async (p: Record<string, unknown>) => {
      const reply = await transport.request("machine.call", { op: "product.task", projectId, params: p }) as { ok?: boolean; result?: Record<string, unknown>; code?: string };
      if (reply?.ok === true) return reply.result ?? null;
      return params.taskId ? null : { state: "done", source: "fallback", reason: reply?.code ?? "draft_unavailable" };
    };
    const started = await call(params).catch(() => null);
    const jobId = typeof started?.jobId === "string" ? started.jobId : null;
    if (!jobId) return started?.state === "done" ? started : null;
    for (const until = Date.now() + 60_000; Date.now() < until;) {
      await new Promise(resolve => setTimeout(resolve, 1200));
      const read = await call({ action: "short_text_poll", ...(params.taskId ? { taskId: params.taskId } : {}), jobId }).catch(() => null);
      if (!read) return null;
      if (read.state === "done") return read;
    }
    return null;
  }
  function integrationText(written: Record<string, unknown> | null): IntegrationText | null {
    if (!written) return null;
    const text = written.text as Record<string, unknown> | undefined;
    const str = (v: unknown) => typeof v === "string" && v.trim() ? v : undefined;
    const route = written.route as { model?: unknown } | null | undefined;
    return {
      source: written.source === "model" || written.source === "template" ? written.source : "fallback",
      message: str(text?.message), title: str(text?.title), body: str(text?.body), model: str(route?.model),
      ...(typeof written.reason === "string" ? { reason: written.reason } : {}),
      ...(written.source === "saved" ? { bodySource: "saved_push_summary" } : {}),
    };
  }
  const projectOfSprint = async (sprintId: string) => {
    const id = sprintProjects.get(sprintId);
    if (id) return id;
    throw new Refusal("unknown_sprint", "Unknown sprint.");
  };

  /** An open question of the planning run reads as a question of the sprint. */
  function withPlanning(sprint: Product["sprints"][number]): Sprint {
    const id = sprint.planningTaskId;
    if (!id || sprint.planning !== "planning") return sprint;
    const asked = [...decisions.values()].find(d => d.projectId === sprint.projectId && d.wire.status === "pending" && !isHeld(d.wire) && d.wire.executionId === tasks.get(id)?.executionId);
    if (!asked) return sprint;
    return { ...sprint, planning: "question", conversation: [...sprint.conversation, { kind: "question", text: asked.wire.prompt, options: asked.wire.options.map(o => o.label) }] };
  }
  // Hybrid and Automatic queues are advanced by the service, never by a screen read.

  /** Why this project cannot be worked on now. The machine can be perfectly well: this is the project's own state. */
  async function issueOf(project: ProjectLocator): Promise<string | null> {
    if (project.activated === false || !serviceMethods.includes("projects.status")) return null;
    const status = await transport.request("projects.status", { projectId: project.projectId }) as { ready?: boolean; state?: string; reason?: string | null; message?: string };
    if (status?.ready === true) return null;
    if (typeof status?.message === "string" && status.message) return status.message;
    if (["starting","recovering"].includes(status?.state ?? "") && !status.reason) return null;
    if (status?.reason?.startsWith("dev_project_refused")) return "The dev store ownership changed. Release data was not opened. Check the dev project folder before retrying.";
    return PROJECT_ISSUES[status?.reason ?? ""] ?? "This project could not start.";
  }

  type RepoRead = ProjectRepo & { openBranch: string; defaultBranch: string; taskDiffs?: Record<string,{files:FileDiff[];reason?:string}> };
  const repoReads = new Map<string, { at: number; pending: Promise<RepoRead | null>; running: boolean; value: RepoRead | null; error: boolean; reason?: string }>();
  /** A failed repository read is retried on its own, soon and then less often, while a screen
   *  showing that project's Git is on view: a cause fixed outside the app (a folder's permissions,
   *  a Git lock) clears without an unrelated event or a restart. No screen, no retry. */
  const repoRetries = new Map<string, { timer?: ReturnType<typeof setTimeout>; failures: number }>();
  function retryRepository(id: string) {
    const retry = repoRetries.get(id) ?? { failures: 0 };
    retry.failures++;
    repoRetries.set(id, retry);
    if (retry.timer !== undefined || !observers.size || !wantsGit(id)) return;
    retry.timer = setTimeout(() => {
      retry.timer = undefined;
      if (observers.size && wantsGit(id) && projects.some(p => p.projectId === id)) void repositoryFacts(id, true, true);
    }, Math.min(REPO_RETRY_MS * 2 ** (retry.failures - 1), REPO_RETRY_MAX_MS));
  }
  async function repositoryFacts(id: string, fresh = false, publish = false) {
    let entry = repoReads.get(id);
    if (!entry || (!entry.running && (fresh || Date.now() - entry.at > 3000))) {
      const previous = entry?.value ?? null;
      entry = { at: Date.now(), pending: machine<RepoRead | null>("repo.snapshot", id), running: true, value: previous, error: false };
      repoReads.set(id, entry);
      const current = entry;
      // A retry after a failure publishes its result itself: no screen read is waiting for it.
      let deferred = previous !== null || publish;
      current.pending = current.pending.then(value => {
        const retry = repoRetries.get(id);
        if (retry) { clearTimeout(retry.timer); repoRetries.delete(id); }
        current.value = value;
        if (deferred && repoReads.get(id) === current) {
          // A slow Git reply is new facts, not a new task snapshot. A full refresh
          // could outlast the cache and spawn the next slow Git read forever.
          const project = projects.find(p => p.projectId === id), generation = presentationRead;
          if (project) void factsOf(project).then(facts => {
            if (generation === presentationRead && repoReads.get(id) === current) { sent.set(id, facts); for (const notify of observers) notify({kind:"facts",facts}); }
          }).catch(() => {}); // Keep the last confirmed facts; the normal poll retries.
        }
        return value;
      }).catch(error => {
        current.error = true;
        // The machine's own words say what is wrong and where; anything else stays generic.
        current.reason = error instanceof Refusal && error.code !== "machine_tool_failed" ? error.message : undefined;
        if (repoReads.get(id) === current) retryRepository(id);
        return current.value;
      }).finally(() => { current.running = false; current.at = Date.now(); });
      // Cached facts render immediately; the single in-flight read publishes its replacement.
      if (previous !== null) return entry;
      // Only fast Git belongs on the coherent task refresh path. A large checkout finishes separately.
      let timer: ReturnType<typeof setTimeout> | undefined;
      await Promise.race([current.pending, new Promise<void>(resolve => { timer = setTimeout(() => { deferred = true; resolve(); }, 50); })]);
      clearTimeout(timer);
    }
    return entry;
  }

  /** `git` false keeps the repository last read (no Git work at all), except for a project never read, whose branches are
   *  read once so its header and task branches can show. */
  /** `settle` false (every read but `settleFacts`) asks the machine not to wait for its slow parts: a task in Ready to
   *  merge whose files are still being checked, or a result whose check against the project folder has not finished, comes
   *  marked pending, and one settling read follows (`settleFacts`). */
  async function factsOf(project: ProjectLocator, fresh = false, git = true, settle = false): Promise<ScopedFacts> {
    const id = project.projectId;
    if (serviceMethods.includes("native.board")) await presentations.get(id);
    const readOrder = ++factsBegun;
    const started = [...tasks].filter(([, t]) => t.projectId === id && t.executionId).map(([taskId]) => taskId);
    const cachedRepo = repoReads.get(id);
    const scope = project.activated === false ? [] : workerScope(id, started);
    const workerExecutions = Object.fromEntries(scope.map(taskId => [taskId, tasks.get(taskId)?.executionId ?? null]));
    // Independent reads overlap; neither Git nor workers delays sending the product read.
    const [bundle, repository] = await Promise.all([
      machine<{ product: Product; workers: Worker[] }>("product.facts", id, { taskIds: scope, ...(settle ? {} : { defer: true }) }),
      git || !cachedRepo ? repositoryFacts(id, fresh) : cachedRepo,
    ]);
    const unsettledRead = !settle && unsettled(bundle.product);
    const { workers } = bundle;
    // A deferring read answers a result it is still checking in a pending shape (no check notes, warnings or digest).
    // About the same result that is no news: the last settled facts stay until a settled answer or another version comes.
    const product = keepSettled(bundle.product, sent.get(id));
    factsReadAt.set(id, Date.now());
    // Polls and explicit facts reads must obey the same membership fence as the first board.
    if (serviceMethods.includes("native.board")) {
      const confirmed = boardTasks.get(id);
      if (!confirmed) throw new BoardPending();
      // A member the service already deleted is never on the board again; only the members that still exist must be.
      if (product.sprints.some(s => liveMembers(product.tasks, s).some(taskId => !confirmed.has(taskId)))) {
        invalidated.add(id);
        for (const notify of observers) notify({kind:"refresh"});
        throw new BoardPending();
      }
    }
    rememberSprints(id, product);
    const read = repository.value;
    // A copy, never the cached read itself: the next reads share that one, and the poll compares these facts with the last sent.
    const repo = read && { ...read, checkouts: read.checkouts.map(c => ({ ...c, draft: localDrafts.get(c.id) ?? c.draft })) };
    for (const w of workers) {
      workerTasks.set(w.id, w.taskId);
      if (w.result) w.result = {...w.result, checks:w.result.checks.map(c => ({...c, detail:readableCheckReason(c.detail)}))};
    }
    const ownMessages = [...(product.messages ?? []), ...workers.flatMap(w => w.messages ?? [])];
    for (let i = messages.length - 1; i >= 0; i--) if (tasks.get(messages[i].taskId)?.projectId === id) messages.splice(i, 1);
    messages.push(...ownMessages);
    // A task closed as is keeps its changes where they are, so its Changes tab keeps showing them. In the shared project
    // folder only the files its workers recorded writing are its own; without such a record nothing is attributed to it.
    const recorded = (taskId: string) => new Set(workers.filter(w => w.taskId === taskId).flatMap(w => w.result?.edits ?? []).map(e => e.path));
    const keptFiles = (taskId: string) => {
      const files = changesOf(repo, taskId, product.tasks[taskId]);
      if (product.tasks[taskId]?.checkout?.kind === "worktree") return files;
      const own = recorded(taskId);
      return files.filter(f => own.has(f.path));
    };
    const reviewed = (taskId: string) => product.tasks[taskId]?.status !== "done" || (product.tasks[taskId]?.closedAs === "as_is" && (!!repo?.taskDiffs?.[taskId] || recorded(taskId).size > 0));
    const reviews: TaskChanges[] = started.filter(reviewed).map(taskId => ({ taskId, sha: "", unavailableReason: repo?.taskDiffs?.[taskId]?.reason ?? (!repo || repository.error ? (repository.error && repository.reason) || "Repository details are updating or unavailable. Refresh Code or inspect Git before reviewing changes." : undefined), files: repo?.taskDiffs?.[taskId]?.files ?? (product.tasks[taskId]?.status === "done" ? keptFiles(taskId) : changesOf(repo, taskId, product.tasks[taskId])), scopes: [], comments: product.comments?.[taskId] ?? [] }));
    const issue = await issueOf(project);
    // After this read is published: a settling read of the same project follows it (`settleFacts`).
    if (unsettledRead) queueMicrotask(() => settleFacts(project));
    return { projectId: id, hasGit: product.hasGit, issue, openBranch: repo?.openBranch ?? "", defaultBranch: repo?.defaultBranch ?? "", repo, workers, reviews, overlays: product.tasks, sprints: product.sprints.map(s => withPlanning(presentSprint(product.tasks, s))), messages: messages.filter(m => tasks.get(m.taskId)?.projectId === id),
      heldAnswers: Object.fromEntries([...held.values()].flatMap(batch => [...batch].filter(([decisionId]) => decisions.get(decisionId)?.projectId === id).map(([decisionId, answer]) => [decisionId, answer.answer.text]))), readOrder, workerScope: scope, workerExecutions };
  }

  const configBases = new Map<string, Record<string, unknown>>();
  /** Told after every settings read, wherever it came from, so the machine poll lines up with the service's next limits snapshot. */
  const machineReads = new Set<(observed: number[]) => void>();
  let lastObserved: number[] | null = null;
  /** What the first screen needs: settings, connections, engine usage and the composer's options. Never the overview. */
  async function machineFacts(): Promise<MachineFacts> {
    const facts = await readMachineFacts();
    const observed = lastObserved = usageObserved(facts.engineUsage ?? []);
    for (const read of machineReads) read(observed);
    return facts;
  }
  async function readMachineFacts(): Promise<MachineFacts> {
    const currentRead = await machine<ConfigRead>("config.read");
    // First activation and background engine setup can create files without a UI command.
    configRead = currentRead;
    configTokens = currentRead.tokens;
    for (const [surface, revision] of Object.entries(currentRead.config.revisions ?? {})) configBases.set(`${surface}:${revision}`, currentRead.tokens);
    // One project's refused store must not hide the machine's own settings behind
    // the empty placeholder. Its overrides stay unread, and unsaveable, until a read succeeds.
    const saves = await Promise.all(projects.filter(p => p.activated !== false).map(async project => {
      try { return { project, saved: await machine<{value: MachineConfig["projects"][string]; tokens: Record<string, unknown>}>("project.config.read", project.projectId) }; }
      catch (error) { if (!(error instanceof Refusal)) throw error; projectConfigTokens.delete(project.projectId); return null; }
    }));
    for (const entry of saves) {
      if (!entry) continue;
      const { project, saved } = entry;
      currentRead.config.projects[project.projectId] = saved.value;
      projectConfigTokens.set(project.projectId, saved.tokens);
      for (const [surface,revision] of Object.entries(saved.value.revisions ?? {})) projectConfigBases.set(`${project.projectId}:${surface}:${revision}`, saved.tokens);
    }
    return { machineId: LOCAL_MACHINE_ID, engineUsage: weeklyUsage(currentRead.usage ?? []), options: sessionOptions(currentRead.config), catalogue: currentRead.catalogueRefresh, patch: { config: currentRead.config, connections: currentRead.connections, lastConfirmedAt: new Date().toISOString() } };
  }
  /** Read when a screen that shows storage, retention, worktrees, the version or GitHub CLI opens. While one is on view
   *  (`watchOverview`), the machine poll keeps it current; with none on view it is not read again. */
  async function machineOverview(): Promise<MachineFacts> {
    const { name, retentionDays, autoArchiveDays, automaticArchivePurge, unreferencedAttachmentRetentionDays, worktreeRoot, defaultWorktreeRoot, version, diskMb, storage, cleanup, diagnostics, ghAvailable, reclamationFailures } = await machine<MachineRead>("overview");
    return { machineId: LOCAL_MACHINE_ID, patch: { ghAvailable, reclamationFailures, name, retentionDays, autoArchiveDays, automaticArchivePurge, unreferencedAttachmentRetentionDays, worktreeRoot, defaultWorktreeRoot, version, diskMb, storage, cleanup, diagnostics } };
  }

  async function nativeRead(projectId: string, operation: string, payload: unknown) {
    const raw = await transport.request(operation === "tasks.board" ? "native.board" : "native.query", { protocol: { major: 2, mapping: 2 }, requestId: `native-${operation}-${projectId}`, projectId, operation, payload });
    const reply = raw as { protocol?: { major?: number; mapping?: number }; generation?: string; projectId?: string; highWatermark?: string; version?: string; data?: { type?: string; value?: unknown } };
    if (reply?.protocol?.major !== 2 || reply.protocol.mapping !== 2 || reply.projectId !== projectId || !reply.generation || !nativeVersion(reply.highWatermark) || !nativeVersion(reply.version) || !reply.data) throw new Error("Invalid native read reply");
    return reply as { generation: string; projectId: string; highWatermark: string; version: string; data: { type: string; value: unknown } };
  }
  const read = async <K extends keyof WireTypes>(type: K, operation: string, message: unknown): Promise<WireTypes[K]> => {
    const m = message as { projectId?: string; target?: string; cursor?: string; limit?: number };
    if (serviceMethods.includes("native.query") && m.projectId && ["tasks.get", "history.page"].includes(operation)) {
      const reply = await nativeRead(m.projectId, operation, operation === "tasks.get" ? { id: m.target } : { target: m.target, cursor: m.cursor ?? null, limit: m.limit ?? 100 });
      if (reply.data.type !== (operation === "tasks.get" ? "task" : "history")) throw new Error("Native read type mismatch");
      return decodeResponse(type, reply.data.value).value;
    }
    return decodeResponse(type, await transport.request(operation, message)).value;
  };

  async function capabilitiesGet(): Promise<PortCapabilities> {
    try {
      return await negotiateCapabilities();
    } catch (error) {
      const message = compatibilityRefusal(error instanceof Error ? error.message : String(error));
      if (message) throw new Error(message);
      throw error;
    }
  }

  async function negotiateCapabilities(): Promise<PortCapabilities> {
    if (options.registry) {
      const service = await transport.request("service.capabilities", {}) as { methods?: unknown; codeLsp?: unknown };
      if (!Array.isArray(service.methods) || !service.methods.every(m => typeof m === "string")) throw new Error("Invalid service capabilities");
      serviceMethods = service.methods;
      codeLanguageOn = options.localCode === true && service.codeLsp === true;
      machineOn = (service as { machine?: unknown }).machine === true;
      // Older services omit this field; only explicit support enables the feature.
      taskWorktrees = (service as { taskWorktrees?: unknown }).taskWorktrees === true;
      const registry = await transport.request("projects.list", {});
      if (!Array.isArray(registry)) throw new Error("Invalid project registry");
      const next: ProjectLocator[] = registry.map(p => {
        if (typeof p.projectId !== "string" || typeof p.displayName !== "string" || typeof p.canonicalRoot !== "string") throw new Error("Invalid project registry");
        return {projectId:p.projectId,name:p.displayName,repository:p.canonicalRoot,openBranch:"",defaultBranch:"",activated:p.activation != null};
      });
      projects.splice(0, projects.length, ...next);
      for (const [id,b] of cachedBoards) if (!next.some(p => p.projectId === id && p.repository === b.project.repository)) cachedBoards.delete(id);
      writeBoards(options.boardStorage,actorId,[...cachedBoards.values()]);
      recheckConfirmations();
      for (const notify of observers) notify({kind:"registry",projectIds:next.map(p => p.projectId)});
      for (const project of next) projectState(project, "registry", null);
    }
    const remote = await read("Capabilities", "capabilities.get", { protocol: { ...PROTOCOL }, requestId: "read-capabilities" });
    const shared = negotiate(localCapabilities("workspace-ui", "0", remote.operations), remote);
    offered = shared.operations;
    attachmentSupport = remote.attachments === true;
    return { everything: false, operations: [...(attachmentSupport ? ["workspace.attachments"] : []), ...WIRE_OPERATIONS.filter(operation => offered.includes(operation)), ...(serviceMethods.includes("native.pick_folder") ? ["workspace.pick_folder"] : []), ...(serviceMethods.includes("directories.search") ? ["workspace.search_folders"] : []), ...(serviceMethods.includes("projects.register") ? ["workspace.add_project"] : []), ...(serviceMethods.includes("projects.rename") ? ["workspace.rename_project"] : []), ...(serviceMethods.includes("projects.activate") ? ["workspace.activate_project"] : []), ...(serviceMethods.includes("projects.remove") ? ["workspace.remove_project"] : []), ...(serviceMethods.includes("native.open_editor") ? ["workspace.open_editor"] : []), ...(serviceMethods.includes("native.open_editor.vscode") ? ["workspace.open_vscode"] : []), ...(serviceMethods.includes("native.reveal_file") ? ["workspace.reveal_file"] : []), ...(serviceMethods.includes("native.open_logs") ? ["workspace.open_logs"] : []), ...(serviceMethods.includes("native.open_pull_request") ? ["workspace.open_pull_request"] : []), ...(serviceMethods.includes("remote.status") ? [FEATURES.remoteAccess] : []), ...(machineOn ? [...MACHINE_COMMANDS.map(capabilityOf), ...(options.localCode ? [FEATURES.editFiles] : []), ...(codeLanguageOn ? [CODE_LANGUAGE] : []), "workspace.worker_messages", ...(serviceMethods.includes("native.query") ? [] : [FEATURES.workerAttachments]), ...(taskWorktrees ? ["workspace.task_worktrees"] : []), "workspace.task_route"] : [])] };
  }

  async function projectTasks(project: ProjectLocator, cached = false) {
    if (serviceMethods.includes("native.query")) {
      const held = nativeSnapshots.get(project.projectId);
      if (cached && held) return { version: held.highWatermark, watermark: held.highWatermark, eventCursor: null, items: held.items };
      if (serviceMethods.includes("native.board")) {
        const reply = await nativeRead(project.projectId, "tasks.board", {id:project.projectId});
        if (reply.data.type !== "board") throw new Error("Native board type mismatch");
        const value = reply.data.value as {items?: unknown[];nextCursor?: unknown;highWatermark?: unknown};
        if (!Array.isArray(value.items) || value.items.length > 1000 || value.nextCursor !== null || value.highWatermark !== reply.highWatermark) throw new Error("Incomplete native board");
        const items = value.items.map(t => validate("Task",t));
        if (new Set(items.map(t => t.taskId)).size !== items.length) throw new Error("Duplicate task identities");
        const snapshot = {projectId:project.projectId,generation:reply.generation,highWatermark:reply.highWatermark,version:reply.version,items};
        nativeSnapshots.set(project.projectId,snapshot);
        return {version:reply.highWatermark,watermark:reply.highWatermark,eventCursor:null,items};
      }
      let cursor: string | null = null, snapshot: NativeTaskSnapshot | undefined;
      for (let page = 0; page < PAGE_LIMIT; page++) {
        const reply = await nativeRead(project.projectId, "tasks.page", { target: project.projectId, cursor, limit: 50 });
        if (reply.data.type !== "tasks") throw new Error("Native read type mismatch");
        const value = decodeResponse("TaskPage", reply.data.value).value;
        if (snapshot && (snapshot.generation !== reply.generation || snapshot.highWatermark !== reply.highWatermark)) throw new Error("Native snapshot changed between pages");
        snapshot ??= { projectId: project.projectId, generation: reply.generation, highWatermark: reply.highWatermark, version: reply.version, items: [] };
        snapshot.items.push(...value.items);
        cursor = value.nextCursor;
        if (cursor === null) break;
      }
      if (cursor !== null || !snapshot) throw new Error("Task history exceeds the read budget");
      if (new Set(snapshot.items.map(t => t.taskId)).size !== snapshot.items.length) throw new Error("Duplicate task identities");
      const current = nativeSnapshots.get(project.projectId);
      if (current?.generation === snapshot.generation && BigInt(current.highWatermark) > BigInt(snapshot.highWatermark)) snapshot = current;
      nativeSnapshots.set(project.projectId, snapshot);
      return { version: snapshot.highWatermark, watermark: snapshot.highWatermark, eventCursor: null, items: snapshot.items };
    }
    const snapshot = await read("ProjectSnapshot", "project.snapshot", request(project.projectId, "project.snapshot"));
    if (snapshot.projectId !== project.projectId) throw new Error("Snapshot project identity mismatch");
    const items = [...snapshot.tasks.items];
    let cursor = snapshot.tasks.nextCursor;
    for (let page = 0; cursor !== null && page < PAGE_LIMIT; page++) {
      const next = await read("TaskPage", "tasks.list", request(project.projectId, "tasks.list", { cursor }));
      items.push(...next.items);
      cursor = next.nextCursor;
    }
    if (cursor !== null) throw new Error("Task history exceeds the read budget");
    if (new Set(items.map(t => t.taskId)).size !== items.length) throw new Error("Duplicate task identities");
    return { version: snapshot.version, watermark: snapshot.highWatermark, eventCursor: typeof snapshot.eventCursor === "string" ? snapshot.eventCursor : null, items };
  }
  /** One execution's questions, page by page. A read of the same execution already under way is shared, never sent twice. */
  const decisionReads = new Map<string, Promise<WireDecision[]>>();
  function executionDecisions(project: ProjectLocator, executionId: string): Promise<WireDecision[]> {
    const running = decisionReads.get(executionId);
    if (running) return running;
    const work = (async () => {
      const items: WireDecision[] = [];
      let cursor: string | null = null;
      for (let page = 0; page < PAGE_LIMIT; page++) {
        const next: WireTypes["DecisionPage"] = await read("DecisionPage", "decisions.list", request(project.projectId, "decisions.list", { target: executionId, cursor }));
        if (next.items.some(d => d.executionId !== executionId)) throw new Error("Question execution identity mismatch");
        items.push(...next.items); cursor = next.nextCursor;
        if (cursor === null) break;
      }
      if (cursor !== null) throw new Error("Question history exceeds the read budget");
      return items;
    })();
    decisionReads.set(executionId, work);
    void work.finally(() => { if (decisionReads.get(executionId) === work) decisionReads.delete(executionId); }).catch(() => {});
    return work;
  }
  async function projectDecisions(project: ProjectLocator, tasks: WireTask[]): Promise<WireDecision[]> {
    if (!offered.includes("decisions.list")) return [];
    const items: WireDecision[] = [];
    // The tasks on screen first, chosen again before each read: their conversations wait for these questions, the board's
    // badges do not. A screen's own first read of them, under way, goes first; this read resumes after it.
    const shown = (t: WireTask) => (shownTasks.get(project.projectId)?.get(t.taskId) ?? 0) > 0;
    const remaining = tasks.filter(t => t.executionId !== null);
    while (remaining.length) {
      await shownFirst(project.projectId);
      const [task] = remaining.splice(Math.max(0, remaining.findIndex(shown)), 1);
      items.push(...await executionDecisions(project, task.executionId!));
    }
    return items;
  }
  /** Executions whose questions a read has confirmed in this window. Until then a conversation on screen is not read: its
   *  empty or partial questions say nothing yet (`decisionsRead`). A read of them that failed keeps its reason here. */
  const decisionsConfirmed = new Set<string>(), decisionIssues = new Map<string, string>();
  /** What a screen's first reveal waits on in each project (the questions of the tasks on screen): the board's own reads of
   *  questions start after these, so the conversation on view is answered first. */
  const priorityWork = new Map<string, Set<Promise<unknown>>>();
  /** The questions of a task on screen, read on their own as soon as its identity is confirmed, before the rest of the board's.
   *  One read per execution at a time; nothing once they are confirmed. */
  function readShownDecisions(project: ProjectLocator, taskId: string) {
    if (!perTaskHistory() || !offered.includes("decisions.list") || !observers.size) return;
    const task = nativeSnapshots.get(project.projectId)?.items.find(t => t.taskId === taskId);
    const executionId = task?.executionId;
    if (!executionId || decisionsConfirmed.has(executionId) || decisionReads.has(executionId)) return;
    const id = project.projectId;
    const work = executionDecisions(project, executionId).then(items => {
      // Another execution of this task by now: these questions are not its.
      if (nativeSnapshots.get(id)?.items.find(t => t.taskId === taskId)?.executionId !== executionId) return;
      wireDecisions.set(id, [...(wireDecisions.get(id) ?? []).filter(d => d.executionId !== executionId), ...items]);
      for (const [key, value] of decisions) if (value.projectId === id && value.wire.executionId === executionId) decisions.delete(key);
      for (const wire of items) decisions.set(wire.decisionId, { projectId: id, wire });
      decisionsConfirmed.add(executionId); decisionIssues.delete(executionId);
    }, error => { if (!decisionsConfirmed.has(executionId)) decisionIssues.set(executionId, readCause(error)); })
      .then(() => observers.size ? publishHistory(project) : undefined);
    const held = priorityWork.get(id) ?? new Set<Promise<unknown>>();
    priorityWork.set(id, held.add(work));
    void work.finally(() => { held.delete(work); if (!held.size && priorityWork.get(id) === held) priorityWork.delete(id); });
  }
  /** Lets the screen's own first reads go first, for at most one first-view budget: the board's reads never wait longer. */
  async function shownFirst(projectId: string) {
    const pending = [...(priorityWork.get(projectId) ?? [])];
    if (!pending.length) return;
    let timer: ReturnType<typeof setTimeout> | undefined;
    await Promise.race([Promise.allSettled(pending), new Promise<void>(resolve => { timer = setTimeout(resolve, FIRST_VIEW_MS); })]);
    clearTimeout(timer);
  }
  async function projectResults(project: ProjectLocator, items: WireTask[]): Promise<WireResult[]> {
    if (!offered.includes("results.get")) return [];
    const ended = items.filter(task => task.phase === "result");
    return Promise.all(ended.map(async task => {
      const execution = await read("Execution", "executions.get", request(project.projectId,"executions.get",{target:task.executionId}));
      if (execution.executionId !== task.executionId || execution.taskId !== task.taskId) throw new Error("Execution identity mismatch");
      if (typeof execution.resultId !== "string") throw new Error("Runtime result identity is unavailable");
      const result = await read("Result", "results.get", request(project.projectId, "results.get", {target:execution.resultId}));
      if (result.resultId !== execution.resultId || result.executionId !== task.executionId || result.taskId !== task.taskId) throw new Error("Result identity mismatch");
      return result;
    }));
  }
  // Task versions alone do not reliably describe conversation changes.
  // `tail` is where a later read continues: the service's forward cursor past the rows held (null when it offers none).
  // `earlier` is where the rows before the held ones are read (a backward cursor): absent or null, the held rows start at the
  // conversation's beginning.
  // `limit` is the page size its cursors were read with (absent: the service's default): every read from them asks for it.
  const historyReads = new Map<string, { projectId: string; version: string; executionId: string; rows: HistoryRow[]; tail?: string | null; earlier?: string | null; limit?: number; reset?: boolean }>();
  // A change makes a held conversation due for a reread, never unread. A board read, which reads no conversation, keeps the
  // rows already confirmed until the details read that follows it replaces them: dropping them would empty a running task's
  // page on every event and redraw all of it a moment later. The mark is the change's turn, so a read already under way
  // when another change arrives leaves its task due.
  const historyDue = new Map<string, number>();
  let historyTurn = 0;
  const historyChanged = (taskId: string) => { historyDue.set(taskId, ++historyTurn); };
  const historyPositions = new Map<string, { watermark: string; cursor: string | null }>();
  /** One task's retained conversation, page by page: all of it, or with `from` (a forward cursor) only the rows written
   *  after the ones already held. `tail` is the forward cursor of the last page, null from a service that offers none. A
   *  page that does not answer within `HISTORY_PAGE_MS` fails the read. `limit` is the page size `from` was read with
   *  (absent: the service's default), asked for by every page of this read. */
  async function readHistory(projectId: string, taskId: string, from: string | null = null, limit?: number) {
    const rows: HistoryRow[] = [];
    let cursor: string | null = from, tail: string | null = null;
    // A conversation has no page budget: a long task's history is read to its end. Only a service that stops moving
    // forward (the same cursor twice) fails the read.
    for (;;) {
      const asked = cursor;
      let timer: ReturnType<typeof setTimeout> | undefined;
      const late = new Promise<never>((_, reject) => { timer = setTimeout(() => reject(new Error("The conversation read did not answer within a minute")), HISTORY_PAGE_MS); });
      try {
        const next: WireTypes["HistoryPage"] = await Promise.race([read("HistoryPage", "history.page", request(projectId, "history.page", {target:taskId,cursor,...(limit === undefined ? {} : {limit})})), late]);
        rows.push(...next.items); cursor = next.nextCursor;
        const forward = (next as { tailCursor?: unknown }).tailCursor;
        tail = typeof forward === "string" ? forward : null;
      } finally { clearTimeout(timer); }
      if (cursor === null) break;
      if (cursor === asked) throw new Error("The conversation read stopped moving forward");
    }
    return { rows, tail };
  }
  /** Whether conversations are read from their end (`history.earlier`): a native service that refuses the operation does not. */
  let readsBack = true;
  const backward = () => readsBack && serviceMethods.includes("native.query");
  /** Read one bounded wire page backwards. A page can be empty even when its cursor continues. `limit` must be the page size
   *  `cursor` was read with. */
  async function readBackPage(projectId: string, taskId: string, cursor: string | null, seen: Set<string>, limit: number) {
    if (cursor !== null) seen.add(cursor);
    let timer: ReturnType<typeof setTimeout> | undefined;
    const late = new Promise<never>((_, reject) => { timer = setTimeout(() => reject(new Error("The conversation read did not answer within a minute")), HISTORY_PAGE_MS); });
    try {
      const reply = await Promise.race([nativeRead(projectId, "history.earlier", {target:taskId, cursor, limit}), late]);
      if (reply.data.type !== "history") throw new Error("Native read type mismatch");
      const next = decodeResponse("HistoryPage", reply.data.value).value as WireTypes["HistoryPage"] & { tailCursor?: unknown; prevCursor?: unknown };
      const earlier = typeof next.prevCursor === "string" ? next.prevCursor : null;
      if (earlier !== null && seen.has(earlier)) throw new Error("The conversation read stopped moving back");
      return { rows: next.items, earlier, tail: typeof next.tailCursor === "string" ? next.tailCursor : null };
    } finally { clearTimeout(timer); }
  }
  /** A conversation read afresh: its latest part from a service that reads backwards, else all of it. */
  /** The entries the conversation on screen would show for these rows: what one screen of it is measured in. */
  const shownEntries = (projectId: string, taskId: string, rows: HistoryRow[]) => {
    const description = nativeSnapshots.get(projectId)?.items.find(t => t.taskId === taskId)?.description ?? "";
    return conversationOf(rows, { description, partial: true, answers: [] }).filter(({ entry }) => entry.kind !== "fold" || !!shownFold(entry, false).text).length;
  };
  const firstWindow = () => Math.max(1, options.firstWindow?.() ?? firstWindowEntries());
  async function readLatest(projectId: string, taskId: string): Promise<{ rows: HistoryRow[]; tail: string | null; earlier: string | null; limit?: number }> {
    if (backward()) {
      try {
        const rows: HistoryRow[] = [], seen = new Set<string>(), screen = firstWindow();
        // Each page asks for one screen's worth of rows, so a dense conversation opens on about one screen, never a whole
        // page of a hundred; when its rows make fewer entries than that (tool steps fold together), further pages of the same
        // size are read. The size never changes along the chain: the service refuses a cursor asked for with another.
        const limit = Math.min(HISTORY_PAGE_ROWS, screen), pages = Math.ceil(HISTORY_FIRST_ROWS / limit);
        let earlier: string | null = null, tail: string | null = null;
        for (let page = 0; page < pages; page++) {
          const next: Awaited<ReturnType<typeof readBackPage>> = await readBackPage(projectId, taskId, earlier, seen, limit);
          rows.unshift(...next.rows);
          if (page === 0) tail = next.tail;
          earlier = next.earlier;
          // About one screen is enough to open on: the rest is read as the owner scrolls up to it.
          if (earlier === null || rows.length >= HISTORY_FIRST_ROWS || shownEntries(projectId, taskId, rows) >= screen) break;
        }
        return { rows, tail, earlier, limit };
      }
      catch (error) {
        // An older service does not know the operation: it is read forward from the beginning, as before.
        if (!(error instanceof Error && /invalid_request|unsupported_operation/.test(error.message))) throw error;
        readsBack = false;
      }
    }
    return { ...await readHistory(projectId, taskId), earlier: null };
  }
  /** The service could not continue from a held position: the history was reset, or the position belongs to another session. */
  const historyMoved = (error: unknown) => error instanceof Error && /history_reset|invalid_cursor/.test(error.message);
  /** The rows held, then the ones read after them. A row already held (the same identity) is never added twice. */
  const appended = (held: HistoryRow[], rows: HistoryRow[]) => {
    const seen = new Set(held.map(r => r.historyId));
    return rows.some(r => !seen.has(r.historyId)) ? [...held, ...rows.filter(r => !seen.has(r.historyId))] : held;
  };
  /** One conversation brought up to date: only what was written after the rows held when the service can read forward
   *  from them, else the whole of it again (the first read, another execution, a history that was reset). */
  async function updateHistory(projectId: string, task: WireTask) {
    const held = historyReads.get(task.taskId);
    if (held?.tail && !held.reset && held.executionId === task.executionId && held.projectId === projectId) {
      try {
        const { rows, tail } = await readHistory(projectId, task.taskId, held.tail, held.limit);
        // Merge into the held object when applying the reply, not now: an earlier read may still prepend to it.
        return {rows, tail, earlier:null, limit:held.limit, continued:held};
      } catch (error) { if (!historyMoved(error)) throw error; }
    }
    return { ...await readLatest(projectId, task.taskId), continued:null };
  }
  /** Whether a conversation is read in the background: its task is on view (`interest`). With no screen declaring what it
   *  shows (a headless store), every conversation is. Another conversation is read when it is opened, from where it was. */
  const conversationShown = (projectId: string, taskId: string) => !interests.size || (shownTasks.get(projectId)?.get(taskId) ?? 0) > 0;
  // With a native board, conversations are read per task, beside the board and its details and never inside their consistency
  // window: a running task moves the watermark every moment, and a whole-project read that had to finish without it moving
  // was discarded again and again, so no conversation was ever shown. Each task's read is published as soon as it completes,
  // a failed one says so on that task and is retried, and rows already confirmed stay shown while a newer read runs.
  const historyIssues = new Map<string, { executionId: string; reason: string }>();
  const historyFailures = new Map<string, number>();
  const historyRetries = new Map<string, ReturnType<typeof setTimeout>>();
  const historyWork = new Map<string, Promise<void>>();
  const historyAgain = new Set<string>();
  const perTaskHistory = () => serviceMethods.includes("native.board") && offered.includes("history.page");
  function historyStale(task: WireTask) {
    const held = historyReads.get(task.taskId);
    return !held || held.executionId !== task.executionId || held.version !== task.version || historyDue.has(task.taskId);
  }
  function queueHistories(project: ProjectLocator) {
    if (!perTaskHistory() || !observers.size) return;
    if (historyWork.has(project.projectId)) { historyAgain.add(project.projectId); return; }
    const work = (async () => {
      do {
        historyAgain.delete(project.projectId);
        const snapshot = nativeSnapshots.get(project.projectId);
        if (!snapshot) return;
        // A conversation never read comes first, then the most recently active ones.
        const unread = (t: WireTask) => historyReads.get(t.taskId)?.executionId === t.executionId ? 1 : 0;
        const started = Date.now();
        const wanted = snapshot.items.filter(t => t.executionId !== null && !historyRetries.has(t.taskId) && conversationShown(project.projectId, t.taskId) && historyStale(t))
          .sort((a, b) => unread(a) - unread(b) || String(b.lastConfirmedAt ?? "").localeCompare(String(a.lastConfirmedAt ?? "")));
        for (const task of wanted) {
          if (!observers.size) return;
          const current = nativeSnapshots.get(project.projectId)?.items.find(t => t.taskId === task.taskId) ?? task;
          if (current.executionId !== task.executionId) continue;
          const due = historyDue.get(task.taskId);
          try {
            const before = historyReads.get(task.taskId)?.rows;
            const { rows: readRows, tail, earlier, limit, continued } = await updateHistory(project.projectId, task);
            const current = historyReads.get(task.taskId);
            // A reply in flight when an earlier cursor was refused must not restore the position that was dropped.
            if (continued && (current !== continued || continued.reset)) { historyAgain.add(project.projectId); continue; }
            const rows = continued ? appended(continued.rows, readRows) : readRows;
            // Keep the older boundary through a forward read. A fresh position doesn't inherit the previous failure.
            if (continued) Object.assign(continued, {version:task.version, rows, tail});
            else {
              if ((current?.earlier !== earlier || current?.executionId !== task.executionId) && earlierReads.get(task.taskId)?.state === "failed") earlierReads.delete(task.taskId);
              historyReads.set(task.taskId, {projectId:project.projectId, version:task.version, executionId:task.executionId!, rows, tail, earlier, limit});
            }
            if (historyDue.get(task.taskId) === due) historyDue.delete(task.taskId);
            const failed = historyIssues.delete(task.taskId); historyFailures.delete(task.taskId);
            // Nothing new on a forward read (a worker's rows, a hidden update): the page on screen stays as it is.
            if (rows === before && !failed) continue;
          } catch (error) {
            const failures = (historyFailures.get(task.taskId) ?? 0) + 1;
            historyFailures.set(task.taskId, failures);
            historyIssues.set(task.taskId, {executionId:task.executionId!, reason:readCause(error)});
            historyRetries.set(task.taskId, setTimeout(() => { historyRetries.delete(task.taskId); queueHistories(project); }, Math.min(HISTORY_RETRY_MS * 2 ** (failures - 1), 30_000)));
          }
          if (!observers.size) return;
          await publishHistory(project);
        }
        // Changes that arrived during this round are read right after it, and never sooner than `HISTORY_AGAIN_MS` after it
        // began: one read in flight per project, the events of a streaming task collapsing into the next one.
        if (historyAgain.has(project.projectId)) await new Promise(resolve => setTimeout(resolve, Math.max(0, HISTORY_AGAIN_MS - (Date.now() - started))));
      } while (historyAgain.has(project.projectId) && observers.size);
    })().catch(() => {}).finally(() => {
      historyWork.delete(project.projectId);
      if (historyAgain.has(project.projectId)) queueHistories(project);
    });
    historyWork.set(project.projectId, work);
  }
  /** What the conversation on screen now holds, published as an update of its project. */
  async function publishHistory(project: ProjectLocator) {
    const update = await snapshotOf(project, new Set()).catch(() => null);
    if (update && observers.size) for (const notify of observers) notify({kind:"enrichment", update});
  }
  /** One earlier read per conversation. Keep the starting cursor until the whole prepend can be published. */
  const earlierReads = new Map<string, { state: "loading" } | { state: "failed"; reason: string }>();
  function loadEarlierHistory(taskId: string, showAllTools = () => false) {
    const held = historyReads.get(taskId), project = projects.find(p => p.projectId === held?.projectId);
    if (!held || !project || typeof held.earlier !== "string" || earlierReads.get(taskId)?.state === "loading") return;
    const from = held.earlier;
    const active = {state:"loading"} as const;
    earlierReads.set(taskId, active);
    const currentTask = () => {
      const task = nativeSnapshots.get(project.projectId)?.items.find(t => t.taskId === taskId);
      return earlierReads.get(taskId) === active && historyReads.get(taskId) === held &&
        task?.executionId === held.executionId ? task : undefined;
    };
    void (async () => {
      await publishHistory(project);
      try {
        const batches: HistoryRow[][] = [], cursors = new Set<string>();
        let cursor: string | null = from;
        while (cursor !== null && observers.size && currentTask()) {
          const { rows, earlier }: Awaited<ReturnType<typeof readBackPage>> = await readBackPage(project.projectId, taskId, cursor, cursors, held.limit ?? HISTORY_PAGE_ROWS);
          const task = currentTask();
          if (!task || !observers.size) break;
          if (rows.length) batches.push(rows);
          cursor = earlier;
          if (!rows.length && cursor !== null) continue;

          // Compare against the same current tail both times. Rows streamed during this read are not prepend progress.
          const seen = new Set(held.rows.map(r => r.historyId));
          const older = batches.slice().reverse().flat().filter(r => {
            if (seen.has(r.historyId)) return false;
            seen.add(r.historyId); return true;
          });
          const candidate = older.length ? [...older, ...held.rows] : held.rows;
          const decisions = (wireDecisions.get(project.projectId) ?? []).filter(d => d.executionId === held.executionId);
          if (cursor !== null && (!older.length || !addsVisibleHistory(held.rows, candidate, task.description, decisions, showAllTools()))) continue;
          held.rows = candidate;
          held.earlier = cursor;
          break;
        }
      } catch (error) {
        if (currentTask()) {
          earlierReads.set(taskId, {state:"failed", reason:readCause(error)});
          // Refresh a lost position. If the refresh returns the same unusable cursor, it still waits for Retry.
          if (historyMoved(error)) { held.tail = null; held.reset = true; historyChanged(taskId); queueHistories(project); }
        }
      } finally {
        if (earlierReads.get(taskId) === active) earlierReads.delete(taskId);
        if (observers.size) await publishHistory(project);
      }
    })();
  }
  function retryHistory(taskId: string) {
    clearTimeout(historyRetries.get(taskId)); historyRetries.delete(taskId);
    const projectId = projectOfTask(taskId), project = projects.find(p => p.projectId === projectId);
    if (!project) return;
    historyChanged(taskId);
    queueHistories(project);
  }
  async function readSnapshot(project: ProjectLocator, changed?: Set<string>, details = false): Promise<ProjectionUpdate> {
    const boardOnly = serviceMethods.includes("native.board") && !details;
    // A project whose Cortex is away still has a page, with its reason on it. A store keeps what it
    // last confirmed, because version zero is never newer. The machine stays connected for the rest.
    const away = !serviceMethods.includes("native.query") && project.activated !== false && serviceMethods.includes("projects.status") && (await transport.request("projects.status", { projectId: project.projectId }) as { ready?: boolean }).ready !== true;
    const { version, watermark, eventCursor, items } = project.activated === false || away ? {version:"0",watermark:"0",eventCursor:null,items:[] as WireTask[]} : await projectTasks(project, changed !== undefined);
    const prior = historyPositions.get(project.projectId);
    if (!changed && prior && prior.watermark !== watermark) {
      let complete = false;
      if (prior.cursor && eventCursor && offered.includes("events.page")) {
        try {
          let cursor: string | null = prior.cursor;
          for (let page = 0; page < PAGE_LIMIT && cursor; page++) {
            const events: WireTypes["EventPage"] = await read("EventPage", "events.page", request(project.projectId, "events.page", {cursor}));
            for (const event of events.items) {
              if (event.kind === "task") historyChanged(event.entityId);
              else if (event.kind === "execution") for (const [id, cached] of historyReads) { if (cached.projectId === project.projectId && cached.executionId === event.entityId) historyChanged(id); }
              else if (event.kind === "unknown") throw new Error("Unknown event target");
            }
            cursor = events.nextCursor;
            if (!cursor) { complete = true; break; }
          }
        } catch { complete = false; }
      }
      if (!complete) for (const [id, cached] of historyReads) if (cached.projectId === project.projectId) historyChanged(id);
    }
    historyPositions.set(project.projectId, {watermark, cursor:eventCursor});
    const selected = changed ? items.filter(t => changed.has(t.taskId)) : items;
    const freshAsked = boardOnly ? wireDecisions.get(project.projectId) ?? [] : project.activated === false || away ? [] : await projectDecisions(project, selected), freshResults = boardOnly ? wireResults.get(project.projectId) ?? [] : away ? [] : await projectResults(project, selected);
    const unchangedExecutions = new Set(items.filter(t => changed && !changed.has(t.taskId)).map(t => t.executionId));
    const asked = [...(!boardOnly && changed ? (wireDecisions.get(project.projectId) ?? []).filter(d => unchangedExecutions.has(d.executionId)) : []), ...freshAsked];
    const results = [...(!boardOnly && changed ? (wireResults.get(project.projectId) ?? []).filter(r => unchangedExecutions.has(r.executionId)) : []), ...freshResults];

    const histories = new Map<string, HistoryRow[]>();
    for (const [id, held] of historyReads) if (held.projectId === project.projectId && !items.some(t => t.taskId === id && t.executionId !== null)) { historyReads.delete(id); historyDue.delete(id); historyIssues.delete(id); earlierReads.delete(id); }
    for (const task of items) { const held = historyReads.get(task.taskId); if (held?.executionId === task.executionId) histories.set(task.taskId, held.rows); }
    // With a native board each conversation is read on its own (`queueHistories`); here it is only what was confirmed.
    if (!boardOnly && !perTaskHistory() && offered.includes("history.page")) for (const task of items.filter(t => t.executionId !== null)) {
      const held = historyReads.get(task.taskId), due = historyDue.get(task.taskId);
      if (due === undefined && held?.projectId === project.projectId && held.version === task.version && held.executionId === task.executionId) {
        histories.set(task.taskId, held.rows);
        continue;
      }
      const { rows } = await readHistory(project.projectId, task.taskId);
      histories.set(task.taskId, rows);
      historyReads.set(task.taskId, {projectId:project.projectId, version:task.version, executionId:task.executionId!, rows});
      if (historyDue.get(task.taskId) === due) historyDue.delete(task.taskId);
    }
    // Questions and results read here are at least as new as the snapshot they were read for, so they are kept even when a
    // newer snapshot supersedes this details read. A running task moves the watermark all the time: discarding them made an
    // answered question stay "Needs you" on the board and in the sidebar for as long as the task kept streaming.
    wireDecisions.set(project.projectId, asked); wireResults.set(project.projectId, results);
    // These executions' questions are read now, whatever a newer snapshot says next.
    if (!boardOnly && offered.includes("decisions.list")) for (const t of selected) if (t.executionId) { decisionsConfirmed.add(t.executionId); decisionIssues.delete(t.executionId); }
    for (const [key, value] of decisions) if (value.projectId === project.projectId) decisions.delete(key);
    for (const wire of asked) decisions.set(wire.decisionId, { projectId: project.projectId, wire });
    if (details && nativeSnapshots.get(project.projectId)?.highWatermark !== watermark) throw new DetailsSuperseded("Native snapshot changed during details read");
    for (const wire of items) {
      // An older read may settle after a mutation acknowledged its newer version.
      const current = tasks.get(wire.taskId);
      if (!current || BigInt(wire.version) >= BigInt(current.version)) tasks.set(wire.taskId, { projectId: project.projectId, version: wire.version, executionId: wire.executionId, runtimeState: wire.runtimeState });
    }
    confirmedProjects.add(project.projectId); recheckConfirmations();
    const { projectId: id, name, repository, openBranch, defaultBranch } = project;
    const stamps = [...items.map(t => t.lastConfirmedAt), ...asked.map(d => d.createdAt), ...[...histories.values()].flatMap(rows => rows.map(r => r.recordedAt))]
      .filter((s): s is string => typeof s === "string").map(Date.parse).filter(Number.isFinite);
    const at = stamps.length ? Math.max(...stamps) : undefined;
    const slice = mapProjectSlice({ id, machineId: LOCAL_MACHINE_ID, name, repository, openBranch, defaultBranch, activated:project.activated, confirmedAt:at }, items, asked, results);
    for (const task of items) {
      let session = slice.sessions.find(s => s.taskId === task.taskId);
      if (!session) { session = emptySession(task.taskId); slice.sessions.push(session); }
      const own = asked.filter(d => d.executionId === task.executionId);
      const held = historyReads.get(task.taskId), rows = histories.get(task.taskId) ?? [];
      const partial = held?.executionId === task.executionId && typeof held.earlier === "string";
      const { dated, since } = projectedConversation(rows, task.description, own, partial);
      session.timeline = dated.sort((a,b) => a.at.localeCompare(b.at)).map(d => d.entry);
      if (task.executionId !== null && perTaskHistory()) {
        const issue = historyIssues.get(task.taskId);
        if (issue?.executionId === task.executionId) session.historyRead = {state:"failed", reason:issue.reason};
        else if (!histories.has(task.taskId)) session.historyRead = {state:"loading"};
        if (partial) session.historyEarlier = {...(earlierReads.get(task.taskId) ?? {state:"available" as const}), since};
        // Until its questions are read, the conversation is not read either: questions that arrive first never stand alone.
        if (offered.includes("decisions.list") && !decisionsConfirmed.has(task.executionId)) {
          const issue = decisionIssues.get(task.executionId);
          (session as typeof session & SessionReads).decisionsRead = issue ? {state:"failed", reason:issue} : {state:"loading"};
        }
      }
    }
    slices.set(project.projectId, slice);
    return { kind: "projection", version, scope: "project", slice };
  }

  // Registry identity and read health do not depend on opening a project's store.
  let commandEpoch = 0;
  const presentationFacts = new Map<string, ProjectFacts>();
  const presentations = new Map<string, Promise<ProjectionUpdate>>();
  const projectReadIssues = new Map<string, Map<string, string>>();
  const projectRetries = new Map<string, ReturnType<typeof setTimeout>>();
  const readFailures = new Map<string, number>();
  /** When each project's facts were last read in full, so a failed refresh says how old what is shown is. */
  const factsReadAt = new Map<string, number>();
  const shownFrom = (projectId: string) => { const at = factsReadAt.get(projectId); return at === undefined ? "" : ` Showing data from ${new Date(at).toLocaleTimeString()}.`; };
  const enrichments = new Map<string, Promise<void>>();
  const projectReadCauses = new Map<string, Map<string, string>>();
  /** `cause` is what actually failed, kept beside the owner's words so the retained reason is the precise one. */
  function projectState(project: ProjectLocator, scope: string, issue: string | null, cause?: unknown) {
    const issues = projectReadIssues.get(project.projectId) ?? new Map<string, string>();
    const causes = projectReadCauses.get(project.projectId) ?? new Map<string, string>();
    projectReadIssues.set(project.projectId, issues);
    projectReadCauses.set(project.projectId, causes);
    if (issue) issues.set(scope, issue); else issues.delete(scope);
    if (issue && cause !== undefined) causes.set(scope, readCause(cause)); else causes.delete(scope);
    const identity = mapProjectSlice({ id: project.projectId, machineId: LOCAL_MACHINE_ID, name: project.name, repository: project.repository, openBranch: project.openBranch, defaultBranch: project.defaultBranch, activated: project.activated }, [], [], []).project;
    const first = [...issues.keys()][0];
    for (const notify of observers) notify({ kind: "project_state", project: identity, issue: first === undefined ? null : issues.get(first)!, cause: first === undefined ? null : causes.get(first) ?? null });
  }
  // Bound only presentation waiting, not the read itself. Late verified results are
  // delivered once; failures retain the prior records and belong to that project.
  const lateReads = new Set<Promise<void>>();
  async function projectReads<T>(wanted: ProjectLocator[], scope: string, read: (p: ProjectLocator) => Promise<T>, publish: (value: T) => PortUpdate): Promise<T[]> {
    if (!options.registry) return Promise.all(wanted.map(read));
    const epoch = commandEpoch;
    const values: T[] = [], pending = new Set(wanted);
    let deferred = false, timer: ReturnType<typeof setTimeout> | undefined;
    const work = wanted.map(async project => {
      try {
        const value = await read(project);
        if (epoch !== commandEpoch) { invalidated.add(project.projectId); for (const notify of observers) notify({kind:"refresh"}); return; }
        projectState(project, scope, null);
        readFailures.delete(`${project.projectId}:${scope}`);
        clearTimeout(projectRetries.get(`${project.projectId}:${scope}`));
        projectRetries.delete(`${project.projectId}:${scope}`);
        if (deferred) { const update = publish(value); for (const notify of observers) notify(update); }
        else values.push(value);
      } catch (error) {
        const key = `${project.projectId}:${scope}`;
        const failures = (readFailures.get(key) ?? 0) + (error instanceof BoardPending ? 0 : 1);
        readFailures.set(key, failures);
        if (failures >= 3) {
          const storage = storageRefusal(error);
          if (storage) projectState(project, scope, storage.message + shownFrom(project.projectId), storage.reason);
          else projectState(project, scope, `This project's data could not be read.${shownFrom(project.projectId)} Other projects remain available. Retrying automatically.`, error);
        }
        if (!projectRetries.has(key) && observers.size) projectRetries.set(key, setTimeout(() => {
          projectRetries.delete(key);
          if (observers.size) { invalidated.add(project.projectId); for (const notify of observers) notify({kind:"refresh"}); }
        }, 3000));
      } finally { pending.delete(project); }
    });
    await Promise.race([Promise.all(work), new Promise<void>(resolve => { timer = setTimeout(resolve, 100); })]);
    deferred = true;
    // A pending read is not a project failure. Retain the confirmed board silently.
    clearTimeout(timer);
    // What is still under way delivers itself as updates; `presented` settles once it has.
    if (pending.size) { const late = Promise.all(work).then(() => {}); lateReads.add(late); void late.finally(() => lateReads.delete(late)); }
    return values;
  }
  const factReads = new Map<string, Promise<ProjectFacts>>();
  function factsRead(project: ProjectLocator, fresh?: boolean, git = true) {
    // A read with Git also answers one without it; one without Git never stands in for a Git read.
    // The tasks shown are part of what is read: a read begun for other tasks does not stand in for this one.
    const scope = [...(shownTasks.get(project.projectId)?.keys() ?? [])].sort().join(",");
    const key = `${project.projectId}|${git}|${scope}`, current = factReads.get(`${project.projectId}|true|${scope}`) ?? factReads.get(key);
    if (current) return current;
    const next = factsOf(project, fresh, git);
    factReads.set(key, next);
    void next.finally(() => { if (factReads.get(key) === next) factReads.delete(key); }).catch(() => {});
    return next;
  }
  async function boardFacts(project: ProjectLocator): Promise<ProjectFacts> {
    const product = await machine<Product>("product.board", project.projectId);
    rememberSprints(project.projectId, product);
    const snapshot = nativeSnapshots.get(project.projectId);
    // A member the service already deleted (a sprint deletion that stopped part-way) is never on the board again.
    const unread = project.activated === false ? null : unreadMembers(product.tasks, product.sprints, id => !!snapshot?.items.some(t => t.taskId === id));
    if (unread) throw new Error(`Sprint membership could not be read completely. ${unread}`);
    const shown = {...product, sprints: product.sprints.map(s => presentSprint(product.tasks, s))};
    if (snapshot) {cachedBoards.set(project.projectId,{project,items:snapshot.items,product:shown});writeBoards(options.boardStorage,actorId,[...cachedBoards.values()]);}
    const previous = sent.get(project.projectId);
    // The board read carries each task's saved record, not what only a facts read knows (its result, review, integration
    // facts). Like the workers, those stay as last confirmed until the next facts read: dropping them hid the result card of a
    // task in review after every board read, until a facts read brought it back.
    const overlays = Object.fromEntries(Object.entries(product.tasks).map(([id, saved]) => {
      const known = previous?.overlays[id];
      if (!known || saved.reworkEvent !== known.reworkEvent) return [id, saved];
      // `workspaceDigest` rides in the machine's overlay beside the result (it is not one of the saved fields).
      const { result, reviewActions, integrationFacts } = known, workspaceDigest = (known as { workspaceDigest?: unknown }).workspaceDigest;
      return [id, { ...(result !== undefined ? { result } : {}), ...(reviewActions !== undefined ? { reviewActions } : {}),
        ...(workspaceDigest !== undefined ? { workspaceDigest } : {}), ...(integrationFacts !== undefined ? { integrationFacts } : {}),
        // Timing is also read with task facts, not the saved board.
        ...(known.spend !== undefined ? { spend: known.spend } : {}),
        ...(known.coordinatorMetrics !== undefined ? { coordinatorMetrics: known.coordinatorMetrics } : {}), ...saved }];
    }));
    return {...previous,projectId:project.projectId,openBranch:previous?.openBranch ?? "",defaultBranch:previous?.defaultBranch ?? "",
      repo:previous?.repo ?? null,workers:previous?.workers ?? [],reviews:previous?.reviews ?? [],
      overlays,sprints:shown.sprints.map(withPlanning),
      // Workers, results and Git are the last facts read's, carried over: so is its place in the read order. A full facts read
      // that began before this board read still replaces it.
      readOrder:previous?.readOrder ?? 0};
  }
  function queueEnrichment(project: ProjectLocator) {
    queueHistories(project);
    // The questions of the task on screen are read first, on their own; the board's own read of every task's follows them.
    for (const taskId of shownTasks.get(project.projectId)?.keys() ?? []) readShownDecisions(project, taskId);
    // Git and machine facts follow on their own throttle (`streamFacts`); the details never wait for them, and a details read
    // already in flight does not hold back the facts of a task whose state just moved.
    if (machineOn) streamFacts(project);
    if (enrichments.has(project.projectId)) return;
    const epoch = commandEpoch, base = nativeSnapshots.get(project.projectId);
    // A running task moves the watermark continuously. Requests made while this read
    // was in flight were not queued, so an obsolete read rereads the newer snapshot.
    let superseded = false;
    // Run after the coherent board has had an opportunity to paint. Detail reads
    // have their own lifetime and cannot hold sprint membership or task placement.
    const work =new Promise<void>(resolve => setTimeout(resolve, 0)).then(() => shownFirst(project.projectId)).then(async () => {
      const update = await readSnapshot(project, new Set(base?.items.map(t => t.taskId)), true);
      const current = nativeSnapshots.get(project.projectId);
      if (!observers.size || epoch !== commandEpoch) return;
      if (current?.generation !== base?.generation || current?.highWatermark !== base?.highWatermark) { superseded = true; return; }
      for (const notify of observers) notify({kind:"enrichment",update});
      readFailures.delete(project.projectId+":details");
      projectState(project,"details",null);
    }).catch(error => {
      if (error instanceof DetailsSuperseded) { superseded = true; return; }
      const key = project.projectId+":details", failures = (readFailures.get(key) ?? 0)+(error instanceof BoardPending ? 0 : 1);
      readFailures.set(key,failures);
      if (failures >= 3) projectState(project,"details",`Task conversation details could not be read.${shownFrom(project.projectId)} The confirmed board is retained; workers last seen running under an ended task show as not confirmed. Retrying automatically.`,error);
      if (!projectRetries.has(key) && observers.size) projectRetries.set(key,setTimeout(()=>{
        projectRetries.delete(key);
        if (observers.size) {invalidated.add(project.projectId);for(const notify of observers) notify({kind:"refresh"});}
      },3000));
    }).finally(() => {
      enrichments.delete(project.projectId);
      // Without a held snapshot a refresh is already pending and will queue its own details read.
      if (superseded && observers.size && epoch === commandEpoch && nativeSnapshots.has(project.projectId)) queueEnrichment(project);
    });
    enrichments.set(project.projectId,work);
  }
  /** Facts reads asked for by a streaming task, only for a project a screen on view shows (`interest`): one in flight per
   *  project, the next no sooner than `streamFactsMs` after the last one ended (requests in between collapse into one
   *  trailing read), none while the window is hidden. A project no screen shows reads nothing here: its board and
   *  questions come from its events. Streamed text moves no task's state; a task whose state did move (a worker started or
   *  finished, a question came) reads its facts at once, or right after the read in flight, so its workers arrive with the
   *  state that shows them instead of an interval later. The repository is read again only with a Git screen on view, and
   *  then only when a task's state moved or the owner's own Git action made it stale, never for streamed text alone. */
  const streamFactsMs = options.streamFactsMs ?? STREAM_FACTS_MS;
  const factsThrottle = new Map<string, { ended: number; timer?: ReturnType<typeof setTimeout>; running: boolean; mark?: string }>();
  /** Deferred observations can settle in a follow-up read. Check-only previews instead wait for runtime completion. */
  const unsettled = (product: Product) => Object.values(product.tasks).some(t =>
    (t?.reviewActions?.pending === true && t.reviewActions.unblockAction !== "wait") ||
    t?.integrationFacts?.pending === true || t?.integrationFacts?.refreshing === true);
  /** The settled result facts of a task whose deferring read came back pending about the same result (same version and
   *  summary): its checks, warnings, digest, review actions and merge facts stay as last confirmed. `unsettled` still sees
   *  the pending read, so one settling read follows. A different version, or no earlier settled facts, shows as it came. */
  const keepSettled = (product: Product, previous: ProjectFacts | undefined): Product => {
    if (!previous) return product;
    let tasks = product.tasks;
    for (const [taskId, now] of Object.entries(product.tasks)) {
      const was = previous.overlays[taskId];
      if (!now || !was) continue;
      const patch: Record<string, unknown> = {};
      const same = !!now.result && !!was.result && was.result.version === now.result.version && was.result.summary === now.result.summary
        && (now.result.sourceVersion === undefined || now.result.sourceVersion === was.result.sourceVersion);
      if (now.reviewActions?.pending === true && was.reviewActions && was.reviewActions.pending !== true && was.result?.digest && same) {
        patch.result = { ...now.result, ...was.result };
        patch.reviewActions = was.reviewActions;
        patch.workspaceDigest = (was as { workspaceDigest?: unknown }).workspaceDigest;
      }
      if (now.integrationFacts?.pending === true && was.integrationFacts && was.integrationFacts.pending !== true && same) {
        patch.integrationFacts = { ...was.integrationFacts, refreshing: true };
      }
      if (Object.keys(patch).length) tasks = { ...tasks, [taskId]: { ...now, ...patch } };
    }
    return tasks === product.tasks ? product : { ...product, tasks };
  };
  /** One settling read per project after a read that came with pending parts: the machine answers it once what it was
   *  checking is known, and its facts replace the pending ones. There is no timer: a pending read that arrives while one
   *  runs is settled once more after it. Only for a project some screen on view shows. */
  const settling = new Map<string, { again: boolean }>();
  function settleFacts(project: ProjectLocator) {
    const id = project.projectId;
    if (!observers.size || !machineOn || !watched(id)) return;
    const running = settling.get(id);
    if (running) { running.again = true; return; }
    const gate = { again: false }, epoch = commandEpoch;
    settling.set(id, gate);
    void factsOf(project, false, false, true).then(facts => {
      if (!observers.size || epoch !== commandEpoch) return;
      sent.set(id, facts); for (const notify of observers) notify({ kind: "facts", facts });
    }).catch(() => {}).finally(() => {
      settling.delete(id);
      if (gate.again) settleFacts(project);
    });
  }
  /** The board states a project's worker facts follow. */
  const workMark = (projectId: string) => (nativeSnapshots.get(projectId)?.items ?? []).filter(t => !shownTasks.get(projectId)?.size || shownTasks.get(projectId)!.has(t.taskId))
    .map(t => [t.taskId, t.executionId, t.phase, t.runtimeState, t.executionState, t.waitReason].join("\u0000")).join("\u0001");
  /** The owner's own Git action (or a merge) made this project's repository stale. */
  const repoStale = (id: string) => repoReads.get(id)?.at === -Infinity;
  function streamFacts(project: ProjectLocator) {
    const id = project.projectId;
    if (!watched(id)) return;
    // A board with no task on view needs no worker rows: its cards come from the board and its events.
    if (!showsGit(id) && !shownTasks.get(id)?.size) return;
    const gate = factsThrottle.get(id) ?? { ended: 0, running: false };
    factsThrottle.set(id, gate);
    const run = () => {
      gate.timer = undefined;
      if (!observers.size || !machineOn || !watched(id) || (typeof document !== "undefined" && document.hidden)) return;
      if (gate.running) { gate.timer = setTimeout(run, streamFactsMs); return; }
      gate.running = true;
      const moved = gate.mark !== workMark(id);
      gate.mark = workMark(id);
      const epoch = commandEpoch;
      void factsRead(project, false, showsGit(id) && (moved || repoStale(id))).then(facts => {
        if (!observers.size || epoch !== commandEpoch) { gate.mark = undefined; return; }
        sent.set(id, facts); for (const notify of observers) notify({ kind: "facts", facts });
      }).catch(() => {}).finally(() => {
        gate.running = false; gate.ended = Date.now();
        // A state that moved while this read ran may not be in it: read again now.
        if (gate.mark !== workMark(id) && observers.size) { clearTimeout(gate.timer); run(); }
      });
    };
    if (!gate.running && (gate.mark !== workMark(id) || (showsGit(id) && repoStale(id)))) { clearTimeout(gate.timer); run(); return; }
    if (gate.timer !== undefined) return;
    const due = gate.running ? streamFactsMs : Math.max(0, gate.ended + streamFactsMs - Date.now());
    if (due === 0) run(); else gate.timer = setTimeout(run, due);
  }
  const reads = new Map<string, Promise<ProjectionUpdate>>();
  function snapshotOf(project: ProjectLocator, changed?: Set<string>): Promise<ProjectionUpdate> {
    const previous = reads.get(project.projectId);
    if (previous && !changed) return previous;
    const next = (previous ?? Promise.resolve()).catch(() => {}).then(() => readSnapshot(project, changed));
    reads.set(project.projectId,next);
    void next.finally(() => { if (reads.get(project.projectId) === next) reads.delete(project.projectId); }).catch(() => {});
    return next;
  }

  function wireCommand(envelope: CommandEnvelope): { operation: string; message: unknown } | "held" | null {
    const { requestId } = envelope, command: WorkspaceCommand = envelope.payload;
    const base = { protocol: { ...PROTOCOL }, requestId, actorId };
    if (command.type === "create_task") {
      return { operation: "tasks.create", message: { ...base, projectId: command.projectId, target: command.projectId, operation: "tasks.create", expectedVersion: "0", ownerEpoch: null, payload: { description: command.description, ...(command.attachments?.length ? {attachments:wireAttachments(command.attachments)} : {}), ...(command.settings?.model ? { settings: command.settings } : {}), checkout: { kind: command.checkout.kind } } } };
    }
    if (command.type === "start_task") {
      const task = tasks.get(command.taskId);
      if (!task) return null;
      return { operation: "tasks.start", message: { ...base, projectId: task.projectId, target: command.taskId, operation: "tasks.start", expectedVersion: task.version, ownerEpoch: null, payload: {} } };
    }
    if (command.type === "answer_decision") {
      const decision = decisions.get(command.decisionId);
      if (!decision || !decision.wire.allowedOperations.includes("decisions.answer") || decision.wire.status !== "pending") return null;
      const { wire } = decision;
      const wireAnswer = (d: WireDecision, text: string, optionId: string | null | undefined, attachments: Message["attachments"]): WireAnswer | null => {
        const option = optionId === undefined ? d.options.find(o => o.label === text) : d.options.find(o => o.optionId === optionId);
        if (optionId && !option) return null;
        // References on the wire; the machine reads the bodies (the generated type still names the inline form).
        return { ...(attachments?.length ? { attachments: wireAttachments(attachments) as WireAnswer["attachments"] } : {}), decisionId: d.decisionId, expectedVersion: d.version, questionId: d.questionId, optionId: option?.optionId ?? null, text };
      };
      const answer = wireAnswer(wire, command.answer, command.optionId, command.attachments);
      if (!answer) return null;
      const members = wire.batchId === null ? [] : [...decisions.values()].filter(d => d.wire.batchId === wire.batchId && d.wire.executionId === wire.executionId);
      let answers = [answer];
      if (wire.batchId !== null && members.length > 1 && command.answers) {
        // The owner's one action carries every answer of the batch, so nothing waits in the app. The batch the machine
        // asked must be exactly the batch answered: a question added, missing or closed since refuses the whole send.
        held.delete(wire.batchId);
        const given = command.answers;
        if (given.length !== members.length || new Set(given.map(a => a.decisionId)).size !== given.length) return null;
        const built = members.map(d => { const a = given.find(x => x.decisionId === d.wire.decisionId); return a && d.wire.status === "pending" && d.wire.allowedOperations.includes("decisions.answer") ? wireAnswer(d.wire, a.answer, a.optionId, a.attachments) : null; });
        if (built.some(a => !a)) return null;
        answers = built as WireAnswer[];
      } else if (wire.batchId !== null && members.length > 1) {
        const batch = held.get(wire.batchId) ?? new Map<string, { answer: WireAnswer; identity: string }>();
        if (members.some(d => { const prior=batch.get(d.wire.decisionId); return prior && prior.identity !== answerIdentity(d.wire); })) {
          held.delete(wire.batchId);
          return null;
        }
        held.set(wire.batchId, batch.set(wire.decisionId, {answer,identity:answerIdentity(wire)}));
        if (members.some(d => !batch.has(d.wire.decisionId))) return "held";
        answers = members.map(d => ({...batch.get(d.wire.decisionId)!.answer,expectedVersion:d.wire.version}));
      }
      return { operation: "decisions.answer", message: { ...base, projectId: decision.projectId, target: wire.batchId ?? wire.decisionId, operation: "decisions.answer", expectedVersion: wire.version, ownerEpoch: wire.ownerEpoch, payload: { executionId: wire.executionId, acpSessionId: wire.acpSessionId, batchId: wire.batchId, answers } } };
    }
    return null;
  }

  const done = (requestId: string, entityId?: string): CommandReceipt => ({ requestId, ok: true, outcome: "recorded", message: "Recorded", ...(entityId ? { entityId } : {}) });
  const taskCall = async (projectId: string, params: unknown) => {
    const reply = await machine<{reclamation?:{vacuum_status?:string}}>("product.task",projectId,params);
    if(reply.reclamation?.vacuum_status==="failed") throw new Refusal("partial_effect","Task rows were deleted, but disk-space reclamation failed. Do not repeat deletion; run maintenance after closing readers.");
    return reply;
  };
  const sprintPatch = (projectId: string, sprintId: string, patch: Record<string, unknown> & Pick<Partial<Sprint>, "runMode">) => machine("product.sprint", projectId, { action: "patch", sprintId, patch });
  const profileCall = (verb: string, fields: Record<string, unknown>) => machine("config.action", undefined, { action: "profile", verb, expected_revision: configTokens.routing_profiles, ...fields });
  const connectionCall = (connection_id: string, operation: string, fields: Record<string, unknown> = {}) => machine("config.action", undefined, { action: "connection_update", expected_revision: configTokens.connections, connection_id, operation, ...fields });

  /** A command names files by reference only. Bytes still in a command (a file attached by an older path) are uploaded here, once, and replaced. */
  async function referenced(envelope: CommandEnvelope): Promise<CommandEnvelope> {
    const walk = async (value: unknown): Promise<unknown> => {
      if (Array.isArray(value)) return Promise.all(value.map(walk));
      if (!value || typeof value !== "object") return value;
      const out: Record<string, unknown> = {};
      for (const [key, v] of Object.entries(value)) {
        if (key === "attachments" && Array.isArray(v) && v.some(f => f && typeof f === "object" && "data" in f)) {
          const inline = v.filter((f): f is Attachment => !!f && typeof f === "object" && "data" in f);
          const refs = await machine<AttachmentRef[]>("product.attachment", undefined, {action:"upload", files:wireAttachments(inline)});
          if (!Array.isArray(refs) || refs.length !== inline.length) throw new Refusal("attachments_unavailable", "Attachments could not be saved. Choose the files again.");
          let next = 0;
          out[key] = v.map(f => f && typeof f === "object" && "data" in f ? refs[next++] : f);
        } else out[key] = await walk(v);
      }
      return out;
    };
    return { ...envelope, payload: await walk(envelope.payload) as WorkspaceCommand };
  }
  /** Everything past the wire. Each case is one real effect on the machine; a command with no case stays unsupported. */
  async function machineCommand(envelope: CommandEnvelope): Promise<CommandReceipt | null> {
    const { requestId } = envelope, command = envelope.payload;
    const ok = () => done(requestId);
    const withCleanup = (receipt: CommandReceipt, reply: unknown, prefix: string): CommandReceipt => {
      const cleanup = (reply as { worktreeCleanup?: { status?: string; reason?: string } } | null)?.worktreeCleanup;
      if (!cleanup) return receipt;
      const message = cleanup.status === "removed" ? "Worktree folder deleted." : cleanup.status === "retained"
        ? "Worktree folder was kept." : "Worktree folder deletion is not confirmed.";
      return { ...receipt, worktreeCleanupNotice: true, message: [prefix, message, plainReason(cleanup.reason, cleanup.reason ? "Safe cleanup could not be confirmed." : "")].filter(Boolean).join(" ") };
    };
    const task = (taskId: string) => { const id = projectOfTask(taskId); if (!id) throw new Refusal("unknown_task", "Unknown task."); return id; };
    // Screens apply machine result facts over the wire slice. Commands must use
    // that same identity, including an explicit null that clears an older result.
    const displayedResult = (taskId: string) => {
      const id = task(taskId), result = sent.get(id)?.overlays[taskId]?.result;
      return result !== undefined ? result : slices.get(id)?.tasks.find(t => t.id === taskId)?.result ?? undefined;
    };
    const execution = (taskId: string) => { const e = tasks.get(taskId)?.executionId; if (!e) throw new Refusal("not_started", "This task has not started."); return e; };
    const prompt = async (taskId: string, text: string, key = requestId, attachments?: Message["attachments"]) => { historyChanged(taskId); const r = await transport.request("executions.prompt", { projectId: task(taskId), executionId: execution(taskId), text, requestId:key, ...(attachments?.length ? {attachments:wireAttachments(attachments)} : {}) }) as {outcome?:string}; if (r.outcome !== "recorded") throw new Refusal("unknown_outcome", OUTCOME_WORDS.unknown_outcome); return ok(); };
    const control = async (taskId: string, params: { action: ControlAction } & Record<string, unknown>) => { historyChanged(taskId); const reply = await transport.request("executions.control", {projectId:task(taskId), executionId:execution(taskId), requestId, ...params}) as {outcome?:string;message?:string}; if (reply.outcome && !RECORDED.has(reply.outcome) && reply.outcome !== "stop_requested") throw new Refusal(reply.outcome, reply.message ?? OUTCOME_WORDS[reply.outcome] ?? "The action could not run. Refresh and try again."); return ok(); };
    const messageCall = async (taskId: string, params: Record<string, unknown>) => { await machine("product.message", task(taskId), params); return ok(); };
    const workerTask = (workerId: string) => { const id = workerTasks.get(workerId); if (!id) throw new Refusal("worker_unavailable", "This worker is not available."); return id; };
    const note = (taskId: string, text: string, delivery: "queued" | "recorded", attachments?: Message["attachments"]) => messageCall(taskId, {action:"add",message:{ id: requestId, taskId, workerId: null, text, mode: "queue", delivery, at: Date.now(), deliveredAt: null, failure: null, ...(attachments?.length ? { attachments: wireAttachments(attachments) } : {}) }});
    // The machine has the message by now: a copy that no longer fits must not turn a delivered message into a failed one.
    const keepSent = async (taskId: string, text: string, attachments?: Message["attachments"]) => { if (attachments?.length) await note(taskId, text, "recorded", attachments).catch(() => undefined); };
    // The runtime reopens a finished result; the saved card follows it, native or not.
    const say = async (taskId: string, text: string, attachments?: Message["attachments"]) => { await prompt(taskId, text, requestId, attachments); await keepSent(taskId, text, attachments); await taskCall(task(taskId), {action:"reopen",taskId}); return ok(); };
    const queued = (id: string) => { const at = messages.findIndex(m => m.id === id && m.delivery === "queued"); if (at < 0) throw new Refusal("unknown_message", "This message is no longer in the queue."); return at; };
    const git = async (checkoutId: string, params: Record<string, unknown>) => { await machine("git", projectOfCheckout(checkoutId), { requestId, checkoutId, ...params }); return ok(); };
    const local = async (patch: unknown) => { await machine("machine.update", undefined, { patch }); return ok(); };
    const dirty = async <T>(run: Promise<T>) => { try { return await run; } finally { configRead = null; } };
    switch (command.type) {
      case "open_editor": await transport.request("native.open_editor", {projectId:command.projectId, ...(command.checkoutId ? {checkoutId:command.checkoutId} : {}), ...(command.file ? {file:command.file} : {}), ...(command.line ? {line:command.line} : {})}); return ok();
      case "reveal_file": await transport.request("native.reveal_file", {projectId:command.projectId, checkoutId:command.checkoutId, file:command.file}); return ok();
      case "remove_project": {
        if (!command.confirmed) return {requestId,ok:false,outcome:"invalid_request",message:"Confirm removing the project first."};
        await transport.request("projects.remove", {projectId:command.projectId,confirmed:true});
        projects.splice(0,projects.length,...projects.filter(p => p.projectId !== command.projectId));
        slices.delete(command.projectId);
        return ok();
      }
      case "stop_task": return control(command.taskId, {action:"pause",expectedVersion:tasks.get(command.taskId)?.version});
      case "continue_task": return control(command.taskId, {action:"resume",expectedVersion:tasks.get(command.taskId)?.version});
      // A task the runtime holds for the owner (an interrupted turn, a stop it decided) continues from its recorded state:
      // the runtime's Retry, which acknowledges any outcome it could not confirm and hands the task back to the coordinator.
      case "retry_task": return control(command.taskId, {action:"retry"});
      // Worker guidance targets one attempt; task steering uses durable source capture.
      case "send_message":
        if (command.workerId) return control(command.taskId, {action:"worker_message",workerId:command.workerId,text:command.text,attachments:wireAttachments(command.attachments)});
        return command.mode === "queue" ? note(command.taskId, command.text, "queued", command.attachments) : say(command.taskId, command.text, command.attachments);
      case "stop_worker": return control(workerTask(command.workerId), {action:"worker_stop",workerId:command.workerId,reason:command.reason});
      // Lifts exactly one of the owner's own write denials; it grants nothing else.
      case "allow_write": return control(command.taskId, {action:"allow_write",path:command.path});
      case "stop_command": {
        const taskId = workerTask(command.workerId), worker = (await machine<Worker[]>("workers", task(taskId), {taskIds:[taskId]})).find(w => w.id === command.workerId);
        const ref = worker?.activity.find(a => a.sequence === command.sequence)?.toolCallRef;
        if (!ref) throw new Refusal("native_command", "Native engine actions have no stop receipt.");
        return control(taskId, {action:"command_stop",toolCallRef:ref,reason:command.reason});
      }
      case "edit_queued": {
        const m = messages[queued(command.messageId)];
        return messageCall(m.taskId, {action:"edit",messageId:m.id,text:command.text,...(command.attachments === undefined ? {} : {attachments:wireAttachments(command.attachments)})});
      }
      case "remove_queued": { const m = messages[queued(command.messageId)]; return messageCall(m.taskId, {action:"remove",messageId:m.id}); }
      case "move_queued": {
        const message = messages[queued(command.messageId)];
        return messageCall(message.taskId, {action:"move",messageId:message.id,before:command.before});
      }
      case "send_queued": {
        // It leaves the queue only once the machine has it, so a failed send never loses a message.
        const message = messages[queued(command.messageId)];
        await prompt(message.taskId, message.text, message.id, message.attachments);
        await messageCall(message.taskId, {action:"remove",messageId:message.id});
        await keepSent(message.taskId, message.text, message.attachments);
        return ok();
      }
      case "request_changes": return say(command.taskId, command.text, command.attachments);
      case "rework_task": {
        const { taskId, executionId, expectedVersion, ownerEpoch, text, attachments } = command;
        historyChanged(taskId);
        const result = await machine<{ done?: boolean; outcome?: string; state?: string } | null>("product.task", task(taskId), {
          action: "rework", taskId, requestId, executionId, expectedVersion, ownerEpoch, text,
          ...(attachments?.length ? { attachments: wireAttachments(attachments) } : {}),
        }).catch(error => {
          if (error instanceof Refusal) throw new Refusal(error.code, plainReason(error.message, plainReason(error.code, "The task could not be returned for rework.")));
          throw error;
        });
        if (result?.done !== true || result.outcome !== "recorded" || result.state !== "PLANNING") throw new Refusal("unknown_outcome", OUTCOME_WORDS.unknown_outcome);
        return ok();
      }
      case "rerun_check": {
        const displayed = displayedResult(command.taskId);
        const result = displayed !== undefined ? displayed : (await machine<Product>("product.read", task(command.taskId))).tasks[command.taskId]?.result;
        const check = result?.checks[command.checkIndex];
        if (!check || result?.digest !== command.digest) throw new Refusal("stale_result", "This result changed. Review it again.");
        if (check.kind === "manual" || check.kind === "policy" || check.rerunnable !== true || !check.command?.length) throw new Refusal("unsupported", "This item has no runnable command or needs owner authorization. Check it manually; it has not passed.");
        return control(command.taskId, {action:"rerun_check",checkIndex:command.checkIndex,checkName:check.name,digest:command.digest,resultVersion:result.sourceVersion});
      }
      case "add_comment": await machine("product.comment", task(command.taskId), {action:"add",taskId:command.taskId,comment:{id:requestId,path:command.path,line:command.line,text:command.text,attachments:wireAttachments(command.attachments),sent:false}}); return ok();
      case "edit_comment": await machine("product.comment", task(command.taskId), {action:"edit",taskId:command.taskId,commentId:command.commentId,text:command.text,...(command.attachments === undefined ? {} : {attachments:wireAttachments(command.attachments)})}); return ok();
      case "remove_comment": await machine("product.comment", task(command.taskId), {action:"remove",taskId:command.taskId,commentId:command.commentId}); return ok();
      case "send_comments": {
        const product = await machine<Product>("product.read", task(command.taskId));
        const pending = (product.comments?.[command.taskId] ?? []).filter(c => !c.sent);
        if (!pending.length) throw new Refusal("no_comments", "No draft comments to send.");
        await say(command.taskId, "Please address these review comments:\n\n" + pending.map(c => `${c.path}:${c.line}\n${c.text}`).join("\n\n"), wireAttachments(pending.flatMap(c => c.attachments ?? [])));
        await machine("product.comment", task(command.taskId), {action:"mark_sent",taskId:command.taskId,ids:pending.map(c => c.id)});
        return ok();
      }
      case "resolve_block": await taskCall(task(command.taskId), {action:"resolve_block",taskId:command.taskId,path:command.path,blockId:command.blockId,choice:command.choice}); return ok();
      case "finish_merge":
        await taskCall(task(command.taskId), {action:"finish_merge",taskId:command.taskId});
        return ok();
      // Bound to the result the owner reviewed; the machine merges the target into the task branch, never the other way.
      case "update_from_target": await machine("product.integrate", task(command.taskId), { requestId, taskId: command.taskId, method: "update", confirmed: command.confirmed, digest: command.digest }); return ok();
      case "abort_merge":
        await taskCall(task(command.taskId), {action:"abort_merge",taskId:command.taskId,
          ...(command.discardLegacy === true ? {discardLegacy:true,requestId} : {})});
        return ok();
      case "resolve_with_cortex":
      case "inspect_merge":
      case "request_merge_checks": {
        const action = command.type === "resolve_with_cortex" ? "handoff_merge" : command.type;
        // Keep the envelope identity. The service owns the manifest, dispatch and any retained outcome.
        const conflictId = command.conflictId;
        if (!conflictId && action !== "inspect_merge") throw new Refusal("stale_conflict", "This merge has no current conflict identity. Inspect merge state.");
        return control(command.taskId, {action, conflictId: conflictId ?? null, ...(command.type === "resolve_with_cortex" ? {
          ...(command.guidance !== undefined ? {text:command.guidance} : {}),
          ...(command.attachments?.length ? {attachments:wireAttachments(command.attachments)} : {}),
        } : {})});
      }
      case "stop_resolution": await transport.request("executions.cancel", {projectId:task(command.taskId),executionId:execution(command.taskId)}); return ok();
      case "set_task_settings": {
        const id = task(command.taskId), current = (await machine<Product>("product.read", id)).tasks[command.taskId]?.settings;
        const settings = { ...(current ?? {model:"",effort:"",mode:"guarded",language:"auto"}), ...command.settings };
        if (tasks.get(command.taskId)?.executionId) return control(command.taskId, {action:"task_settings",settings});
        await taskCall(id, {action:"settings",taskId:command.taskId,settings});
        return ok();
      }
      case "edit_task": {
        const id = task(command.taskId);
        const reply = await machine<{version:string}>("product.task", id, {action:"edit",taskId:command.taskId,expectedVersion:command.expectedVersion,
          payload:{title:command.title,description:command.description,attachments:wireAttachments(command.attachments),...(command.settings.model ? {settings:command.settings} : {}),checkout:command.checkout}});
        const current = tasks.get(command.taskId);
        if (current) tasks.set(command.taskId, {...current,version:reply.version});
        return ok();
      }
      case "create_task": {
        if (!command.sprintId) return null;
        const sprint = (await machine<Product>("product.read", command.projectId)).sprints.find(s => s.id === command.sprintId);
        if (!sprint || sprint.archivedAt || sprint.planning !== "created") throw new Refusal("sprint_unavailable", "Choose an open sprint with created tasks.");
        const made = await send({...envelope,payload:{...command,checkout:sprint.checkout ?? command.checkout}});
        if (!made.ok || !made.entityId) return made;
        try { await taskCall(command.projectId, {action:"sprint",taskId:made.entityId,sprintId:command.sprintId,dependencies:[]}); }
        catch { return {...made,ok:false,outcome:"partial_effect",message:"The task was saved outside the sprint, but joining the sprint was not confirmed. Refresh the project; do not create it again."}; }
        return made;
      }
      case "delete_sprint": {
        const reply = await machine<{done:boolean;reason?:string;taskIds?:string[];keptPaths?:string[]}>("product.sprint", command.projectId,
          {action:"delete",sprintId:command.sprintId,confirmed:command.confirmed});
        invalidated.add(command.projectId);
        const cached = repoReads.get(command.projectId);
        if (cached) repoReads.set(command.projectId, {...cached,at:-Infinity,running:false});
        return {...ok(),ok:reply.done,outcome:reply.done ? "recorded" : "partial_effect",
          message:reply.done ? (reply.keptPaths?.length ? "Sprint deleted. Kept worktrees: " + reply.keptPaths.join(", ") : "Sprint and tasks deleted.")
            : (reply.reason ?? "Deletion is incomplete. Retry Delete sprint.") + (reply.keptPaths?.length ? " Kept worktrees: " + reply.keptPaths.join(", ") : ""),
          deletedTaskIds:reply.taskIds,keptPaths:reply.keptPaths};
      }
      case "close_sprint": await machine("product.sprint", await projectOfSprint(command.sprintId), {action:"close",sprintId:command.sprintId,confirmed:command.confirmed}); return ok();
      case "rename_task": {
        const reply = await machine<{version?:string}>("product.task", task(command.taskId), {action:"rename",taskId:command.taskId,title:command.title});
        const current = tasks.get(command.taskId);
        if (current && nativeVersion(reply.version) && BigInt(reply.version!) > BigInt(current.version)) tasks.set(command.taskId, {...current,version:reply.version!});
        return ok();
      }
      case "generate_title": {
        // The task is already saved under its description label; the machine names it in the background.
        const id = task(command.taskId);
        void shortText(id, { action: "generate_title", taskId: command.taskId }).then(written => {
          if (!written) return;
          invalidated.add(id);
          for (const notify of observers) notify({ kind: "refresh" });
        });
        return ok();
      }
      case "archive_task": await taskCall(task(command.taskId), { action: "archive", taskId: command.taskId }); return ok();
      case "confirm_discard": await taskCall(task(command.taskId), { action: "confirm_discard", taskId: command.taskId }); return ok();
      case "restore_task": await taskCall(task(command.taskId), { action: "restore", taskId: command.taskId }); return ok();
      case "purge_task": await taskCall(task(command.taskId), { action: "purge", taskId: command.taskId, confirmed: command.confirmed }); return ok();
      case "clear_old": {
        if (!command.confirmed || !command.taskIds?.length) throw new Refusal("confirmation_required", "Preview and confirm the exact tasks first.");
        let cleared = 0;
        for (const taskId of command.taskIds) {
          try { await taskCall(task(taskId), {action:"clear_old", taskId, cutoff:command.cutoff, confirmed:true}); cleared++; }
          catch { throw new Refusal("partial_effect", `${cleared} tasks cleared; remaining tasks were not confirmed. Refresh the preview before retrying.`); }
        }
        return ok();
      }
      case "purge_many": for (const id of command.taskIds) await taskCall(task(id), { action: "purge", taskId: id, confirmed: command.confirmed }); return ok();
      case "close_task": {
        if (command.choice === "remove_worktree") {
          if (!taskWorktrees) throw new Refusal("capability_unavailable", "Task worktree cleanup is not supported by this service.");
          const reply = await taskCall(task(command.taskId), { action: "close_remove_worktree", taskId: command.taskId, confirmed: command.confirmed });
          return withCleanup(ok(), reply, "Task closed.");
        }
        if (command.choice === "unconfirmed") {
          await taskCall(task(command.taskId), { action: "close_unconfirmed", taskId: command.taskId,
            executionId: command.executionId, expectedVersion: command.expectedVersion, confirmed: command.confirmed });
          return ok();
        }
        const executionId = tasks.get(command.taskId)?.executionId;
        const close = { action: "close", taskId: command.taskId, choice: command.choice, confirmed: command.confirmed };
        const runtime = !!executionId && command.choice !== "external" && command.choice !== "already_in_target";
        if (runtime) {
          // The saved task's own close is checked first (it writes nothing), so its refusal never leaves the run
          // closed and the task open.
          await taskCall(task(command.taskId), { ...close, preflight: true });
          await control(command.taskId, {action:"close_task",confirmed:command.confirmed,expectedVersion:tasks.get(command.taskId)?.version});
        }
        // The execution records the owner's resolution; the card's closed disposition is the saved task's own.
        try { await taskCall(task(command.taskId), close); }
        catch (error) {
          // The run is already closed: say so, so closing again (which the run accepts) finishes the task.
          if (runtime && error instanceof Refusal) throw new Refusal(error.code, `${error.message} The task's run is already closed; close the task again to finish it.`);
          throw error;
        }
        return ok();
      }
      case "accept_result": {
        const result = displayedResult(command.taskId);
        if (!result?.sourceVersion || result.digest !== command.digest || (command.sourceVersion !== undefined && command.sourceVersion !== result.sourceVersion) ||
            (command.automaticFinish && !command.sourceVersion)) throw new Refusal("stale_result", "This result changed. Review it again.");
        await taskCall(task(command.taskId), {
          action: "accept", taskId: command.taskId, digest: command.digest,
          version: result.sourceVersion,
          acknowledgeOpenChecks: command.openChecks === true,
          ...(command.automaticFinish ? { automaticFinish: command.automaticFinish } : {}),
        });
        return ok();
      }
      case "integrate": { const reply = await machine("product.integrate", task(command.taskId), { taskId: command.taskId, method: command.method, confirmed: command.confirmed, digest: command.digest, targetBranch: command.targetBranch, message: command.method === "pr" ? command.title ?? "" : command.message ?? "", body: command.message ?? "", draft: command.draft, deleteWorktree: command.deleteWorktree }); return withCleanup(ok(), reply, "Integration recorded."); }
      case "set_target":
        if (command.taskId) { await taskCall(task(command.taskId), { action: "target", taskId: command.taskId, branch: command.branch }); return ok(); }
        await sprintPatch(await projectOfSprint(command.sprintId!), command.sprintId!, { targetBranch: command.branch });
        return ok();
      case "git_stage": return git(command.checkoutId, { action: "stage", path: command.path, staged: command.staged, hunk: command.hunk });
      case "git_revert": return git(command.checkoutId, { action: "revert", path: command.path, hunk: command.hunk, confirmed: command.confirmed });
      case "git_draft": localDrafts.set(command.checkoutId, command.message); return { ...ok(), outcome: "local" };
      case "git_commit": { const result = await git(command.checkoutId, { action: "commit", message: command.message, amend: command.amend }); if (localDrafts.get(command.checkoutId) === command.message) localDrafts.delete(command.checkoutId); return result; }
      case "git_fetch": await machine("git", command.projectId, { requestId, action: "fetch" }); return ok();
      // One native owner operation, bound to the command identity; never replay an uncertain Git effect.
      case "git_init": await machine("git", command.projectId, { requestId, action: "git_init", confirmed: command.confirmed }); return ok();
      case "git_pull": return git(command.checkoutId, { action: "pull", ...(command.mode ? { mode: command.mode } : {}) });
      case "git_pull_continue": return git(command.checkoutId, { action: "pull_continue" });
      case "git_pull_abort": return git(command.checkoutId, { action: "pull_abort", confirmed: command.confirmed });
      case "git_push": return git(command.checkoutId, { action: "push", remote: command.remote, branch: command.branch, confirmed: command.confirmed, ...(command.summary !== undefined ? { summary: command.summary } : {}) });
      case "create_branch": await machine("git", command.projectId, { requestId, action: "branch", name: command.name, at: command.at }); return ok();
      case "revert_commit": return git(command.checkoutId, { action: "revert_commit", sha: command.sha, confirmed: command.confirmed });
      case "delete_worktree": return git(command.checkoutId, { action: "delete_worktree", confirmed: command.confirmed, discard: command.discard });
      case "rename_machine": return local({ name: command.name });
      case "set_retention": return local({ retentionDays: command.days });
      case "save_storage": return local({
        retentionDays: command.retentionDays, autoArchiveDays: command.autoArchiveDays,
        automaticArchivePurge: command.automaticArchivePurge,
        unreferencedAttachmentRetentionDays: command.unreferencedAttachmentRetentionDays,
      });
      case "set_worktree_root": return local({ worktreeRoot: command.path });
      case "open_logs": await transport.request("native.open_logs", {}); return ok();
      case "open_pull_request": await transport.request("native.open_pull_request", { projectId: command.projectId, taskId: command.taskId }); return ok();
      case "run_diagnostics": await machine("diagnostics.run"); return ok();
      case "run_cleanup": await machine("cleanup.run"); return ok();
      case "save_surface":
        if (command.projectId) {
          const tokens=projectConfigBases.get(`${command.projectId}:${command.surface}:${command.revision}`);
          if (!tokens) throw new Refusal("stale_version","These settings changed. Refresh and review your draft before saving.");
          await dirty(machine("project.config.save", command.projectId, {surface:command.surface,value:command.value,tokens})); return ok();
        }
        if (command.surface === "project_ceiling") return unsupported(requestId);
        {
          const tokens = command.revision === undefined ? configTokens : configBases.get(`${command.surface}:${command.revision}`);
          if (!tokens) throw new Refusal("stale_version", "These settings changed. Refresh and review your draft before saving.");
          await dirty(machine("config.save", undefined, { surface: command.surface, value: command.value, tokens }));
        }
        return ok();
      case "activate_profile": await dirty(profileCall("activate", { name: command.profileId, expected_revisions: configTokens.profile_sections, discard_modifications: command.keep === "discard" })); return ok();
      case "save_profile": await dirty(profileCall("save", { name: configRead?.config.active, expected_revisions: configTokens.profile_sections })); return ok();
      case "revert_profile": await dirty(profileCall("activate", { name: configRead?.config.active, expected_revisions: configTokens.profile_sections, discard_modifications: true })); return ok();
      case "deactivate_profile": await dirty(profileCall("deactivate", {})); return ok();
      case "create_profile": await dirty(profileCall("create", { name: command.name, description: command.description, expected_revisions: configTokens.profile_sections })); return ok();
      case "rename_profile": await dirty(profileCall("update", { name: command.profileId, rename_to: command.name })); return ok();
      case "describe_profile": await dirty(profileCall("update", { name: command.profileId, description: command.description })); return ok();
      case "delete_profile": await dirty(profileCall("delete", { name: command.profileId })); return ok();
      case "apply_preset": await dirty(machine("config.action", undefined, { action: "apply_default_preset" })); return ok();
      case "reset_preset": await dirty(machine("config.action", undefined, { action: "reset_default_preset", confirm: command.confirmed })); return ok();
      case "add_connection": await dirty(machine("engine.connection", undefined, {action:"recheck", engine: command.engine === "codex" ? "codex-acp" : "claude-agent-acp"})); return ok();
      case "rename_connection": await dirty(connectionCall(command.connectionId, "rename", { name: command.name })); return ok();
      case "set_sessions": await dirty(connectionCall(command.connectionId, "set_max_concurrent_sessions", { max_concurrent_sessions: command.sessions })); return ok();
      case "remove_connection": await dirty(connectionCall(command.connectionId, "remove")); return ok();
      case "check_connection": await dirty(connectionCall(command.connectionId, "validate")); return ok();
      case "refresh_models": await dirty(machine("config.action", undefined, { action: "refresh" })); return ok();
      case "download_engine": await dirty(machine("config.action", undefined, { action: "install_engine", engine: command.engine })); return ok();
      case "enable_opencode": await dirty(machine("config.action", undefined, { action: "onboard_opencode", expected_revision: configTokens.connections, explicit_opt_in: true, name: command.name })); return ok();
      case "create_sprint": {
        const available = await machine<ConfigRead>("config.read");
        const selected = command.settings?.model?.trim();
        if (!selected || !sessionOptions(available.config).models.some(m => m.id === selected)) throw new Refusal("model_required", "Connect an engine and select an available model before starting sprint planning. Your description is kept.");
        const { sprintId } = await machine<{ sprintId: string }>("product.sprint", command.projectId, { action: "create", description: command.description, attachments: wireAttachments(command.attachments), runMode: command.runMode, checkout: command.checkout, settings: command.settings, targetBranch: command.targetBranch });
        sprintProjects.set(sprintId, command.projectId);
        // The service plans the sprint: in a read-only planning task in the project folder (`planningTaskId`), or, for the
        // description-only planner, without a runtime execution or project tools.
        return done(requestId, sprintId);
      }
      case "set_sprint_settings": {
        const id = await projectOfSprint(command.sprintId), sprint = (await machine<Product>("product.read", id)).sprints.find(s => s.id === command.sprintId)!;
        if (sprint.materializedVersion !== null) throw new Refusal("sprint_started", "Tasks already exist. Change each task's route explicitly; the sprint route will not overwrite them.");
        const settings = {model:"",effort:"",mode:"guarded" as const,language:"auto" as const,...sprint.settings,...command.settings};
        if (sprint.planningTaskId && tasks.get(sprint.planningTaskId)?.executionId) {
          await control(sprint.planningTaskId, {action:"task_settings",settings});
        }
        await sprintPatch(id, command.sprintId, {settings});
        return ok();
      }
      case "rename_sprint": await sprintPatch(await projectOfSprint(command.sprintId), command.sprintId, { title: command.title }); return ok();
      case "set_run_mode": await sprintPatch(await projectOfSprint(command.sprintId), command.sprintId, { runMode: command.runMode }); return ok();
      case "run_sprint": await sprintPatch(await projectOfSprint(command.sprintId), command.sprintId, { queueState: "running", pauseReason: null }); return ok();
      case "allow_queue_repair": await machine("product.sprint", await projectOfSprint(command.sprintId), { action: "allow", sprintId: command.sprintId, taskId: command.taskId, code: command.code, holdId: command.holdId ?? null, ownerAction: "allow_repair" }); return ok();
      case "pause_sprint": await sprintPatch(await projectOfSprint(command.sprintId), command.sprintId, { queueState: "paused", pauseReason: "Paused by you" }); return ok();
      case "cancel_planning": {
        const id = await projectOfSprint(command.sprintId), sprint = (await machine<Product>("product.read", id)).sprints.find(s => s.id === command.sprintId)!;
        if (sprint.planningTaskId && tasks.get(sprint.planningTaskId)?.executionId) await control(sprint.planningTaskId, {action:"pause",expectedVersion:tasks.get(sprint.planningTaskId)?.version});
        await sprintPatch(id, command.sprintId, { planning: "cancelled" }); return ok();
      }
      case "resume_planning": {
        const id = await projectOfSprint(command.sprintId), sprint = (await machine<Product>("product.read", id)).sprints.find(s => s.id === command.sprintId)!;
        // Saved failures (including completed without a plan) are recovered by the sprint service.
        // With no diagnostic, Stop paused the execution: do not change the sprint if its Resume is refused.
        if (!sprint.planningFailure && sprint.planningTaskId && tasks.get(sprint.planningTaskId)?.executionId) await control(sprint.planningTaskId, {action:"resume",expectedVersion:tasks.get(sprint.planningTaskId)?.version});
        await sprintPatch(id, command.sprintId, { planning: "planning" }); return ok();
      }
      case "edit_plan": {
        const id = await projectOfSprint(command.sprintId), sprint = (await machine<Product>("product.read", id)).sprints.find(s => s.id === command.sprintId)!;
        validatePlan(command.items);
        const plan = { version: (sprint.plan?.version ?? 0) + 1, items: command.items };
        await sprintPatch(id, command.sprintId, { plan, history: [...(sprint.history ?? []), plan], planning: "ready" });
        return ok();
      }
      case "materialize_plan": {
        // One service operation creates every task of this plan version, with its route, checkout, title and
        // dependencies, together with the sprint's own change to created; or nothing at all. A repeat answers
        // with the tasks already created, never a second set.
        const id = await projectOfSprint(command.sprintId);
        const reply = await machine<{ created: number; taskIds: string[]; strayTaskIds?: string[] }>("product.sprint", id,
          { action: "materialize", sprintId: command.sprintId, planVersion: command.version, requestId });
        // A task just created can be started before its first projection.
        if (reply.created > 0) for (const taskId of reply.taskIds) if (!tasks.has(taskId)) tasks.set(taskId, { projectId: id, version: "1" });
        const strays = reply.strayTaskIds?.length ?? 0;
        return strays ? { ...ok(), message: `Tasks created. ${strays} task${strays === 1 ? "" : "s"} left by an earlier unfinished Create tasks ${strays === 1 ? "is" : "are"} not part of the sprint; delete ${strays === 1 ? "it" : "them"} on the Board.` } : ok();
      }
      case "replace_planning_request": {
        const id = await projectOfSprint(command.sprintId);
        const reply = await machine<{done?: boolean}>("product.sprint", id, {action:"replace_planning_request",sprintId:command.sprintId,description:command.description});
        if (reply?.done !== true) throw new Refusal("unknown_outcome", OUTCOME_WORDS.unknown_outcome);
        return ok();
      }
      case "answer_planning": case "plan_message": {
        const id = await projectOfSprint(command.sprintId), sprint = (await machine<Product>("product.read", id)).sprints.find(s => s.id === command.sprintId)!;
        const taskId = sprint.planningTaskId, text = command.type === "answer_planning" ? command.answer : command.text;
        const linked = taskId ? tasks.get(taskId) : undefined;
        const terminal = linked?.runtimeState === "COMPLETED" || linked?.runtimeState === "FAILED" || linked?.runtimeState === "CANCELLED";
        // A failed start may leave a linked draft without an execution. Let the service check
        // that it is safe to recover and save the amendment as part of the same write.
        if (command.type === "plan_message" && sprint.planning === "cancelled" && sprint.planningFailure
            && (classifiedPlanningFailure(sprint) || !linked?.executionId || terminal)) {
          const reply = await machine<{done?: boolean}>("product.sprint", id, {action:"recover_planning_message",sprintId:command.sprintId,text,attachments:wireAttachments(command.attachments)});
          if (reply?.done !== true) throw new Refusal("unknown_outcome", OUTCOME_WORDS.unknown_outcome);
          return ok();
        }
        if (!taskId) {
          await machine("product.sprint", id, {action:"note",sprintId:command.sprintId,kind:command.type === "answer_planning" ? "answer" : "message",text,attachments:wireAttachments(command.attachments)});
          await sprintPatch(id, command.sprintId, {planning:"planning"});
          return ok();
        }
        const execution = tasks.get(taskId)?.executionId;
        if (command.type === "answer_planning") {
          // The owner answers the questions named on the form; a planning question without a record answers the open one.
          const named = command.decisionId ? decisions.get(command.decisionId) : undefined;
          const asked = named?.wire.status === "pending" && named.wire.executionId === execution ? named : command.decisionId ? undefined : [...decisions.values()].find(d => d.wire.status === "pending" && !isHeld(d.wire) && d.wire.executionId === execution);
          if (!asked) throw new Refusal("superseded", "This question is no longer open. Refresh before answering.");
          // Questions asked together go as one answer of their batch, from the one owner action that answered them all.
          const batch = command.answers && command.answers.length > 1 ? command.answers : undefined;
          const receipt = await send({ requestId, operation: "answer_decision", expectedVersion: null, payload: { type: "answer_decision", taskId, decisionId: asked.wire.decisionId, answer: text, attachments: command.attachments, ...(command.optionId === undefined ? {} : { optionId: command.optionId }), ...(batch ? { answers: batch } : {}) } });
          // The machine has the answers by now: a conversation copy that cannot be saved must not turn them into a failed send.
          if (receipt.ok) for (const a of batch ?? [{ answer: text, attachments: command.attachments }]) await machine("product.sprint", id, {action:"note",sprintId:command.sprintId,kind:"answer",text:a.answer,attachments:wireAttachments(a.attachments)}).catch(() => undefined);
          return {...receipt,requestId};
        }
        await machine("product.sprint", id, {action:"note",sprintId:command.sprintId,kind:"message",text,attachments:wireAttachments(command.attachments)});
        // A change request reopens the same planning task; Create tasks waits for its revised plan.
        await prompt(taskId, text, requestId, command.attachments);
        if (sprint.planning === "ready" || sprint.planning === "cancelled") await sprintPatch(id, command.sprintId, { planning: "planning" });
        return ok();
      }
      default: return null;
    }
  }

  async function send(envelope: CommandEnvelope): Promise<CommandReceipt> {
    const wire = wireCommand(envelope);
    if (envelope.payload.type === "answer_decision") historyChanged(envelope.payload.taskId);
    if (!wire) return unsupported(envelope.requestId);
    if (wire === "held") return done(envelope.requestId);
    // The wire names files by reference; the machine checks each reference and reads its body before its own
    // schema check. Here the rest of the command is checked with each reference standing in as an empty body.
    validate("Command", referencesAsBodies(wire.message));
    // Whatever the machine says, a batch was sent, so its questions read as themselves again until a projection says otherwise.
    const sentBatch = (wire.message as { payload?: { batchId?: string | null } }).payload?.batchId;
    if (sentBatch) held.delete(sentBatch);
    let reply: unknown;
    try { reply = await transport.request(wire.operation, wire.message); }
    catch (error) { return refusalOf(envelope.requestId, error); }
    const receipt = decodeResponse("Receipt", reply).value;
    if (receipt.requestId !== envelope.requestId) return {requestId:envelope.requestId,ok:false,outcome:"unknown_outcome",message:OUTCOME_WORDS.unknown_outcome};
    const result = toReceipt(envelope.requestId, receipt, wire.operation);
    // A task just created can be started before its first projection, so its version is kept from the receipt.
    if (result.ok && envelope.payload.type === "create_task") tasks.set(receipt.target, { projectId: envelope.payload.projectId, version: receipt.entityVersion });
    // The machine recorded these answers: the questions read as answered at once, so the board, the sidebar and the Inbox
    // stop saying "Needs you" with the reconciling read that follows every command, not when a details read next completes.
    if (result.ok && wire.operation === "decisions.answer") {
      const { projectId, payload } = wire.message as { projectId: string; payload: { answers: WireAnswer[] } };
      const answered = new Map(payload.answers.map(a => [a.decisionId, a.text]));
      // The files each answer carried read with it at once, as the machine's next read reports them.
      const files = new Map(payload.answers.map(a => [a.decisionId, a.attachments?.length ? a.attachments : undefined]));
      const at = new Date().toISOString();
      const mark = (d: WireDecision): WireDecision => answered.has(d.decisionId) && d.status === "pending" ? { ...d, status: "answered", answerText: answered.get(d.decisionId), answeredAt: at, ...(files.get(d.decisionId) ? { answerAttachments: files.get(d.decisionId) } : {}) } : d;
      wireDecisions.set(projectId, (wireDecisions.get(projectId) ?? []).map(mark));
      for (const [id, held] of decisions) if (answered.has(id)) decisions.set(id, { ...held, wire: mark(held.wire) });
    }
    return result;
  }
  const toReceipt = (requestId: string, wire: WireReceipt, operation: string): CommandReceipt => ({
    requestId, ok: RECORDED.has(wire.outcome), outcome: wire.outcome, message: wire.reason ?? (operation === "tasks.start" && wire.outcome === "busy" ? "This task was already started. Refresh to see its execution." : OUTCOME_WORDS[wire.outcome] ?? OUTCOME_WORDS.unknown), entityId: wire.target, entityVersion: wire.entityVersion,
  });
  /** One answer to a sudo prompt, sent once. The message is not kept; a refusal comes back in words, never with the message. */
  const SUDO_WORDS: Record<string, string> = {
    unknown_sudo_prompt: "This prompt already closed: it was answered, cancelled or expired, or its command ended.",
    invalid_sudo_password: "The password could not be sent: it is empty, too long or spans more than one line.",
    remote_operation_refused: "Passwords and tokens from phones are turned off on the computer. Answer on the computer, or cancel.",
  };
  /** Owner actions: words for the service's refusals. A token is sent once and never kept or echoed. */
  const OWNER_WORDS: Record<string, string> = {
    unknown_owner_action: "This action already closed: it was resolved or its task ended.",
    owner_token_refused: "The sign-in did not accept this token. Check the token and try again.",
    owner_token_unsupported: "This service has no sign-in with a token. Use the terminal sign-in instead.",
    invalid_owner_token: "The token could not be sent: it is empty, too long or spans more than one line.",
    project_not_found: "This project is no longer on the machine.",
    remote_operation_refused: "Passwords and tokens from phones are turned off on the computer. Do this on the computer.",
  };
  const ownerAct = async (message: OwnerActionRequests["owner_actions.act"]) => {
    if (!serviceMethods.includes("owner_actions.act")) throw new Refusal("unsupported", "This machine cannot take this action.");
    let reply: unknown;
    try { reply = await transport.request("owner_actions.act", message); }
    catch (error) {
      const text = error instanceof Error ? error.message : String(error);
      const code = Object.keys(OWNER_WORDS).find(key => text.includes(key));
      throw new Refusal("refused", code ? OWNER_WORDS[code] : "The machine did not take this action.");
    }
    return readOwnerActionReply(reply);
  };
  const sudoAnswer = async (message: { promptId: string; password: string } | { promptId: string; cancel: true }) => {
    if (!serviceMethods.includes("askpass.answer")) throw new Refusal("unsupported", "This machine cannot take a sudo password.");
    try { await transport.request("askpass.answer", message); }
    catch (error) {
      const text = error instanceof Error ? error.message : String(error);
      const code = Object.keys(SUDO_WORDS).find(key => text.includes(key));
      throw new Refusal("refused", code ? SUDO_WORDS[code] : "The machine did not take the answer.");
    }
  };

  return {
    mode: "real",
    query<O extends QueryOperation>(operation: O, parameters: QueryParameters[O]): Promise<QueryResults[O]> {
      if (operation === "capabilities.get") return capabilitiesGet() as Promise<QueryResults[O]>;
      if (operation === "machine.facts") return (machineOn ? machineFacts().then(f => [f]) : Promise.resolve([])) as Promise<QueryResults[O]>;
      if (operation === "machine.overview") return (machineOn ? machineOverview().then(f => [f]) : Promise.resolve([])) as Promise<QueryResults[O]>;
      const { projectId } = parameters as QueryParameters["project.snapshot"];
      // A refresh caused by one project's events rereads that project, not every project on the machine.
      if (operation === "project.snapshot" && !projectId) { readScope = (parameters as QueryParameters["project.snapshot"]).full ? new Set() : invalidated; invalidated = new Set(); }
      const wanted = (p: ProjectLocator) => projectId ? p.projectId === projectId : !readScope.size || readScope.has(p.projectId);
      if (operation === "project.facts") {
        presentationRead++;
        const facts = machineOn ? projectReads(projects.filter(wanted), "facts", async p => {
          // An independent facts request must not expose sprint membership while
          // the first coherent board is still waiting for its task read.
          if (serviceMethods.includes("native.board")) {
            await presentations.get(p.projectId);
            if (!boardTasks.has(p.projectId)) throw new BoardPending();
          }
          const prepared = presentationFacts.get(p.projectId);
          presentationFacts.delete(p.projectId);
          const { fresh, git } = parameters as QueryParameters["project.facts"];
          return prepared && !fresh ? Promise.resolve(prepared) : factsRead(p, fresh, git !== false);
        }, facts => ({kind:"facts",facts})) : Promise.resolve([]);
        if (!projectId) readScope = new Set();
        return facts.then(all => { for (const f of all) sent.set(f.projectId, f); return all; }).finally(() => { presentationRead++; }) as Promise<QueryResults[O]>;
      }
      const epoch = commandEpoch;
      return projectReads(projects.filter(wanted), "snapshot", p => {
        const work = (async () => {
        const update = await snapshotOf(p);
        // Do not expose planning/purged rows without their owner overlays.
        if (machineOn) {
          const facts = serviceMethods.includes("native.board") ? await boardFacts(p) : await factsRead(p, false, showsGit(p.projectId));
          if (epoch === commandEpoch) presentationFacts.set(p.projectId, facts);
        }
        if (serviceMethods.includes("native.board")) {
          if (update.scope === "project") boardTasks.set(p.projectId,new Set(update.slice.tasks.map(t => t.id)));
          queueEnrichment(p);
        }
        return update;
        })();
        presentations.set(p.projectId,work);
        void work.finally(() => { if (presentations.get(p.projectId) === work) presentations.delete(p.projectId); }).catch(() => {});
        return work;
      }, update => {
        if (update.scope === "project") {
          const facts = presentationFacts.get(update.slice.project.id);
          if (facts) { sent.set(facts.projectId, facts); for (const notify of observers) notify({kind:"facts",facts}); }
        }
        return update;
      }) as Promise<QueryResults[O]>;
    },
    async command(envelope) {
      // A title job only starts here and changes nothing yet: reads under way stay current. Its end rereads the project.
      if (envelope.payload.type !== "generate_title") {
        commandEpoch++;
        presentationFacts.clear();
        factReads.clear();
      }
      try { envelope = await referenced(envelope); }
      catch (error) { return error instanceof Refusal ? { requestId: envelope.requestId, ok: false, outcome: error.code, message: error.message } : refusalOf(envelope.requestId, error); }
      const command = envelope.payload;
      if (options.registry && (command.type === "add_project" || command.type === "activate_project")) {
        const operation = command.type === "add_project" ? "projects.register" : "projects.activate";
        if (!serviceMethods.includes(operation)) return unsupported(envelope.requestId);
        if (command.type === "add_project" && !/^(\/|~\/|~$)/.test(command.repository.trim())) return { requestId: envelope.requestId, ok: false, outcome: "refused", message: "Enter the full path of the folder, starting with / or ~/." };
        try {
          const reply = await transport.request(operation, command.type === "add_project" ? {root:command.repository.trim(),displayName:command.name.trim(),...(command.createFolder ? {create:true} : {}),...(command.newFolder ? {newFolder:true} : {})} : {projectId:command.projectId}) as {projectId?: string; ready?: boolean};
          if (command.type === "activate_project") {
            configRead = null;
            const status = await transport.request("projects.status", {projectId:command.projectId}) as {ready?: boolean};
            if (status.ready !== true) return {requestId:envelope.requestId,ok:false,outcome:"unavailable",message:"Activation is not ready. Refresh the project before trying again."};
          }
          if (command.type === "add_project") validate("Identity",reply.projectId);
          return {requestId:envelope.requestId,ok:true,outcome:"recorded",message:"Recorded",entityId:command.type === "add_project" ? reply.projectId : command.projectId};
        } catch (error) { return refusalOf(envelope.requestId, error); }
      }
      if (options.registry && command.type === "rename_project") {
        if (!serviceMethods.includes("projects.rename")) return unsupported(envelope.requestId);
        try {
          await transport.request("projects.rename", {projectId:command.projectId,displayName:command.name.trim()});
          for (const notify of observers) notify({kind:"refresh"});
          return {requestId:envelope.requestId,ok:true,outcome:"recorded",message:"Recorded",entityId:command.projectId};
        } catch (error) { return refusalOf(envelope.requestId, error); }
      }
      if (machineOn) {
        try {
          const handled = await machineCommand(envelope);
          if (handled) return handled;
        } catch (error) {
          // A refusal is the machine's answer. A lost reply says nothing about what happened, so it stays unknown.
          if (error instanceof Refusal) return { requestId: envelope.requestId, ok: false, outcome: error.code, message: error.message };
          return refusalOf(envelope.requestId, error);
        } finally {
          if (["resolve_with_cortex", "inspect_merge", "request_merge_checks", "finish_merge", "abort_merge", "stop_resolution"].includes(command.type)) {
            for (const notify of observers) notify({kind:"refresh"});
          }
        }
      }
      return send(envelope);
    },
    async discoverMcp(server) {
      if (!machineOn) return {ok:false,code:"offline",message:"The machine service is unavailable."};
      return machine("config.mcp.discover", undefined, {server});
    },
    async draftIntegrationText(request) {
      const id = machineOn ? projectOfTask(request.taskId) : null;
      if (!id) return null;
      const written = await shortText(id, { action: "draft_integration_text", taskId: request.taskId, method: request.method, targetBranch: request.targetBranch,
        ...(request.paths ? { paths: request.paths } : {}), ...(request.summary ? { summary: request.summary } : {}) });
      return integrationText(written);
    },
    async draftCheckoutCommit({ checkoutId }) {
      if (!machineOn) return null;
      const id = projectOfCheckout(checkoutId);
      return integrationText(await shortText(id, { action: "draft_checkout_commit", checkoutId }));
    },
    async draftPushSummary({ checkoutId, remote, branch }) {
      if (!machineOn) return null;
      const id = projectOfCheckout(checkoutId);
      return integrationText(await shortText(id, { action: "draft_push_summary", checkoutId, remote, branch }));
    },
    async takeDroppedFiles() {
      if (!serviceMethods.includes("native.take_dropped_files")) return [];
      return wireAttachments(await transport.request("native.take_dropped_files", {}) as Message["attachments"]) as Attachment[];
    },
    async pickFolder() {
      if (!serviceMethods.includes("native.pick_folder")) return null;
      return ((await transport.request("native.pick_folder", {})) as { path?: string | null }).path ?? null;
    },
    async machineAction(op, params) {
      if (op === "native.engine_login") return await transport.request(op, params) as Record<string, unknown>;
      if (op === "engine.connection" && params.action === "add_key") params = {...params, revision: configTokens.connections};
      const result = await machine<Record<string, unknown>>(op, undefined, params);
      if (op === "engine.connection" && params.action !== "login_command") { configRead = null; for (const listener of observers) listener({kind:"refresh"}); }
      return result;
    },
    async searchDirectories({ machineId, session, query, limit }) {
      if (machineId !== LOCAL_MACHINE_ID || !serviceMethods.includes("directories.search")) throw new Refusal("unsupported", OUTCOME_WORDS.unsupported);
      try {
        const reply = await transport.request("directories.search", { session, query, ...(limit ? { limit } : {}) }) as DirectorySearch;
        if (!Array.isArray(reply.entries) || typeof reply.home !== "string") throw new Error("invalid_reply");
        return reply;
      } catch (error) {
        const text = error instanceof Error ? error.message : String(error);
        throw new Refusal("refused", text.startsWith(REFUSED) ? text.slice(REFUSED.length) : "Unable to search folders");
      }
    },
    async cancelDirectorySearch(machineId, session) {
      if (machineId === LOCAL_MACHINE_ID && serviceMethods.includes("directories.cancel")) await transport.request("directories.cancel", { session }).catch(() => undefined);
    },
    async readCommandOutput({ projectId, ...params }) {
      return machine<CommandOutput>("command.output", projectId, params);
    },
    async readTaskAttachments(taskId) {
      // A page restored at launch is painted from the cached board before the service has
      // confirmed its task. Its files are read once that project's first read says where it is.
      const cachedIn = () => [...cachedBoards.values()].find(b => b.items.some(t => t.taskId === taskId))?.project.projectId;
      if (!projectOfTask(taskId) && cachedIn() && !confirmedProjects.has(cachedIn()!)) await new Promise<void>(resolve => {
        const check = () => { const id = cachedIn(); if (projectOfTask(taskId) || !id || confirmedProjects.has(id)) { confirmations.delete(check); resolve(); } };
        confirmations.add(check);
      });
      const projectId = projectOfTask(taskId);
      if (!projectId) throw new Refusal("unknown_task", "Unknown task.");
      // An older service answers this with its whole product state, which has no files to offer.
      const files = await machine<Message["attachments"]>("product.read", projectId, {taskId, attachments:true});
      if (!Array.isArray(files)) throw new Refusal("attachments_unavailable", "Saved attachments could not be read. Refresh or update the service before editing.");
      return wireAttachments(files);
    },
    async uploadAttachments(files) {
      const refs = await machine<AttachmentRef[]>("product.attachment", undefined, {action:"upload", files:wireAttachments(files)});
      if (!Array.isArray(refs) || refs.length !== files.length) throw new Refusal("attachments_unavailable", "Attachments could not be saved. Choose the files again.");
      return wireAttachments(refs) as AttachmentRef[];
    },
    // Bounded by the machine: one saved body by its digest, checked against the reference.
    async readAttachment(file) {
      const read = await machine<{data?: unknown; size?: unknown}>("product.attachment", undefined, {action:"read", sha256:file.sha256});
      if (typeof read?.data !== "string" || read.size !== file.size) throw new Refusal("attachment_unavailable", "This file is no longer saved here.");
      return {name:file.name, mimeType:file.mimeType, data:read.data};
    },
    async saveAttachment(file) {
      if (!serviceMethods.includes("native.save_attachment")) throw new Refusal("unsupported", "Saving a copy needs the desktop app.");
      return await transport.request("native.save_attachment", {name:file.name, data:file.data}) as {path:string|null};
    },
    async readClipboardImage() {
      if (!serviceMethods.includes("native.clipboard_image")) return [];
      return wireAttachments(await transport.request("native.clipboard_image", {}) as Message["attachments"]) as Attachment[];
    },
    retryHistory,
    loadEarlierHistory,
    scopesFacts: () => machineOn,
    retryFirstView(taskId) {
      const projectId = projectOfTask(taskId), project = projects.find(p => p.projectId === projectId);
      if (!project) return;
      const executionId = nativeSnapshots.get(project.projectId)?.items.find(t => t.taskId === taskId)?.executionId;
      if (executionId) decisionIssues.delete(executionId);
      readShownDecisions(project, taskId);
      if (!historyReads.has(taskId) || historyIssues.has(taskId)) retryHistory(taskId);
      else void publishHistory(project);
    },
    interest(projectId, kind, taskId) {
      const key = `${projectId}|${kind}`;
      interests.set(key, (interests.get(key) ?? 0) + 1);
      if (taskId) { const shown = shownTasks.get(projectId) ?? new Map<string, number>(); shownTasks.set(projectId, shown.set(taskId, (shown.get(taskId) ?? 0) + 1)); }
      if (kind === "git") for (const plan of gitShown) plan();
      // A conversation comes on view: it is read now, or continued from the rows already held.
      const opened = taskId ? projects.find(p => p.projectId === projectId) : undefined;
      if (opened) { queueHistories(opened); readShownDecisions(opened, taskId!); }
      let released = false;
      return () => {
        if (released) return;
        released = true;
        const left = (interests.get(key) ?? 1) - 1;
        if (left > 0) interests.set(key, left); else interests.delete(key);
        const shown = taskId ? shownTasks.get(projectId) : undefined;
        if (shown && taskId) {
          const n = (shown.get(taskId) ?? 1) - 1;
          if (n > 0) shown.set(taskId, n); else shown.delete(taskId);
          if (!shown.size) shownTasks.delete(projectId);
        }
        if (kind === "git") for (const plan of gitShown) plan();
      };
    },
    remoteAccess: {
      // The local machine's own remote access. An older service has none, and says nothing it cannot do.
      async call(operation, message) {
        if (!serviceMethods.includes("remote.status")) throw new Refusal("unsupported", "Remote access is not available in this build.");
        let reply: unknown;
        try { reply = await transport.request(operation, message); }
        catch (error) { const text = error instanceof Error ? error.message : String(error); throw new Refusal("refused", text.startsWith(REFUSED) ? text.slice(REFUSED.length) : text); }
        return readRemoteStatus(reply);
      },
      watch: listener => transport.subscribe(event => { if (event.type === "refresh") listener(); }),
    },
    sudoPrompts: {
      // The machine's sudo password prompts. An older service has none: nothing to show, nothing to send.
      async list() {
        if (!serviceMethods.includes("askpass.list")) return null;
        return readSudoPrompts(await transport.request("askpass.list", {}));
      },
      async answer(promptId, password) { await sudoAnswer({ promptId, password }); },
      async cancel(promptId) { await sudoAnswer({ promptId, cancel: true }); },
    },
    ownerActions: {
      // Access the machine's tasks lack. An older service has none: nothing to show, nothing to send.
      async list() {
        if (!serviceMethods.includes("owner_actions.list")) return null;
        return readOwnerActions(await transport.request("owner_actions.list", {}));
      },
      act: (projectId, actionId, action) => ownerAct({ projectId, actionId, action }),
      token: (projectId, actionId, token) => ownerAct({ projectId, actionId, action: "token", token }),
    },
    sprintGraph: {
      // Desktop owner only. A service that does not know the action, or answers another shape, keeps the sprint read-only.
      async read(sprintId) {
        if (!machineOn) return { kind: "unsupported" };
        const projectId = sprintProjects.get(sprintId);
        if (!projectId) return { kind: "refused", code: "unknown_sprint", message: "Unknown sprint." };
        const reply = await transport.request("machine.call", { op: "product.sprint", projectId, params: { action: "live_graph", sprintId } }) as { ok?: boolean; result?: unknown; code?: string; message?: string } | null;
        if (reply?.ok !== true) {
          const code = typeof reply?.code === "string" ? reply.code : "refused";
          if (code === "unsupported_action") return { kind: "unsupported" };
          return { kind: "refused", code, message: typeof reply?.message === "string" && reply.message ? reply.message : "The sprint graph could not be read." };
        }
        const graph = parseLiveSprintGraph(reply.result);
        return graph && graph.sprintId === sprintId ? { kind: "graph", graph } : { kind: "unsupported" };
      },
      // Sent once. A lost answer, or a save the service could not confirm, is unknown: the caller reads again first.
      async edit(edit) {
        if (!machineOn) return { outcome: "refused", code: "not_available", message: OUTCOME_WORDS.unsupported };
        const projectId = sprintProjects.get(edit.sprintId);
        if (!projectId) return { outcome: "refused", code: "unknown_sprint", message: "Unknown sprint." };
        let reply: { ok?: boolean; result?: { changed?: unknown }; code?: string; message?: string } | null;
        try { reply = await transport.request("machine.call", { op: "product.sprint", projectId, params: { ...edit } }) as typeof reply; }
        catch { return { outcome: "unknown" }; }
        if (reply?.ok !== true) {
          const message = typeof reply?.message === "string" && reply.message ? reply.message : "The change was not made.";
          if (!reply || reply.code === "graph_save_unconfirmed") return { outcome: "unknown", message };
          return { outcome: "refused", code: typeof reply.code === "string" ? reply.code : "refused", message };
        }
        return reply.result?.changed === true ? { outcome: "changed" } : reply.result?.changed === false ? { outcome: "unchanged" } : { outcome: "unknown" };
      },
    },
    presented: () => Promise.all([...lateReads]).then(() => {}),
    watchOverview() {
      overviewWatchers++;
      let released = false;
      return () => { if (!released) { released = true; overviewWatchers--; } };
    },
    async readWorkerActivity(taskId, workerId, cursor) {
      const projectId = projectOfTask(taskId);
      if (!projectId) throw new Refusal("unknown_task", "Unknown task.");
      const rows = await machine<Worker[]>("workers", projectId, {taskIds:[taskId],workerId,cursor});
      const row = rows.find(w => w.id === workerId && w.taskId === taskId);
      if (!row) throw new Refusal("worker_unavailable", "This worker attempt is no longer available.");
      return {activity:row.activity,activityCursor:row.activityCursor,messages:row.messages};
    },
    async readReport(taskId, reportId, cursor) {
      const projectId = projectOfTask(taskId);
      if (!projectId) throw new Refusal("unknown_task", "Unknown task.");
      let reply: unknown;
      try { reply = await transport.request("reports.read", request(projectId, "reports.read", {target:reportId,limit:4000,...(cursor ? {cursor} : {})})); }
      catch (error) {
        // A read is never shown as a raw service refusal.
        const text = error instanceof Error ? error.message : "";
        throw new Error(text.includes("not_found") ? "This report is no longer available." : text.includes("native_operation_unavailable") || text.startsWith(REFUSED) ? "This report cannot be opened right now. Retry." : "Report could not be loaded. Retry.");
      }
      const result = decodeResponse("ReportPage", reply).value;
      if (result.reportId !== reportId) throw new Error("Report identity mismatch.");
      return result;
    },
    async readFile(checkoutId, file) {
      if (!machineOn) return { text: null };
      return machine<FileRead>("repo.file", projectOfCheckout(checkoutId), { checkoutId, file });
    },
    async readMedia(checkoutId, file) {
      if (!machineOn) return { text: null };
      return machine<FileRead>("repo.file", projectOfCheckout(checkoutId), { checkoutId, file, media: true });
    },
    async listProjectSkills(projectId) {
      if (!machineOn) return { skills: [], omitted: 0 };
      return machine<ProjectSkills>("repo.skills", projectId, {});
    },
    async listFolders(checkoutId, request) {
      if (!machineOn) return { overview: null, folders: [] };
      return machine<FolderListing>("repo.list", projectOfCheckout(checkoutId), { checkoutId, dirs: request.dirs, overview: !!request.overview });
    },
    ...(options.localCode ? {
      codeLease: {
        acquire: (file: CodeFile) => codeCall("CodeLeaseAcquireResult", "repo.file.lease.acquire", projectOfCheckout(file.checkoutId), fileParams(file)),
        renew: (file: CodeFile, leaseToken: string) => codeCall("CodeLeaseRenewResult", "repo.file.lease.renew", projectOfCheckout(file.checkoutId), { ...fileParams(file), leaseToken }),
        release: (file: CodeFile, leaseToken: string) => codeCall("CodeLeaseReleaseResult", "repo.file.lease.release", projectOfCheckout(file.checkoutId), { ...fileParams(file), leaseToken }),
        save: (file: CodeFile, text: string, revision: string | null, leaseToken: string) =>
          codeCall("CodeSaveResult", "repo.file.save", projectOfCheckout(file.checkoutId), { ...fileParams(file), text, revision, leaseToken }),
      },
      codeLanguage: {
        open: (project, params) => codeCall("CodeLspOpenResult", "repo.lsp.open", project, params),
        change: (project, documentId, text, version) => codeCall("CodeLspSnapshot", "repo.lsp.change", project, { documentId, text, version }),
        status: (project, documentId) => codeCall("CodeLspSnapshot", "repo.lsp.status", project, { documentId }),
        definition: (project, params) => codeCall("CodeLspDefinitionResult", "repo.lsp.definition", project, params),
        close: (project, documentId) => codeCall("CodeLspCloseResult", "repo.lsp.close", project, { documentId }),
        subscribe: listener => transport.subscribe(event => { if (event.type === "code_lsp" || event.type === "code_lsp_resync") listener(event); }),
      },
    } satisfies LocalCodePort : {}),
    subscribe(listener: (update: PortUpdate) => void) {
      observers.add(listener);
      for (const cached of cachedBoards.values()) {
        const p = cached.project;
        const slice = mapProjectSlice({id:p.projectId,machineId:LOCAL_MACHINE_ID,name:p.name,repository:p.repository,
          openBranch:p.openBranch,defaultBranch:p.defaultBranch,activated:p.activated},cached.items,[],[]);
        const facts: ProjectFacts = {projectId:p.projectId,openBranch:p.openBranch,defaultBranch:p.defaultBranch,repo:null,
          workers:[],reviews:[],overlays:cached.product.tasks,sprints:cached.product.sprints};
        listener({kind:"preview",slice,facts});
      }
      // No project is polled. A project on screen is read when its screen opens (`interest`), on its events and on return to
      // the window; its Git, while a Git screen is on view and the window focused, also every GIT_VISIBLE_REFRESH_MS.
      let active = true, focused = true, gitTimer: ReturnType<typeof setTimeout> | undefined;
      // The machine has its own timer, lined up with the service's limits snapshots (machineReadDelay), never behind a slow repository.
      let machineTimer: ReturnType<typeof setTimeout> | undefined, machineDue = Date.now() + MACHINE_POLL_MS, catchUp = true, overviewRead = Date.now();
      // A slow repository must not stack reads behind itself: the next read starts when this one ended.
      const running = new Set<string>();
      const once = (key: string, read: () => Promise<void>) => { if (running.has(key)) return Promise.resolve(); running.add(key); return read().catch(() => {}).finally(() => { running.delete(key); }); };
      const hidden = () => typeof document !== "undefined" && document.hidden;
      /** One facts read of a project on screen, with a fresh repository when `git`; sent only when something differs. */
      const readShown = (project: ProjectLocator, git: boolean) => once(project.projectId, () => {
        const generation = presentationRead;
        return factsOf(project, git, git).then(facts => {
          if (!active || generation !== presentationRead || sameFacts(sent.get(project.projectId), facts)) return;
          sent.set(project.projectId, facts);
          listener({ kind: "facts", facts });
        });
      });
      const planGit = () => {
        clearTimeout(gitTimer); gitTimer = undefined;
        if (!active || hidden() || !focused || !projects.some(p => wantsGit(p.projectId))) return;
        gitTimer = setTimeout(() => {
          gitTimer = undefined;
          if (!active || hidden() || !focused || !machineOn) return;
          void Promise.all(projects.filter(p => wantsGit(p.projectId)).map(p => readShown(p, true))).then(planGit);
        }, GIT_VISIBLE_REFRESH_MS);
      };
      gitShown.add(planGit);
      const planMachine = () => { clearTimeout(machineTimer); machineTimer = active && !hidden() ? setTimeout(machineRound, Math.max(0, machineDue - Date.now())) : undefined; };
      // Any settings read, this poll's or one after an action, sets when the next one is due.
      const machineRead = (observed: number[]) => {
        if (!active) return;
        machineDue = Date.now() + machineReadDelay(observed, Date.now(), catchUp);
        if (observed.length) catchUp = false;
        planMachine();
      };
      machineReads.add(machineRead);
      const machineRound = () => {
        if (!active || hidden()) return;
        // A failed read is tried again on the plain poll; a successful one has already set the next.
        const retry = () => { if (machineDue <= Date.now()) machineDue = Date.now() + MACHINE_POLL_MS; planMachine(); };
        if (!machineOn) { retry(); return; }
        void once("machine", () => machineFacts().then(facts => { if (active) listener({ kind: "machine", facts }); })).then(retry);
        if (overviewWatchers > 0 && Date.now() - overviewRead >= MACHINE_POLL_MS) { overviewRead = Date.now(); void once("overview", () => machineOverview().then(facts => { if (active) listener({ kind: "machine", facts }); })); }
      };
      // Coming back from another window is exactly when Git may have moved, and when the limits shown may be old. Only the
      // projects on screen are read, and only what is older than FOCUS_STALE_MS.
      const back = (event?: Event) => {
        if (event?.type === "focus") focused = true;
        if (hidden()) { planGit(); planMachine(); return; }
        if (machineOn) for (const project of projects.filter(p => watched(p.projectId))) {
          const id = project.projectId, old = (at: number | undefined) => at === undefined || Date.now() - at >= FOCUS_STALE_MS;
          const git = showsGit(id) && old(repoReads.get(id)?.at);
          if (git || old(factsReadAt.get(id))) void readShown(project, git);
        }
        planGit();
        if (machineDue <= Date.now()) { catchUp = true; machineRound(); } else planMachine();
      };
      // A window in the background is not looked at: its Git screen's periodic refresh waits for focus.
      const away = () => { focused = false; planGit(); };
      const page = typeof document !== "undefined" ? document : undefined, screen = typeof window !== "undefined" ? window : undefined;
      page?.addEventListener("visibilitychange", back);
      screen?.addEventListener("focus", back);
      screen?.addEventListener("blur", away);
      planGit();
      // The startup read usually came first: line up with what it showed.
      if (lastObserved) machineRead(lastObserved); else planMachine();
      const stop = transport.subscribe(event => {
        if (event.type === "connection") { listener({ kind: "connection", connection: { state: event.state, reason: event.reason ?? null, at: Date.now() } }); return; }
        if (event.type === "refresh") { for (const id of nativeWork.keys()) nativeEpoch.set(id, (nativeEpoch.get(id) ?? 0) + 1); nativeSnapshots.clear(); listener({ kind: "refresh" }); return; }
        if (event.type === "native_delta") {
          const checked = nativeDelta(event), project = projects.find(p => p.projectId === event.projectId);
          if (!checked || !project) return;
          const epoch = nativeEpoch.get(project.projectId) ?? 0, queued = nativeQueued.get(project.projectId) ?? 0;
          if (queued >= 128) {
            nativeEpoch.set(project.projectId, epoch + 1); nativeSnapshots.delete(project.projectId);
            invalidated.add(project.projectId); listener({kind:"refresh"}); return;
          }
          nativeQueued.set(project.projectId, queued + 1);
          const work = (nativeWork.get(project.projectId) ?? Promise.resolve()).catch(() => {}).then(async () => {
            // Finish a snapshot already in flight before applying its buffered continuation.
            await reads.get(project.projectId)?.catch(() => {});
            if ((nativeEpoch.get(project.projectId) ?? 0) !== epoch) return;
            const snapshot = nativeSnapshots.get(project.projectId);
            if (!snapshot) throw new Error("Native snapshot unavailable");
            const next = applyNativeTaskDelta(snapshot, checked);
            if (next === snapshot) return;
            nativeSnapshots.set(project.projectId, next);
            const changed = new Set(checked.batch.changes.filter(c => c.kind === "task").map(c => c.id));
            for (const id of changed) historyChanged(id);
            if (!changed.size) return;
            const update = await snapshotOf(project, changed);
            if (active && (nativeEpoch.get(project.projectId) ?? 0) === epoch) {
              if (update.scope === "project") boardTasks.set(project.projectId,new Set(update.slice.tasks.map(t => t.id)));
              listener({ kind: "delta", update }); if (serviceMethods.includes("native.board")) queueEnrichment(project);
            }
          }).catch(() => {
            if ((nativeEpoch.get(project.projectId) ?? 0) !== epoch) return;
            nativeEpoch.set(project.projectId, epoch + 1);
            nativeSnapshots.delete(project.projectId);
            invalidated.add(project.projectId);
            if (active) listener({ kind: "refresh" });
          });
          nativeWork.set(project.projectId, work);
          void work.finally(() => { nativeQueued.set(project.projectId, Math.max(0, (nativeQueued.get(project.projectId) ?? 1) - 1)); if (nativeWork.get(project.projectId) === work) nativeWork.delete(project.projectId); });
          return;
        }
        if (event.type === "native_resync") {
          nativeEpoch.set(event.projectId, (nativeEpoch.get(event.projectId) ?? 0) + 1);
          nativeSnapshots.delete(event.projectId);
          invalidated.add(event.projectId);
          listener({ kind: "refresh" }); return;
        }
        if (event.type === "code_lsp" || event.type === "code_lsp_resync") return;
        if (event.type === "invalidated" && event.source === "runtime" && nativeSnapshots.has(event.projectId)) return;
        const project = projects.find(p => p.projectId === event.projectId);
        if (!project) return;
        // A failed reread leaves the last confirmed state on screen. It says nothing about the machine.
        // The store rereads the projection and product facts as one presentation commit.
        // A wire-only update would momentarily undo sprint/archive/owner state.
        invalidated.add(project.projectId);
        listener({ kind: "refresh" });
      });
      return () => { observers.delete(listener); if (!observers.size) { for (const retry of [...projectRetries.values(), ...historyRetries.values()]) clearTimeout(retry); projectRetries.clear(); historyRetries.clear(); } active = false; clearTimeout(gitTimer); gitShown.delete(planGit); clearTimeout(machineTimer); machineReads.delete(machineRead); page?.removeEventListener("visibilitychange", back); screen?.removeEventListener("focus", back); screen?.removeEventListener("blur", away); stop(); };
    },
  };
}
