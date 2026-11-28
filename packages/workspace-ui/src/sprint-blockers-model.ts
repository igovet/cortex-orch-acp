import { dependencyReady, type Sprint } from "@cortex/workspace-domain";
import { graphKey } from "./live-graph";
import { orderOf, sprintOutputWait, type TaskView, type WorkspaceIndex } from "./selectors";
import type { LiveGraphState } from "./store";

type Blocker = {
  view: TaskView;
  immediate: string[];
  later: string[];
  chain: string[];
};
export type SprintBlockerAnalysis =
  | { kind: "offline" | "incomplete" | "cycle" }
  | { kind: "ready"; blocked: number; clear: number; blockers: Blocker[]; outputs: Map<string, string>; waiting: Map<string, string[]> };

/** Use current membership and saved links, never approved-plan keys or inferred queue order. */
export function sprintBlockers(index: WorkspaceIndex, sprint: Sprint, cached?: LiveGraphState): SprintBlockerAnalysis {
  const project = index.summaries.get(sprint.projectId);
  if (project?.offline) return { kind: "offline" };
  if (!project || project.project.issue) return { kind: "incomplete" };
  const roster = new Set(sprint.taskIds);
  const views = new Map<string, TaskView>();
  for (const id of roster) {
    const view = index.byId.get(id);
    if (!view || view.unknown || view.project.id !== sprint.projectId || view.sprint?.id !== sprint.id) return { kind: "incomplete" };
    views.set(id, view);
  }
  let links = new Map([...views].map(([id, v]) => [id, v.task.dependencies]));
  if (cached && cached.status !== "unsupported") {
    const graph = cached.graph;
    if (cached.status !== "ready" || cached.key !== graphKey(index, sprint) || !graph?.complete
      || graph.members.length !== roster.size || new Set(graph.members.map(m => m.taskId)).size !== roster.size
      || graph.members.some(m => !roster.has(m.taskId) || m.readStatus !== "confirmed" || m.dependencies === null)) {
      return { kind: "incomplete" };
    }
    links = new Map(graph.members.map(m => [m.taskId, m.dependencies!]));
  }
  if ([...links.values()].some(deps => deps.some(id => !roster.has(id)))) return { kind: "incomplete" };

  const downstream = new Map<string, string[]>();
  const incoming = new Map<string, number>();
  for (const [id, deps] of links) {
    const unique = new Set(deps);
    incoming.set(id, unique.size);
    for (const dep of unique) {
      const next = downstream.get(dep) ?? [];
      next.push(id);
      downstream.set(dep, next);
    }
  }
  const order = [...roster].filter(id => incoming.get(id) === 0);
  for (let i = 0; i < order.length; i++) {
    for (const next of downstream.get(order[i]) ?? []) {
      const left = incoming.get(next)! - 1;
      incoming.set(next, left);
      if (left === 0) order.push(next);
    }
  }
  if (order.length !== roster.size) return { kind: "cycle" };

  const waiting = new Map<string, string[]>();
  const dependentDrafts = new Map<string, string[]>();
  const roots = new Set<string>();
  const currentOrder = cached?.status === "ready" ? cached.graph?.order : orderOf(index, sprint);
  const ordered = new Map((currentOrder?.mode === sprint.runMode && currentOrder.queueState === sprint.queueState
    ? currentOrder.notStarted : []).map(p => [p.taskId, p]));
  const outputs = new Map<string, string>();
  let clear = 0;
  for (const [id, view] of views) {
    if (view.task.status !== "draft" || view.task.archive) continue;
    const deps = [...new Set(links.get(id))].filter(d => !dependencyReady(views.get(d)!.task, sprint.runMode));
    waiting.set(id, deps);
    const output = sprintOutputWait(sprint, id, ordered.get(id));
    if (output) outputs.set(id, output);
    if (!deps.length && !output) clear++;
    for (const dep of deps) {
      const next = dependentDrafts.get(dep) ?? [];
      next.push(id);
      dependentDrafts.set(dep, next);
    }
  }
  // A running/paused/owner-blocked task has already started. Its old links
  // do not take away its ability to resume or answer in the present.
  for (const id of dependentDrafts.keys()) if (!waiting.get(id)?.length) roots.add(id);
  const blocked = waiting.size - clear;
  const blockers: Blocker[] = [];
  for (const id of roots) {
    const paths = new Map<string, { depth: number; previous?: string }>([[id, { depth: 0 }]]);
    let deepest = id;
    for (const current of order) {
      const path = paths.get(current);
      if (!path) continue;
      for (const next of dependentDrafts.get(current) ?? []) {
        const previous = paths.get(next);
        const depth = path.depth + 1;
        if (previous && previous.depth >= depth) continue;
        paths.set(next, { depth, previous: current });
        if (depth > paths.get(deepest)!.depth) deepest = next;
      }
    }
    const affected = [...paths.keys()].filter(k => k !== id);
    const immediate = affected.filter(k => !outputs.has(k) && waiting.get(k)?.length === 1 && waiting.get(k)![0] === id);
    const immediately = new Set(immediate);
    const chain: string[] = [];
    for (let node: string | undefined = deepest; node; node = paths.get(node)?.previous) chain.push(node);
    blockers.push({ view: views.get(id)!, immediate, later: affected.filter(k => !immediately.has(k)), chain: chain.reverse() });
  }
  const impact = (b: Blocker) => b.immediate.length + b.later.length;
  blockers.sort((a, b) => impact(b) - impact(a) || b.immediate.length - a.immediate.length);
  return { kind: "ready", blocked, clear, blockers, outputs, waiting };
}
