import { createEffect, untrack, type Accessor } from "solid-js";
import { dependencyProblem, queueDrivenMode, type LiveSprintMember, type PlanItem, type Sprint, type SprintGraphWrite, type SprintOrder } from "@cortex/workspace-domain";
import type { LiveGraphState, WorkspaceStore } from "./store";
import type { WorkspaceIndex } from "./selectors";

/*
 * A created sprint's live graph: every task bound to it now, later drafts included, with the links they wait on today. It is
 * the owner's working order, kept apart from the approved plan, which stays as it was approved. The service reads and edits
 * it; this side only shows it and asks. A member that could not be read is shown as such and turns editing off, never read as
 * "no dependency".
 */

/** What a read of a sprint depends on: when any of it changes, the graph and its order are read again. */
export function graphKey(index: WorkspaceIndex, sprint: Sprint): string {
  const tasks = sprint.taskIds.map(id => {
    const t = index.byId.get(id)?.task;
    return t ? [id, t.status, t.process, t.closedAs ?? "", t.work ?? "", t.executionId ?? "", t.dependencies.join(","), t.acceptance?.version ?? "", t.conflict] : [id];
  });
  const machine = index.summaries.get(sprint.projectId)?.machine.connection ?? "";
  const priority = (sprint as Sprint & { taskPriority?: unknown }).taskPriority;
  return JSON.stringify([sprint.id, sprint.runMode, sprint.queueState, sprint.planning, sprint.archivedAt ?? null, sprint.materializedVersion,
    sprint.automaticQueue?.holdId ?? sprint.automaticQueue?.code ?? null, priority ?? null, machine, tasks]);
}

/** Reads the live graph of each sprint shown, once, and again whenever what it depends on changes. */
export function watchLiveGraphs(store: WorkspaceStore, sprints: Accessor<readonly Sprint[]>) {
  createEffect(() => {
    const index = store.index();
    for (const sprint of sprints()) {
      if (sprint.planning !== "created" || sprint.archivedAt) continue;
      const key = graphKey(index, sprint);
      untrack(() => void store.readLiveGraph(sprint.id, key));
    }
  });
}

export type LiveModel = {
  /** `service`: the service's live graph, the one that can be edited. `local`: built from this app's facts, read only. */
  source: "service" | "local";
  /** Roster order, keyed by task id: node numbers stay put whatever the priority. */
  items: PlanItem[];
  member: (taskId: string) => LiveSprintMember | null;
  /** Task ids in the priority the service saved (roster order without one). */
  priority: string[];
  order: SprintOrder | null;
  revision: string | null;
  /** Members listed but not read or not loaded here. Any of them turns editing off. */
  unread: number;
  /** Why links and priority cannot be changed now, or null when they can. */
  why: string | null;
  /** Plan keys of the approved version to the tasks made from them, as the service proves it. */
  created: Map<string, string>;
  /** How the live graph differs from the approved plan, in words, or null when it does not (or that cannot be told). */
  divergence: string | null;
  /** Said before a save while a queue could start the next task by itself. */
  queueNote: string | null;
  /** The last read failed: what is shown is the read before it. */
  stale: string | null;
};

const PLACEHOLDER = "Task not loaded";
const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? "" : "s"}`;
const approvedItems = (sprint: Sprint) => {
  const v = sprint.materializedVersion;
  if (v === null) return null;
  return sprint.plan?.version === v ? sprint.plan : sprint.history?.find(h => h.version === v) ?? null;
};

export function liveModel(index: WorkspaceIndex, sprint: Sprint, state: LiveGraphState | undefined): LiveModel {
  const graph = state?.graph ?? null, offline = index.summaries.get(sprint.projectId)?.offline ?? false;
  const title = (id: string) => index.byId.get(id)?.task.title ?? PLACEHOLDER;
  const description = (id: string) => index.byId.get(id)?.task.description ?? "";
  const queueNote = sprint.queueState === "running" && queueDrivenMode(sprint.runMode)
    ? "The queue is running, so this can change which task starts next. Pause the queue first if nothing should start in between." : null;
  if (!graph) {
    // Only this app's facts: the members it has loaded, with their saved links. Nothing here can be edited.
    const ids = sprint.taskIds, known = new Set(ids);
    const items = ids.map((id): PlanItem => ({ key: id, title: title(id), description: description(id), dependsOn: (index.byId.get(id)?.task.dependencies ?? []).filter(d => known.has(d)) }));
    const unread = ids.filter(id => !index.byId.has(id)).length;
    const why = state?.status === "unsupported" ? "This machine's service cannot change sprint links."
      : state?.status === "failed" ? `The live graph could not be read: ${state.message ?? "try again"}.`
      : offline ? "The machine is offline." : "Reading the live graph…";
    return { source: "local", items, member: () => null, priority: ids, order: null, revision: null, unread, why, created: new Map(), divergence: null, queueNote, stale: null };
  }
  const members = new Map(graph.members.map(m => [m.taskId, m]));
  // A deleted member is no longer the sprint's: it is not drawn, and a link to it is said in its dependent's details.
  const shown = graph.members.filter(m => m.readStatus !== "purged"), keys = new Set(shown.map(m => m.taskId));
  const items = shown.map((m): PlanItem => ({ key: m.taskId, title: m.readStatus === "confirmed" ? title(m.taskId) : PLACEHOLDER,
    description: m.readStatus === "confirmed" ? description(m.taskId) : "", dependsOn: (m.dependencies ?? []).filter(d => keys.has(d)) }));
  const unread = shown.filter(m => m.readStatus !== "confirmed" || !index.byId.has(m.taskId)).length;
  const created = new Map(graph.members.flatMap(m => m.approvedPlanKey ? [[m.approvedPlanKey, m.taskId] as const] : []));
  const why = offline ? "The machine is offline."
    : state?.status === "failed" ? `The live graph could not be read again: ${state.message ?? "try again"}.`
    : state?.status === "loading" ? "Reading the live graph…"
    : !graph.complete || unread ? `${plural(unread, "task")} could not be read, so links cannot be changed until the sprint reads completely.`
    : sprint.archivedAt || sprint.deletion ? "This sprint is closed." : null;
  return { source: "service", items, member: id => members.get(id) ?? null, priority: graph.priority.filter(id => keys.has(id)), order: graph.order,
    revision: graph.graphRevision, unread, why, created, divergence: divergence(sprint, graph.members, created), queueNote,
    stale: state?.status === "failed" ? state.message ?? "The last read failed." : null };
}

/** The live links set against the approved plan. Tasks with no proven plan origin are counted, never called added later. */
function divergence(sprint: Sprint, members: LiveSprintMember[], created: Map<string, string>): string | null {
  const plan = approvedItems(sprint);
  if (!plan || !created.size) return null;
  const changed = plan.items.filter(item => {
    const id = created.get(item.key), m = id ? members.find(x => x.taskId === id) : undefined;
    if (!m || m.dependencies === null) return false;
    const want = new Set(item.dependsOn.map(k => created.get(k)).filter((x): x is string => !!x));
    return m.dependencies.length !== want.size || m.dependencies.some(d => !want.has(d));
  }).length;
  const outside = members.filter(m => m.readStatus !== "purged" && !m.approvedPlanKey).length;
  const parts = [changed ? `links of ${plural(changed, "task")} differ from Plan v${plan.version}` : "", outside ? `${plural(outside, "task")} not matched to Plan v${plan.version}` : ""].filter(Boolean);
  return parts.length ? `${parts.join("; ").replace(/^./, c => c.toUpperCase())}.` : null;
}

/** Why `taskId` cannot gain or lose a prerequisite, or null. */
export function dependentWhy(model: LiveModel, taskId: string): string | null {
  if (model.why) return model.why;
  const m = model.member(taskId);
  return !m ? "This task is not in the live graph." : m.editable ? null : "Only a saved draft that has not started can change its links.";
}
/** Why `taskId` cannot become a new prerequisite, or null. */
export function prerequisiteWhy(model: LiveModel, taskId: string): string | null {
  if (model.why) return model.why;
  const m = model.member(taskId);
  return !m ? "This task is not in the live graph." : m.canBePredecessor ? null : "This task was cancelled, is not confirmed or cannot finish, so nothing can wait for it.";
}
const numberOf = (model: LiveModel, id: string) => model.items.findIndex(i => i.key === id) + 1;
const named = (model: LiveModel, id: string) => { const n = numberOf(model, id), item = model.items[n - 1]; return item ? `task ${n} “${item.title}”` : "a task that is no longer in the sprint"; };
/** A hint for a new link, before it is sent: the same words as the plan editor. The service decides. */
export function linkProblem(model: LiveModel, dependent: string, prerequisite: string): string | null {
  const exists = model.items.find(i => i.key === dependent)?.dependsOn.includes(prerequisite);
  if (exists) return null;
  return dependentWhy(model, dependent) ?? prerequisiteWhy(model, prerequisite) ?? dependencyProblem(model.items, dependent, prerequisite);
}
/** The change in words, direction first: what will wait for what. */
export function linkWords(model: LiveModel, prerequisite: string, dependent: string): string {
  const exists = model.member(dependent)?.dependencies?.includes(prerequisite);
  const a = named(model, dependent), b = named(model, prerequisite);
  return exists ? `Remove the link: ${a.replace(/^t/, "T")} will no longer wait for ${b}.` : `Add a link: ${a.replace(/^t/, "T")} will wait for ${b} to finish first.`;
}
/** The words for an edit's answer, or null when it went through. */
export function editNotice(result: SprintGraphWrite): string | null {
  switch (result.outcome) {
    case "changed": return null;
    case "unchanged": return "Nothing changed: the sprint already was this way.";
    case "unknown": return `The change could not be confirmed. The graph was read again; check it before trying again.${result.message ? ` ${result.message}` : ""}`;
    case "refused": return result.code === "stale_graph_revision" ? "The sprint changed since it was shown. It was read again; check the links and try again." : result.message;
  }
}
/** A not-started member's place in the service's order, in words, or null. */
export function placeWords(model: LiveModel, taskId: string): string | null {
  const p = model.order?.notStarted.find(x => x.taskId === taskId);
  if (!p) return null;
  if (p.next) return "Next to start";
  if (p.rank === null) return null;
  const n = p.rank, ord = `${n}${n % 100 >= 11 && n % 100 <= 13 ? "th" : (["th", "st", "nd", "rd"][n % 10] ?? "th")}`;
  return model.order!.mode === "manual" ? `Suggested ${ord} to start` : `${ord} to start${p.conditional ? ", once the work before it finishes" : ""}`;
}
