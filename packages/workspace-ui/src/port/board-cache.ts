import { validate } from "@cortex/workspace-contract";
import type { Task as WireTask } from "@cortex/workspace-contract";
import { isAutomaticContinuation, isAutomaticQueue, isSprintRunMode, type Sprint, type TaskOverlay } from "@cortex/workspace-domain";
import type { ProjectLocator } from "./real";

export type BoardStorage = Pick<Storage, "getItem" | "setItem">;
export type CachedBoard = { project: ProjectLocator; items: WireTask[]; product: { tasks: Record<string, TaskOverlay>; sprints: Sprint[] } };
const KEY = "cw.confirmed-board.v1", MAX = 2_000_000;
const object = (v: unknown): v is Record<string, unknown> => !!v && typeof v === "object" && !Array.isArray(v);
const text = (v: unknown): v is string => typeof v === "string" && v.length <= 32000;
const ids = (v: unknown): v is string[] => Array.isArray(v) && v.length <= 1000 && v.every(x => typeof x === "string" && /^[a-zA-Z0-9_:.-]{1,128}$/.test(x));
/** Display only: no credentials, commands, capabilities, results, Git contents or conversation bodies. */
function board(raw: unknown, legacy = false): CachedBoard {
  if (!object(raw) || !object(raw.project) || !Array.isArray(raw.items) || raw.items.length > 1000 || !object(raw.product)) throw Error("cache");
  const p = raw.project;
  if (![p.projectId,p.name,p.repository,p.openBranch,p.defaultBranch].every(text) || !String(p.repository).startsWith("/") || typeof p.activated !== "boolean") throw Error("cache");
  const project = p as ProjectLocator, items = raw.items.map(t => validate("Task",t));
  if (new Set(items.map(t => t.taskId)).size !== items.length
    || items.some(t => t.automaticContinuation !== undefined && !isAutomaticContinuation(t.automaticContinuation))) throw Error("cache");
  const product = raw.product;
  if (!object(product.tasks) || !Array.isArray(product.sprints) || product.sprints.length > 100) throw Error("cache");
  const tasks: Record<string, TaskOverlay> = {};
  for (const [id,value] of Object.entries(product.tasks)) {
    if (!object(value)) throw Error("cache");
    const overlay: TaskOverlay = text(value.title) ? {title:value.title,titleState:"generated"} : {};
    if (ids(value.dependencies)) overlay.dependencies=value.dependencies;
    if (typeof value.planning === "boolean") overlay.planning=value.planning;
    if (typeof value.purged === "boolean") overlay.purged=value.purged;
    if (text(value.sprintId)) overlay.sprintId=value.sprintId;
    if (object(value.archive) && typeof value.archive.at === "number" && Number.isFinite(value.archive.at)) {
      const {at, previousStatus, retentionAt} = value.archive;
      overlay.archive = {at, previousStatus: previousStatus === "draft" ? "draft" : "done",
        ...("retentionAt" in value.archive ? {retentionAt: typeof retentionAt === "number" && Number.isFinite(retentionAt) ? retentionAt : null} : {})};
    }
    // Only the display facts for a recorded automatic finish, never the task result or Git contents.
    const integration = value.integration;
    if (value.status === "done" && object(integration) && integration.method === "commit" && integration.automatic === true
      && typeof integration.committed === "boolean" && text(integration.targetBranch) && text(integration.digest)) {
      overlay.status = "done";
      overlay.integration = { method:"commit", automatic:true, committed:integration.committed, targetBranch:integration.targetBranch, digest:integration.digest };
    }
    tasks[id]=overlay;
  }
  const sprints = product.sprints.map((s): Sprint => {
    if (!object(s) || !text(s.id) || s.projectId !== project.projectId || !text(s.description) || !ids(s.taskIds)
      || !isSprintRunMode(s.runMode) || !["planning","question","ready","created","cancelled"].includes(String(s.planning))
      || !["idle","running","paused","complete"].includes(String(s.queueState)) || !(s.pauseReason === null || text(s.pauseReason))) throw Error("cache");
    if (s.automaticQueue !== undefined && (legacy || (s.runMode !== "automatic" && s.runMode !== "hybrid") || !isAutomaticQueue(s.automaticQueue))) throw Error("cache");
    const hold = isAutomaticQueue(s.automaticQueue) ? s.automaticQueue : null;
    // A member the service already deleted is never on the board again.
    if (s.taskIds.some(id => tasks[id]?.purged !== true && !items.some(t => t.taskId === id))) throw Error("incomplete cache");
    const deletion = object(s.deletion) && typeof s.deletion.total === "number" && Number.isSafeInteger(s.deletion.total) && s.deletion.total >= 0 && ids(s.deletion.left)
      ? {deletion:{total:s.deletion.total,left:s.deletion.left}} : {};
    return {id:s.id,projectId:project.projectId,description:s.description,title:text(s.title)?s.title:null,
      runMode:legacy && s.runMode === "automatic" ? "hybrid" : s.runMode,planning:s.planning as Sprint["planning"],taskIds:s.taskIds.filter(id => tasks[id]?.purged !== true),
      queueState:s.queueState as Sprint["queueState"],pauseReason:s.pauseReason as string|null,
      ...(hold ? {automaticQueue:{kind:hold.kind,taskId:hold.taskId,code:hold.code,reason:hold.reason,needsOwner:hold.needsOwner,
        ...(hold.nextActor ? {nextActor:hold.nextActor,ownerActions:hold.ownerActions,asks:hold.asks} : {}),
        ...(typeof hold.allowedAt === "number" ? {allowedAt:hold.allowedAt} : {}),
        ...(typeof hold.holdId === "string" && hold.holdId ? {holdId:hold.holdId} : {})}} : {}),
      // Planning diagnostics stay live-only, like the matching conversation entries.
      targetBranch:null,conversation:[],plan:null,materializedVersion:null,
      ...(typeof s.archivedAt === "number" && Number.isFinite(s.archivedAt)?{archivedAt:s.archivedAt}:{}),...deletion};
  });
  return {project,items,product:{tasks,sprints}};
}
export function readBoards(storage: BoardStorage | undefined, actor: string): CachedBoard[] {
  try {
    const raw = storage?.getItem(KEY);
    if (!raw || raw.length > MAX) return [];
    const value: unknown = JSON.parse(raw);
    if (!object(value) || value.actor !== actor || !Array.isArray(value.boards) || value.boards.length > 100) return [];
    if (value.sprintRunModeVersion !== undefined && value.sprintRunModeVersion !== 1) return [];
    return value.boards.map(b => board(b, value.sprintRunModeVersion === undefined));
  } catch { return []; }
}
export function writeBoards(storage: BoardStorage | undefined, actor: string, boards: CachedBoard[]) {
  try {
    const safe = boards.map(b => board(b)), raw = JSON.stringify({actor,sprintRunModeVersion:1,boards:safe});
    if (raw.length <= MAX) storage?.setItem(KEY,raw);
  } catch { /* A corrupt or quota-limited cache never replaces live data or blocks it. */ }
}
