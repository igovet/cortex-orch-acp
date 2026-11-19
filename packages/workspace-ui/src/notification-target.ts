import type { WorkspaceState } from "@cortex/workspace-domain";
import { hrefOf, type Route, type Router } from "./router";
import { planningPlace, sprintRoute } from "./selectors";

/**
 * Where a desktop notification leads. It names things, never an address: the place is worked out from what the window knows
 * when the notification is clicked, so a stale target still opens the nearest valid place.
 */
export type NotificationTarget =
  /** A question waiting in a task: its Conversation, at that question. */
  | { kind: "question"; project: string; task: string; decision: string }
  /** A question waiting in a sprint's planning: that sprint's planning. */
  | { kind: "planning"; project: string; sprint: string; decision: string }
  /** A result ready for the owner's review: the task, with its result card in view. */
  | { kind: "result"; project: string; task: string }
  /** A task that failed: the task. */
  | { kind: "task"; project: string; task: string }
  /** A sprint that paused itself: the sprint. */
  | { kind: "sprint"; project: string; sprint: string };
/** The kinds of owner moment the service announces, each with its own setting. */
export const NOTIFICATION_KINDS = [
  { id: "question", label: "Questions and permissions" },
  { id: "stop", label: "A task stopped and asks you" },
  { id: "result", label: "Results ready for review" },
  { id: "failed", label: "Failed tasks" },
  { id: "planning", label: "Sprint planning questions" },
  { id: "paused", label: "Sprints that paused" },
] as const;
export type NotificationKind = typeof NOTIFICATION_KINDS[number]["id"];
/** What the focused window shows (a task or sprint), and the kinds the owner wants; the service raises nothing for that place. */
export type NotificationReport = { focused: { project: string; task: string } | { project: string; sprint: string } | null; kinds: NotificationKind[] };
/** The machine service's notifications, when it raises them: the window reports to it and collects a click that started it. */
export type NotificationService = {
  report(report: NotificationReport): Promise<{ raising?: boolean }>;
  take(): Promise<{ target?: unknown }>;
};
export type Notification = { label: string; target: NotificationTarget };
/** Raises one desktop notification. */
export type Notify = (notification: Notification) => Promise<void>;
/** Clicks on this window's notifications, each with the target it was raised with (checked again by `parseTarget`). Returns the
 *  way to stop listening. */
export type NotificationClicks = (listener: (raw: unknown) => void) => () => void;

const id = (v: unknown): v is string => typeof v === "string" && v.length > 0 && v.length <= 256 && !/[\s\p{Cc}]/u.test(v);
/** The one shape a click may carry; anything else is ignored. */
export function parseTarget(raw: unknown): NotificationTarget | null {
  if (!raw || typeof raw !== "object") return null;
  const r = raw as Record<string, unknown>, keys = Object.keys(r).sort().join(",");
  if (r.kind === "question" && keys === "decision,kind,project,task" && id(r.project) && id(r.task) && id(r.decision))
    return { kind: "question", project: r.project, task: r.task, decision: r.decision };
  if (r.kind === "planning" && keys === "decision,kind,project,sprint" && id(r.project) && id(r.sprint) && id(r.decision))
    return { kind: "planning", project: r.project, sprint: r.sprint, decision: r.decision };
  if ((r.kind === "result" || r.kind === "task") && keys === "kind,project,task" && id(r.project) && id(r.task))
    return { kind: r.kind, project: r.project, task: r.task };
  if (r.kind === "sprint" && keys === "kind,project,sprint" && id(r.project) && id(r.sprint))
    return { kind: "sprint", project: r.project, sprint: r.sprint };
  return null;
}

const decisionCard = (decision: string) => `[id="decision-${decision.replace(/["\\]/g, "\\$&")}"]`;
/** The newest result card on a task's Conversation. */
export const RESULT_CARD = ".rcard";

/** The place a target opens now, and what to bring into view there (a question while it still waits, a result card). */
export function targetPlace(state: WorkspaceState, target: NotificationTarget, fallbackMachine: string): { route: Route; reveal: string | null } {
  const project = state.projects.find(p => p.id === target.project);
  const waiting = (taskId: string, decision: string) => state.sessions.find(s => s.taskId === taskId)?.decisions
    .some(d => d.id === decision && !d.answer && !d.withdrawn) ?? false;
  if (target.kind === "question" || target.kind === "result" || target.kind === "task") {
    // A sprint's planning task has no page of its own: what it asks or ends with opens its sprint's conversation.
    const planned = state.sprints.find(s => s.planningTaskId === target.task);
    if (planned) {
      const machine = state.projects.find(p => p.id === planned.projectId)?.machineId ?? project?.machineId ?? fallbackMachine;
      return { route: planningPlace(machine, planned, {}), reveal: target.kind === "question" && waiting(target.task, target.decision) ? decisionCard(target.decision) : null };
    }
    const task = state.tasks.find(t => t.id === target.task);
    const machine = state.projects.find(p => p.id === task?.projectId)?.machineId ?? project?.machineId ?? fallbackMachine;
    const reveal = target.kind === "question" ? (task && waiting(task.id, target.decision) ? decisionCard(target.decision) : null)
      : target.kind === "result" ? RESULT_CARD : null;
    if (task) return { route: { name: "task", machine, task: task.id }, reveal };
  } else {
    const sprint = state.sprints.find(s => s.id === target.sprint && !s.archivedAt);
    const machine = state.projects.find(p => p.id === sprint?.projectId)?.machineId ?? project?.machineId ?? fallbackMachine;
    const reveal = target.kind === "planning" && sprint?.planningTaskId && waiting(sprint.planningTaskId, target.decision) ? decisionCard(target.decision) : null;
    // A planning question still waiting is answered in the sprint's conversation, even once its tasks exist.
    if (sprint && reveal) return { route: planningPlace(machine, sprint, {}), reveal };
    if (sprint) return { route: sprintRoute(machine, sprint), reveal };
  }
  if (project) return { route: { name: "project", machine: project.machineId, project: project.id, view: "board", scope: { kind: "all" } }, reveal: null };
  return { route: { name: "home", machine: fallbackMachine }, reveal: null };
}

type TabsLike = { ids: () => number[]; activeId: () => number; routerOf: (id: number) => Router; select: (id: number) => void };
const hrefIn = (router: Router) => `${router.path()}${router.search()}`;
/** The same task or sprint, whichever of its views a tab shows. */
function samePlace(shown: Route, wanted: Route): boolean {
  if (wanted.name === "task") return shown.name === "task" && shown.task === wanted.task;
  if (wanted.name === "sprint") return shown.name === "sprint" && shown.sprint === wanted.sprint
    || shown.name === "project" && shown.scope.kind === "sprint" && shown.scope.id === wanted.sprint;
  if (wanted.name === "project" && wanted.scope.kind === "sprint")
    return (shown.name === "project" && shown.scope.kind === "sprint" && shown.scope.id === wanted.scope.id) || (shown.name === "sprint" && shown.sprint === wanted.scope.id);
  return hrefOf(shown) === hrefOf(wanted);
}

/**
 * Opens a place the way opening it by hand would: a tab that already shows it comes forward (the active one first, then one at
 * exactly that address, then one on another view of the same task or sprint) and is moved to the place; otherwise the active tab
 * goes there through `go`, which keeps its own guards (unsaved Settings ask first). Returns the tab that shows it.
 */
export function openPlace(tabs: TabsLike, go: (route: Route) => void, route: Route): number {
  const href = hrefOf(route), ids = tabs.ids(), active = tabs.activeId();
  const order = [active, ...ids.filter(id => id !== active)];
  const found = order.find(id => hrefIn(tabs.routerOf(id)) === href) ?? order.find(id => samePlace(tabs.routerOf(id).route(), route));
  if (found === undefined) { go(route); return active; }
  if (found !== active) tabs.select(found);
  if (hrefIn(tabs.routerOf(found)) !== href) tabs.routerOf(found).go(route);
  return found;
}

/** Brings a waiting question or the result card into view once the page has drawn it. Gives up quietly: the page is already open. */
export function reveal(selector: string, root: () => ParentNode, tries = 40, every = 50) {
  const look = (left: number) => {
    const card = [...root().querySelectorAll<HTMLElement>(selector)].at(-1);
    if (card) { card.scrollIntoView?.({ block: "center" }); return; }
    if (left > 0) setTimeout(() => look(left - 1), every);
  };
  look(tries);
}
