import { createMemo, Show } from "solid-js";
import { useWs } from "./ctx";
import { Confirmed } from "./freshness";
import { Limit, Tile } from "./frame";
import { A, Count, I, Keyed } from "./kit";
import { age, checkCounts, decisionOf, engineLimits, homeView, readyLabel, sameBranch, sprintView, type ProjectSummary, type TaskView } from "./selectors";

const NEEDS_LIMIT = 8;
const taskRoute = (v: TaskView) => ({ name: "task" as const, machine: v.machine.id, task: v.id });

export function NeedRow(props: { decision: ReturnType<typeof decisionOf>; waiting: string; to: Parameters<typeof A>[0]["to"]; selected?: boolean; onPreview?: () => void }) {
  return <A class="need" to={props.to} aria-current={props.selected ? "true" : undefined}
    onKeyDown={e => { if (e.key === " " && props.onPreview) { e.preventDefault(); props.onPreview(); } }}>
    <span class={`kind ${props.decision.tone}`}><I n={props.decision.icon} s={14} />{props.decision.kind}</span>
    <span class="ask trunc">{props.decision.ask}</span>
    <span class="where trunc">{props.decision.where}</span><span class="age">{props.waiting}</span>
  </A>;
}

function CheckFacts(props: { view: TaskView }) {
  const counts = () => checkCounts(props.view.task.result?.checks ?? []);
  return <>
    <Show when={counts().passed}><span class="st st-ok"><I n="check-circle" s={12} />{counts().passed} passed</span></Show>
    <Show when={counts().failed}><span class="st st-bad"><I n="x-circle" s={12} />{counts().failed} failed</span></Show>
    <Show when={counts().not_run}><span class="st st-idle"><I n="circle" s={12} />{counts().not_run} not run</span></Show>
    <Show when={counts().unverified}><span class="st st-idle"><I n="help" s={12} />{counts().unverified} unverified</span></Show>
  </>;
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

/** All projects on the machine in view. Counters and empty states read the same lists. */
export function Home() {
  const ws = useWs();
  const view = createMemo(() => homeView(ws.store.index(), ws.machine().id));
  const limits = createMemo(() => engineLimits(ws.store.state(), ws.machine()));
  const inbox = () => ({ name: "inbox" as const, machine: ws.machine().id });
  return <Show when={view().projects.length > 0} fallback={<div class="note-panel"><span class="w500">No projects on this machine yet</span><span>Add a repository to start working here.</span>
    <button class="btn btn-primary ws-mt8" {...ws.needs("add_project")} onClick={() => { if (!ws.gate("add_project")) ws.open({ kind: "add-project" }); }}><I n="folder-plus" s={14} />Add project</button></div>}>
    <div class="home"><div class="home-in">
      <section class="full" aria-labelledby="h-need">
        <h2 class="sec-h" id="h-need">Needs you <Count n={view().needs.length} /><A class="more" to={inbox()}>All in Inbox</A></h2>
        <Show when={view().needs.length > 0} fallback={<div class="surface ws-empty">Nothing needs you on this machine.</div>}>
          <div class="surface"><Keyed each={view().needs.slice(0, NEEDS_LIMIT)} by={v => v.id}>{v =>
            <NeedRow decision={decisionOf(v())} waiting={v().at === null ? "" : `waiting ${age(v().at, ws.store.clock())}`} to={taskRoute(v())} selected={ws.selected() === v().id} onPreview={() => ws.select(v().id)} />
          }</Keyed></div>
        </Show>
      </section>

      <section aria-labelledby="h-review">
        <h2 class="sec-h" id="h-review">Ready for review <Count n={view().review.length} /></h2>
        <Show when={view().review.length > 0} fallback={<div class="surface ws-empty">Nothing is waiting for review.</div>}>
          <div class="surface"><Keyed each={view().review} by={v => v.id}>{v => <div class="item">
            <div class="grow">
              <div class="t trunc">{v().task.title}</div>
              <div class="s"><span>{v().project.name}</span><CheckFacts view={v()} /></div>
            </div>
            <A class="btn btn-line btn-sm" to={taskRoute(v())}>Review</A>
          </div>}</Keyed></div>
        </Show>
      </section>

      <section aria-labelledby="h-merge">
        <h2 class="sec-h" id="h-merge">Ready to finish <Count n={view().merge.length} /></h2>
        <Show when={view().merge.length > 0} fallback={<div class="surface ws-empty">Nothing is ready to finish.</div>}>
          <div class="surface"><Keyed each={view().merge} by={v => v.id}>{v => <div class="item">
            <div class="grow">
              <div class="t trunc">{v().task.title}</div>
              <div class="s"><span>{v().project.name}</span><span class="mono trunc">{sameBranch(v().task, v().target) ? readyLabel(v().task, v().target) : `${v().task.checkout.branch} → ${v().target}`}</span></div>
            </div>
            <A class="btn btn-line btn-sm" to={taskRoute(v())}>{sameBranch(v().task, v().target) ? "Review task" : "Merge…"}</A>
          </div>}</Keyed></div>
        </Show>
      </section>

      <section class="full" aria-labelledby="h-limits">
        <h2 class="sec-h" id="h-limits">Engine limits <span class="n">{ws.machine().name}</span><A class="more" to={{ name: "settings", section: "connections", machine: ws.machine().id }}>All limits and windows</A></h2>
        <Show when={limits().rows.length > 0} fallback={<div class="surface ws-empty">{limits().note}</div>}>
          <div class="surface u-grid"><Keyed each={limits().rows} by={r => r.engine}>{r => <Limit row={r()} />}</Keyed></div>
        </Show>
      </section>

      <section class="full" aria-labelledby="h-proj">
        <h2 class="sec-h" id="h-proj">Projects <Count n={view().projects.length} /></h2>
        <div class="surface table">
          <div class="tr th"><span>Project</span><span>Working</span><span>Needs you</span><span>Review</span><span>Sprints</span><span>Worktrees</span><span>Last activity</span><span></span></div>
          <Keyed each={view().projects} by={s => s.project.id}>{(s, i) => <ProjectLine summary={s()} n={i()} />}</Keyed>
        </div>
      </section>
    </div></div>
  </Show>;
}
