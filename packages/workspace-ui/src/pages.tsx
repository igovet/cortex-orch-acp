import { createMemo, Show } from "solid-js";
import { taskRoute } from "./board";
import { useWs } from "./ctx";
import { NeedRow } from "./home";
import { A, Keyed } from "./kit";
import type { Route } from "./router";
import { age, inboxItems, sprintRoute } from "./selectors";

/** Every connected machine, with the decisions no board column shows. Each row opens where that decision is made. */
export function Inbox() {
  const ws = useWs();
  const items = createMemo(() => inboxItems(ws.store.index()));
  const to = (d: ReturnType<typeof inboxItems>[number]): Route => {
    if (d.key.startsWith("archive:")) return { name: "archive", machine: d.project.machineId };
    if (d.view) return taskRoute(d.view);
    const sprint = ws.store.state().sprints.find(s => s.id === d.sprintId);
    return sprint ? sprintRoute(d.project.machineId, sprint) : { name: "project", machine: d.project.machineId, project: d.project.id, view: "board", scope: { kind: "all" } };
  };
  const machines = () => ws.store.state().machines.filter(m => m.connection === "connected").length;
  return <div class="page"><div class="page-in">
    <div class="page-h"><div><h1>Inbox</h1><p>Decisions from {machines() === 1 ? "this machine" : `all ${machines()} connected machines`}.</p></div></div>
    <Show when={items().length > 0} fallback={<div class="surface ws-empty">Nothing needs you right now.</div>}>
      <div class="surface"><Keyed each={items()} by={d => d.key}>{d => <NeedRow decision={d()} waiting={d().at !== null ? `waiting ${age(d().at, ws.store.clock())}` : ""} to={to(d())} />}</Keyed></div>
    </Show>
  </div></div>;
}

export const NotFound = (props: { message: string }) => {
  const ws = useWs();
  return <div class="note-panel"><span class="w500">{props.message}</span><A class="lnk" to={{ name: "home", machine: ws.machine().id }}>Back to Home</A></div>;
};
