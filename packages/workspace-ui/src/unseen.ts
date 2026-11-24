import { createEffect, createSignal, onCleanup, untrack, type Accessor } from "solid-js";
import { automaticQueueOf, isOpen, type Project, type Sprint, type TaskStatus, type WorkspaceState } from "@cortex/workspace-domain";
import type { Route } from "./router";
import { awaitsOwner, onView, planningFailureDetails, planningNeedsOwner, planningWaits, type SideKeep, type TaskView, type WorkspaceIndex } from "./selectors";

/** Where this app keeps what the owner has already seen of each sidebar row, so a relaunch does not show it as new again. */
export const SEEN_CHANGES_KEY = "cw.seen-sidebar-changes";
/** At most this many rows are remembered across a relaunch; the rows changed longest ago go first. */
export const SEEN_CHANGES_LIMIT = 1000;
/** At most this many projects are remembered as taken in; one dropped is taken in afresh, its present rows as seen. */
export const SEEN_PROJECTS_LIMIT = 200;
/** At most this many short codes of rows the row bound dropped are kept; a project whose codes do not fit takes its unremembered rows as seen. */
export const SEEN_DROPPED_LIMIT = 10000;
/** The saved layout. Layout 2 lacks the projects taken in, so its rows are read and every project is taken in afresh. A record
 *  in any other layout (an earlier one kept readable text) is not read: every row is taken as seen. */
const FORMAT = 3;
/** A row that asks nothing of the owner. */
const QUIET = "";
/** Finished columns: a row there is listed only while its finish is new to the owner, or open. */
const FINISHED = new Set(["done", "closed_unverified"]);
/** A run's end, however it came: by itself, by the agent, in another window or client, or by the owner's hand. */
const ENDED = new Set<TaskStatus>(["done", "closed_unverified", "failed", "cancelled"]);
const storage = () => { try { return typeof localStorage === "undefined" ? undefined : localStorage; } catch { return undefined; } };

/**
 * A state identity as a fixed-size code (64 bits, in hex), so what is kept never holds a question's words, a failure's text or
 * any other task content. Equal parts give the same code; any changed part gives another.
 */
export function stateCode(parts: readonly (string | number | null | undefined)[]): string {
  const text = parts.map(p => p === null || p === undefined ? "\u0000" : String(p)).join("\u0001");
  let h1 = 0xdeadbeef, h2 = 0x41c6ce57;
  for (let i = 0; i < text.length; i++) { const c = text.charCodeAt(i); h1 = Math.imul(h1 ^ c, 2654435761); h2 = Math.imul(h2 ^ c, 1597334677); }
  h1 = Math.imul(h1 ^ (h1 >>> 16), 2246822507); h1 ^= Math.imul(h2 ^ (h2 >>> 13), 3266489909);
  h2 = Math.imul(h2 ^ (h2 >>> 16), 2246822507); h2 ^= Math.imul(h1 ^ (h1 >>> 13), 3266489909);
  return (h2 >>> 0).toString(16).padStart(8, "0") + (h1 >>> 0).toString(16).padStart(8, "0");
}

/**
 * What a task row asks the owner to look at, as one identity: a run's end (Done, closed, failed or cancelled, whoever ended it
 * and whatever column it sits in), or a question, permission, result, acceptance, conflict or queue hold waiting on the owner.
 * A waiting identity is built from the open questions' and permissions' ids, the queue hold's id, the run and the result and
 * acceptance versions, so another question in its place is a new identity even when its words repeat. Null while the task asks
 * nothing of the owner, or while its state is not confirmed.
 */
export function taskChange(view: TaskView, state: Pick<WorkspaceState, "sessions">): string | null {
  const t = view.task;
  if (view.unknown || view.column === "archive" || t.archive) return null;
  const queue = automaticQueueOf(view.sprint);
  const hold = queue && queue.taskId === view.id && queue.needsOwner ? queue : null;
  const holdId = hold ? hold.holdId ?? `${hold.kind}:${hold.code}` : null;
  // A queue that later holds on a failed run for the owner is a new thing to look at.
  if (ENDED.has(t.status)) return stateCode(["ended", t.status, t.executionId, holdId]);
  if (!hold && !awaitsOwner(view)) return null;
  const open = (state.sessions.find(s => s.taskId === view.id)?.decisions ?? []).filter(d => isOpen(d) && !d.suspended).map(d => d.id).sort();
  return stateCode([
    "asks", view.column, t.status, t.executionId, open.join(","),
    // Only a wait with no question or permission of its own (a recovery, a stop) is told apart by its recorded reason.
    open.length ? null : t.issue,
    t.conflict, t.result?.version, t.acceptance?.version, holdId,
  ]);
}

/** What a sprint row asks the owner to look at: a planning question or permission, a plan ready to create, a planning failure, or the finished sprint. */
export function sprintChange(state: Pick<WorkspaceState, "tasks" | "sessions">, sprint: Sprint): string | null {
  if (sprint.deletion) return null;
  const failure = planningFailureDetails(sprint);
  if (failure) return stateCode(["failed", failure.at ?? failure.text]);
  if (planningNeedsOwner(state, sprint)) {
    const asked = sprint.conversation.filter(m => m.kind === "question").length;
    return stateCode(["asks", sprint.planning, asked, planningWaits(state, sprint).map(d => d.id).sort().join(",")]);
  }
  if (sprint.planning === "ready") return stateCode(["plan", sprint.plan?.version]);
  if (sprint.planning === "created" && sprint.queueState === "complete") return stateCode(["complete"]);
  return null;
}

/** What the owner has seen of one row: the state identity, or `QUIET` when the row asked nothing then. */
type Mark = { key: string; project: string };
/**
 * A project this app has taken in: every row it held then has a mark, or, when the row bound dropped its mark, a short code in
 * `dropped`. So a row of it with neither is one that came after, while the app was closed too. `lossy`: the codes did not fit, so
 * a row with no mark may be an old one and is taken as seen.
 */
type Taken = { dropped: Set<string>; lossy: boolean };
const CODE = /^[0-9a-f]{16}$/;
const ROWS = /^(?:[0-9a-f]{8})*$/;
/** A row's short code among the dropped ones: a fixed-size code of its id, never the id itself. */
const rowCode = (id: string) => stateCode(["row", id]).slice(0, 8);
function readSaved(): { seen: Map<string, Mark>; projects: Map<string, Taken> } {
  const seen = new Map<string, Mark>(), projects = new Map<string, Taken>();
  try {
    const raw: unknown = JSON.parse(storage()?.getItem(SEEN_CHANGES_KEY) ?? "null");
    const v = raw && typeof raw === "object" ? (raw as { v?: unknown }).v : undefined;
    if (v !== FORMAT && v !== 2) return { seen, projects };
    const marks = (raw as { seen?: unknown }).seen;
    if (Array.isArray(marks)) for (const mark of marks.slice(-SEEN_CHANGES_LIMIT))
      if (Array.isArray(mark) && mark.length === 3 && mark.every(x => typeof x === "string") && /^[ts]:./.test(mark[0]) && (mark[1] === QUIET || CODE.test(mark[1])))
        seen.set(mark[0], { key: mark[1], project: mark[2] });
    const taken = v === FORMAT ? (raw as { projects?: unknown }).projects : undefined;
    if (Array.isArray(taken)) for (const entry of taken.slice(-SEEN_PROJECTS_LIMIT))
      if (Array.isArray(entry) && entry.length === 3 && typeof entry[0] === "string" && entry[0] && typeof entry[1] === "string" && ROWS.test(entry[1]) && (entry[2] === 0 || entry[2] === 1))
        projects.set(entry[0], { dropped: new Set(entry[1].match(/.{8}/g) ?? []), lossy: entry[2] === 1 });
  } catch { /* nothing readable is kept: every row is taken as seen */ }
  return { seen, projects };
}
/**
 * The bound keeps the rows changed most recently, and before them every row with a change the owner has not opened yet. A row
 * it drops from a project taken in leaves its short code there, so it is not mistaken for a new row on the next launch.
 */
function writeSaved(seen: Map<string, Mark>, pending: ReadonlySet<string>, projects: Map<string, Taken>) {
  const all = [...seen];
  const ordered = [...all.filter(([id]) => !pending.has(id)), ...all.filter(([id]) => pending.has(id))].slice(-SEEN_CHANGES_LIMIT);
  const kept = new Set(ordered.map(([id]) => id));
  const out = new Map([...projects].slice(-SEEN_PROJECTS_LIMIT).map(([id, p]) => [id, { dropped: new Set(p.dropped), lossy: p.lossy }]));
  for (const [id, mark] of all) if (!kept.has(id)) out.get(mark.project)?.dropped.add(rowCode(id));
  // Codes beyond their bound: the projects holding the most give theirs up first and take their unremembered rows as seen.
  let total = 0;
  for (const p of out.values()) total += p.dropped.size;
  for (const p of [...out.values()].sort((a, b) => b.dropped.size - a.dropped.size)) {
    if (total <= SEEN_DROPPED_LIMIT) break;
    total -= p.dropped.size; p.dropped.clear(); p.lossy = true;
  }
  try {
    storage()?.setItem(SEEN_CHANGES_KEY, JSON.stringify({
      v: FORMAT, seen: ordered.map(([id, m]) => [id, m.key, m.project]),
      projects: [...out].map(([id, p]) => [id, [...p.dropped].join(""), p.lossy ? 1 : 0]),
    }));
  } catch { /* this run still remembers */ }
}

export type Unseen = {
  /** The task row has a change the owner has not opened yet. */
  task: (view: TaskView) => boolean;
  /** The sprint row has a change the owner has not opened yet. */
  sprint: (sprint: Sprint) => boolean;
  /** Finished tasks and sprints that stay in the sidebar until opened, and while open. */
  keep: SideKeep;
};

const taskOpen = (route: Route, id: string) => route.name === "task" && route.task === id;
const sprintOpen = (route: Route, sprint: Sprint) => (route.name === "sprint" && route.sprint === sprint.id)
  || (route.name === "project" && route.project === sprint.projectId && route.scope.kind === "sprint" && route.scope.id === sprint.id);
/** A project whose present tasks and sprints are known: a confirmed read delivered them, and it is readable and activated now. */
const known = (project: Project, read: ReadonlySet<string>) => read.has(project.id) && !project.issue && project.activated !== false;

/**
 * Which sidebar rows changed since the owner last opened them. A row is new when its task or sprint reaches a state the owner
 * should look at (see `taskChange` and `sprintChange`) other than the one the owner last saw it in. What is open on screen is
 * seen as it changes, and a row that comes to ask nothing is seen too, so its next such state reads as new again.
 *
 * Only a change positively observed reads as new. The rows of a project this app has never taken in (every row on the first
 * run, the rows of a project added since, or one the project bound dropped) have their present state taken as seen. Once a
 * project is taken in, a row of it with nothing remembered is one that came after: it reads as new once it asks something,
 * whether it appeared while the app was open or while it was closed. A row the row bound dropped is told apart from such a
 * new row by its short code; if those codes did not fit, the project's unremembered rows are taken as seen again, so the bound
 * never floods the sidebar. What is seen persists in this app's local storage, bounded, as row ids, project ids and fixed-size
 * codes only. A task or sprint gone from a project whose read confirmed it, even an empty one, takes its record along; an
 * unreadable or unread project keeps them.
 */
export function createUnseen(opts: { index: Accessor<WorkspaceIndex>; ready: Accessor<boolean>; read: Accessor<ReadonlySet<string>>; route: Accessor<Route> }): Unseen {
  const { seen, projects } = readSaved();
  // Projects whose rows this run has taken in. A row first met in one of them is new.
  const taken = new Set<string>();
  /** A row met for the first time is new when its project was taken in, now or on an earlier run, and the row was not there then. */
  const cameAfter = (project: string, id: string) => {
    if (taken.has(project)) return true;
    const p = projects.get(project);
    return !!p && !p.lossy && !p.dropped.has(rowCode(id));
  };
  // Finished rows opened in this run stay in their list while they are open, so the row the owner pressed does not vanish under the pointer.
  const opened = new Set<string>();
  const [version, setVersion] = createSignal(0);
  const visible = () => typeof document === "undefined" || !document.hidden;
  const [shown, setShown] = createSignal(visible());
  const onVisibility = () => setShown(visible());
  if (typeof document !== "undefined") {
    document.addEventListener("visibilitychange", onVisibility);
    onCleanup(() => document.removeEventListener("visibilitychange", onVisibility));
  }

  const reconcile = (index: WorkspaceIndex, route: Route, onScreen: boolean, read: ReadonlySet<string>) => {
    let stored = false, moved = false;
    const live = new Set(index.state.projects.filter(p => known(p, read)).map(p => p.id));
    const pending = new Set<string>();
    const note = (id: string, key: string | null, project: string, open: boolean) => {
      // A project not confirmed yet says nothing about its rows: a read on its way, an unreadable one or a cached first frame.
      if (!live.has(project)) return;
      const now = key ?? QUIET;
      let mark = seen.get(id);
      if (!mark) {
        mark = { key: cameAfter(project, id) ? QUIET : now, project };
        seen.set(id, mark); stored = true;
      }
      if (mark.key === now) return;
      if (now !== QUIET && !(open && onScreen)) { pending.add(id); return; }
      if (now !== QUIET && !opened.has(id)) { opened.add(id); moved = true; }
      // Re-inserted, so the rows changed most recently are the ones the bound keeps.
      seen.delete(id); seen.set(id, { key: now, project }); stored = true;
    };
    for (const v of index.views) if (!v.unknown) note(`t:${v.id}`, taskChange(v, index.state), v.project.id, taskOpen(route, v.id));
    const sprints = index.state.sprints.filter(onView);
    for (const s of sprints) note(`s:${s.id}`, sprintChange(index.state, s), s.projectId, sprintOpen(route, s));
    // Every row of a project taken in now has its mark, so its dropped codes are spent; it moves last, to be kept longest.
    for (const id of live) if (!taken.has(id)) { taken.add(id); projects.delete(id); projects.set(id, { dropped: new Set(), lossy: false }); moved = stored = true; }
    for (const id of opened) {
      const open = id.startsWith("t:") ? taskOpen(route, id.slice(2)) : sprints.some(s => s.id === id.slice(2) && sprintOpen(route, s));
      if (!open) { opened.delete(id); moved = true; }
    }
    // A task or sprint gone from a project whose read confirmed what it holds (an empty one too) takes its record along. One whose
    // project is not listed, not read yet or unreadable for now keeps it: a read still on its way is not a deletion.
    const tasks = new Set(index.views.map(v => v.id)), rows = new Set(sprints.map(s => s.id));
    for (const [id, mark] of seen) {
      if (id.startsWith("t:") ? tasks.has(id.slice(2)) : rows.has(id.slice(2))) continue;
      if (live.has(mark.project)) { seen.delete(id); opened.delete(id); stored = true; }
    }
    if (stored) writeSaved(seen, pending, projects);
    if (stored || moved) setVersion(n => n + 1);
  };
  createEffect(() => {
    const index = opts.index(), route = opts.route(), onScreen = shown(), read = opts.read();
    if (opts.ready()) untrack(() => reconcile(index, route, onScreen, read));
  });

  /** New, without asking whether it is open: what keeps a finished row listed. */
  const changed = (id: string, key: string | null, project: string) => {
    version();
    const mark = seen.get(id);
    return key !== null && taken.has(project) && !!mark && mark.key !== key;
  };
  const taskChanged = (view: TaskView) => !view.unknown && changed(`t:${view.id}`, taskChange(view, opts.index().state), view.project.id);
  const sprintChanged = (sprint: Sprint) => changed(`s:${sprint.id}`, sprintChange(opts.index().state, sprint), sprint.projectId);
  return {
    task: view => taskChanged(view) && !(shown() && taskOpen(opts.route(), view.id)),
    sprint: sprint => sprintChanged(sprint) && !(shown() && sprintOpen(opts.route(), sprint)),
    keep: {
      task: view => FINISHED.has(view.column) && (taskChanged(view) || (version(), opened.has(`t:${view.id}`))),
      sprint: sprint => sprint.queueState === "complete" && (sprintChanged(sprint) || (version(), opened.has(`s:${sprint.id}`))),
    },
  };
}
