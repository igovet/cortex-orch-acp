import { DAY, purgeBlocker } from "@cortex/workspace-domain";
import type { TaskView, WorkspaceIndex } from "./selectors";

const MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
export const shortDate = (at: number) => { const d = new Date(at); return `${d.getDate()} ${MONTHS[d.getMonth()]}`; };
export const sizeText = (mb: number) => mb >= 1024 ? `${(mb / 1024).toFixed(1)} GB` : `${Math.round(mb)} MB`;

export type Outcome = "committed" | "merged" | "as_is" | "cancel" | "external" | "done";
const OUTCOMES: Record<Outcome, { text: string; tone: string }> = {
  committed: { text: "Committed", tone: "st-idle" },
  external: { text: "Done outside the app", tone: "st-idle" },
  merged: { text: "Merged", tone: "st-merge" }, as_is: { text: "Closed as is", tone: "st-idle" }, cancel: { text: "Cancelled", tone: "st-idle" }, done: { text: "Done", tone: "st-idle" },
};
export const OUTCOME_FILTERS: { id: Outcome; label: string }[] = [{ id: "committed", label: "Committed" }, { id: "merged", label: "Merged" }, { id: "as_is", label: "Closed as is" }, { id: "cancel", label: "Cancelled" }];
const outcomeOf = (v: TaskView): Outcome => v.task.integration?.method === "commit" ? "committed" : v.task.work === "merged" ? "merged" : v.task.closedAs ?? "done";

export type ArchiveRow = {
  view: TaskView; group: "soon" | "later" | "blocked"; left: number; percent: number; outcome: Outcome; outcomeText: string; outcomeTone: string;
  sub: string; size: number | null; blocker: string | null;
  /** Blocked rows read Needs you when the owner can decide, Overdue when the machine has to come back first. */
  status: { text: string; icon: string } | null;
};
function subline(v: TaskView, outcome: Outcome, blocker: string | null) {
  if (blocker) return blocker;
  const when = v.task.doneAt === null ? "" : ` ${shortDate(v.task.doneAt)}`;
  if (outcome === "committed") return `Committed to ${v.task.integration!.targetBranch}${when}`;
  if (outcome === "merged") return `Merged${when}${v.task.integration?.method === "pr" ? ", through a pull request" : ""}`;
  if (outcome === "external") return "Done outside the app; not accepted or merged here";
  if (outcome === "as_is") return "Closed as is, not verified";
  if (outcome === "cancel") return `Cancelled${when}`;
  return "Done";
}
export function archiveRow(index: WorkspaceIndex, v: TaskView): ArchiveRow {
  const retention = v.machine.retentionDays, archivedAt = v.task.archive?.at ?? index.now;
  const left = Math.ceil(retention - (index.now - archivedAt) / DAY), outcome = outcomeOf(v);
  const stated = v.task.purgeBlocker;
  const live = v.task.unexpectedChanges || v.task.process !== "stopped" || left <= 0 ? purgeBlocker(index.state, v.task) : null;
  const blocker = stated ?? live;
  const offline = v.machine.connection !== "connected";
  const percent = Math.max(0, Math.min(100, Math.round(left / retention * 100)));
  return {
    view: v, left, percent, outcome, outcomeText: OUTCOMES[outcome].text, outcomeTone: OUTCOMES[outcome].tone, sub: subline(v, outcome, blocker), size: v.task.archive?.sizeMb ?? null, blocker,
    group: blocker ? "blocked" : left <= 3 ? "soon" : "later",
    status: blocker ? { text: offline ? "Overdue" : "Needs you", icon: "alert" } : null,
  };
}
export const daysLeft = (n: number) => n <= 0 ? "Due now" : `${n} day${n === 1 ? "" : "s"} left`;

export type ArchiveFilter = { text: string; machine: string | null; project: string | null; outcome: Outcome | null };
export const NO_FILTER: ArchiveFilter = { text: "", machine: null, project: null, outcome: null };
/** Every machine at once: the Archive is shared, like the Inbox. */
export function archiveView(index: WorkspaceIndex, filter: ArchiveFilter) {
  const fits = (v: TaskView) => (!filter.machine || v.machine.id === filter.machine) && (!filter.project || v.project.id === filter.project) && (!filter.text.trim() || v.task.title.toLowerCase().includes(filter.text.trim().toLowerCase()));
  const all = index.archive.map(v => archiveRow(index, v));
  const rows = all.filter(r => fits(r.view) && (!filter.outcome || r.outcome === filter.outcome)).sort((a, b) => a.left - b.left);
  const waiting = index.views.filter(v => v.task.archiveQuestion && !v.task.archive && fits(v));
  return {
    total: all.length, soonTotal: all.filter(r => r.group === "soon").length, waiting, rows,
    soon: rows.filter(r => r.group === "soon"), later: rows.filter(r => r.group === "later"), blocked: rows.filter(r => r.group === "blocked"),
  };
}
const CLEANUP_HOUR = 3;
/** The daily run, for the machine in view. An offline machine runs it when it comes back. */
export function nextCleanup(machine: { name: string; connection: string }, now: number) {
  if (machine.connection !== "connected") return `${machine.name} is offline. It cleans up at its next run`;
  const today = new Date(now).getUTCHours() < CLEANUP_HOUR;
  return `Next cleanup ${today ? "today" : "tonight"} at 0${CLEANUP_HOUR}:00 on ${machine.name}`;
}
