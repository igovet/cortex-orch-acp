import { batch, createMemo, createSignal, onCleanup } from "solid-js";
import { applyFacts, applyMachineFacts, applyProjection, applySprintDeletion, capabilityOf, compareVersions, emptyWorkspace, FEATURES, hasCapability, noCapabilities, projectionKey } from "@cortex/workspace-domain";
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
/** How long the first screen waits for the machine's settings (models, efforts) before it shows the interface and says they are missing. */
export const MACHINE_WAIT_MS = 15_000;
/** How long the first screen waits for the machine to fetch the engines' model lists when it has none saved, and how often it asks. */
export const MODELS_WAIT_MS = 90_000, MODELS_POLL_MS = 1500;
/** While an engine's model list has failed, the settings are read again after this, doubling up to the bound. */
export const CATALOGUE_RETRY_MS = 10_000, CATALOGUE_RETRY_MAX_MS = 300_000;
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
 * The screens' only view of a backend. Versioned projections and acknowledged rename/queue patches change the display.
 * A command receipt settles before its background reconciliation. A failure stays a failure; no requested effect is guessed.
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
  const deletions = new Map<string, Set<string>>();
  const projectIssues = new Map<string, string | null>(), projectCauses = new Map<string, string | null>();
  const setProjected = (change: (current: WorkspaceState) => WorkspaceState) => setHeld(current => {
    let next = change(current);
    // An older in-flight read cannot resurrect an acknowledged deletion.
    for (const [sprintId, taskIds] of deletions) next = applySprintDeletion(next, sprintId, taskIds);
    next = { ...next, projects: next.projects.map(p => {
      if (!projectIssues.has(p.id)) return p;
      // The precise cause stays beside the owner's words while the port's read issue stands, and goes with it.
      const { issueCause: _old, ...rest } = p, cause = projectIssues.get(p.id) ? projectCauses.get(p.id) : null;
      return {...rest, issue: projectIssues.get(p.id) ?? p.issue ?? null, ...(cause ? {issueCause: cause} : {})};
    }) };
    return share(current, next);
  });
  const track = options.track ?? (<T,>(work: Promise<T>) => work);
  const running = new Map<string, Promise<CommandReceipt>>();
  // A read that changed nothing leaves these alone: every screen and the whole index hang off them.
  const [connection, setConnection] = createSignal<PortConnection>({ state: options.initial ? "connected" : "connecting", reason: null, at: null }, { equals: sameConnection });
  const [capabilities, setCapabilities] = createSignal(options.initial?.capabilities ?? noCapabilities(), { equals: (a, b) => JSON.stringify(a) === JSON.stringify(b) });
  const [ready, setReady] = createSignal(!!options.initial);
  // The machine's own settings (the models and efforts a composer offers) arrive in a read of their own. Until they have, an empty
  // list means "not yet", never "none"; `settled` ends the wait for them, whether they came, failed, or ran out of time.
  const [machineLoaded, setMachineLoaded] = createSignal(!!options.initial);
  const [machineSettled, setMachineSettled] = createSignal(!!options.initial);
  const [machineIssue, setMachineIssue] = createSignal<string | null>(null);
  // The machine fetches the model lists no connection has saved yet, on its own. While it does and no model is offered, the composer's
  // pickers say they are loading and the first screen waits (a bounded time). `modelsIssue` is the honest end of that wait.
  const [modelsPending, setModelsPending] = createSignal(false);
  const [modelsIssue, setModelsIssue] = createSignal<"failed" | "slow" | null>(null);
  // Storage, retention, worktrees, the version and GitHub CLI are read only once a screen that shows them asks (`wantOverview`).
  // They never gate the first screen; until they arrive, the screens that show them say so instead of a stand-in value.
  const [overviewLoaded, setOverviewLoaded] = createSignal(!!options.initial);
  const [overviewIssue, setOverviewIssue] = createSignal<string | null>(null);
  const [readIssue, setReadIssue] = createSignal<string | null>(null);
  const [syncing, setSyncing] = createSignal(false);
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
  let machineTimer: ReturnType<typeof setTimeout> | undefined, machineRetry: ReturnType<typeof setTimeout> | undefined;
  let modelsTimer: ReturnType<typeof setTimeout> | undefined, modelsPoll: ReturnType<typeof setTimeout> | undefined, modelsGaveUp = false;
  let overviewWanted = false, overviewReading: Promise<void> | undefined, overviewAgain = false, overviewRetry: ReturnType<typeof setTimeout> | undefined, machineAsked = false;
  const readOverview = () => {
    // A read already under way may predate the command that asked for this one: read once more after it.
    if (overviewReading) { overviewAgain = true; return; }
    clearTimeout(overviewRetry);
    overviewReading = port.query("machine.overview", {}).then(updates => {
      if (!active) return;
      batch(() => { for (const update of updates ?? []) installMachine(update); setOverviewIssue(null); setOverviewLoaded(true); });
    }).catch(error => {
      if (!active) return;
      setOverviewIssue(reasonOf(error));
      // Like the machine's settings: nothing else would ask again soon. The rest of the interface never waits for it.
      clearTimeout(overviewRetry);
      overviewRetry = setTimeout(readOverview, RETRY_MS);
    }).finally(() => {
      overviewReading = undefined;
      if (overviewAgain && active) { overviewAgain = false; readOverview(); }
    });
  };
  /** A screen shows the machine's overview fields. The first call reads them, once the service has said what it offers; later calls change nothing. */
  const wantOverview = () => {
    if (overviewWanted) return;
    overviewWanted = true;
    if (machineAsked) readOverview();
  };
  const giveUpOnModels = () => {
    modelsGaveUp = true;
    batch(() => { setModelsPending(false); setModelsIssue("slow"); setMachineSettled(true); });
  };
  let failedReads = 0;
  /**
   * What any settings read (the store's own or the port's machine poll) said about the engines' model lists: whether the
   * first screen waits, what the composer says, and when to read again. The options themselves were installed with the read.
   */
  const followCatalogue = (updates: readonly MachineFacts[]) => {
    const catalogue = updates.map(u => u.catalogue).find(Boolean);
    const withOptions = updates.filter(u => u.options), offered = withOptions.length ? withOptions.some(u => u.options!.models.length > 0) : state().options.models.length > 0, running = catalogue?.running === true;
    const waiting = running && !offered && !modelsGaveUp;
    setModelsPending(waiting);
    setModelsIssue(offered ? null : running ? (modelsGaveUp ? "slow" : null) : catalogue?.failed.length ? "failed" : null);
    if (waiting && modelsTimer === undefined) modelsTimer = setTimeout(giveUpOnModels, MODELS_WAIT_MS);
    if (!waiting) { clearTimeout(modelsTimer); modelsTimer = undefined; setMachineSettled(true); }
    clearTimeout(modelsPoll); modelsPoll = undefined;
    // Still fetching: ask again soon, so the lists appear as soon as they are saved, without anyone pressing anything.
    if (running) { failedReads = 0; modelsPoll = setTimeout(() => readMachine(), modelsGaveUp ? RETRY_MS : MODELS_POLL_MS); return; }
    // An engine's list failed (other engines' may be offered). The machine tries it again soon after a later read, so keep
    // reading, less and less often: its models then reach the composer without a relaunch.
    if (catalogue?.failed.length) { modelsPoll = setTimeout(() => readMachine(), Math.min(CATALOGUE_RETRY_MS * 2 ** failedReads, CATALOGUE_RETRY_MAX_MS)); failedReads++; }
    else if (catalogue) failedReads = 0;
  };
  /** `overview`: this is a refresh after the service answered or a command, so an overview a screen asked for is read again too. */
  const readMachine = (overview = false) => {
    machineAsked = true;
    if (overview && overviewWanted) readOverview();
    if (machineReading) return;
    machineReading = port.query("machine.facts", {}).then(updates => {
      if (!active) return;
      batch(() => {
        for (const update of updates ?? []) installMachine(update);
        setMachineIssue(null); setMachineLoaded(true);
        followCatalogue(updates ?? []);
      });
      clearTimeout(machineTimer);
    }).catch(error => {
      if (!active) return;
      setMachineIssue(reasonOf(error));
      // Busy or a lost read is worth the wait; a refusal is an answer: open the interface now and say so, still retrying.
      if (!transientRead(error)) { clearTimeout(machineTimer); setMachineSettled(true); }
      // Nothing else would ask again soon: the composer must not stay without its models until some later refresh.
      clearTimeout(machineRetry);
      machineRetry = setTimeout(() => readMachine(), RETRY_MS);
    }).finally(() => { machineReading = undefined; });
  };
  let refreshing: Promise<void> | undefined, transientFailures = 0;
  const queuedReads = new Set<string | undefined>();
  const queuedDeltas = new Map<string, ProjectionUpdate>();
  let actionEpoch = 0, fullReadQueued = false;
  const requeueRead = (projectId?: string) => {
    queuedReads.add(projectId);
    // The port may already have consumed an event's invalidation set.
    if (!projectId) fullReadQueued = true;
  };
  let machineReading: Promise<void> | undefined;
  const refresh = (projectId?: string): Promise<void> => {
    if (refreshing) { queuedReads.add(projectId); return refreshing; }
    const full = !projectId && fullReadQueued;
    if (!projectId) fullReadQueued = false;
    refreshing = readCoherent(projectId, full).finally(() => {
      refreshing = undefined;
      for (const update of queuedDeltas.values()) install(update);
      queuedDeltas.clear();
      if (queuedReads.size && active) {
        // Always read again after a command reply, even when an older poll is in flight.
        const next = queuedReads.has(undefined) ? undefined : queuedReads.values().next().value;
        if (next === undefined) {
          // Event-scoped reads may omit explicitly queued projects.
          if (queuedReads.size > 1) fullReadQueued = true;
          queuedReads.clear();
        } else queuedReads.delete(next);
        void refresh(next);
      } else setSyncing(false);
    });
    return refreshing;
  };
  const readCoherent = async (projectId?: string, full = false) => {
    const epoch = actionEpoch;
    let serviceAnswered = false;
    try {
      const offered = projectId ? capabilities() : await port.query("capabilities.get", {});
      serviceAnswered = true;
      // The machine's settings are asked for as soon as the service has said what it offers, beside the task reads and not after
      // them: the first screen needs both, and neither should wait for the other.
      if (!projectId) { if (!machineSettled() && machineTimer === undefined) machineTimer = setTimeout(() => { if (!modelsPending()) setMachineSettled(true); }, MACHINE_WAIT_MS); readMachine(true); }
      const parameters = projectId ? { projectId } : {};
      const snapshots = await port.query("project.snapshot", full ? { full: true } : parameters);
      // Facts own sprint membership, hidden planning/purged rows and owner decisions.
      const product = await port.query("project.facts", parameters) ?? [];
      if (!active) return;
      if (epoch !== actionEpoch) { requeueRead(projectId); return; }
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
      if (!active) return;
      if (epoch !== actionEpoch) { requeueRead(projectId); return; }
      const quiet = ready() && transientRead(error) && ++transientFailures < 3;
      if (!quiet) setReadIssue(reasonOf(error));
      // A refused product/repository read is not evidence that the service is offline.
      if (serviceAnswered) setConnection({ state: "connected", reason: null, at: Date.now() });
      else if (!quiet && !projectId) {
        setCapabilities(noCapabilities());
        setConnection({ state: "unavailable", reason: reasonOf(error), at: Date.now() });
      }
      // Nothing else would ask again, so a passing failure must not leave the app offline until the owner notices.
      clearTimeout(again);
      if (active) again = setTimeout(() => void refresh(projectId), RETRY_MS);
      return;
    }
    // Machine settings do not determine task placement.
    // Discovery/disk accounting must not keep ordinary actions spinning after task facts arrived.

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
  onCleanup(() => { active = false; clearTimeout(again); clearTimeout(pushTimer); clearTimeout(machineTimer); clearTimeout(machineRetry); clearTimeout(modelsTimer); clearTimeout(modelsPoll); clearTimeout(overviewRetry); });
  const reload = () => { if (!ready()) setConnection({ state: "connecting", reason: null, at: null }); return refresh(); };
  const onUpdate = (update: PortUpdate) => {
    if (update.kind === "preview") {
      // Cached data paints the first frame but cannot seed command versions,
      // capabilities or a native event generation.
      if (!registered && !versions.size) batch(() => {
        setProjected(current => applyFacts(applyProjection(current,{kind:"projection",scope:"project",version:"0",slice:update.slice}),update.facts));
        setReady(true);
      });
      return;
    }
    if (update.kind === "project_state") {
      if (registered && !registered.has(update.project.id)) return;
      projectIssues.set(update.project.id, update.issue);
      projectCauses.set(update.project.id, update.cause ?? null);
      setProjected(current => ({...current, projects: current.projects.some(p => p.id === update.project.id)
        ? current.projects.map(p => p.id === update.project.id ? {...p, name:update.project.name, repository:update.project.repository, issue:update.issue} : p)
        : [...current.projects, {...update.project, issue:update.issue}]}));
      return;
    }
    if (update.kind === "enrichment") {
      if (refreshing) { requeueRead(update.update.scope === "project" ? update.update.slice.project.id : undefined); return; }
      batch(() => {
        if (!install(update.update, true) || update.update.scope !== "project") return;
        const held = facts.get(update.update.slice.project.id);
        if (held) installFacts(held);
      });
      return;
    }
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
    if (update.kind === "delta") {
      if (refreshing) queuedDeltas.set(projectionKey(update.update), update.update);
      else install(update.update);
      return;
    }
    if (update.kind === "projection") { if (refreshing) queuedDeltas.set(projectionKey(update), update); else install(update); return; }
    if (update.kind === "facts") {
      // The port remembers this notification, so unchanged polls will not send it again.
      // Reread rather than replaying potentially older facts over the coherent commit.
      if (refreshing) requeueRead(update.facts.projectId); else installFacts(update.facts);
      return;
    }
    // The port's own machine poll carries the same settings read: what it says about the model lists is followed too.
    if (update.kind === "machine") { batch(() => { installMachine(update.facts); if (update.facts.catalogue) followCatalogue([update.facts]); }); return; }
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
    mode: port.mode, state, index, ready, readIssue, machineLoaded, machineIssue, modelsPending, modelsIssue,
    /** The composer's lists are known: the machine answered and is not still fetching the models. Until then an empty list means "not yet". */
    optionsKnown: () => machineLoaded() && !modelsPending(),
    /** Storage, retention, worktrees, the version and GitHub CLI have arrived. Until then a screen that shows them says they are loading. */
    overviewLoaded, overviewIssue, wantOverview,
    /** The first screen's gate: the machine has answered and its settings are in, or waiting for them has run out. */
    loaded: () => ready() && machineSettled(),
    syncing, now, receipt, acceptanceIssue, connection, rejected, can, reload: () => track(reload()),
    /** Opening Git keeps the confirmed display and asks only for this project's moving facts. */
    refreshProjectFacts: async (projectId: string) => {
      if (port.mode !== "real") return;
      await refreshing;
      if (!active) return;
      const issue = "The project's latest facts could not be read. The last confirmed state is still shown; it will retry automatically.";
      try {
        for (const update of await port.query("project.facts", { projectId, fresh: true })) if (active) installFacts(update);
        if (active) setReadIssue(current => current === issue ? null : current);
      } catch {
        if (active) setReadIssue(issue);
      }
    },
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
      const work = port.command(envelope).catch((): CommandReceipt => ({ requestId: id, ok: false, outcome: "unknown_outcome", message: UNKNOWN_OUTCOME })).then(result => {
        actionEpoch++;
        if (result.ok) setProjected(state => ({ ...state,
          tasks: command.type === "rename_task" ? state.tasks.map(t => t.id === command.taskId ? {...t,title:command.title} : t) : state.tasks,
          sprints: state.sprints.map(s => {
            if (!("sprintId" in command) || s.id !== command.sprintId) return s;
            if (command.type === "rename_sprint") return {...s,title:command.title};
            if (command.type === "run_sprint") return {...s,queueState:"running",pauseReason:null};
            if (command.type === "pause_sprint") return {...s,queueState:"paused",pauseReason:"Paused by you"};
            if (command.type === "set_run_mode") return {...s,runMode:command.runMode};
            return s;
          }),
        }));
        if (result.ok && command.type === "git_commit" && checkoutDrafts.get(command.checkoutId) === command.message) checkoutDrafts.delete(command.checkoutId);
        if (command.type === "delete_sprint" && result.ok) {
          // This is the service's confirmed deletion, not an optimistic project refresh.
          deletions.set(command.sprintId, new Set(result.deletedTaskIds ?? []));
          setProjected(state => state);
        }
        // An acknowledgement is not a projection. Reconciliation never holds its button busy.
        if (port.mode === "real" && result.outcome !== "local") {
          setSyncing(true);
          const projectId = ("projectId" in command && command.projectId)
            || ("taskId" in command && state().tasks.find(t => t.id === command.taskId)?.projectId)
            || ("sprintId" in command && state().sprints.find(s => s.id === command.sprintId)?.projectId)
            || ("checkoutId" in command && state().repos.find(r => r.checkouts.some(c => c.id === command.checkoutId))?.projectId);
          const registryChange = ["add_project","activate_project","rename_project","remove_project"].includes(command.type);
          void refresh(registryChange ? undefined : projectId || undefined);
        }
        return settle(result);
      }).finally(() => { running.delete(key); });
      running.set(key, work);
      return track(work);
    },
  };
}
export type WorkspaceStore = ReturnType<typeof createWorkspaceStore>;
