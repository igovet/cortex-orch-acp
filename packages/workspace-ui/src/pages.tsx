import { createMemo, Show } from "solid-js";
import { taskRoute } from "./board";
import { useWs } from "./ctx";
import { NeedRow } from "./home";
import { A, Keyed } from "./kit";
import type { Route } from "./router";
import { age, decisionRoute, inboxItems, sprintRoute, type Decision } from "./selectors";

/**
 * Home's Needs you list for every connected machine: each row a compact summary of one decision, in the order it matters,
 * that opens where it is made. A question, permission or result opens the task's conversation, where it is answered or
 * accepted inline; a conflict opens its resolution; a planning question its sprint's planning; unmerged work the Archive.
 * Nothing is answered here.
 */
export function Inbox() {
  const ws = useWs();
  const items = createMemo(() => inboxItems(ws.store.index()));
  const to = (d: Decision): Route => {
    if (d.key.startsWith("archive:")) return { name: "archive", machine: d.project.machineId };
    if (d.view) return d.kind === "Conflict" ? decisionRoute(d.view) : taskRoute(d.view);
    const sprint = ws.store.state().sprints.find(s => s.id === d.sprintId);
    return sprint ? sprintRoute(d.project.machineId, sprint) : { name: "project", machine: d.project.machineId, project: d.project.id, view: "board", scope: { kind: "all" } };
  };
  // A decision has waited since it was asked; a result or accepted work was simply made some time ago.
  const when = (d: Decision) => d.at === null ? "" : d.kind === "Review" || d.kind === "Finish" ? `${age(d.at, ws.store.clock())} ago`.replace("now ago", "just now") : `waiting ${age(d.at, ws.store.clock())}`;
  const machines = () => ws.store.state().machines.filter(m => m.connection === "connected").length;
  return <div class="page"><div class="page-in">
    <div class="page-h"><div><h1>Inbox</h1><p>What waits on you {machines() === 1 ? "on this machine" : `on all ${machines()} connected machines`}, most urgent first.</p></div></div>
    <Show when={items().length > 0} fallback={<div class="surface ws-empty">Nothing needs you right now.</div>}>
      <div class="surface"><Keyed each={items()} by={d => d.key}>{d => <NeedRow decision={d()} waiting={when(d())} to={to(d())} />}</Keyed></div>
    </Show>
  </div></div>;
}

export const NotFound = (props: { message: string }) => {
  const ws = useWs();
  return <div class="note-panel"><span class="w500">{props.message}</span><A class="lnk" to={{ name: "home", machine: ws.machine().id }}>Back to Home</A></div>;
};
