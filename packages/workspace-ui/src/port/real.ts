import { wireAttachments as checkedAttachments } from "../attachments";
import { capabilities as localCapabilities, decodeResponse, negotiate, PROTOCOL, validate } from "@cortex/workspace-contract";
import type { Answer as WireAnswer, Decision as WireDecision, Receipt as WireReceipt, Result as WireResult, Task as WireTask, HistoryRow, WireTypes } from "@cortex/workspace-contract";
import { ACCESS_MODES, capabilityOf, checksPassed, DomainError, effortLabel, ENGINE_KEYS, engineLabel, FEATURES, LANGUAGES, LOCAL_MACHINE_ID, MACHINE_COMMANDS, mapProjectSlice, readableCheckReason, emptySession, overlayTask, runOrder, validatePlan, WIRE_OPERATIONS } from "@cortex/workspace-domain";
import { conversationOf } from "./history";
import type { CommandEnvelope, CommandReceipt, DirectorySearch, EngineConnection, EngineWeeklyUsage, FileDiff, FileRead, FileSaveResult, Machine, MachineConfig, MachineFacts, Message, PlanItem, PortCapabilities, PortUpdate, ProjectFacts, ProjectionUpdate, ProjectRepo, ProjectSlice, QueryOperation, QueryParameters, QueryResults, SessionOptions, Sprint, TaskChanges, TaskOverlay, Worker, WorkspaceCommand, WorkspacePort } from "@cortex/workspace-domain";

/** How the native bridge reaches the machine service. The bridge owns the socket; this side only ever sees typed messages. */
export type TransportEvent = { type: "connection"; state: "connected" | "disconnected" | "unavailable"; reason?: string } | { type: "invalidated"; projectId: string } | { type: "refresh" };
export type RealTransport = {
  request(operation: string, message: unknown): Promise<unknown>;
  subscribe(listener: (event: TransportEvent) => void): () => void;
};
/** One project the service has registered. Listing them is the service's; here they arrive already resolved. */
export type ProjectLocator = { projectId: string; name: string; repository: string; openBranch: string; defaultBranch: string; activated?: boolean };
export type RealPortOptions = { transport: RealTransport; actorId: string; projects?: ProjectLocator[]; registry?: boolean };

/** For a build with no bridge yet. It fails every call, so the screen says so instead of showing anything. */
export function unavailableTransport(): RealTransport {
  return { request: () => Promise.reject(new Error("The machine service is not connected")), subscribe: () => () => {} };
}

const PAGE_LIMIT = 200;
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
const refusalOf = (requestId: string, error: unknown): CommandReceipt => {
  const text = error instanceof Error ? error.message : String(error);
  if (text === FOLDER_MISSING) return { requestId, ok: false, outcome: FOLDER_MISSING, message: "This folder does not exist yet." };
  if (text === "service_request_failed" || text === "bridge_request_refused") return { requestId, ok: false, outcome: "refused", message: "The machine refused this request. Nothing changed." };
  if (text.startsWith(REFUSED)) return { requestId, ok: false, outcome: "refused", message: text.slice(REFUSED.length) };
  return { requestId, ok: false, outcome: "unknown_outcome", message: OUTCOME_WORDS.unknown_outcome };
};
const unsupported = (requestId: string): CommandReceipt => ({ requestId, ok: false, outcome: "unsupported", message: OUTCOME_WORDS.unsupported });

/** How often a connected app rereads what moves without telling it: Git, workers and the owner's own records. */
const FACTS_POLL_MS = 5000, MACHINE_POLL_EVERY = 6;
type Product = { tasks: Record<string, TaskOverlay>; sprints: (Sprint & { planningTaskId?: string })[]; drafts?: Record<string, string>; messages?: Message[]; comments?: Record<string, TaskChanges["comments"]> };
type MachineRead = Partial<Machine> & { name: string };
type ConfigRead = { config: MachineConfig; connections: EngineConnection[]; tokens: Record<string, unknown>; usage?: EngineWeeklyUsage[] };
class Refusal extends Error { constructor(readonly code: string, message: string) { super(message); } }
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
function sessionOptions(config: MachineConfig): SessionOptions {
  const rules = config.surfaces?.routing;
  const reported = ENGINE_KEYS.flatMap(engine => {
    if (config.setup?.[engine]?.status !== "connected" || config.catalogue?.[engine]?.failed || !rules) return [];
    return (config.catalogue?.[engine]?.models ?? []).filter(m => rules.engines[engine].enabled.includes(m.id)
      && !rules.deny.some(token => token === m.id || token === m.routeId || token === `${engine}/${m.id}` || token === `${engine}/*`))
      .map(m => ({ ...m, engine: engineLabel(engine) }));
  });
  const models = reported.map(({ id, routeId, label, engine, efforts }) => ({ id: routeId ?? id, label, engine, efforts }));
  const efforts = [...new Set(reported.flatMap(m => m.efforts))].map(id => ({ id, label: effortLabel(id) }));
  return { models, efforts, modes: [...ACCESS_MODES], languages: [...LANGUAGES] };
}
/** The files a task in the main checkout left there, once per path. */
function changesOf(repo: ProjectRepo | null, taskId?: string, overlay?: TaskOverlay): FileDiff[] {
  const main = repo?.checkouts.find(c => overlay?.checkout?.kind === "worktree" ? c.taskId === taskId : c.kind === "main");
  const byPath = new Map<string, FileDiff>();
  for (const file of main?.files ?? []) {
    const seen = byPath.get(file.path);
    byPath.set(file.path, seen ? { ...seen, add: seen.add + file.add, del: seen.del + file.del, hunks: [...seen.hunks, ...file.hunks] } : { path: file.path, status: file.status, add: file.add, del: file.del, hunks: file.hunks });
  }
  return [...byPath.values()];
}

/**
 * The production port. It speaks only the public contract, validates every response, and holds the wire identities the
 * view models leave out. A command gets one attempt: a lost response is an unknown outcome and is never sent again.
 */
export function createRealPort(options: RealPortOptions): WorkspacePort {
  const { transport, actorId } = options, projects = options.projects ?? [];
  const observers = new Set<(update: PortUpdate) => void>();
  const tasks = new Map<string, { projectId: string; version: string; executionId?: string | null }>(), decisions = new Map<string, { projectId: string; wire: WireDecision }>();
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
  let offered: string[] = [], serviceMethods: string[] = [], machineOn = false, configTokens: Record<string, unknown> = {}, configRead: ConfigRead | null = null;

  async function machine<T>(op: string, projectId?: string, params?: unknown): Promise<T> {
    const reply = await transport.request("machine.call", { op, ...(projectId ? { projectId } : {}), ...(params === undefined ? {} : { params }) }) as { ok?: boolean; result?: T; code?: string; message?: string };
    if (reply?.ok !== true) throw new Refusal(reply?.code ?? "refused", reply?.message ?? OUTCOME_WORDS.unsupported);
    if (projectId && ["git", "git_init", "repo.file.save", "product.task", "product.integrate"].includes(op)) repoReads.delete(projectId);
    return reply.result as T;
  }
  const projectOfCheckout = (checkoutId: string) => checkoutId.split(":")[0];
  const projectOfTask = (taskId: string) => tasks.get(taskId)?.projectId ?? null;
  const projectOfSprint = async (sprintId: string) => {
    for (const project of projects) if ((await machine<Product>("product.read", project.projectId)).sprints.some(s => s.id === sprintId)) return project.projectId;
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
  // Automatic queues are advanced by the service, never by a screen read.

  /** Why this project cannot be worked on now. The machine can be perfectly well: this is the project's own state. */
  async function issueOf(project: ProjectLocator): Promise<string | null> {
    if (project.activated === false || !serviceMethods.includes("projects.status")) return null;
    const status = await transport.request("projects.status", { projectId: project.projectId }) as { ready?: boolean; state?: string; reason?: string | null };
    if (status?.ready === true) return null;
    if (status?.state === "starting" && !status.reason) return "This project is starting.";
    if (status?.state === "recovering") return "This project is busy and catching up.";
    if (status?.reason?.startsWith("dev_project_refused")) return "The dev store ownership changed. Release data was not opened. Check the dev project folder before retrying.";
    return PROJECT_ISSUES[status?.reason ?? ""] ?? "This project could not start.";
  }

  type RepoRead = ProjectRepo & { openBranch: string; defaultBranch: string; taskDiffs?: Record<string,{files:FileDiff[];reason?:string}> };
  const repoReads = new Map<string, { at: number; pending: Promise<RepoRead | null>; running: boolean; value: RepoRead | null; error: boolean }>();
  async function repositoryFacts(id: string) {
    let entry = repoReads.get(id);
    if (!entry || (!entry.running && Date.now() - entry.at > 3000)) {
      const previous = entry?.value ?? null;
      entry = { at: Date.now(), pending: machine<RepoRead | null>("repo.snapshot", id), running: true, value: previous, error: false };
      repoReads.set(id, entry);
      const current = entry;
      let deferred = false;
      current.pending = current.pending.then(value => {
        current.value = value;
        if (deferred && repoReads.get(id) === current) for (const notify of observers) notify({kind:"refresh"});
        return value;
      }).catch(() => { current.error = true; return current.value; }).finally(() => { current.running = false; current.at = Date.now(); });
      // Only fast Git belongs on the coherent task refresh path. A large checkout finishes separately.
      let timer: ReturnType<typeof setTimeout> | undefined;
      await Promise.race([current.pending, new Promise<void>(resolve => { timer = setTimeout(() => { deferred = true; resolve(); }, 50); })]);
      clearTimeout(timer);
    }
    return entry;
  }

  async function factsOf(project: ProjectLocator): Promise<ProjectFacts> {
    const id = project.projectId;
    let product = await machine<Product>("product.read", id);
    // Queue scheduling and planning result adoption belong to the owning service child.
    // Reading the screen never starts work.
    const repository = await repositoryFacts(id), repo = repository.value;
    if (repo) for (const c of repo.checkouts) c.draft = localDrafts.get(c.id) ?? c.draft;
    const started = [...tasks].filter(([, t]) => t.projectId === id && t.executionId).map(([taskId]) => taskId);
    const workers = project.activated === false || !started.length ? [] : await machine<Worker[]>("workers", id, { taskIds: started });
    for (const w of workers) {
      workerTasks.set(w.id, w.taskId);
      if (w.result) w.result = {...w.result, checks:w.result.checks.map(c => ({...c, detail:readableCheckReason(c.detail)}))};
    }
    const ownMessages = [...(product.messages ?? []), ...workers.flatMap(w => w.messages ?? [])];
    for (let i = messages.length - 1; i >= 0; i--) if (tasks.get(messages[i].taskId)?.projectId === id) messages.splice(i, 1);
    messages.push(...ownMessages);
    const reviews: TaskChanges[] = started.filter(taskId => product.tasks[taskId]?.status !== "done").map(taskId => ({ taskId, sha: "", unavailableReason: repo?.taskDiffs?.[taskId]?.reason ?? (!repo || repository.error ? "Repository details are updating or unavailable. Refresh Code or inspect Git before reviewing changes." : undefined), files: repo?.taskDiffs?.[taskId]?.files ?? changesOf(repo, taskId, product.tasks[taskId]), scopes: [], comments: product.comments?.[taskId] ?? [] }));
    return { projectId: id, issue: await issueOf(project), openBranch: repo?.openBranch ?? "", defaultBranch: repo?.defaultBranch ?? "", repo, workers, reviews, overlays: product.tasks, sprints: product.sprints.map(withPlanning), messages: messages.filter(m => tasks.get(m.taskId)?.projectId === id),
      heldAnswers: Object.fromEntries([...held.values()].flatMap(batch => [...batch].filter(([decisionId]) => decisions.get(decisionId)?.projectId === id).map(([decisionId, answer]) => [decisionId, answer.answer.text]))) };
  }

  const configBases = new Map<string, Record<string, unknown>>();
  async function machineFacts(): Promise<MachineFacts> {
    const overview = await machine<MachineRead>("overview");
    // First activation and background engine setup can create files without a UI command.
    configRead = await machine<ConfigRead>("config.read");
    configTokens = configRead.tokens;
    for (const [surface, revision] of Object.entries(configRead.config.revisions ?? {})) configBases.set(`${surface}:${revision}`, configTokens);
    for (const project of projects.filter(p => p.activated !== false)) {
      const saved = await machine<{value: MachineConfig["projects"][string]; tokens: Record<string, unknown>}>("project.config.read", project.projectId);
      configRead.config.projects[project.projectId] = saved.value;
      projectConfigTokens.set(project.projectId, saved.tokens);
      for (const [surface,revision] of Object.entries(saved.value.revisions ?? {})) projectConfigBases.set(`${project.projectId}:${surface}:${revision}`, saved.tokens);
    }
    const { name, retentionDays, autoArchiveDays, worktreeRoot, defaultWorktreeRoot, version, diskMb, storage, cleanup, diagnostics, ghAvailable, reclamationFailures } = overview;
    return { machineId: LOCAL_MACHINE_ID, engineUsage: configRead.usage ?? [], options: sessionOptions(configRead.config), patch: { ghAvailable, reclamationFailures, name, retentionDays, autoArchiveDays, worktreeRoot, defaultWorktreeRoot, version, diskMb, storage, cleanup, diagnostics, config: configRead.config, connections: configRead.connections, lastConfirmedAt: new Date().toISOString() } };
  }

  const read = async <K extends keyof WireTypes>(type: K, operation: string, message: unknown) => decodeResponse(type, await transport.request(operation, message)).value;

  async function capabilitiesGet(): Promise<PortCapabilities> {
    if (options.registry) {
      const service = await transport.request("service.capabilities", {}) as { methods?: unknown };
      if (!Array.isArray(service.methods) || !service.methods.every(m => typeof m === "string")) throw new Error("Invalid service capabilities");
      serviceMethods = service.methods;
      machineOn = (service as { machine?: unknown }).machine === true;
      const registry = await transport.request("projects.list", {});
      if (!Array.isArray(registry)) throw new Error("Invalid project registry");
      const next: ProjectLocator[] = registry.map(p => {
        if (typeof p.projectId !== "string" || typeof p.displayName !== "string" || typeof p.canonicalRoot !== "string") throw new Error("Invalid project registry");
        return {projectId:p.projectId,name:p.displayName,repository:p.canonicalRoot,openBranch:"",defaultBranch:"",activated:p.activation != null};
      });
      projects.splice(0, projects.length, ...next);
      for (const notify of observers) notify({kind:"registry",projectIds:next.map(p => p.projectId)});
    }
    const remote = await read("Capabilities", "capabilities.get", { protocol: { ...PROTOCOL }, requestId: "read-capabilities" });
    const shared = negotiate(localCapabilities("workspace-ui", "0", remote.operations), remote);
    offered = shared.operations;
    attachmentSupport = remote.attachments === true;
    return { everything: false, operations: [...(attachmentSupport ? ["workspace.attachments"] : []), ...WIRE_OPERATIONS.filter(operation => offered.includes(operation)), ...(serviceMethods.includes("native.pick_folder") ? ["workspace.pick_folder"] : []), ...(serviceMethods.includes("directories.search") ? ["workspace.search_folders"] : []), ...(serviceMethods.includes("projects.register") ? ["workspace.add_project"] : []), ...(serviceMethods.includes("projects.rename") ? ["workspace.rename_project"] : []), ...(serviceMethods.includes("projects.activate") ? ["workspace.activate_project"] : []), ...(serviceMethods.includes("projects.remove") ? ["workspace.remove_project"] : []), ...(serviceMethods.includes("native.open_editor") ? ["workspace.open_editor"] : []), ...(serviceMethods.includes("native.open_editor.vscode") ? ["workspace.open_vscode"] : []), ...(serviceMethods.includes("native.open_logs") ? ["workspace.open_logs"] : []), ...(machineOn ? [...MACHINE_COMMANDS.map(capabilityOf), FEATURES.editFiles, "workspace.worker_messages", "workspace.task_worktrees", "workspace.task_route"] : [])] };
  }

  async function projectTasks(project: ProjectLocator) {
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
    return { version: snapshot.version, items };
  }
  async function projectDecisions(project: ProjectLocator, tasks: WireTask[]): Promise<WireDecision[]> {
    if (!offered.includes("decisions.list")) return [];
    const items: WireDecision[] = [];
    for (const task of tasks.filter(t => t.executionId !== null)) {
      let cursor: string | null = null;
      for (let page = 0; page < PAGE_LIMIT; page++) {
        const next: WireTypes["DecisionPage"] = await read("DecisionPage", "decisions.list", request(project.projectId, "decisions.list", { target: task.executionId, cursor }));
        if (next.items.some(d => d.executionId !== task.executionId)) throw new Error("Question execution identity mismatch");
        items.push(...next.items); cursor = next.nextCursor;
        if (cursor === null) break;
      }
      if (cursor !== null) throw new Error("Question history exceeds the read budget");
    }
    return items;
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
  async function readSnapshot(project: ProjectLocator): Promise<ProjectionUpdate> {
    // A project whose Cortex is away still has a page, with its reason on it. A store keeps what it
    // last confirmed, because version zero is never newer. The machine stays connected for the rest.
    const away = project.activated !== false && serviceMethods.includes("projects.status") && (await transport.request("projects.status", { projectId: project.projectId }) as { ready?: boolean }).ready !== true;
    const { version, items } = project.activated === false || away ? {version:"0",items:[] as WireTask[]} : await projectTasks(project);
    const asked = project.activated === false || away ? [] : await projectDecisions(project, items), results = away ? [] : await projectResults(project, items);
    const histories = new Map<string, HistoryRow[]>();
    if (offered.includes("history.page")) for (const task of items.filter(t => t.executionId !== null)) {
      const rows: HistoryRow[] = [];
      let cursor: string | null = null;
      for (let page = 0; page < PAGE_LIMIT; page++) {
        const next: WireTypes["HistoryPage"] = await read("HistoryPage", "history.page", request(project.projectId, "history.page", {target:task.taskId,cursor}));
        rows.push(...next.items); cursor = next.nextCursor;
        if (cursor === null) break;
      }
      if (cursor !== null) throw new Error("Conversation exceeds the read budget");
      histories.set(task.taskId, rows);
    }
    for (const wire of items) tasks.set(wire.taskId, { projectId: project.projectId, version: wire.version, executionId: wire.executionId });
    for (const [key, value] of decisions) if (value.projectId === project.projectId) decisions.delete(key);
    for (const wire of asked) decisions.set(wire.decisionId, { projectId: project.projectId, wire });
    const { projectId: id, name, repository, openBranch, defaultBranch } = project;
    const stamps = [...items.map(t => t.lastConfirmedAt), ...asked.map(d => d.createdAt), ...[...histories.values()].flatMap(rows => rows.map(r => r.recordedAt))]
      .filter((s): s is string => typeof s === "string").map(Date.parse).filter(Number.isFinite);
    const at = stamps.length ? Math.max(...stamps) : undefined;
    const slice = mapProjectSlice({ id, machineId: LOCAL_MACHINE_ID, name, repository, openBranch, defaultBranch, activated:project.activated, confirmedAt:at }, items, asked, results);
    for (const task of items) {
      let session = slice.sessions.find(s => s.taskId === task.taskId);
      if (!session) { session = emptySession(task.taskId); slice.sessions.push(session); }
      const own = asked.filter(d => d.executionId === task.executionId);
      const dated = [
        ...conversationOf(histories.get(task.taskId) ?? [], { description: task.description, answers: own.flatMap(d => typeof d.answerText === "string" ? [d.answerText.trim()] : []) }),
        ...own.map(d => ({at:typeof d.createdAt === "string" ? d.createdAt : "",entry:{id:`decision:${d.decisionId}`,kind:"decision" as const,decisionId:d.decisionId}})),
      ];
      session.timeline = dated.sort((a,b) => a.at.localeCompare(b.at)).map(d => d.entry);
    }
    slices.set(project.projectId, slice);
    return { kind: "projection", version, scope: "project", slice };
  }

  const reads = new Map<string, Promise<ProjectionUpdate>>();
  function snapshotOf(project: ProjectLocator): Promise<ProjectionUpdate> {
    const previous = reads.get(project.projectId) ?? Promise.resolve();
    const next = previous.catch(() => {}).then(() => readSnapshot(project));
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
      const { wire } = decision, option = command.optionId === undefined ? wire.options.find(o => o.label === command.answer) : wire.options.find(o => o.optionId === command.optionId);
      if (command.optionId && !option) return null;
      const answer: WireAnswer = { ...(command.attachments?.length ? { attachments: wireAttachments(command.attachments) } : {}), decisionId: wire.decisionId, expectedVersion: wire.version, questionId: wire.questionId, optionId: option?.optionId ?? null, text: command.answer };
      const members = wire.batchId === null ? [] : [...decisions.values()].filter(d => d.wire.batchId === wire.batchId && d.wire.executionId === wire.executionId);
      let answers = [answer];
      if (wire.batchId !== null && members.length > 1) {
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
  const sprintPatch = (projectId: string, sprintId: string, patch: unknown) => machine("product.sprint", projectId, { action: "patch", sprintId, patch });
  const profileCall = (verb: string, fields: Record<string, unknown>) => machine("config.action", undefined, { action: "profile", verb, expected_revision: configTokens.routing_profiles, ...fields });
  const connectionCall = (connection_id: string, operation: string, fields: Record<string, unknown> = {}) => machine("config.action", undefined, { action: "connection_update", expected_revision: configTokens.connections, connection_id, operation, ...fields });

  /** Everything past the wire. Each case is one real effect on the machine; a command with no case stays unsupported. */
  async function machineCommand(envelope: CommandEnvelope): Promise<CommandReceipt | null> {
    const { requestId } = envelope, command = envelope.payload;
    const ok = () => done(requestId);
    const task = (taskId: string) => { const id = projectOfTask(taskId); if (!id) throw new Refusal("unknown_task", "Unknown task."); return id; };
    const execution = (taskId: string) => { const e = tasks.get(taskId)?.executionId; if (!e) throw new Refusal("not_started", "This task has not started."); return e; };
    const prompt = async (taskId: string, text: string, key = requestId, attachments?: Message["attachments"]) => { const r = await transport.request("executions.prompt", { projectId: task(taskId), executionId: execution(taskId), text, requestId:key, ...(attachments?.length ? {attachments:wireAttachments(attachments)} : {}) }) as {outcome?:string}; if (r.outcome !== "recorded") throw new Refusal("unknown_outcome", OUTCOME_WORDS.unknown_outcome); return ok(); };
    const control = async (taskId: string, params: Record<string, unknown>) => { const reply = await transport.request("executions.control", {projectId:task(taskId), executionId:execution(taskId), requestId, ...params}) as {outcome?:string;message?:string}; if (reply.outcome && !RECORDED.has(reply.outcome) && reply.outcome !== "stop_requested") throw new Refusal(reply.outcome, reply.message ?? OUTCOME_WORDS[reply.outcome] ?? "The action could not run. Refresh and try again."); return ok(); };
    const messageCall = async (taskId: string, params: Record<string, unknown>) => { await machine("product.message", task(taskId), params); return ok(); };
    const workerTask = (workerId: string) => { const id = workerTasks.get(workerId); if (!id) throw new Refusal("worker_unavailable", "This worker is not available."); return id; };
    const note = (taskId: string, text: string, delivery: "queued" | "recorded", attachments?: Message["attachments"]) => messageCall(taskId, {action:"add",message:{ id: requestId, taskId, workerId: null, text, mode: "queue", delivery, at: Date.now(), deliveredAt: null, failure: null, ...(attachments?.length ? { attachments: wireAttachments(attachments) } : {}) }});
    const say = async (taskId: string, text: string, attachments?: Message["attachments"]) => { await prompt(taskId, text, requestId, attachments); await taskCall(task(taskId), {action:"reopen",taskId}); return ok(); };
    const queued = (id: string) => { const at = messages.findIndex(m => m.id === id && m.delivery === "queued"); if (at < 0) throw new Refusal("unknown_message", "This message is no longer in the queue."); return at; };
    const git = async (checkoutId: string, params: Record<string, unknown>) => { await machine("git", projectOfCheckout(checkoutId), { checkoutId, ...params }); return ok(); };
    const local = async (patch: unknown) => { await machine("machine.update", undefined, { patch }); return ok(); };
    const dirty = async <T>(run: Promise<T>) => { try { return await run; } finally { configRead = null; } };
    switch (command.type) {
      case "open_editor": await transport.request("native.open_editor", {projectId:command.projectId, ...(command.checkoutId ? {checkoutId:command.checkoutId} : {}), ...(command.file ? {file:command.file} : {}), ...(command.line ? {line:command.line} : {})}); return ok();
      case "remove_project": {
        if (!command.confirmed) return {requestId,ok:false,outcome:"invalid_request",message:"Confirm removing the project first."};
        await transport.request("projects.remove", {projectId:command.projectId,confirmed:true});
        projects.splice(0,projects.length,...projects.filter(p => p.projectId !== command.projectId));
        slices.delete(command.projectId);
        return ok();
      }
      case "stop_task": return control(command.taskId, {action:"pause",expectedVersion:tasks.get(command.taskId)?.version});
      case "continue_task": return control(command.taskId, {action:"resume",expectedVersion:tasks.get(command.taskId)?.version});
      // Worker guidance targets one attempt; task steering uses durable source capture.
      case "send_message":
        if (command.workerId) return control(command.taskId, {action:"worker_message",workerId:command.workerId,text:command.text,attachments:wireAttachments(command.attachments)});
        return command.mode === "queue" ? note(command.taskId, command.text, "queued", command.attachments) : say(command.taskId, command.text, command.attachments);
      case "stop_worker": return control(workerTask(command.workerId), {action:"worker_stop",workerId:command.workerId,reason:command.reason});
      case "stop_command": {
        const taskId = workerTask(command.workerId), worker = (await machine<Worker[]>("workers", task(taskId), {taskIds:[taskId]})).find(w => w.id === command.workerId);
        const ref = worker?.activity.find(a => a.sequence === command.sequence)?.toolCallRef;
        if (!ref) throw new Refusal("native_command", "Native engine actions have no stop receipt.");
        return control(taskId, {action:"command_stop",toolCallRef:ref,reason:command.reason});
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
        return messageCall(message.taskId, {action:"remove",messageId:message.id});
      }
      case "request_changes": return say(command.taskId, command.text, command.attachments);
      case "rerun_check": {
        const result = slices.get(task(command.taskId))?.tasks.find(t => t.id === command.taskId)?.result;
        const check = result?.checks[command.checkIndex];
        if (!check || result?.digest !== command.digest) throw new Refusal("stale_result", "This result changed. Review it again.");
        if (check.kind === "manual" || check.kind === "policy" || check.rerunnable !== true || !check.command?.length) throw new Refusal("unsupported", "This item has no runnable command or needs owner authorization. Check it manually; it has not passed.");
        return control(command.taskId, {action:"rerun_check",checkIndex:command.checkIndex,checkName:check.name,digest:command.digest,resultVersion:result.sourceVersion});
      }
      case "add_comment": await machine("product.comment", task(command.taskId), {action:"add",taskId:command.taskId,comment:{id:requestId,path:command.path,line:command.line,text:command.text,sent:false}}); return ok();
      case "edit_comment": await machine("product.comment", task(command.taskId), {action:"edit",taskId:command.taskId,commentId:command.commentId,text:command.text}); return ok();
      case "remove_comment": await machine("product.comment", task(command.taskId), {action:"remove",taskId:command.taskId,commentId:command.commentId}); return ok();
      case "send_comments": {
        const product = await machine<Product>("product.read", task(command.taskId));
        const pending = (product.comments?.[command.taskId] ?? []).filter(c => !c.sent);
        if (!pending.length) throw new Refusal("no_comments", "No draft comments to send.");
        await say(command.taskId, "Please address these review comments:\n\n" + pending.map(c => `${c.path}:${c.line}\n${c.text}`).join("\n\n"));
        await machine("product.comment", task(command.taskId), {action:"mark_sent",taskId:command.taskId,ids:pending.map(c => c.id)});
        return ok();
      }
      case "resolve_block": await taskCall(task(command.taskId), {action:"resolve_block",taskId:command.taskId,path:command.path,blockId:command.blockId,choice:command.choice}); return ok();
      case "finish_merge":
        await taskCall(task(command.taskId), {action:"finish_merge",taskId:command.taskId});
        return say(command.taskId, "I resolved the conflict in the task checkout. Rerun the required checks and return a fresh result for review. Do not merge or push the target branch.");
      case "abort_merge": await taskCall(task(command.taskId), {action:"abort_merge",taskId:command.taskId}); return ok();
      case "resolve_with_cortex":
        await say(command.taskId, "Resolve the retained Git merge conflict in this task's own checkout. Preserve both intended changes, run the required checks, and return a fresh result for my review. Do not merge or push the target branch. Owner guidance: " + command.guidance);
        await taskCall(task(command.taskId), {action:"resolution_started",taskId:command.taskId,guidance:command.guidance});
        return ok();
      case "stop_resolution": await transport.request("executions.cancel", {projectId:task(command.taskId),executionId:execution(command.taskId)}); return ok();
      case "set_task_settings": {
        const id = task(command.taskId), current = (await machine<Product>("product.read", id)).tasks[command.taskId]?.settings;
        const settings = { ...(current ?? {model:"",effort:"",mode:"guarded",language:"auto"}), ...command.settings };
        if (tasks.get(command.taskId)?.executionId) return control(command.taskId, {action:"task_settings",settings});
        await taskCall(id, {action:"settings",taskId:command.taskId,settings});
        return ok();
      }
      case "rename_task": await taskCall(task(command.taskId), { action: "rename", taskId: command.taskId, title: command.title }); return ok();
      case "archive_task": await taskCall(task(command.taskId), { action: "archive", taskId: command.taskId }); return ok();
      case "answer_archive": await taskCall(task(command.taskId), { action: "answer_archive", taskId: command.taskId, discard: command.discard }); return ok();
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
        const executionId = tasks.get(command.taskId)?.executionId;
        if (executionId && command.choice !== "external" && command.choice !== "already_in_target") await control(command.taskId, {action:"close_task",confirmed:command.confirmed});
        await taskCall(task(command.taskId), { action: "close", taskId: command.taskId, choice: command.choice, confirmed: command.confirmed });
        return ok();
      }
      case "accept_result": {
        const result = slices.get(task(command.taskId))?.tasks.find(t => t.id === command.taskId)?.result;
        if (!result?.sourceVersion || result.digest !== command.digest) throw new Refusal("stale_result", "This result changed. Review it again.");
        await taskCall(task(command.taskId), { action: "accept", taskId: command.taskId, digest: command.digest, version: result.sourceVersion, acknowledgeOpenChecks: command.openChecks === true });
        return ok();
      }
      case "integrate": await machine("product.integrate", task(command.taskId), { taskId: command.taskId, method: command.method, confirmed: command.confirmed, digest: command.digest, targetBranch: command.targetBranch, message: command.method === "pr" ? command.title ?? "" : command.message ?? "", body: command.message ?? "", draft: command.draft, deleteWorktree: command.deleteWorktree }); return ok();
      case "set_target":
        if (command.taskId) { await taskCall(task(command.taskId), { action: "target", taskId: command.taskId, branch: command.branch }); return ok(); }
        await sprintPatch(await projectOfSprint(command.sprintId!), command.sprintId!, { targetBranch: command.branch });
        return ok();
      case "git_stage": return git(command.checkoutId, { action: "stage", path: command.path, staged: command.staged, hunk: command.hunk });
      case "git_revert": return git(command.checkoutId, { action: "revert", path: command.path, hunk: command.hunk, confirmed: command.confirmed });
      case "git_draft": localDrafts.set(command.checkoutId, command.message); return { ...ok(), outcome: "local" };
      case "git_commit": { const result = await git(command.checkoutId, { action: "commit", message: command.message, amend: command.amend }); if (localDrafts.get(command.checkoutId) === command.message) localDrafts.delete(command.checkoutId); return result; }
      case "git_fetch": await machine("git", command.projectId, { action: "fetch" }); return ok();
      // Same helper call as every other Git action; the helper creates an empty repository in the project folder.
      case "git_init": await machine("git", command.projectId, { action: "git_init", confirmed: command.confirmed }); return ok();
      case "git_pull": return git(command.checkoutId, { action: "pull" });
      case "git_push": return git(command.checkoutId, { action: "push", remote: command.remote, branch: command.branch, confirmed: command.confirmed });
      case "create_branch": await machine("git", command.projectId, { action: "branch", name: command.name, at: command.at }); return ok();
      case "revert_commit": return git(command.checkoutId, { action: "revert_commit", sha: command.sha, confirmed: command.confirmed });
      case "delete_worktree": return git(command.checkoutId, { action: "delete_worktree", confirmed: command.confirmed, discard: command.discard });
      case "rename_machine": return local({ name: command.name });
      case "set_retention": return local({ retentionDays: command.days });
      case "save_storage": return local({ retentionDays: command.retentionDays, autoArchiveDays: command.autoArchiveDays });
      case "set_worktree_root": return local({ worktreeRoot: command.path });
      case "open_logs": await transport.request("native.open_logs", {}); return ok();
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
        if (!selected || !ENGINE_KEYS.some(engine => available.config.catalogue[engine].models.some(m => (m.routeId ?? m.id) === selected || m.id === selected))) throw new Refusal("model_required", "Connect an engine and select an available model before starting sprint planning. Your description is kept.");
        const { sprintId } = await machine<{ sprintId: string }>("product.sprint", command.projectId, { action: "create", description: command.description, runMode: command.runMode, settings: command.settings, targetBranch: command.targetBranch });
        // The service plans from descriptions without a runtime execution or project tools.
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
      case "pause_sprint": await sprintPatch(await projectOfSprint(command.sprintId), command.sprintId, { queueState: "paused", pauseReason: "Paused by you" }); return ok();
      case "cancel_planning": {
        const id = await projectOfSprint(command.sprintId), sprint = (await machine<Product>("product.read", id)).sprints.find(s => s.id === command.sprintId)!;
        if (sprint.planningTaskId && tasks.get(sprint.planningTaskId)?.executionId) await control(sprint.planningTaskId, {action:"pause",expectedVersion:tasks.get(sprint.planningTaskId)?.version});
        await sprintPatch(id, command.sprintId, { planning: "cancelled" }); return ok();
      }
      case "resume_planning": await sprintPatch(await projectOfSprint(command.sprintId), command.sprintId, { planning: "planning" }); return ok();
      case "edit_plan": {
        const id = await projectOfSprint(command.sprintId), sprint = (await machine<Product>("product.read", id)).sprints.find(s => s.id === command.sprintId)!;
        validatePlan(command.items);
        const plan = { version: (sprint.plan?.version ?? 0) + 1, items: command.items };
        await sprintPatch(id, command.sprintId, { plan, history: [...(sprint.history ?? []), plan], planning: "ready" });
        return ok();
      }
      case "materialize_plan": {
        const id = await projectOfSprint(command.sprintId), sprint = (await machine<Product>("product.read", id)).sprints.find(s => s.id === command.sprintId)!;
        const items = sprint.plan?.items ?? [], created = new Map<string, string>();
        for (const key of runOrder(items)) {
          const item = items.find(i => i.key === key)!;
          const made = await send({ requestId: `${requestId}-${key}`, operation: "create_task", expectedVersion: null, payload: { type: "create_task", projectId: id, description: item.description, settings: sprint.settings ? {mode:"guarded",language:"auto",...sprint.settings} : undefined, checkout: item.checkout ?? { kind: "main" } } });
          if (!made.ok || !made.entityId) return { ...made, requestId };
          created.set(key, made.entityId);
          await taskCall(id, { action: "sprint", taskId: made.entityId, sprintId: command.sprintId, dependencies: item.dependsOn.map(d => created.get(d)) });
          // Saved sprint descriptions use the same service-owned naming flow as standalone tasks.
        }
        await sprintPatch(id, command.sprintId, { planning: "created", materializedVersion: command.version, taskIds: [...created.values()] });
        return ok();
      }
      case "answer_planning": case "plan_message": {
        const id = await projectOfSprint(command.sprintId), sprint = (await machine<Product>("product.read", id)).sprints.find(s => s.id === command.sprintId)!;
        const taskId = sprint.planningTaskId, text = command.type === "answer_planning" ? command.answer : command.text;
        if (!taskId) {
          await machine("product.sprint", id, {action:"note",sprintId:command.sprintId,kind:command.type === "answer_planning" ? "answer" : "message",text});
          await sprintPatch(id, command.sprintId, {planning:"planning"});
          return ok();
        }
        const execution = tasks.get(taskId)?.executionId;
        if (command.type === "answer_planning") {
          // The owner answers the tab in front of them, and a batch may be answered in any order or changed before it goes.
          const named = command.decisionId ? decisions.get(command.decisionId) : undefined;
          const asked = named?.wire.status === "pending" && named.wire.executionId === execution ? named : command.decisionId ? undefined : [...decisions.values()].find(d => d.wire.status === "pending" && !isHeld(d.wire) && d.wire.executionId === execution);
          if (!asked) throw new Refusal("superseded", "This question is no longer open. Refresh before answering.");
          const receipt = await send({ requestId, operation: "answer_decision", expectedVersion: null, payload: { type: "answer_decision", taskId, decisionId: asked.wire.decisionId, answer: text, attachments: command.attachments, ...(command.optionId === undefined ? {} : { optionId: command.optionId }) } });
          if (receipt.ok) await machine("product.sprint", id, {action:"note",sprintId:command.sprintId,kind:"answer",text});
          return {...receipt,requestId};
        }
        await machine("product.sprint", id, {action:"note",sprintId:command.sprintId,kind:"message",text});
        return prompt(taskId, text);
      }
      default: return null;
    }
  }

  async function send(envelope: CommandEnvelope): Promise<CommandReceipt> {
    const wire = wireCommand(envelope);
    if (!wire) return unsupported(envelope.requestId);
    if (wire === "held") return done(envelope.requestId);
    validate("Command", wire.message);
    // Whatever the machine says, a batch was sent, so its questions read as themselves again until a projection says otherwise.
    const sentBatch = (wire.message as { payload?: { batchId?: string | null } }).payload?.batchId;
    if (sentBatch) held.delete(sentBatch);
    let reply: unknown;
    try { reply = await transport.request(wire.operation, wire.message); }
    catch (error) { return refusalOf(envelope.requestId, error); }
    const receipt = decodeResponse("Receipt", reply).value;
    if (receipt.requestId !== envelope.requestId) return {requestId:envelope.requestId,ok:false,outcome:"unknown_outcome",message:OUTCOME_WORDS.unknown_outcome};
    const result = toReceipt(envelope.requestId, receipt);
    // A task just created can be started before its first projection, so its version is kept from the receipt.
    if (result.ok && envelope.payload.type === "create_task") tasks.set(receipt.target, { projectId: envelope.payload.projectId, version: receipt.entityVersion });
    return result;
  }
  const toReceipt = (requestId: string, wire: WireReceipt): CommandReceipt => ({
    requestId, ok: RECORDED.has(wire.outcome), outcome: wire.outcome, message: wire.reason ?? OUTCOME_WORDS[wire.outcome] ?? OUTCOME_WORDS.unknown, entityId: wire.target, entityVersion: wire.entityVersion,
  });

  return {
    mode: "real",
    query<O extends QueryOperation>(operation: O, parameters: QueryParameters[O]): Promise<QueryResults[O]> {
      if (operation === "capabilities.get") return capabilitiesGet() as Promise<QueryResults[O]>;
      if (operation === "machine.facts") return (machineOn ? machineFacts().then(f => [f]) : Promise.resolve([])) as Promise<QueryResults[O]>;
      const { projectId } = parameters as QueryParameters["project.snapshot"];
      if (operation === "project.facts") {
        presentationRead++;
        const facts = machineOn ? Promise.all(projects.filter(p => !projectId || p.projectId === projectId).map(factsOf)) : Promise.resolve([]);
        return facts.finally(() => { presentationRead++; }) as Promise<QueryResults[O]>;
      }
      return Promise.all(projects.filter(p => !projectId || p.projectId === projectId).map(snapshotOf)) as Promise<QueryResults[O]>;
    },
    async command(envelope) {
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
        }
      }
      return send(envelope);
    },
    async discoverMcp(server) {
      if (!machineOn) return {ok:false,code:"offline",message:"The machine service is unavailable."};
      return machine("config.mcp.discover", undefined, {server});
    },
    async takeDroppedFiles() {
      if (!serviceMethods.includes("native.take_dropped_files")) return [];
      return wireAttachments(await transport.request("native.take_dropped_files", {}) as Message["attachments"]);
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
    async readClipboardImage() {
      if (!serviceMethods.includes("native.clipboard_image")) return [];
      return wireAttachments(await transport.request("native.clipboard_image", {}) as Message["attachments"]);
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
      const result = decodeResponse("ReportPage", await transport.request("reports.read", request(projectId, "reports.read", {target:reportId,limit:4000,...(cursor ? {cursor} : {})}))).value;
      if (result.reportId !== reportId) throw new Error("Report identity mismatch.");
      return result;
    },
    async readFile(checkoutId, file) {
      if (!machineOn) return { text: null };
      return machine<FileRead>("repo.file", projectOfCheckout(checkoutId), { checkoutId, file });
    },
    async saveFile(checkoutId, file, text, revision) {
      // Only the machine's own answer says what happened. Anything else may be a write whose reply never came.
      try { return await machine<FileSaveResult>("repo.file.save", projectOfCheckout(checkoutId), { checkoutId, file, text, revision }); }
      catch (error) { return error instanceof Refusal ? { outcome: "refused", message: error.message } : { outcome: "unknown" }; }
    },
    subscribe(listener: (update: PortUpdate) => void) {
      observers.add(listener);
      let active = true, tick = 0;
      // A slow repository must not stack reads behind itself: the next round starts when this one ended.
      const running = new Set<string>();
      const once = (key: string, read: () => Promise<void>) => { if (running.has(key)) return; running.add(key); void read().catch(() => {}).finally(() => running.delete(key)); };
      const poll = setInterval(() => {
        if (!active || !machineOn) return;
        for (const project of projects) once(project.projectId, () => {
          const generation = presentationRead;
          return factsOf(project).then(facts => { if (active && generation === presentationRead) listener({ kind: "facts", facts }); });
        });
        if (++tick % MACHINE_POLL_EVERY === 0) once("machine", () => machineFacts().then(facts => { if (active) listener({ kind: "machine", facts }); }));
      }, FACTS_POLL_MS);
      const stop = transport.subscribe(event => {
        if (event.type === "connection") { listener({ kind: "connection", connection: { state: event.state, reason: event.reason ?? null, at: Date.now() } }); return; }
        if (event.type === "refresh") { listener({ kind: "refresh" }); return; }
        const project = projects.find(p => p.projectId === event.projectId);
        if (!project) return;
        // A failed reread leaves the last confirmed state on screen. It says nothing about the machine.
        // The store rereads the projection and product facts as one presentation commit.
        // A wire-only update would momentarily undo sprint/archive/owner state.
        listener({ kind: "refresh" });
      });
      return () => { observers.delete(listener); active = false; clearInterval(poll); stop(); };
    },
  };
}
