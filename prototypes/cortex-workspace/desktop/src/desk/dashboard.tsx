import { createMemo, createSignal, For, Match, Show, Switch } from "solid-js";
import { useDesk } from "./ctx";
import { Icon } from "./icons";
import { AS_OF, AS_OF_SHORT, absTime, hhmm, LAST_VISIT, relTime, type Decision, type ProjectSummary, type TaskView } from "./model";
import { A, ErrorState, Loading, Notice, Skeleton, VirtualList } from "./ui";

export function DecisionRow(props: { decision: Decision; tv: TaskView; showProject?: boolean }) {
  const desk = useDesk();
  const kind = () => props.decision.kind === "permission" ? "Permission" : "Question";
  return <li>
    <A class="needs-row" href={`/p/${props.tv.slug}/t/${props.tv.num}/decisions`}>
      <Icon name={props.decision.kind === "permission" ? "key" : "question"} class="tone-warn-fg" />
      <span class="needs-title"><span class="needs-kind">{kind()}</span> · {props.decision.title}</span>
      <span class="needs-ctx">{props.showProject !== false ? `${props.tv.project.name} · ` : ""}{props.tv.ref}</span>
      <span class="needs-meta">{desk.blocked(props.tv.project.id) ? `Actions unavailable · ${props.tv.disconnected ? "disconnected" : "offline"}` : "Requested: unknown"}</span>
      <Icon name="chevronRight" class="row-chev" />
    </A>
  </li>;
}

/** Needs-you list. The empty copy renders only when the same selector returns 0 and the load succeeded. */
export function NeedsList(props: { items: { decision: Decision; tv: TaskView }[]; showProject?: boolean; label: string; asOf?: string }) {
  const desk = useDesk();
  return <Switch>
    <Match when={desk.data.status() === "loading"}><div class="group"><Loading label={`Loading ${props.label}`} rows={2} h="1.25rem" /></div></Match>
    <Match when={desk.data.status() === "error"}><ErrorState text="Couldn't load decisions (fixture error E_PAGE). Nothing was changed." retry={() => desk.store.selectScenario("default")} /></Match>
    <Match when={props.items.length === 0}><div class="group"><p class="empty-row">{props.asOf
      ? `Nothing needed your decision as of ${props.asOf} (last confirmed); the machine is disconnected.`
      : `Nothing needs your decision. Checked ${desk.data.offline() ? `${AS_OF_SHORT}, before this device went offline` : AS_OF_SHORT} (fixture).`}</p></div></Match>
    <Match when={props.items.length > 0}><ul class="group needs-list" role="list"><For each={props.items}>{x => <DecisionRow decision={x.decision} tv={x.tv} showProject={props.showProject} />}</For></ul></Match>
  </Switch>;
}

function ChecksCell(props: { ps: ProjectSummary }) {
  const parts = () => [props.ps.failed ? `${props.ps.failed} failed` : "", props.ps.notRun ? `${props.ps.notRun} not run` : ""].filter(Boolean);
  return <Show when={parts().length} fallback={<span class="tertiary sm-text">No failed or unrun checks</span>}>
    <span class={props.ps.failed ? "tone-danger-fg" : ""}>{props.ps.failed ? <Icon name="xCircle" size={14} /> : <Icon name="notRun" size={14} />} {parts().join(" · ")}</span>
  </Show>;
}

export function ProjectTable() {
  const desk = useDesk();
  const wide = desk.wide;
  /** Device offline: every cell is the last known value, never a present-tense fact. */
  const lastKnown = () => desk.data.offline();
  return <div class={`proj-table ${wide() ? "wide" : ""}`}>
    <div class="proj-head" aria-hidden="true">
      <span>Project</span><Show when={wide()}><span>Tasks</span></Show><span>Machine</span><span>Needs you</span><span>Checks</span>
      <Show when={wide()}><span>Stale</span></Show><span>Workers</span><span>Last activity</span><span />
    </div>
    <Switch>
      <Match when={desk.data.status() === "loading"}>
        <div role="status" aria-label="Loading projects"><For each={desk.data.projects()}>{() => <div class="proj-row skeleton-row" aria-hidden="true"><Skeleton w="1.25rem" h="1.25rem" /><Skeleton w="40%" /></div>}</For></div>
      </Match>
      <Match when={desk.data.projects().length === 0}><p class="empty-row">No projects yet. Projects appear when a machine is paired.</p></Match>
      <Match when={true}>
        <ul role="list" aria-label="Projects">
          <For each={desk.data.projects()}>{ps => <li>
            <A class="proj-row" href={`/p/${ps.slug}`} aria-describedby={`proj-desc-${ps.index}`}>
              <span class="proj-name-cell">
                <span class="mono-wrap"><span class={`mono-tile hue-${ps.index % 8}`} style={{ "--tile": "1.25rem" }} aria-hidden="true">{ps.project.name[0]}</span></span>
                <span class="proj-name-text"><span class="proj-name">{ps.project.name}</span><span class="proj-repo" title={ps.project.repository}>{ps.project.repository}</span></span>
              </span>
              <Show when={wide()}><span class="cell tabular">
                <Show when={desk.data.status() === "ready"} fallback="—"><span>{ps.taskCount}</span><span class="tertiary sm-text">Backlog {ps.phases.backlog} · Working {ps.phases.working} · Review {ps.phases.review} · Done {ps.phases.done}</span></Show>
              </span></Show>
              <span class="cell machine-cell">
                <Show when={ps.disconnected} fallback={<span class="conn-ok"><span class={`status-dot ${lastKnown() ? "" : "tone-success"}`} aria-hidden="true" />Connected</span>}>
                  <span class="tone-danger-fg"><Icon name="plugOff" size={14} /> Disconnected</span>
                </Show>
                <Show when={ps.conflict}><span class="tone-danger-fg"><Icon name="gitMerge" size={14} /> Git conflict</span></Show>
                <Show when={lastKnown()}><span class="tertiary sm-text">as of {AS_OF_SHORT}</span></Show>
              </span>
              <span class="cell tabular">
                <Switch>
                  <Match when={desk.data.status() === "error"}><span title="Couldn't load">?</span></Match>
                  <Match when={ps.needsYou > 0}><span class="count-chip tone-warn">{ps.needsYou}</span></Match>
                  <Match when={true}><span aria-label="none" class="tertiary">—</span></Match>
                </Switch>
              </span>
              <span class="cell"><Show when={desk.data.status() === "ready"} fallback={<span class="tertiary">—</span>}><ChecksCell ps={ps} /></Show></span>
              <Show when={wide()}><span class="cell tabular">{ps.stale ? <span class="tone-warn-fg"><Icon name="history" size={14} /> {ps.stale}</span> : <span class="tertiary">0</span>}</span></Show>
              <span class="cell tabular">
                <Show when={ps.running !== "unknown"} fallback={<span title="Machine disconnected; current worker count unknown" aria-description="Machine disconnected; current worker count unknown">Unknown <Icon name="help" size={14} /></span>}>
                  {desk.data.status() === "ready" ? String(ps.running) : "—"}
                  <Show when={lastKnown() && desk.data.status() === "ready"}><span class="tertiary sm-text">last known</span></Show>
                </Show>
              </span>
              <span class="cell" title={absTime(ps.lastActivity)}>
                {desk.data.status() !== "ready" ? <span class="tertiary" aria-label="unavailable">—</span> : ps.lastActivity
                  ? (ps.disconnected ? `as of ${hhmm(ps.lastActivity)}` : lastKnown() ? `at ${hhmm(ps.lastActivity)}` : relTime(ps.lastActivity)) : "No activity"}
              </span>
              <Icon name="chevronRight" class="row-chev" />
              <span id={`proj-desc-${ps.index}`} class="sr-only">
                {ps.disconnected ? "Machine disconnected; current worker count unknown." : ""} {ps.conflict ? "Git conflict." : ""}
              </span>
            </A>
          </li>}</For>
        </ul>
      </Match>
    </Switch>
    <Show when={desk.data.status() === "error"}><ErrorState text="Couldn't load project tasks (fixture error E_PAGE). Counts are unavailable. Nothing was changed." retry={() => desk.store.selectScenario("default")} /></Show>
  </div>;
}

export function SinceLastVisit() {
  const desk = useDesk();
  const changes = () => [...desk.local().changes].reverse();
  return <Show when={changes().length} fallback={<p class="empty-row">No changes since {LAST_VISIT}.</p>}>
    <ul class="since-list" role="list"><For each={changes()}>{c => <li class="since-row"><Icon name="flask" size={14} class="tone-mock-fg" /><span>{c.text}</span></li>}</For></ul>
  </Show>;
}

/** All fixture events, virtualised: only visible rows exist in the DOM. */
function ActivityLog() {
  const desk = useDesk();
  const events = () => desk.store.visibleTasks().length ? desk.data.fixture().events : [];
  const names = createMemo(() => new Map(desk.data.fixture().projects.map(p => [p.id, p.name])));
  const nums = createMemo(() => new Map(desk.data.tasks().map(t => [t.task.id, t.ref])));
  const ordered = createMemo(() => [...events()].reverse());
  return <Show when={ordered().length} fallback={<p class="empty-row">No events in this view.</p>}>
    <VirtualList items={ordered()} rem={1.75} class="activity-log" label={`Activity, ${ordered().length.toLocaleString("en")} fixture events, newest first`} role="region" tabindex={0}
      render={e => <div class="activity-row">
        <span class="mono tertiary">{hhmm(e.occurredAt)}:{e.occurredAt.slice(17, 19)}</span>
        <span class="activity-proj">{names().get(e.projectId)}</span>
        <span class="mono tertiary">{nums().get(e.taskId)}</span>
        <span class="tertiary">{e.kind}</span>
        <span class="activity-text">{e.text} · reported</span>
      </div>} />
  </Show>;
}

export function Dashboard() {
  const desk = useDesk();
  const t = desk.data.totals;
  const [activityOpen, setActivityOpen] = createSignal(false);
  return <div class="scroll-pane"><div class={`page dash ${desk.wide() ? "wide" : ""}`}>
    <div class="dash-main">
      <h1 id="view-title" tabindex="-1">All projects</h1>
      <p class="subline tabular">
        <Switch>
          <Match when={desk.data.status() === "loading"}><Skeleton w="22rem" /></Match>
          <Match when={desk.data.status() === "error"}>{t().projects} projects · task counts unavailable · {t().connected} of {t().projects} machines connected</Match>
          <Match when={true}>{t().projects} projects · {t().tasks} tasks · {t().connected} of {t().projects} machines connected · as of {AS_OF}</Match>
        </Switch>
      </p>
      <Show when={desk.data.offline()}><Notice tone="neutral" icon="plugZap">This device is offline. Showing data as of {AS_OF_SHORT} (fixture). Actions are unavailable until it reconnects.</Notice></Show>
      <section class="dash-section" aria-labelledby="needs-h">
        <div class="group-label"><h2 id="needs-h">Needs you</h2><Show when={desk.data.status() === "ready"}><span class="tabular tertiary">{desk.data.needs().length}</span></Show></div>
        <NeedsList items={desk.data.needs()} label="decisions" />
      </section>
      <section class="dash-section" aria-labelledby="projects-h">
        <div class="group-label"><h2 id="projects-h">Projects</h2></div>
        <ProjectTable />
      </section>
      <Show when={!desk.wide()}>
        <section class="dash-section" aria-labelledby="since-h">
          <button type="button" class="disclosure" aria-expanded={desk.prefs.sinceOpen()} aria-controls="since-body" onClick={() => desk.prefs.setSinceOpen(!desk.prefs.sinceOpen())}>
            <Icon name={desk.prefs.sinceOpen() ? "chevronDown" : "chevronRight"} size={14} />
            <span id="since-h">Since your last visit</span><span class="tertiary tabular">{desk.local().changes.length}</span>
          </button>
          <Show when={desk.prefs.sinceOpen()}><div id="since-body" class="group"><SinceLastVisit /></div></Show>
        </section>
      </Show>
      <section class="dash-section" aria-labelledby="activity-h">
        <button type="button" class="disclosure" aria-expanded={activityOpen()} aria-controls="activity-body" onClick={() => setActivityOpen(!activityOpen())}>
          <Icon name={activityOpen() ? "chevronDown" : "chevronRight"} size={14} />
          <span id="activity-h">All activity</span>
          <span class="tertiary tabular">{desk.data.status() === "ready" ? `${(desk.store.visibleTasks().length ? desk.data.fixture().events.length : 0).toLocaleString("en")} fixture events` : ""}</span>
        </button>
        <Show when={activityOpen()}><div id="activity-body" class="group">
          <Switch>
            <Match when={desk.data.status() === "loading"}><Loading label="Loading activity" /></Match>
            <Match when={desk.data.status() === "error"}><ErrorState text="Couldn't load activity (fixture error E_PAGE)." /></Match>
            <Match when={true}><ActivityLog /></Match>
          </Switch>
        </div></Show>
      </section>
    </div>
    <Show when={desk.wide()}>
      <aside class="dash-rail" aria-labelledby="rail-h">
        <div class="group-label"><h2 id="rail-h">Since your last visit</h2></div>
        <div class="group"><SinceLastVisit /></div>
      </aside>
    </Show>
  </div></div>;
}

export function NeedsYouPage() {
  const desk = useDesk();
  return <div class="scroll-pane"><div class="page narrow">
    <h1 id="view-title" tabindex="-1">Needs you</h1>
    <p class="subline">Open decisions across all projects. Decisions have no keyboard shortcuts; open one to answer it.</p>
    <NeedsList items={desk.data.needs()} label="decisions" />
  </div></div>;
}
