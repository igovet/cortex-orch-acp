import { createMemo, Show } from "solid-js";
import { useWs } from "./ctx";
import { Confirmed } from "./freshness";
import { Tile } from "./frame";
import { A, I, Keyed } from "./kit";
import { SectionHead } from "./layout";
import { age, attentionOf, homeView, sprintView, type Decision, type ProjectSummary, type TaskView } from "./selectors";

const NEEDS_LIMIT = 12;
const taskRoute = (v: TaskView) => ({ name: "task" as const, machine: v.machine.id, task: v.id });

export function NeedRow(props: { decision: Decision; waiting: string; to: Parameters<typeof A>[0]["to"]; selected?: boolean; onPreview?: () => void }) {
  return <A class="need" to={props.to} aria-current={props.selected ? "true" : undefined}
    onKeyDown={e => { if (e.key === " " && props.onPreview) { e.preventDefault(); props.onPreview(); } }}>
    <span class={`kind ${props.decision.tone}`}><I n={props.decision.icon} s={14} />{props.decision.kind}</span>
    <span class="ask trunc">{props.decision.ask}</span>
    <span class="where trunc">{props.decision.where}</span><span class="age">{props.waiting}</span>
  </A>;
}

function ProjectLine(props: { summary: ProjectSummary; n: number }) {
  const ws = useWs();
  const route = () => ({ name: "project" as const, machine: props.summary.machine.id, project: props.summary.project.id, view: "board" as const, scope: { kind: "all" as const } });
  const count = (n: number, tone = "") => <Show when={n > 0} fallback={<span class="none"></span>}><span class={`tnum ${tone}`}>{n}</span></Show>;
  const sprints = () => props.summary.sprints.filter(s => s.queueState !== "complete").map(s => sprintView(ws.store.index(), s).title);
  return <div class="tr ws-tr" onClick={e => { if (!(e.target as Element).closest("a,button")) ws.go(route()); }}>
    <span class="row"><Tile name={props.summary.project.name} n={props.n} /><A class="w500 trunc" to={route()}>{props.summary.project.name}</A></span>
    {count(props.summary.counts.work)}{count(props.summary.counts.needs, "st-warn")}{count(props.summary.counts.review)}
    <Show when={sprints().length} fallback={<span class="none"></span>}><span class="trunc">{sprints()[0]}{sprints().length > 1 ? ` +${sprints().length - 1}` : ""}</span></Show>
    <Show when={props.summary.conflicts} fallback={<Show when={props.summary.worktrees} fallback={<span class="none"></span>}><span>{props.summary.worktrees} active</span></Show>}>
      <span class="st st-bad"><I n="alert" s={12} />{props.summary.conflicts} conflict{props.summary.conflicts === 1 ? "" : "s"}</span>
    </Show>
    <Show when={ws.store.mode === "real" && props.summary.offline} fallback={<span class="muted">{props.summary.at === null ? "" : `${age(props.summary.at, ws.store.clock())} ago`.replace("now ago", "Just now")}</span>}><Confirmed at={props.summary.project.confirmedAt} /></Show>
    <A class="ib" to={route()} aria-label={`Open ${props.summary.project.name}`}><I n="arrow-right" s={14} /></A>
  </div>;
}

/**
 * All projects on the machine in view, after one Needs you list: each task waiting on the owner appears once, with what it
 * waits for, and opens where that is decided. Engine limits live in the sidebar and in Connections. Counters and empty
 * states read the same lists.
 */
export function Home() {
  const ws = useWs();
  const view = createMemo(() => homeView(ws.store.index(), ws.machine().id));
  const inbox = () => ({ name: "inbox" as const, machine: ws.machine().id });
  return <Show when={view().projects.length > 0} fallback={<div class="note-panel"><span class="w500">No projects on this machine yet</span><span>Add a repository to start working here.</span>
    <button class="btn btn-primary ws-mt8" {...ws.needs("add_project")} onClick={() => { if (!ws.gate("add_project")) ws.open({ kind: "add-project" }); }}><I n="folder-plus" s={14} />Add project</button></div>}>
    <div class="home"><div class="home-in">
      <section class="full" aria-labelledby="h-need">
        <SectionHead id="h-need" title="Needs you" count={view().attention.length}><A class="more" to={inbox()}>Inbox</A></SectionHead>
        <Show when={view().attention.length > 0} fallback={<div class="surface ws-empty">Nothing needs you on this machine.</div>}>
          <div class="surface"><Keyed each={view().attention.slice(0, NEEDS_LIMIT)} by={v => v.id}>{v =>
            <NeedRow decision={attentionOf(v())} waiting={v().at === null ? "" : v().column === "needs" ? `waiting ${age(v().at, ws.store.clock())}` : `${age(v().at, ws.store.clock())} ago`.replace("now ago", "just now")} to={taskRoute(v())} selected={ws.selected() === v().id} onPreview={() => ws.select(v().id)} />
          }</Keyed></div>
        </Show>
      </section>

      <section class="full" aria-labelledby="h-proj">
        <SectionHead id="h-proj" title="Projects" count={view().projects.length} />
        <div class="surface table">
          <div class="tr th"><span>Project</span><span>Working</span><span>Needs you</span><span>Review</span><span>Sprints</span><span>Worktrees</span><span>Last activity</span><span></span></div>
          <Keyed each={view().projects} by={s => s.project.id}>{(s, i) => <ProjectLine summary={s()} n={i()} />}</Keyed>
        </div>
      </section>
    </div></div>
  </Show>;
}
