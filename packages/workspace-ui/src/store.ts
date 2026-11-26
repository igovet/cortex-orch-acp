import { CODE_LANGUAGE, type LocalCodePort } from "./code-support";
import { batch, createMemo, createSignal, onCleanup } from "solid-js";
import { applyFacts, applyMachineFacts, applyProjection, applySprintDeletion, capabilityOf, compareVersions, emptyWorkspace, FEATURES, hasCapability, noCapabilities, projectionKey } from "@cortex/workspace-domain";
import type { Attachment, AttachmentInput, AttachmentRef, CommandEnvelope, CommandReceipt, FileRead, GitInterest, FolderListing, FolderListRequest, IntegrationText, IntegrationTextRequest, LiveSprintGraph, MachineFacts, RemoteOperation, RemoteRequests, RemoteStatus, PortCapabilities, PortConnection, PortUpdate, ProjectFacts, ProjectionUpdate, ProjectSkills, SprintGraphEdit, SprintGraphWrite, SprintOrder, WorkspaceCommand, WorkspacePort, WorkspaceState } from "@cortex/workspace-domain";
import type { OwnerActionVerb } from "@cortex/workspace-domain";
import { downloadAttachment } from "./attachments";
import { COPY, REQUESTING } from "./copy";
import { buildIndex, revisionHeld, stableIndex, type WorkspaceIndex } from "./selectors";
import { share } from "./stable";
import { createSends } from "./sends";
import { slowRead } from "./first-view";
import type { FirstViewPort, ScopedFacts } from "./port/real";

/** Not a machine capability: whether this shell may save an attachment where the owner picks. Only a shell withholds it. */
export const SAVE_ATTACHMENT = "workspace.save_attachment";
export type StoreOptions = {
  /** The clock figures start from. A real machine uses the current time. */
  now?: number;
  /** What to show on the first frame. Only a backend that already holds its state can give one. */
  initial?: { state: WorkspaceState; version: string; capabilities: PortCapabilities };
  /** Told about every action that takes time, so the control that started it can say so. */
  track?: <T>(work: Promise<T>) => Promise<T>;
  /** Capabilities this shell never uses, whatever the machine offers: a phone has no terminal, editor or system folder chooser. */
  withhold?: readonly string[];
};
const requestId = () => globalThis.crypto?.randomUUID?.() ?? `request-${Date.now().toString(36)}-${Math.random().toString(36).slice(2)}`;
const reasonOf = (error: unknown) => error instanceof Error ? error.message : String(error);
const UNKNOWN_OUTCOME = "The result of this action is not known. It was not sent again.";
/** The same command, whenever it was pressed. Its clock reading is not part of what it asks for. */
const commandKey = (command: WorkspaceCommand) => JSON.stringify({ ...command, now: undefined });
const RETRY_MS = 3000;
/** The longest a read the port answers late keeps "Updating confirmed state" on after the coherent read itself ended. */
const LATE_CONFIRM_MS = 10_000;
/** How long the first screen waits for the machine's settings (models, efforts) before it shows the interface and says they are missing. */
export const MACHINE_WAIT_MS = 15_000;
/** How long the first screen waits for the machine to fetch the engines' model lists when it has none saved, and how often it asks. */
export const MODELS_WAIT_MS = 90_000, MODELS_POLL_MS = 1500;
/** While an engine's model list has failed, the settings are read again after this, doubling up to the bound. */
export const CATALOGUE_RETRY_MS = 10_000, CATALOGUE_RETRY_MAX_MS = 300_000;
/**
 * The shortest gap between refreshes a stream of one project's machine events can cause: the first event reads at once,
 * the rest of a burst arrive together in one read after the gap. Each project has its own gap, so one project's events
 * never delay another's. A hidden window needs them even less often.
 */
export const PUSH_GAP_MS = 250, HIDDEN_PUSH_GAP_MS = 15_000;
/** A screen showing the machine overview that opens again rereads it when the last read is older than this. */
export const OVERVIEW_STALE_MS = 60_000;
const sameConnection = (a: PortConnection, b: PortConnection) => a.state === b.state && a.reason === b.reason;
/**
 * A sprint's live graph as last read. `unsupported`: this backend has no live graph, so the sprint is read-only. A failed read
 * keeps the graph read before it, marked as not current. `key` names what the screens showed when it was asked for.
 */
export type LiveGraphState = { status: "loading" | "ready" | "unsupported" | "failed"; graph: LiveSprintGraph | null; message: string | null; key: string | null };
const transientRead = (error: unknown) => /^(machine_busy|machine_tool_timeout|service_request_failed|bridge_request_refused|resync_required)$/.test(String((error as { code?: unknown })?.code ?? reasonOf(error)).replace(/^refused:/, ""));

/** The machine this app runs on is connected exactly when its service is. */
/** The local machine as the connection shows it, made once per record: a read while offline must not hand every screen a new machine. */
const shownLocal = new WeakMap<WorkspaceState["machines"][number], WorkspaceState["machines"][number]>();
function withConnection(state: WorkspaceState, connection: PortConnection): WorkspaceState {
  const live = connection.state === "connected" ? "connected" : "disconnected";
  const local = state.machines.find(m => m.kind === "local");
  if (!local || local.connection === live) return state;
  let shown = shownLocal.get(local);
  if (shown?.connection !== live) { shown = { ...local, connection: live, lastConfirmedAt: local.lastConfirmedAt }; shownLocal.set(local, shown); }
  return { ...state, machines: state.machines.map(m => m === local ? shown! : m) };
}

/**
 * The screens' only view of a backend. Versioned projections and acknowledged rename/queue patches change the display.
 * A command receipt settles before its background reconciliation. A failure stays a failure; no requested effect is guessed.
 */
export function createWorkspaceStore(port: WorkspacePort & LocalCodePort, options: StoreOptions = {}) {
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
  /** The latest coherent read failed (not only slow), shown or not: an owner's explicit re-check says so at once. */
  const [lastReadFailed, setLastReadFailed] = createSignal(false);
  const [syncing, setSyncing] = createSignal(false);
  const [rejected, setRejected] = createSignal(0);
  const [receipt, setReceipt] = createSignal<CommandReceipt | null>(null);
  // Presentation only: one entry per dispatched request, never per click or per reconciliation read.
  const [pendingRequests, setPendingRequests] = createSignal<ReadonlyMap<string, WorkspaceCommand>>(new Map());
  const [savedProjects, setSavedProjects] = createSignal<ReadonlyMap<string, string>>(new Map());
  const [acceptanceIssue, setAcceptanceIssue] = createSignal<{ taskId: string; message: string } | null>(null);
  const state = createMemo(() => port.mode === "real" ? withConnection(projected(), connection()) : projected());
  // Unchanged task views and summaries stay the same objects across reads, so rows built from them are not rebuilt.
  // ---------- Live sprint graphs: read when a screen shows the sprint, and again when what it shows changes ----------
  const [graphs, setGraphs] = createSignal<ReadonlyMap<string, LiveGraphState>>(new Map());
  const graphReads = new Map<string, { seq: number; running: boolean; again: string | null }>();
  const putGraph = (sprintId: string, next: LiveGraphState) => setGraphs(held => new Map(held).set(sprintId, next));
  const readGraph = (sprintId: string, key: string | null, force = false): Promise<void> => {
    const held = graphs().get(sprintId), read = graphReads.get(sprintId) ?? { seq: 0, running: false, again: null };
    graphReads.set(sprintId, read);
    if (!port.sprintGraph) { if (held?.status !== "unsupported") putGraph(sprintId, { status: "unsupported", graph: null, message: null, key }); return Promise.resolve(); }
    if (read.running) { if (force || key !== held?.key) read.again = key ?? ""; return Promise.resolve(); }
    if (!force && held && held.key === key && held.status !== "failed") return Promise.resolve();
    const seq = ++read.seq;
    read.running = true;
    putGraph(sprintId, { status: "loading", graph: held?.graph ?? null, message: null, key });
    return port.sprintGraph.read(sprintId).then(reply => {
      if (read.seq !== seq) return;
      putGraph(sprintId, reply.kind === "graph" ? { status: "ready", graph: reply.graph, message: null, key }
        : reply.kind === "unsupported" ? { status: "unsupported", graph: null, message: null, key }
        : { status: "failed", graph: held?.graph ?? null, message: reply.message, key });
    }, error => {
      if (read.seq === seq) putGraph(sprintId, { status: "failed", graph: held?.graph ?? null, message: reasonOf(error) || "The sprint graph could not be read.", key });
    }).finally(() => {
      read.running = false;
      const again = read.again;
      read.again = null;
      if (again !== null) void readGraph(sprintId, again === "" ? key : again, true);
    });
  };
  /** The service's execution order of each sprint whose live graph is current. A stale or failed read orders nothing. */
  const orders = createMemo<ReadonlyMap<string, SprintOrder>>(() => new Map([...graphs()].flatMap(([id, g]) => g.status === "ready" && g.graph ? [[id, g.graph.order] as const] : [])),
    new Map(), { equals: (a, b) => a.size === b.size && [...a].every(([id, o]) => b.get(id) === o) });
  /** The drafts a sprint revision still holds back, from every sprint graph read (a failed reread keeps the last graph's). */
  const revisionHolds = createMemo<ReadonlySet<string>>(() => new Set([...graphs().values()].flatMap(g => revisionHeld(g.graph))),
    new Set(), { equals: (a, b) => a.size === b.size && [...a].every(id => b.has(id)) });
  const index = createMemo<WorkspaceIndex>(prev => stableIndex(prev, buildIndex(state(), now, orders(), revisionHolds())));
  // The projects whose tasks a confirmed read delivered in this run. A cached first frame is not one, and neither is the empty
  // stand-in (`standIn`) of a project whose Cortex is away or not activated: neither says what the project holds.
  const [readProjects, setReadProjects] = createSignal<ReadonlySet<string>>(new Set(options.initial?.state.projects.map(p => p.id) ?? []));
  const confirmRead = (update: ProjectionUpdate) => {
    const ids = update.scope === "workspace" ? update.state.projects.map(p => p.id) : update.standIn ? [] : [update.slice.project.id];
    if (ids.some(id => !readProjects().has(id))) setReadProjects(held => new Set([...held, ...ids]));
  };

  /**
   * One update applied on its own: one that cannot be projected (a malformed record, say) is left out and logged, and every
   * other update, and every later read, still applies. A throw here once stopped the queued reads after a coherent read, and
   * the sidebar, header and menus stayed as they were for the rest of the session.
   */
  const guarded = <T,>(apply: () => T, otherwise: T): T => {
    try { return apply(); } catch (error) { console.error("Workspace: an update could not be applied", error); return otherwise; }
  };
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
    confirmRead(update);
    // Even an empty replacement (for example, a deactivated project) stops showing the saved board.
    if (update.scope === "workspace") {
      if (savedProjects().size) setSavedProjects(new Map());
    } else if (savedProjects().has(update.slice.project.id)) {
      setSavedProjects(held => { const next = new Map(held); next.delete(update.slice.project.id); return next; });
    }
    return true;
  };
  // Facts sit over a projection, so a newer one never wipes what the machine said about Git or the owner's decisions.
  const facts = new Map<string, ProjectFacts>();
  const sources = new Map<string, string>();
  const checkoutDrafts = new Map<string, string>();
  const withSources = (held: ProjectFacts): ProjectFacts => !held.repo ? held : { ...held, repo: { ...held.repo, checkouts: held.repo.checkouts.map(c => ({ ...c, draft: checkoutDrafts.get(c.id) ?? c.draft, sources: Object.fromEntries([...sources].flatMap(([key, text]) => key.startsWith(`${c.id}|`) ? [[key.slice(c.id.length + 1), text]] : [])) })) } };
  // A port that numbers its facts reads (`readOrder`, in the order they began) lets a read that began earlier never replace
  // facts from one that began later, however their replies cross.
  const factsOrder = new Map<string, number>();
  /**
   * Per project, the tasks that were already on the board when its last facts read was installed: for those, a missing saved
   * record is an answer (the task has none). A task that appeared after it (just created) is not covered until the next one.
   * The signal moves on every install, so a screen waiting for that read hears of it even when nothing else changed.
   */
  const factsCovered = new Map<string, ReadonlySet<string>>();
  const [factsInstalls, setFactsInstalls] = createSignal(0);
  const installFacts = (update: ProjectFacts) => {
    if (registered && !registered.has(update.projectId)) return;
    const order = update.readOrder;
    if (order !== undefined) { if (order < (factsOrder.get(update.projectId) ?? -Infinity)) return; factsOrder.set(update.projectId, order); }
    const scoped = update as ScopedFacts, scope = scoped.workerScope;
    let next: ProjectFacts = update;
    if (scope) {
      // A read scoped to the tasks on view carries no worker rows for the others. A task whose workers an earlier read
      // confirmed for its current execution keeps them (the last confirmed view, until a read for it replaces them):
      // replaced by another task's rows, a task opened again read as ready with its workers gone. Another execution keeps
      // nothing and is not ready until a read for it arrives.
      const executionOf = (taskId: string) => projected().tasks.find(t => t.id === taskId)?.executionId ?? null;
      const inScope = new Set(scope), kept = new Map<string, string>();
      for (const [taskId, execution] of factsScoped().get(update.projectId) ?? []) if (!inScope.has(taskId) && execution !== null && execution === executionOf(taskId)) kept.set(taskId, execution);
      const retained = facts.get(update.projectId)?.workers.filter(w => kept.has(w.taskId) && !update.workers.some(x => x.id === w.id)) ?? [];
      if (retained.length) {
        const said = new Set((update.messages ?? []).map(x => x.id));
        next = { ...update, workers: [...update.workers, ...retained], messages: [...(update.messages ?? []), ...retained.flatMap(w => w.messages ?? []).filter(x => !said.has(x.id))] };
      }
      // The tasks whose workers, results and messages are now known, with the execution they were read for.
      const confirmed = new Map<string, string | null>([...kept, ...scope.map(taskId => [taskId, scoped.workerExecutions?.[taskId] ?? executionOf(taskId)] as const)]);
      setFactsScoped(all => new Map(all).set(update.projectId, confirmed));
    }
    const held = withSources(next); facts.set(update.projectId, held);
    setProjected(current => {
      factsCovered.set(update.projectId, new Set(current.tasks.flatMap(t => t.projectId === update.projectId ? [t.id] : [])));
      return applyFacts(current, held);
    });
    setFactsInstalls(n => n + 1);
  };
  /** Per project, the tasks a facts read confirmed the workers of, and the execution each was read for. */
  const [factsScoped, setFactsScoped] = createSignal<ReadonlyMap<string, ReadonlyMap<string, string | null>>>(new Map());
  const firstViewPort = port as Partial<FirstViewPort>;
  const installMachine = (update: MachineFacts) => setProjected(current => applyMachineFacts(current, update));
  let machineTimer: ReturnType<typeof setTimeout> | undefined, machineRetry: ReturnType<typeof setTimeout> | undefined;
  let modelsTimer: ReturnType<typeof setTimeout> | undefined, modelsPoll: ReturnType<typeof setTimeout> | undefined, modelsGaveUp = false;
  let overviewWanted = false, overviewReading: Promise<void> | undefined, overviewAgain = false, overviewRetry: ReturnType<typeof setTimeout> | undefined, machineAsked = false;
  let overviewShown = 0, overviewAt: number | null = null;
  const readOverview = () => {
    // A read already under way may predate the command that asked for this one: read once more after it.
    if (overviewReading) { overviewAgain = true; return; }
    clearTimeout(overviewRetry);
    overviewReading = port.query("machine.overview", {}).then(updates => {
      if (!active) return;
      overviewAt = Date.now();
      batch(() => { for (const update of updates ?? []) installMachine(update); setOverviewIssue(null); setOverviewLoaded(true); });
    }).catch(error => {
      if (!active) return;
      setOverviewIssue(reasonOf(error));
      // Like the machine's settings: nothing else would ask again soon, while a screen showing it is on view.
      // The rest of the interface never waits for it.
      clearTimeout(overviewRetry);
      if (overviewShown > 0) overviewRetry = setTimeout(readOverview, RETRY_MS);
    }).finally(() => {
      overviewReading = undefined;
      if (overviewAgain && active) { overviewAgain = false; readOverview(); }
    });
  };
  /**
   * A screen on view shows the machine's overview fields, until the returned function is called. The first call reads them,
   * once the service has said what it offers; a later one rereads them only when the last read is older than
   * `OVERVIEW_STALE_MS`. While such a screen is on view the port's machine poll keeps them current; with none, nothing does.
   */
  const wantOverview = () => {
    overviewShown++;
    const release = port.watchOverview?.() ?? (() => {});
    const first = !overviewWanted;
    overviewWanted = true;
    if (machineAsked && (first || (overviewAt !== null && Date.now() - overviewAt >= OVERVIEW_STALE_MS))) readOverview();
    let released = false;
    return () => { if (released) return; released = true; overviewShown--; if (!overviewShown) clearTimeout(overviewRetry); release(); };
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
      // A read that was only slow is no issue: what the machine last said stays, and it is read again.
      if (!slowRead(error)) setMachineIssue(reasonOf(error));
      // Busy or a lost read is worth the wait; a refusal is an answer: open the interface now and say so, still retrying.
      if (!transientRead(error) && !slowRead(error)) { clearTimeout(machineTimer); setMachineSettled(true); }
      // Nothing else would ask again soon: the composer must not stay without its models until some later refresh.
      clearTimeout(machineRetry);
      machineRetry = setTimeout(() => readMachine(), RETRY_MS);
    }).finally(() => { machineReading = undefined; });
  };
  let refreshing: Promise<void> | undefined, transientFailures = 0;
  const queuedReads = new Set<string | undefined>();
  const queuedDeltas = new Map<string, ProjectionUpdate>();
  // What the port's own follow-up reads delivered while a coherent read ran: the details of a board just read, and numbered
  // facts. They are applied after it, never answered with another read: such a reread would deliver them again during the
  // next read, and a slow machine would never stop confirming.
  const queuedEnrichments = new Map<string, ProjectionUpdate>();
  const queuedFacts = new Map<string, ProjectFacts>();
  const installEnrichment = (update: ProjectionUpdate) => batch(() => {
    if (!install(update, true) || update.scope !== "project") return;
    const held = facts.get(update.slice.project.id);
    if (held) installFacts(held);
  });
  let actionEpoch = 0, fullReadQueued = false;
  /** A discarded read is read again with its own scope: one project's read never turns into a reread of every project. */
  const requeueRead = (projectId?: string) => {
    queuedReads.add(projectId);
    // The port may already have consumed an event's invalidation set.
    if (!projectId) fullReadQueued = true;
  };
  /** The project an owner action concerns is read first after the read under way, before other queued reads. */
  const queueFirst = (projectId: string) => {
    const rest = [...queuedReads].filter(id => id !== projectId);
    queuedReads.clear(); queuedReads.add(projectId);
    for (const id of rest) queuedReads.add(id);
  };
  // The confirmed state is settled once the coherent read ended and what the port answers late of it has arrived too.
  let settling = 0;
  const settleSyncing = () => {
    const late = port.presented?.();
    if (!late) { setSyncing(false); return; }
    const mine = ++settling;
    let timer: ReturnType<typeof setTimeout> | undefined;
    void Promise.race([late, new Promise<void>(resolve => { timer = setTimeout(resolve, LATE_CONFIRM_MS); })]).then(() => {
      clearTimeout(timer);
      if (active && mine === settling && !refreshing) setSyncing(false);
    });
  };
  let machineReading: Promise<void> | undefined;
  /** `first`: an owner action's project, read before anything else already queued. */
  const refresh = (projectId?: string, first = false): Promise<void> => {
    if (refreshing) { if (first && projectId) queueFirst(projectId); else queuedReads.add(projectId); return refreshing; }
    const full = !projectId && fullReadQueued;
    if (!projectId) fullReadQueued = false;
    coveredBy(projectId);
    refreshing = readCoherent(projectId, full).finally(() => {
      refreshing = undefined;
      for (const update of queuedDeltas.values()) guarded(() => install(update), false);
      queuedDeltas.clear();
      if (active) {
        for (const update of queuedEnrichments.values()) guarded(() => installEnrichment(update), undefined);
        for (const update of queuedFacts.values()) guarded(() => installFacts(update), undefined);
      }
      queuedEnrichments.clear(); queuedFacts.clear();
      if (queuedReads.size && active) {
        // Always read again after a command reply, even when an older poll is in flight. One project at a time, in the
        // order queued (an owner action's project at the head), and a read of every project last: a project read never
        // widens into a reread of the whole machine.
        const next = [...queuedReads].find(id => id !== undefined);
        queuedReads.delete(next);
        void refresh(next);
      } else settleSyncing();
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
      // Facts own sprint membership, hidden planning/purged rows and owner decisions. Both are asked for at once, in this
      // order: the port binds the facts read to the task read it follows (the same projects, and the board it prepares).
      const [snapshots, read] = await Promise.all([port.query("project.snapshot", full ? { full: true } : parameters), port.query("project.facts", parameters)]);
      const product = read ?? [];
      if (!active) return;
      if (epoch !== actionEpoch) { requeueRead(projectId); return; }
      batch(() => {
        setCapabilities(offered);
        for (const update of snapshots) guarded(() => install(update, true), false);
        for (const update of product) guarded(() => installFacts(update), undefined);
        // A coherent board is installed without the facts held for its project (they own archived, purged and planning rows).
        // When this read brought none for it (its facts read was still under way, or failed), the last confirmed facts stay
        // over the new board: rows are never shown from the board alone, then hidden when the facts arrive a moment later.
        const delivered = new Set(product.map(f => f.projectId));
        const boards = snapshots.flatMap(u => u.scope === "workspace" ? u.state.projects.map(p => p.id) : [u.slice.project.id]);
        for (const id of new Set(boards)) {
          const held = facts.get(id);
          if (held && !delivered.has(id)) guarded(() => setProjected(current => applyFacts(current, held)), undefined);
        }
        setConnection({ state: "connected", reason: null, at: Date.now() });
        setReady(true);
        setReadIssue(null);
        setLastReadFailed(false);
        transientFailures = 0;
        lastRead = Date.now();
      });
    } catch (error) {
      if (!active) return;
      if (epoch !== actionEpoch) { requeueRead(projectId); return; }
      // A slow read is never shown as a failure, however often it recurs; it is simply read again. Once a view is confirmed,
      // a failed read keeps it silently: only a failure that persists over three reads in a row is shown.
      const quiet = slowRead(error) || (ready() && ++transientFailures < 3);
      if (!slowRead(error)) setLastReadFailed(true);
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
  // Coalesces a burst of machine events, per project (the key "" is a refresh of everything): the first refreshes at once,
  // the rest arrive together after the gap. While a read runs, an event only queues its project, with no timer: the read
  // after it starts as soon as the one under way ends.
  const pushes = new Map<string, { at: number; timer?: ReturnType<typeof setTimeout> }>();
  /** A read that starts now covers every event that came before it: its project's (or, read whole, every) wait ends. */
  const coveredBy = (projectId?: string) => {
    for (const [key, push] of pushes) if (!projectId || key === projectId) { clearTimeout(push.timer); push.timer = undefined; }
  };
  const pushRefresh = (projectId?: string) => {
    const push = pushes.get(projectId ?? "") ?? { at: -Infinity };
    pushes.set(projectId ?? "", push);
    if (push.timer !== undefined) return;
    if (refreshing) { queuedReads.add(projectId); return; }
    const gap = typeof document !== "undefined" && document.hidden ? HIDDEN_PUSH_GAP_MS : PUSH_GAP_MS;
    const wait = push.at + gap - Date.now();
    if (wait <= 0) { push.at = Date.now(); void refresh(projectId); return; }
    push.timer = setTimeout(() => { push.timer = undefined; push.at = Date.now(); if (active) void refresh(projectId); }, wait);
  };
  onCleanup(() => { active = false; for (const push of pushes.values()) clearTimeout(push.timer); clearTimeout(machineTimer); clearTimeout(machineRetry); clearTimeout(modelsTimer); clearTimeout(modelsPoll); clearTimeout(overviewRetry); });
  const reload = () => { if (!ready()) setConnection({ state: "connecting", reason: null, at: null }); return refresh(); };
  const onUpdate = (update: PortUpdate) => {
    if (update.kind === "preview") {
      // Cached data paints the first frame but cannot seed command versions,
      // capabilities or a native event generation.
      if (!registered && !versions.size) batch(() => {
        const p = update.slice.project;
        setSavedProjects(held => new Map(held).set(p.id, p.repository));
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
      if (refreshing) {
        const key = projectionKey(update.update), queued = queuedEnrichments.get(key);
        if (!queued || compareVersions(update.update.version, queued.version) >= 0) queuedEnrichments.set(key, update.update);
        return;
      }
      installEnrichment(update.update);
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
      if (!refreshing) { installFacts(update.facts); return; }
      // Numbered facts are applied after the coherent read, and only when they began after the facts it installs.
      const order = update.facts.readOrder, queued = queuedFacts.get(update.facts.projectId);
      if (order !== undefined) { if (queued?.readOrder === undefined || order >= queued.readOrder) queuedFacts.set(update.facts.projectId, update.facts); return; }
      // Unnumbered facts cannot be ordered against it: reread rather than replay possibly older ones over it.
      requeueRead(update.facts.projectId);
      return;
    }
    // The port's own machine poll carries the same settings read: what it says about the model lists is followed too.
    if (update.kind === "machine") { batch(() => { installMachine(update.facts); if (update.facts.catalogue) followCatalogue([update.facts]); }); return; }
    if (update.kind === "refresh") { pushRefresh(update.projectId); return; }
    const back = update.connection.state === "connected" && connection().state !== "connected";
    setConnection(update.connection);
    // What happened while it was away arrives as a fresh snapshot, never as a guess.
    if (back) void refresh();
  };
  onCleanup(port.subscribe(update => guarded(() => onUpdate(update), undefined)));
  if (!options.initial) void reload();

  const withheld = new Set(options.withhold ?? []);
  const can = (capability: string) => connection().state === "connected" && !withheld.has(capability) && hasCapability(capabilities(), capability);
  const rememberSource = (projectId: string, checkoutId: string, file: string, text: string) => {
    sources.set(`${checkoutId}|${file}`, text);
    const held = facts.get(projectId);
    if (held) installFacts(held);
  };
  // A refused message is sent again through the store's own dispatch, as one press would.
  const sends = createSends(state, (command: WorkspaceCommand): Promise<CommandReceipt> => self.dispatch(command));
  const self = {
    sends,
    pendingRequest: (command: WorkspaceCommand) => pendingRequests().has(commandKey(command)),
    pendingTask: (type: "start_task" | "close_task" | "accept_result" | "archive_task", taskId: string) =>
      [...pendingRequests().values()].some(c => c.type === type && c.taskId === taskId),
    savedView: (project: Pick<WorkspaceState["projects"][number], "id" | "repository">) => savedProjects().get(project.id) === project.repository,
    mode: port.mode, state, index, ready, readProjects, readIssue, lastReadFailed, machineLoaded, machineIssue, modelsPending, modelsIssue,
    /** The composer's lists are known: the machine answered and is not still fetching the models. Until then an empty list means "not yet". */
    optionsKnown: () => machineLoaded() && !modelsPending(),
    /**
     * The task's own route (its saved settings) has been read. A task's model, effort, access and language come only with its
     * saved record, read with the project's facts; until then the task is not known to use the machine's defaults, and a
     * screen shows a placeholder rather than those defaults. A task that appeared after the project's last facts read (just
     * created) is known once a facts read installed while it was on the board has its record, or still has none.
     */
    /** Moves on every confirmed facts install: a screen holding its last confirmed mode releases it when this moves. */
    factsInstalls,
    taskRouteKnown: (taskId: string): boolean => {
      if (port.mode !== "real") return true;
      const task = state().tasks.find(t => t.id === taskId);
      if (!task) return false;
      // A project whose facts were never read (its machine reads no saved records) has nothing more to say: the machine's route.
      const held = facts.get(task.projectId);
      factsInstalls();
      return !!task.settings.model || !held || !!held.overlays[taskId] || !!factsCovered.get(task.projectId)?.has(taskId);
    },
    /** Storage, retention, worktrees, the version and GitHub CLI have arrived. Until then a screen that shows them says they are loading. */
    overviewLoaded, overviewIssue, wantOverview,
    /** The first screen's gate: the machine has answered and its settings are in, or waiting for them has run out. */
    loaded: () => ready() && machineSettled(),
    syncing, now, receipt, acceptanceIssue, connection, rejected, can, reload: () => track(reload()),
    /** Opening a project's screen, or its manual refresh, keeps the confirmed display and asks only for this project's moving
     *  facts; with `git`, its repository too, read afresh. */
    refreshProjectFacts: async (projectId: string, git = true) => {
      if (port.mode !== "real") return;
      await refreshing;
      if (!active) return;
      const issue = "The project's latest facts could not be read. The last confirmed state is still shown; it will retry automatically.";
      try {
        for (const update of await port.query("project.facts", { projectId, fresh: true, git })) if (active) installFacts(update);
        if (active) setReadIssue(current => current === issue ? null : current);
      } catch (error) {
        if (active && !slowRead(error)) setReadIssue(issue);
      }
    },
    /**
     * A screen on view shows a project's facts (`facts`) or its Git data (`git`) until the returned function is called. The
     * port reads in the background only what some screen holds interest in; opening one reads it once, now. `taskId` is
     * the task it shows: only such tasks' worker rows are read.
     */
    interest: (projectId: string, kind: GitInterest, taskId?: string) => {
      const release = port.interest?.(projectId, kind, taskId) ?? (() => {});
      void self.refreshProjectFacts(projectId, kind !== "facts");
      return release;
    },
    /** When this app last heard from the machine, for a record with no stamp of its own. */
    lastRead: () => lastRead,
    takeDroppedFiles: () => port.takeDroppedFiles?.() ?? Promise.resolve([]),
    readClipboardImage: () => port.readClipboardImage?.() ?? Promise.resolve([]),
    /** The files of a saved task, read when its page opens. Nothing here when this backend keeps none. */
    readTaskAttachments: (taskId: string) => port.readTaskAttachments?.(taskId) ?? Promise.resolve([]),
    /** Files just attached go to the machine once and come back as references. A backend with no machine keeps the bytes in the draft. */
    uploadAttachments: (files: Attachment[]): Promise<AttachmentInput[]> => port.uploadAttachments ? port.uploadAttachments(files) : Promise.resolve(files),
    /** One saved file's bytes, read only when it is opened. */
    readAttachment: (file: AttachmentRef) => port.readAttachment ? port.readAttachment(file) : Promise.reject(new Error("This file can only be opened on the machine that keeps it.")),
    /** Whether this shell can save a copy of an attachment at all. */
    canSaveAttachment: () => !withheld.has(SAVE_ATTACHMENT),
    saveAttachment: (file: Attachment) => { if (withheld.has(SAVE_ATTACHMENT)) return Promise.reject(new Error(COPY.notOffered)); if (port.saveAttachment) return track(port.saveAttachment(file)); downloadAttachment(file); return Promise.resolve({ path: null }); },
    pickFolder: () => withheld.has(FEATURES.pickFolder) ? Promise.resolve(null) : track(port.pickFolder?.() ?? Promise.resolve(null)),
    /** The local machine's remote access, when its service offers it; otherwise a refusal in words. */
    remoteAccess: <O extends RemoteOperation>(operation: O, message: RemoteRequests[O]): Promise<RemoteStatus> => port.remoteAccess && can(FEATURES.remoteAccess) ? port.remoteAccess.call(operation, message) : Promise.reject(new Error("Remote access is not available in this build.")),
    watchRemoteAccess: (listener: () => void) => port.remoteAccess?.watch(listener) ?? (() => {}),
    /** The machine's open sudo password prompts; null when its service has none. The password is sent once and never kept. */
    sudoPrompts: () => port.sudoPrompts ? port.sudoPrompts.list() : Promise.resolve(null),
    answerSudo: (promptId: string, password: string) => port.sudoPrompts ? port.sudoPrompts.answer(promptId, password) : Promise.reject(new Error(COPY.notOffered)),
    cancelSudo: (promptId: string) => port.sudoPrompts ? port.sudoPrompts.cancel(promptId) : Promise.reject(new Error(COPY.notOffered)),
    /** Access the machine's tasks lack (sign-ins, sudo); null when its service has none. A token is sent once and never kept. */
    ownerActions: () => port.ownerActions ? port.ownerActions.list() : Promise.resolve(null),
    actOwner: (projectId: string, actionId: string, action: OwnerActionVerb) => port.ownerActions ? port.ownerActions.act(projectId, actionId, action) : Promise.reject(new Error(COPY.notOffered)),
    ownerToken: (projectId: string, actionId: string, token: string) => port.ownerActions ? port.ownerActions.token(projectId, actionId, token) : Promise.reject(new Error(COPY.notOffered)),
    openSignIn: (actionId: string) => port.ownerActions?.openSignIn ? port.ownerActions.openSignIn(actionId) : Promise.reject(new Error(COPY.notOffered)),
    canOpenSignIn: () => !!port.ownerActions?.openSignIn,
    /** A sprint's live graph as last read, or undefined before any screen asked for it. */
    liveGraph: (sprintId: string): LiveGraphState | undefined => graphs().get(sprintId),
    /** Reads the live graph when `key` (what the screen shows) differs from the last read's, or the last read failed. */
    readLiveGraph: (sprintId: string, key: string | null = null, force = false) => readGraph(sprintId, key, force),
    /**
     * One owner edit of a live graph, sent once with the revision the owner saw. Whatever its answer, the graph is read again
     * at once: a refusal or a lost answer never leaves the old revision on screen as if it were current.
     */
    editLiveGraph: async (edit: SprintGraphEdit): Promise<SprintGraphWrite> => {
      if (!port.sprintGraph) return { outcome: "refused", code: "not_available", message: COPY.notOffered };
      const held = graphs().get(edit.sprintId);
      let result: SprintGraphWrite;
      try { result = await track(port.sprintGraph.edit(edit)); } catch { result = { outcome: "unknown" }; }
      if (result.outcome !== "unchanged" && held) putGraph(edit.sprintId, { ...held, status: "loading" });
      await readGraph(edit.sprintId, held?.key ?? null, true);
      return result;
    },
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
    readCommandOutput: port.readCommandOutput?.bind(port),
    /** Reads a task's conversation again after a read that did not complete. */
    retryHistory: (taskId: string) => port.retryHistory?.(taskId),
    /**
     * Whether a facts read for this task's screen has arrived: its workers, result and messages are then what the machine
     * says, and an empty list means none. Null when this port does not say which tasks a facts read was for, so nothing waits.
     */
    factsShown: (projectId: string, taskId: string): boolean | null => {
      if (!firstViewPort.scopesFacts?.()) return null;
      const read = factsScoped().get(projectId);
      // Read for this execution: readiness never carries over into a new one.
      return !!read?.has(taskId) && read.get(taskId) === (projected().tasks.find(t => t.id === taskId)?.executionId ?? null);
    },
    /** Reads again what a task's screen first waits for (its conversation, questions and facts), after that did not finish. */
    retryFirstView: (projectId: string, taskId: string) => { firstViewPort.retryFirstView?.(taskId); void self.refreshProjectFacts(projectId, false); },
    /** Reads the next older part of a conversation of which only the latest part is held. */
    loadEarlierHistory: (taskId: string, showAllTools?: () => boolean) => port.loadEarlierHistory?.(taskId, showAllTools),
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
    codeLease: withheld.has(FEATURES.editFiles) ? undefined : port.codeLease,
    codeLanguage: () => can(CODE_LANGUAGE) ? port.codeLanguage : undefined,
    canEdit: () => !!port.codeLease && can(FEATURES.editFiles),
    readFile: (checkoutId: string, file: string): Promise<FileRead> => port.readFile?.(checkoutId, file) ?? Promise.resolve({ text: null }),
    /** A commit or pull request text written from the task's changes, or null when this port has none. */
    draftIntegrationText: (request: IntegrationTextRequest): Promise<IntegrationText | null> => port.draftIntegrationText?.(request).catch(() => null) ?? Promise.resolve(null),
    draftCheckoutCommit: (request: Parameters<NonNullable<WorkspacePort["draftCheckoutCommit"]>>[0]) => port.draftCheckoutCommit?.(request).catch(() => null) ?? Promise.resolve(null),
    draftPushSummary: (request: Parameters<NonNullable<WorkspacePort["draftPushSummary"]>>[0]) => port.draftPushSummary?.(request).catch(() => null) ?? Promise.resolve(null),
    /** The task side chat, or undefined where this port has none. It changes nothing the store holds. */
    sideChat: port.sideChat,
    /** Bytes of one picture, video or sound; a port without them shows the file as not viewable. */
    readMedia: (checkoutId: string, file: string): Promise<FileRead> => port.readMedia?.(checkoutId, file) ?? Promise.resolve({ text: null, reason: "Viewing this file is not available here." }),
    /** The project's own skills for a composer's `/` list; null when this port has no such read or it failed. */
    listProjectSkills: (projectId: string): Promise<ProjectSkills | null> => port.listProjectSkills?.(projectId).catch(() => null) ?? Promise.resolve(null),
    /** One level of checkout folders for Code's tree. A port without it lists nothing beyond the repository snapshot. */
    listFolders: async (checkoutId: string, request: FolderListRequest): Promise<FolderListing> => {
      const read = await port.listFolders?.(checkoutId, request);
      return { overview: read?.overview ?? null, folders: Array.isArray(read?.folders) ? read.folders : [] };
    },
    dispatch(command: WorkspaceCommand, opts: { quiet?: boolean } = {}): Promise<CommandReceipt> {
      const settle = (result: CommandReceipt) => {
        // A result that moved on is followed by a fresh read at once: the screen shows the new version's Accept, not an error strip.
        if (command.type === "accept_result") setAcceptanceIssue(result.ok || result.outcome === "stale_result" ? null : { taskId: command.taskId, message: result.message });
        if (!opts.quiet) setReceipt(result); return result;
      };
      // However it was pressed, one action runs once: a repeat joins the one already under way.
      const key = commandKey(command), joined = running.get(key);
      if (joined) return joined;
      const id = requestId();
      // A message reads where it was sent from the press on; its receipt only says whether it arrived.
      sends.begin(id, command);
      if (!can(capabilityOf(command.type))) { const refused: CommandReceipt = { requestId: id, ok: false, outcome: "unsupported", message: COPY.notOffered }; sends.settle(id, refused); return Promise.resolve(settle(refused)); }
      // Owner text is local, not a machine projection. Publish it before a same-tick Commit.
      if (command.type === "git_draft") {
        checkoutDrafts.set(command.checkoutId, command.message);
        const held = [...facts.values()].find(f => f.repo?.checkouts.some(c => c.id === command.checkoutId));
        if (held) installFacts(held);
      }
      const envelope: CommandEnvelope = { requestId: id, operation: command.type, expectedVersion: latest, payload: command };
      const runFromHold = command.type === "run_sprint"
        ? projected().sprints.find(s => s.id === command.sprintId)?.automaticQueue?.holdId
        : undefined;
      const showPending = Object.hasOwn(REQUESTING, command.type);
      const work = port.command(envelope).catch((): CommandReceipt => ({ requestId: id, ok: false, outcome: "unknown_outcome", message: UNKNOWN_OUTCOME })).then(result => {
        // A title job only starts: it changes nothing a read could confirm now. The task shows its description label until
        // the job ends, and then the port rereads that project (an ordinary refresh, never "Updating confirmed state").
        const background = command.type === "generate_title" || command.type === "generate_sprint_title";
        if (!background) actionEpoch++;
        if (result.ok) setProjected(state => ({ ...state,
          tasks: command.type === "rename_task" ? state.tasks.map(t => t.id === command.taskId ? {...t,title:command.title} : t) : state.tasks,
          sprints: state.sprints.map(s => {
            if (!("sprintId" in command) || s.id !== command.sprintId) return s;
            if (command.type === "rename_sprint") return {...s,title:command.title};
            if (command.type === "run_sprint") {
              // The patch receipt acknowledges a start, not a hold published by a later scheduler pass.
              const hold = s.automaticQueue?.holdId;
              if (s.queueState === "paused" && hold && hold !== runFromHold) return s;
              return {...s,queueState:"running",pauseReason:null};
            }
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
        if (port.mode === "real" && result.outcome !== "local" && !background) {
          setSyncing(true);
          const projectId = ("projectId" in command && command.projectId)
            || ("taskId" in command && state().tasks.find(t => t.id === command.taskId)?.projectId)
            || ("sprintId" in command && state().sprints.find(s => s.id === command.sprintId)?.projectId)
            || ("checkoutId" in command && state().repos.find(r => r.checkouts.some(c => c.id === command.checkoutId))?.projectId);
          const registryChange = ["add_project","activate_project","rename_project","remove_project"].includes(command.type);
          void refresh(registryChange ? undefined : projectId || undefined, true);
          // A refusal that says the displayed state is stale is followed at once by a fresh read of the project's facts.
          if (!result.ok && projectId && STALE_STATE_REFUSALS.has(result.outcome)) void self.refreshProjectFacts(projectId, false);
        }
        sends.settle(id, result);
        return settle(result);
      }).finally(() => {
        running.delete(key);
        if (showPending) setPendingRequests(held => { const next = new Map(held); next.delete(key); return next; });
      });
      running.set(key, work);
      if (showPending) setPendingRequests(held => new Map(held).set(key, command));
      return track(work);
    },
  };
  return self;
}
/** Receipt outcomes that mean the owner acted on a state that has moved on. */
const STALE_STATE_REFUSALS = new Set(["task_not_paused", "task_paused", "stale_version", "stale_epoch", "task_not_resumable", "pause_settling", "stale_result"]);
export type WorkspaceStore = ReturnType<typeof createWorkspaceStore>;
