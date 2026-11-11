import { batch, createMemo, createSignal, onCleanup } from "solid-js";
import { applyFacts, applyMachineFacts, applyProjection, capabilityOf, compareVersions, emptyWorkspace, FEATURES, hasCapability, noCapabilities, projectionKey } from "@cortex/workspace-domain";
import type { Attachment, CommandEnvelope, CommandReceipt, FileRead, FileSaveResult, MachineFacts, PortCapabilities, PortConnection, PortUpdate, ProjectFacts, ProjectionUpdate, WorkspaceCommand, WorkspacePort, WorkspaceState } from "@cortex/workspace-domain";
import { downloadAttachment } from "./attachments";
import { COPY } from "./copy";
import { buildIndex } from "./selectors";
import { share } from "./stable";

export type StoreOptions = {
  /** The clock figures start from. A real machine uses the current time. */
  now?: number;
  /** What to show on the first frame. Only a backend that already holds its state can give one. */
  initial?: { state: WorkspaceState; version: string; capabilities: PortCapabilities };
  /** Told about every action that takes time, so the control that started it can say so. */
  track?: <T>(work: Promise<T>) => Promise<T>;
};
const requestId = () => globalThis.crypto?.randomUUID?.() ?? `request-${Date.now().toString(36)}-${Math.random().toString(36).slice(2)}`;
const reasonOf = (error: unknown) => error instanceof Error ? error.message : String(error);
const UNKNOWN_OUTCOME = "The result of this action is not known. It was not sent again.";
/** The same command, whenever it was pressed. Its clock reading is not part of what it asks for. */
const commandKey = (command: WorkspaceCommand) => JSON.stringify({ ...command, now: undefined });
const RETRY_MS = 3000;
/** The shortest gap between refreshes a stream of machine events can cause. A hidden window needs them even less often. */
export const PUSH_GAP_MS = 2000, HIDDEN_PUSH_GAP_MS = 15_000;
const sameConnection = (a: PortConnection, b: PortConnection) => a.state === b.state && a.reason === b.reason;
const transientRead = (error: unknown) => /^(machine_busy|machine_tool_timeout|service_request_failed|bridge_request_refused|resync_required)$/.test(String((error as { code?: unknown })?.code ?? reasonOf(error)).replace(/^refused:/, ""));

/** The machine this app runs on is connected exactly when its service is. */
function withConnection(state: WorkspaceState, connection: PortConnection): WorkspaceState {
  const live = connection.state === "connected" ? "connected" : "disconnected";
  const local = state.machines.find(m => m.kind === "local");
  if (!local || local.connection === live) return state;
  const lastConfirmedAt = local.lastConfirmedAt;
  return { ...state, machines: state.machines.map(m => m === local ? { ...m, connection: live, lastConfirmedAt } : m) };
}

/**
 * The screens' only view of a backend. A snapshot goes in by its version and nothing else changes what is shown: a command
 * returns a receipt, and the state moves when a confirmed projection arrives. A failure stays a failure; there is nothing
 * here to fall back to.
 */
export function createWorkspaceStore(port: WorkspacePort, options: StoreOptions = {}) {
  const now = options.now ?? Date.now();
  const opened = Date.now();
  const versions = new Map<string, string>();
  let registered: Set<string> | undefined;
  let latest: string | null = options.initial?.version ?? null;
  if (options.initial) versions.set("workspace", options.initial.version);
  const [projected, setHeld] = createSignal(options.initial?.state ?? emptyWorkspace());
  // Only what really changed reaches the screens: an unchanged read leaves every record the same object.
  const setProjected = (change: (current: WorkspaceState) => WorkspaceState) => setHeld(current => share(current, change(current)));
  const track = options.track ?? (<T,>(work: Promise<T>) => work);
  const running = new Map<string, Promise<CommandReceipt>>();
  // A read that changed nothing leaves these alone: every screen and the whole index hang off them.
  const [connection, setConnection] = createSignal<PortConnection>({ state: options.initial ? "connected" : "connecting", reason: null, at: null }, { equals: sameConnection });
  const [capabilities, setCapabilities] = createSignal(options.initial?.capabilities ?? noCapabilities(), { equals: (a, b) => JSON.stringify(a) === JSON.stringify(b) });
  const [ready, setReady] = createSignal(!!options.initial);
  const [readIssue, setReadIssue] = createSignal<string | null>(null);
  const [rejected, setRejected] = createSignal(0);
  const [receipt, setReceipt] = createSignal<CommandReceipt | null>(null);
  const [acceptanceIssue, setAcceptanceIssue] = createSignal<{ taskId: string; message: string } | null>(null);
  const state = createMemo(() => port.mode === "real" ? withConnection(projected(), connection()) : projected());
  const index = createMemo(() => buildIndex(state(), now));

  const install = (update: ProjectionUpdate, coherent = false) => {
    if (update.scope === "project" && registered && !registered.has(update.slice.project.id)) { setRejected(n => n + 1); return false; }
    const key = projectionKey(update), have = versions.get(key);
    if (have !== undefined && (compareVersions(update.version, have) < 0 || (!coherent && compareVersions(update.version, have) === 0))) {
      // An equal-version read confirms the same record again, without replacing its contents.
      if (port.mode === "real" && update.scope === "project" && compareVersions(update.version, have) === 0) {
        const project = update.slice.project;
        setProjected(state => ({...state, projects:state.projects.map(p => p.id === project.id ? {...p,name:project.name,confirmedAt:project.confirmedAt} : p)}));
      }
      setRejected(n => n + 1); return false;
    }
    versions.set(key, update.version);
    latest = update.version;
    setProjected(current => {
      const next = applyProjection(current, update), held = !coherent && update.scope === "project" ? facts.get(update.slice.project.id) : undefined;
      return held ? applyFacts(next, held) : next;
    });
    return true;
  };
  // Facts sit over a projection, so a newer one never wipes what the machine said about Git or the owner's decisions.
  const facts = new Map<string, ProjectFacts>();
  const sources = new Map<string, string>();
  const checkoutDrafts = new Map<string, string>();
  const withSources = (held: ProjectFacts): ProjectFacts => !held.repo ? held : { ...held, repo: { ...held.repo, checkouts: held.repo.checkouts.map(c => ({ ...c, draft: checkoutDrafts.get(c.id) ?? c.draft, sources: Object.fromEntries([...sources].flatMap(([key, text]) => key.startsWith(`${c.id}|`) ? [[key.slice(c.id.length + 1), text]] : [])) })) } };
  const installFacts = (update: ProjectFacts) => { if (registered && !registered.has(update.projectId)) return; const held = withSources(update); facts.set(update.projectId, held); setProjected(current => applyFacts(current, held)); };
  const installMachine = (update: MachineFacts) => setProjected(current => applyMachineFacts(current, update));
  const readFacts = async () => {
    for (const update of await port.query("project.facts", {}) ?? []) installFacts(update);
    for (const update of await port.query("machine.facts", {}) ?? []) installMachine(update);
  };
  let refreshing: Promise<void> | undefined, refreshAgain = false, transientFailures = 0;
  let machineReading: Promise<void> | undefined;
  const refresh = (): Promise<void> => {
    if (refreshing) { refreshAgain = true; return refreshing; }
    refreshing = readCoherent().finally(() => {
      refreshing = undefined;
      if (refreshAgain && active) { refreshAgain = false; void refresh(); }
    });
    return refreshing;
  };
  const readCoherent = async () => {
    let serviceAnswered = false;
    try {
      const offered = await port.query("capabilities.get", {});
      const snapshots = await port.query("project.snapshot", {});
      serviceAnswered = true;
      // Facts own sprint membership, hidden planning/purged rows and owner decisions.
      const product = await port.query("project.facts", {}) ?? [];
      if (!active) return;
      batch(() => {
        setCapabilities(offered);
        for (const update of snapshots) install(update, true);
        for (const update of product) installFacts(update);
        setConnection({ state: "connected", reason: null, at: Date.now() });
        setReady(true);
        setReadIssue(null);
        transientFailures = 0;
        lastRead = Date.now();
      });
    } catch (error) {
      const quiet = ready() && transientRead(error) && ++transientFailures < 3;
      if (!quiet) setReadIssue(reasonOf(error));
      // A refused product/repository read is not evidence that the service is offline.
      if (serviceAnswered) setConnection({ state: "connected", reason: null, at: Date.now() });
      else if (!quiet) {
        setCapabilities(noCapabilities());
        setConnection({ state: "unavailable", reason: reasonOf(error), at: Date.now() });
      }
      // Nothing else would ask again, so a passing failure must not leave the app offline until the owner notices.
      clearTimeout(again);
      if (active) again = setTimeout(() => void refresh(), RETRY_MS);
      return;
    }
    // Machine settings do not determine task placement.
    // Discovery/disk accounting must not keep ordinary actions spinning after task facts arrived.
    if (!machineReading) machineReading = port.query("machine.facts", {}).then(updates => {
      for (const update of updates ?? []) if (active) installMachine(update);
    }).catch(() => {}).finally(() => { machineReading = undefined; });
  };
  let active = true, again: ReturnType<typeof setTimeout> | undefined, lastRead: number | null = null;
  // Coalesces a burst of machine events: the first refreshes at once, the rest arrive together after the gap.
  let pushAt = 0, pushTimer: ReturnType<typeof setTimeout> | undefined;
  const pushRefresh = () => {
    if (pushTimer !== undefined) return;
    const gap = typeof document !== "undefined" && document.hidden ? HIDDEN_PUSH_GAP_MS : PUSH_GAP_MS;
    const wait = pushAt + gap - Date.now();
    if (wait <= 0) { pushAt = Date.now(); void refresh(); return; }
    pushTimer = setTimeout(() => { pushTimer = undefined; pushAt = Date.now(); if (active) void refresh(); }, wait);
  };
  onCleanup(() => { active = false; clearTimeout(again); clearTimeout(pushTimer); });
  const reload = () => { if (!ready()) setConnection({ state: "connecting", reason: null, at: null }); return refresh(); };
  const onUpdate = (update: PortUpdate) => {
    if (update.kind === "registry") {
      const keep = new Set(update.projectIds);
      registered = keep;
      // Re-adding a detached project may legitimately return its unchanged projection.
      for (const key of versions.keys()) if (key.startsWith("project:") && !keep.has(key.slice(8))) versions.delete(key);
      for (const id of facts.keys()) if (!keep.has(id)) facts.delete(id);
      setProjected(current => {
        // A planning run's task never reaches the board, but its session holds the sprint's questions.
        const taskIds = new Set([...current.tasks.filter(t => keep.has(t.projectId)).map(t => t.id), ...current.sprints.flatMap(s => keep.has(s.projectId) && s.planningTaskId ? [s.planningTaskId] : [])]);
        return {...current,projects:current.projects.filter(p => keep.has(p.id)),tasks:current.tasks.filter(t => taskIds.has(t.id)),
          sessions:current.sessions.filter(s => taskIds.has(s.taskId)),workers:current.workers.filter(w => taskIds.has(w.taskId)),
          reviews:current.reviews.filter(r => taskIds.has(r.taskId)),messages:current.messages.filter(m => taskIds.has(m.taskId)),
          repos:current.repos.filter(r => keep.has(r.projectId)),sprints:current.sprints.filter(s => keep.has(s.projectId))};
      });
      return;
    }
    // Discard polls begun before this coherent read; they must not overwrite its newer facts.
    if (update.kind === "projection") { if (!refreshing) install(update); return; }
    if (update.kind === "facts") { if (!refreshing) installFacts(update.facts); return; }
    if (update.kind === "machine") { installMachine(update.facts); return; }
    if (update.kind === "refresh") { pushRefresh(); return; }
    const back = update.connection.state === "connected" && connection().state !== "connected";
    setConnection(update.connection);
    // What happened while it was away arrives as a fresh snapshot, never as a guess.
    if (back) void refresh();
  };
  onCleanup(port.subscribe(onUpdate));
  if (!options.initial) void reload();

  const can = (capability: string) => connection().state === "connected" && hasCapability(capabilities(), capability);
  const rememberSource = (projectId: string, checkoutId: string, file: string, text: string) => {
    sources.set(`${checkoutId}|${file}`, text);
    const held = facts.get(projectId);
    if (held) installFacts(held);
  };
  return {
    mode: port.mode, state, index, ready, readIssue, now, receipt, acceptanceIssue, connection, rejected, can, reload: () => track(reload()),
    /** When this app last heard from the machine, for a record with no stamp of its own. */
    lastRead: () => lastRead,
    takeDroppedFiles: () => port.takeDroppedFiles?.() ?? Promise.resolve([]),
    readClipboardImage: () => port.readClipboardImage?.() ?? Promise.resolve([]),
    /** The files of a saved task, read when its page opens. Nothing here when this backend keeps none. */
    readTaskAttachments: (taskId: string) => port.readTaskAttachments?.(taskId) ?? Promise.resolve([]),
    saveAttachment: (file: Attachment) => { if (port.saveAttachment) return track(port.saveAttachment(file)); downloadAttachment(file); return Promise.resolve({ path: null }); },
    pickFolder: () => track(port.pickFolder?.() ?? Promise.resolve(null)),
    /** Null when this backend has no folder search. */
    searchDirectories: (request: Parameters<NonNullable<WorkspacePort["searchDirectories"]>>[0]) => port.searchDirectories?.(request) ?? Promise.resolve(null),
    cancelDirectorySearch: (machineId: string, session: string) => port.cancelDirectorySearch?.(machineId, session) ?? Promise.resolve(),
    discoverMcp: port.discoverMcp,
    machineAction: (op: "workflow.describe" | "maintenance" | "engine.connection" | "native.engine_login", params: Record<string, unknown>) => track(port.machineAction?.(op, params) ?? Promise.reject(new Error("This action needs the local desktop service."))),
    version: (key: string) => versions.get(key) ?? null,
    /** The time figures start from, plus the real time since the app opened, so stamps and elapsed figures move on. */
    clock: () => now + (Date.now() - opened),
    clearReceipt: () => setReceipt(null),
    readWorkerActivity: (taskId: string, workerId: string, cursor?: string) => {
      if (!port.readWorkerActivity) return Promise.reject(new Error("Older activity is not available on this machine."));
      return track(port.readWorkerActivity(taskId, workerId, cursor));
    },
    readReport: (taskId: string, reportId: string, cursor?: string) => {
      if (!port.readReport) return Promise.reject(new Error("Reports are not available on this machine."));
      return track(port.readReport(taskId, reportId, cursor));
    },
    /** Reads one file of a checkout into its record, so the Code view and search can show it. */
    async loadSource(projectId: string, checkoutId: string, file: string) {
      const read = await port.readFile?.(checkoutId, file).catch(() => null);
      if (typeof read?.text === "string") rememberSource(projectId, checkoutId, file, read.text);
    },
    rememberSource,
    /** A port with no writes has no editor, and Code stays a viewer. */
    canEdit: () => port.saveFile !== undefined && can(FEATURES.editFiles),
    readFile: (checkoutId: string, file: string): Promise<FileRead> => port.readFile?.(checkoutId, file) ?? Promise.resolve({ text: null }),
    saveFile: (checkoutId: string, file: string, text: string, revision: string | null): Promise<FileSaveResult> => track(port.saveFile?.(checkoutId, file, text, revision) ?? Promise.resolve({ outcome: "refused", message: COPY.notOffered })),
    dispatch(command: WorkspaceCommand, opts: { quiet?: boolean } = {}): Promise<CommandReceipt> {
      const settle = (result: CommandReceipt) => {
        if (command.type === "accept_result") setAcceptanceIssue(result.ok ? null : { taskId: command.taskId, message: result.message });
        if (!opts.quiet) setReceipt(result); return result;
      };
      // However it was pressed, one action runs once: a repeat joins the one already under way.
      const key = commandKey(command), joined = running.get(key);
      if (joined) return joined;
      const id = requestId();
      if (!can(capabilityOf(command.type))) return Promise.resolve(settle({ requestId: id, ok: false, outcome: "unsupported", message: COPY.notOffered }));
      // Owner text is local, not a machine projection. Publish it before a same-tick Commit.
      if (command.type === "git_draft") {
        checkoutDrafts.set(command.checkoutId, command.message);
        const held = [...facts.values()].find(f => f.repo?.checkouts.some(c => c.id === command.checkoutId));
        if (held) installFacts(held);
      }
      const envelope: CommandEnvelope = { requestId: id, operation: command.type, expectedVersion: latest, payload: command };
      const work = port.command(envelope).catch((): CommandReceipt => ({ requestId: id, ok: false, outcome: "unknown_outcome", message: UNKNOWN_OUTCOME })).then(async result => {
        if (result.ok && command.type === "git_commit" && checkoutDrafts.get(command.checkoutId) === command.message) checkoutDrafts.delete(command.checkoutId);
        if (port.mode === "real" && result.outcome !== "local" && (result.ok || command.type === "rerun_check" || ["stale_version","stale_epoch","stale_result","superseded","expired"].includes(result.outcome))) await refresh();
        // A refusal changes nothing on the machine, but what the app held for it, such as a batch's answers, may be gone.
        else if (port.mode === "real" && !result.ok) await readFacts().catch(() => undefined);
        return settle(result);
      }).finally(() => { running.delete(key); });
      running.set(key, work);
      return track(work);
    },
  };
}
export type WorkspaceStore = ReturnType<typeof createWorkspaceStore>;
