import { createEffect, createMemo, createSignal, For, Match, onMount, Show, Switch, type JSX } from "solid-js";
import { useDesk } from "./ctx";
import { NeedsList } from "./dashboard";
import { Icon } from "./icons";
import {
  AS_OF_SHORT, FILTERS, FILTER_LABEL, absTime, hhmm, matchesFilters, PHASES, PHASE_LABEL, relTime,
  type FilterId, type Phase, type ProjectSummary, type TaskView,
} from "./model";
import { GitPanel, MachinePanel } from "./panels";
import { PROJECT_VIEWS, type ProjectViewId } from "./router";
import { A, Chip, ErrorState, hasChips, lineCount, Loading, Monogram, Notice, Popover, Skeleton, TabOverflow, TaskChips, VirtualList, type VirtualApi } from "./ui";

const VIEW_LABEL: Record<ProjectViewId, string> = { overview: "Overview", board: "Board", list: "List", decisions: "Decisions", git: "Git", machine: "Machine" };
const VIEW_ICON = { overview: "overview", board: "board", list: "list", decisions: "inbox", git: "gitBranch", machine: "plug" } as const;

/** Remembered per project: roving card position and the task to refocus when returning. */
export const boardMemory = new Map<string, { col: number; idx: number; scroll: number[] }>();
export const returnFocus = { taskId: "" };

export function ProjectPage(props: { ps: ProjectSummary; view: ProjectViewId }) {
  const desk = useDesk();
  createEffect(() => desk.prefs.setLastView(props.ps.slug, props.view));
  const filters = () => desk.local().filters[props.ps.project.id] ?? [];
  const setFilters = (f: FilterId[]) => desk.setLocal(l => ({ ...l, filters: { ...l.filters, [props.ps.project.id]: f } }));
  return <div class="project-page">
    <ViewTabs ps={props.ps} view={props.view} filters={filters()} setFilters={setFilters} />
    <Switch>
      <Match when={props.view === "board"}><Board ps={props.ps} filters={filters()} setFilters={setFilters} /></Match>
      <Match when={props.view === "list"}><TaskList ps={props.ps} filters={filters()} setFilters={setFilters} /></Match>
      <Match when={true}>
        <div class="scroll-pane"><div class={`page ${props.view === "overview" ? "" : "narrow"}`}>
          <ProjectHeader ps={props.ps} />
          <Switch>
            <Match when={props.view === "overview"}><Overview ps={props.ps} setFilters={setFilters} /></Match>
            <Match when={props.view === "decisions"}>
              <NeedsList items={desk.data.needs().filter(n => n.tv.project.id === props.ps.project.id)} showProject={false} label="project decisions" />
            </Match>
            <Match when={props.view === "git"}><GitPanel ps={props.ps} /></Match>
            <Match when={props.view === "machine"}><MachinePanel ps={props.ps} /></Match>
          </Switch>
        </div></div>
      </Match>
    </Switch>
  </div>;
}

function ViewTabs(props: { ps: ProjectSummary; view: ProjectViewId; filters: FilterId[]; setFilters: (f: FilterId[]) => void }) {
  const desk = useDesk();
  let list!: HTMLDivElement;
  const onKey = (e: KeyboardEvent) => {
    const tabs = [...list.querySelectorAll<HTMLElement>("[role=tab]")];
    const i = tabs.indexOf(document.activeElement as HTMLElement);
    const next = e.key === "ArrowRight" ? tabs[(i + 1) % tabs.length] : e.key === "ArrowLeft" ? tabs[(i - 1 + tabs.length) % tabs.length]
      : e.key === "Home" ? tabs[0] : e.key === "End" ? tabs[tabs.length - 1] : undefined;
    if (next) { e.preventDefault(); tabs.forEach(t => t.setAttribute("tabindex", t === next ? "0" : "-1")); next.focus(); }
    if (e.key === " " && i >= 0) { e.preventDefault(); tabs[i].click(); }
  };
  const decisions = () => props.ps.needsYou;
  return <div class="viewtabs"><div class="tabs-wrap">
    <div role="tablist" aria-label={`${props.ps.project.name} views`} ref={list} onKeyDown={onKey} class="tabstrip">
      <For each={PROJECT_VIEWS}>{v => <A role="tab" class="tab" href={`/p/${props.ps.slug}/${v}`} aria-selected={props.view === v} tabindex={props.view === v ? 0 : -1}
        aria-label={`${VIEW_LABEL[v]}${v === "decisions" ? `, ${desk.data.status() === "ready" ? decisions() : "unknown"} open${props.ps.disconnected ? `, as of ${AS_OF_SHORT}` : ""}` : ""}`}>
        <Icon name={VIEW_ICON[v]} size={14} />{VIEW_LABEL[v]}
        <Show when={v === "decisions" && desk.data.status() === "ready" && decisions() > 0}><span class="count-chip tone-warn" aria-hidden="true">{decisions()}</span></Show>
        <Show when={v === "decisions" && desk.data.status() === "loading"}><Skeleton w="0.75rem" h="0.75rem" /></Show>
      </A>}</For>
    </div>
    <TabOverflow strip={() => list} label={`All ${props.ps.project.name} views`} items={PROJECT_VIEWS.map(v => ({
      label: `${VIEW_LABEL[v]}${v === "decisions" && desk.data.status() === "ready" && decisions() > 0 ? ` (${decisions()} open)` : ""}`, href: `/p/${props.ps.slug}/${v}`, selected: props.view === v }))} />
    </div>
    <Show when={props.view === "board" || props.view === "list"}>
      <div class="filter-area">
        <For each={props.filters}>{f => <span class="filter-chip">{FILTER_LABEL[f]}
          <button type="button" aria-label={`Remove filter ${FILTER_LABEL[f]}`} onClick={() => props.setFilters(props.filters.filter(x => x !== f))}><Icon name="x" size={12} /></button></span>}</For>
        <Popover label="Filter tasks" place="below-end" trigger={p => <button type="button" {...p} class="btn btn-ghost sm"><Icon name="filter" size={14} /><span>Filter</span></button>}>
          {close => <div class="menu" role="menu" aria-label="Filter tasks" onKeyDown={e => {
            const items = [...(e.currentTarget as HTMLElement).querySelectorAll<HTMLElement>("[role=menuitemcheckbox]")];
            const i = items.indexOf(document.activeElement as HTMLElement);
            const n = e.key === "ArrowDown" ? items[(i + 1) % items.length] : e.key === "ArrowUp" ? items[(i - 1 + items.length) % items.length] : undefined;
            if (n) { e.preventDefault(); n.focus(); }
          }}>
            <For each={FILTERS}>{f => <button type="button" role="menuitemcheckbox" class="menu-item" tabindex="-1" aria-checked={props.filters.includes(f)}
              onClick={() => props.setFilters(props.filters.includes(f) ? props.filters.filter(x => x !== f) : [...props.filters, f])}>
              <span class="menu-icon">{props.filters.includes(f) ? <Icon name="check" /> : null}</span><span class="menu-label">{FILTER_LABEL[f]}</span>
            </button>}</For>
            <div class="menu-sep" role="separator" />
            <button type="button" role="menuitem" class="menu-item" tabindex="-1" disabled={!props.filters.length} onClick={() => { props.setFilters([]); close(); }}>
              <span class="menu-icon" /><span class="menu-label">Clear filters</span></button>
          </div>}
        </Popover>
      </div>
    </Show>
  </div>;
}

export function ProjectHeader(props: { ps: ProjectSummary }) {
  return <div class="project-header">
    <Monogram name={props.ps.project.name} index={props.ps.index} size={24} />
    <div class="ph-text">
      <h1 id="view-title" tabindex="-1">{props.ps.project.name}</h1>
      <p class="mono tertiary">{props.ps.project.repository} · {props.ps.project.branch}</p>
    </div>
    <div class="chips">
      <Chip tone={props.ps.disconnected ? "danger" : "neutral"} icon={props.ps.disconnected ? "plugOff" : "plug"}>{props.ps.disconnected ? "Disconnected" : "Connected"}</Chip>
      <Chip icon="gitBranch">{props.ps.project.branch}</Chip>
      <Chip tone={props.ps.conflict ? "danger" : "neutral"} icon={props.ps.conflict ? "gitMerge" : "gitCommit"}>{props.ps.conflict ? "Conflict" : "Clean"}{props.ps.disconnected ? " · last known" : ""}</Chip>
    </div>
  </div>;
}

function Stat(props: { label: string; value: number | "unknown"; attention?: boolean; onClick?: () => void; href?: string; asOf?: string }) {
  const desk = useDesk();
  return <div class="stat">
    <div class="stat-label">{props.label}</div>
    <Switch>
      <Match when={desk.data.status() === "loading"}><Skeleton w="2rem" h="1.25rem" /></Match>
      <Match when={desk.data.status() === "error"}><div class="stat-value">—</div><button type="button" class="link-btn" onClick={() => desk.store.selectScenario("default")}>Couldn't load · retry</button></Match>
      <Match when={props.value === "unknown"}><div class="stat-unknown">Unknown <Icon name="help" size={14} /></div><div class="tertiary sm-text">Machine disconnected</div></Match>
      <Match when={props.attention && (props.value as number) > 0 && props.href}>
        <A class="stat-value stat-link" href={props.href!} onClick={props.onClick} aria-label={`${props.value} ${props.label}, show on board`}>{props.value}</A>
      </Match>
      <Match when={true}><div class={`stat-value ${props.value === 0 ? "zero" : ""}`}>{props.value}</div></Match>
    </Switch>
    <Show when={props.asOf && props.value !== "unknown" && desk.data.status() === "ready"}><div class="tertiary sm-text" title="Last known value: the machine is disconnected">last known, {props.asOf}</div></Show>
  </div>;
}

function Overview(props: { ps: ProjectSummary; setFilters: (f: FilterId[]) => void }) {
  const desk = useDesk();
  const ps = () => props.ps;
  const tasks = () => desk.data.tasksByProject().get(ps().project.id) ?? [];
  const attention = createMemo(() => tasks().filter(t => t.check || t.stale || t.conflict || t.capability || t.exec === "failed"));
  const board = `/p/${ps().slug}/board`;
  const machine = () => desk.data.machineFor(ps().project);
  const events = createMemo(() => tasks().length ? desk.data.fixture().events.filter(e => e.projectId === ps().project.id).reverse() : []);
  const [allEvents, setAllEvents] = createSignal(false);
  const nums = createMemo(() => new Map(tasks().map(t => [t.task.id, t])));
  /** On a disconnected machine every value is the last confirmed one: say so next to the value. */
  const asOf = () => ps().disconnected ? hhmm(machine()?.lastConfirmedAt ?? null) : undefined;
  return <div class="overview">
    <Show when={ps().disconnected}><Notice tone="danger" icon="plugOff" role="status">
      {machine()?.name} is disconnected. Last confirmed {absTime(machine()?.lastConfirmedAt ?? null)}. Current execution state is unknown; actions are unavailable.
    </Notice></Show>
    <div class="stat-row" role="group" aria-label="Project statistics">
      <Stat label="Tasks" value={ps().taskCount} asOf={asOf()} />
      <Stat label="Needs you" value={ps().needsYou} attention href={`/p/${ps().slug}/decisions`} asOf={asOf()} />
      <Stat label="Failed checks" value={ps().failed} attention href={board} onClick={() => props.setFilters(["failed"])} asOf={asOf()} />
      <Stat label="Not run" value={ps().notRun} attention href={board} onClick={() => props.setFilters(["notRun"])} asOf={asOf()} />
      <Stat label="Running workers" value={ps().running} asOf={asOf()} />
      <Stat label="Stale results" value={ps().stale} attention href={board} onClick={() => props.setFilters(["stale"])} asOf={asOf()} />
    </div>
    <div class="group phases-row">
      <span class="row-label">Phases</span>
      <span class="phase-links tabular">
        <For each={PHASES}>{(ph, i) => <><Show when={i() > 0}><span class="tertiary"> · </span></Show>
          <A href={board} class="inline-link">{PHASE_LABEL[ph]} {desk.data.status() === "ready" ? ps().phases[ph] : "—"}</A></>}</For>
        <Show when={asOf()}><span class="tertiary"> · last known, as of {asOf()}</span></Show>
      </span>
    </div>
    <div class={`ov-cols ${desk.wide() ? "two" : ""}`}>
      <div>
        <section class="dash-section" aria-labelledby="ov-needs"><div class="group-label"><h2 id="ov-needs">Needs you</h2></div>
          <NeedsList items={desk.data.needs().filter(n => n.tv.project.id === ps().project.id)} showProject={false} label="project decisions" asOf={asOf()} />
        </section>
        <section class="dash-section" aria-labelledby="ov-att"><div class="group-label"><h2 id="ov-att">Attention</h2></div>
          <Switch>
            <Match when={desk.data.status() === "loading"}><div class="group"><Loading label="Loading attention items" rows={2} /></div></Match>
            <Match when={desk.data.status() === "error"}><ErrorState text="Couldn't load attention items (fixture error E_PAGE)." /></Match>
            <Match when={attention().length === 0 && !ps().disconnected}><div class="group"><p class="empty-row">No failed checks, stale results, conflicts or unavailable capabilities.</p></div></Match>
            <Match when={true}><ul class="group" role="list">
              <Show when={ps().disconnected}><li><A class="needs-row" href={`/p/${ps().slug}/machine`}>
                <Icon name="plugOff" class="tone-danger-fg" />
                <span class="needs-title">Machine disconnected · actions unavailable</span>
                <span class="needs-ctx">last confirmed {asOf()}</span><span class="needs-meta" /><Icon name="chevronRight" class="row-chev" />
              </A></li></Show>
              <For each={attention()}>{tv => <li>
              <A class="needs-row" href={`/p/${tv.slug}/t/${tv.num}/${tv.conflict ? "git" : tv.capability ? "discussion" : "result"}`}>
                <Icon name={tv.conflict ? "gitMerge" : tv.capability ? "circleSlash" : tv.stale ? "history" : tv.check === "failed" ? "xCircle" : "notRun"}
                  class={tv.conflict || tv.check === "failed" ? "tone-danger-fg" : tv.stale ? "tone-warn-fg" : ""} />
                <span class="needs-title">{tv.conflict ? "Conflict" : tv.capability ? `${tv.capability} unavailable` : tv.stale ? "Result stale" : tv.check === "failed" ? "Failed check" : "Check not run"} · {tv.task.title}</span>
                <span class="needs-ctx">{tv.ref}</span><span class="needs-meta" /><Icon name="chevronRight" class="row-chev" />
              </A></li>}</For></ul></Match>
          </Switch>
        </section>
      </div>
      <div>
        <section class="dash-section" aria-labelledby="ov-machine"><div class="group-label"><h2 id="ov-machine">Machine</h2>
          <A class="inline-link sm-text" href={`/p/${ps().slug}/machine`}>Connection settings</A></div>
          <div class="group">
            <div class="row"><div class="row-label">Machine</div><div class="row-value">{machine()?.name}</div></div>
            <div class="row"><div class="row-label">State</div><div class={`row-value ${ps().disconnected ? "tone-danger-fg" : ""}`}><Icon name={ps().disconnected ? "plugOff" : "plug"} size={14} /> {ps().disconnected ? "Disconnected" : "Connected"}</div></div>
            <div class="row"><div class="row-label">Last confirmed</div><div class="row-value">{absTime(machine()?.lastConfirmedAt ?? null)}</div></div>
          </div>
        </section>
        <section class="dash-section" aria-labelledby="ov-act"><div class="group-label"><h2 id="ov-act">Recent activity</h2>
          <Show when={events().length > 10}><button type="button" class="link-btn" aria-expanded={allEvents()} onClick={() => setAllEvents(!allEvents())}>
            {allEvents() ? "Show last 10" : `Show all ${events().length.toLocaleString("en")}`}</button></Show></div>
          <Switch>
            <Match when={desk.data.status() === "loading"}><div class="group"><Loading label="Loading activity" /></div></Match>
            <Match when={desk.data.status() === "error"}><ErrorState text="Couldn't load activity (fixture error E_PAGE)." /></Match>
            <Match when={events().length === 0}><div class="group"><p class="empty-row">No activity in this project yet.</p></div></Match>
            <Match when={allEvents()}><div class="group"><VirtualList items={events()} rem={1.75} class="activity-log" role="region" tabindex={0}
              label={`All ${events().length} events, newest first`} render={e => <ActivityRow e={e} tv={nums().get(e.taskId)} />} /></div></Match>
            <Match when={true}><ul class="group act-list" role="list"><For each={events().slice(0, 10)}>{e => <li><ActivityRow e={e} tv={nums().get(e.taskId)} /></li>}</For></ul></Match>
          </Switch>
        </section>
      </div>
    </div>
  </div>;
}

function ActivityRow(props: { e: { occurredAt: string; kind: string; text: string }; tv?: TaskView }) {
  return <div class="activity-row">
    <span class="mono tertiary">{hhmm(props.e.occurredAt)}</span>
    <Show when={props.tv} fallback={<span />}>{tv => <A class="mono inline-link" href={`/p/${tv().slug}/t/${tv().num}/discussion`}>{tv().ref}</A>}</Show>
    <span class="tertiary">{props.e.kind}</span>
    <span class="activity-text">{props.e.text} · reported</span>
  </div>;
}

function SummaryStrip(props: { ps: ProjectSummary }) {
  const desk = useDesk();
  const ps = () => props.ps;
  return <div class="summary-strip tabular" role="status" aria-live="off">
    <Switch>
      <Match when={desk.data.status() === "loading"}><Skeleton w="24rem" /></Match>
      <Match when={desk.data.status() === "error"}><span>Task counts unavailable</span></Match>
      <Match when={true}>
        <span><b>{ps().taskCount}</b> tasks</span><span class="dot-sep">·</span>
        <span><b>{ps().needsYou}</b> needs you</span><span class="dot-sep">·</span>
        <span><b>{ps().failed}</b> failed</span><span class="dot-sep">·</span>
        <span><b>{ps().notRun}</b> not run</span><span class="dot-sep">·</span>
        <span>{ps().running === "unknown" ? <><b>Unknown</b> running</> : <><b>{ps().running}</b> running</>}</span>
        <Show when={desk.wide()}><span class="dot-sep">·</span><span><b>{ps().stale}</b> stale</span></Show>
        <span class="dot-sep">·</span>
        <span title={absTime(ps().lastActivity)}>last activity {ps().disconnected ? `as of ${hhmm(ps().lastActivity)}` : relTime(ps().lastActivity).replace("No activity", "none")}</span>
      </Match>
    </Switch>
    <Show when={ps().disconnected}><span class="strip-flag tone-danger-fg"><Icon name="plugOff" size={14} /> Disconnected · last confirmed 00:00</span></Show>
  </div>;
}

function emptyColumn(count: number, shown: number, status: string, clear: () => void): JSX.Element | null {
  if (status !== "ready") return null;
  if (count === 0) return <p class="col-empty">No tasks</p>;
  if (shown === 0) return <p class="col-empty">No tasks match filters · {count} hidden · <button type="button" class="link-btn" onClick={clear}>Clear filters</button></p>;
  return null;
}

/** Card geometry (rem): padding 10 top and bottom, title 13/18 per line, 4px gaps, chip row 20, meta 16; min height 76; 8px between cards. */
const titleLines = (tv: TaskView, width: number, remPx: number) => lineCount(tv.task.title, width - 4 - 2 - 1.5 * remPx, 0.8125 * remPx, 2);
function cardPitch(tv: TaskView, width: number, remPx: number) {
  const content = 2 + remPx * (1.25 + 1.125 * titleLines(tv, width, remPx) + 0.25 + 1 + (hasChips(tv) ? 1.5 : 0));
  return Math.max(4.75 * remPx, content) + 0.5 * remPx;
}

export function TaskCard(props: { tv: TaskView; pos: string; active: boolean; selected: boolean; onFocus: () => void; href: string; posLabel: string; lines?: number }) {
  const tv = () => props.tv;
  // With "Not started" in the line, the time drops the "updated" verb so the meta fits a 236px column.
  const when = () => tv().updatedAt ? `${tv().exec === "not_started" ? "" : "updated "}${relTime(tv().updatedAt)}` : tv().created ? "created just now" : "no events";
  return <A href={props.href} class={`card ${tv().needsYou ? "waiting" : ""} ${props.selected ? "selected" : ""}`} data-pos={props.pos} data-task={tv().task.id}
    tabindex={props.active ? 0 : -1} onFocus={props.onFocus} title={tv().task.title} aria-current={props.selected ? "true" : undefined}>
    <span class="card-title" style={{ "-webkit-line-clamp": String(props.lines ?? 2) }}>{tv().task.title}</span>
    <Show when={hasChips(tv())}><span class="card-chips"><TaskChips tv={tv()} compact noIdle /></span></Show>
    <span class="card-meta"><span class="mono">{tv().ref}</span>
      <Show when={tv().exec === "not_started"}><span aria-hidden="true">·</span><span>Not started</span></Show>
      <span aria-hidden="true">·</span><span class="card-when">{when()}</span>
      <Show when={tv().created}><span class="mock-tag">Mock</span></Show><span class="sr-only">, {props.posLabel}</span></span>
  </A>;
}

function Board(props: { ps: ProjectSummary; filters: FilterId[]; setFilters: (f: FilterId[]) => void }) {
  const desk = useDesk();
  const slug = props.ps.slug;
  const mem = boardMemory.get(slug) ?? { col: 0, idx: 0, scroll: [0, 0, 0, 0] };
  boardMemory.set(slug, mem);
  const [pos, setPos] = createSignal({ col: mem.col, idx: mem.idx });
  const apis: VirtualApi[] = [];
  let board!: HTMLDivElement;
  const columns = createMemo(() => {
    const all = desk.data.tasksByProject().get(props.ps.project.id) ?? [];
    return PHASES.map(phase => {
      const inPhase = all.filter(t => t.task.phase === phase);
      return { phase, count: inPhase.length, shown: inPhase.filter(t => matchesFilters(t, props.filters)) };
    });
  });
  const clampPos = (p: { col: number; idx: number }) => {
    const col = columns()[p.col];
    return { col: p.col, idx: Math.max(0, Math.min(p.idx, (col?.shown.length ?? 1) - 1)) };
  };
  const activePos = () => {
    const p = clampPos(pos());
    if (columns()[p.col]?.shown.length) return p;
    const first = columns().findIndex(c => c.shown.length > 0);
    return { col: Math.max(0, first), idx: 0 };
  };
  const focusPos = (p: { col: number; idx: number }) => {
    setPos(p); mem.col = p.col; mem.idx = p.idx;
    apis[p.col]?.scrollToIndex(p.idx);
    queueMicrotask(() => board.querySelector<HTMLElement>(`[data-pos="${p.col}:${p.idx}"]`)?.focus());
  };
  const onKey = (e: KeyboardEvent) => {
    if (e.ctrlKey || e.altKey || e.metaKey) return;
    const p = activePos(), cols = columns();
    const single = desk.prefs.singleKeys();
    const k = e.key;
    let next: { col: number; idx: number } | undefined;
    const horiz = (dir: 1 | -1) => { for (let c = p.col + dir; c >= 0 && c < cols.length; c += dir) if (cols[c].shown.length) return { col: c, idx: Math.min(p.idx, cols[c].shown.length - 1) }; return undefined; };
    if (k === "ArrowDown" || (single && k === "j")) next = { col: p.col, idx: Math.min(cols[p.col].shown.length - 1, p.idx + 1) };
    else if (k === "ArrowUp" || (single && k === "k")) next = { col: p.col, idx: Math.max(0, p.idx - 1) };
    else if (k === "ArrowRight" || (single && k === "l")) next = horiz(1);
    else if (k === "ArrowLeft" || (single && k === "h")) next = horiz(-1);
    else if (k === "Home") next = { col: p.col, idx: 0 };
    else if (k === "End") next = { col: p.col, idx: cols[p.col].shown.length - 1 };
    else return;
    e.preventDefault();
    if (next) focusPos(next);
  };
  onMount(() => {
    mem.scroll.forEach((s, i) => { if (apis[i]) apis[i].el.scrollTop = s; });
    const id = returnFocus.taskId;
    if (!id) return;
    returnFocus.taskId = "";
    columns().forEach((c, ci) => { const idx = c.shown.findIndex(t => t.task.id === id); if (idx >= 0) focusPos({ col: ci, idx }); });
  });
  const selected = () => desk.store.state().selectedTaskId;
  return <div class="board-wrap">
    <SummaryStrip ps={props.ps} />
    <h1 class="sr-only" id="view-title" tabindex="-1">{props.ps.project.name} board</h1>
    <Show when={desk.data.offline()}><div class="board-notice"><Notice icon="plugZap">Offline. Showing tasks as of {AS_OF_SHORT} (fixture).</Notice></div></Show>
    <div class={`board ${desk.wide() ? "wide" : ""}`} ref={board} role="region" aria-label={`${props.ps.project.name} board. Arrow keys move between cards; Enter opens a task.`} onKeyDown={onKey}>
      <For each={columns()}>{(col, ci) => <section class="column" aria-labelledby={`col-h-${col.phase}`}>
        <h2 class="col-head" id={`col-h-${col.phase}`} tabindex="-1">
          <span>{PHASE_LABEL[col.phase]}</span>
          <Show when={desk.data.status() === "ready"}><span class="col-count tabular">{props.filters.length ? `${col.shown.length} of ${col.count}` : col.count}</span></Show>
          <Show when={props.ps.disconnected}><span class="col-asof">as of 00:00</span></Show>
        </h2>
        <Switch>
          <Match when={desk.data.status() === "loading"}><div class="col-body" role="status" aria-label={`Loading ${PHASE_LABEL[col.phase]}`}>
            <For each={[0, 1, 2, 3]}>{() => <div class="card skeleton-card" aria-hidden="true"><Skeleton w="80%" /><Skeleton w="40%" /><Skeleton w="30%" /></div>}</For></div></Match>
          <Match when={desk.data.status() === "error"}><div class="col-body"><ErrorState text="Couldn't load tasks (fixture error E_PAGE). Nothing was changed." retry={() => desk.store.selectScenario("default")} /></div></Match>
          <Match when={col.shown.length === 0}>{emptyColumn(col.count, col.shown.length, desk.data.status(), () => props.setFilters([]))}</Match>
          <Match when={true}>
            <VirtualList items={col.shown} rem={6.5} sizeOf={(tv, _i, w, r) => cardPitch(tv, w, r)} class="col-body" pin={activePos().col === ci() ? activePos().idx : undefined}
              label={`${PHASE_LABEL[col.phase]}, ${col.shown.length} tasks`} role="list" wrapRole="listitem"
              api={a => (apis[ci()] = a)} onScroll={el => (mem.scroll[ci()] = el.scrollTop)}
              render={(tv, i, box) => <TaskCard tv={tv} pos={`${ci()}:${i}`} lines={titleLines(tv, box.width(), box.remPx())} href={`/p/${tv.slug}/t/${tv.num}/${tv.needsYou ? "decisions" : "discussion"}`}
                active={activePos().col === ci() && activePos().idx === i} selected={selected() === tv.task.id}
                posLabel={`${PHASE_LABEL[col.phase]}, ${i + 1} of ${col.shown.length}`}
                onFocus={() => { setPos({ col: ci(), idx: i }); mem.col = ci(); mem.idx = i; returnFocus.taskId = tv.task.id; }} />} />
          </Match>
        </Switch>
      </section>}</For>
    </div>
  </div>;
}

type SortKey = "title" | "status" | "wait" | "phase" | "updated" | "id" | "checks";
function TaskList(props: { ps: ProjectSummary; filters: FilterId[]; setFilters: (f: FilterId[]) => void }) {
  const desk = useDesk();
  const [sort, setSort] = createSignal<{ key: SortKey; dir: 1 | -1 }>({ key: "id", dir: 1 });
  const [active, setActive] = createSignal(0);
  let api: VirtualApi | undefined, wrap!: HTMLDivElement;
  const all = () => desk.data.tasksByProject().get(props.ps.project.id) ?? [];
  const rows = createMemo(() => {
    const list = all().filter(t => matchesFilters(t, props.filters));
    const { key, dir } = sort();
    const val = (t: TaskView): string | number => key === "title" ? t.task.title : key === "status" ? t.exec : key === "wait" ? (t.wait ?? "")
      : key === "phase" ? PHASES.indexOf(t.task.phase as Phase) : key === "updated" ? (t.updatedAt ?? "") : key === "checks" ? (t.check ?? "") : t.no;
    return [...list].sort((a, b) => (val(a) < val(b) ? -1 : val(a) > val(b) ? 1 : 0) * dir);
  });
  const cols = (): [SortKey, string][] => desk.wide()
    ? [["id", "ID"], ["title", "Title"], ["status", "Status"], ["wait", "Wait reason"], ["checks", "Checks"], ["phase", "Phase"], ["updated", "Updated"]]
    : [["title", "Title"], ["status", "Status"], ["wait", "Wait reason"], ["phase", "Phase"], ["updated", "Updated"]];
  const focusRow = (i: number) => {
    const n = Math.max(0, Math.min(rows().length - 1, i));
    setActive(n); api?.scrollToIndex(n);
    queueMicrotask(() => wrap.querySelector<HTMLElement>(`[data-row="${n}"]`)?.focus());
  };
  const onKey = (e: KeyboardEvent) => {
    const i = active();
    if (e.key === "ArrowDown") focusRow(i + 1); else if (e.key === "ArrowUp") focusRow(i - 1);
    else if (e.key === "Home") focusRow(0); else if (e.key === "End") focusRow(rows().length - 1);
    else if (e.key === "Enter") { const tv = rows()[i]; if (tv) { returnFocus.taskId = tv.task.id; desk.go(`/p/${tv.slug}/t/${tv.num}/discussion`); } }
    else return;
    e.preventDefault();
  };
  return <div class="board-wrap">
    <SummaryStrip ps={props.ps} />
    <h1 class="sr-only" id="view-title" tabindex="-1">{props.ps.project.name} task list</h1>
    <div class={`table ${desk.wide() ? "wide" : ""}`} role="table" aria-label={`${props.ps.project.name} tasks`} aria-rowcount={rows().length + 1} ref={wrap}>
      <div role="rowgroup" class="thead"><div role="row" class="tr">
        <For each={cols()}>{([key, label]) => <div role="columnheader" class={`th col-${key}`} aria-sort={sort().key === key ? (sort().dir === 1 ? "ascending" : "descending") : "none"}>
          <button type="button" class="th-btn" onClick={() => setSort(s => ({ key, dir: s.key === key ? (s.dir === 1 ? -1 : 1) : 1 }))}>
            {label}<Show when={sort().key === key}><Icon name={sort().dir === 1 ? "arrowUp" : "arrowDown"} size={12} /></Show></button>
        </div>}</For>
      </div></div>
      <Switch>
        <Match when={desk.data.status() === "loading"}><Loading label="Loading tasks" rows={8} h="1.25rem" /></Match>
        <Match when={desk.data.status() === "error"}><ErrorState text="Couldn't load tasks (fixture error E_PAGE). Nothing was changed." retry={() => desk.store.selectScenario("default")} /></Match>
        <Match when={all().length === 0}><p class="empty-row">No tasks</p></Match>
        <Match when={rows().length === 0}>{emptyColumn(all().length, 0, "ready", () => props.setFilters([]))}</Match>
        <Match when={true}>
          <VirtualList items={rows()} rem={2.25} class="tbody" innerRole="rowgroup" wrapRole="none" pin={active()} api={a => (api = a)} onKeyDown={onKey}
            render={(tv, i) => <A role="row" class={`tr ${desk.store.state().selectedTaskId === tv.task.id ? "selected" : ""}`} data-row={i} aria-rowindex={i + 2}
              aria-selected={desk.store.state().selectedTaskId === tv.task.id} tabindex={active() === i ? 0 : -1} onFocus={() => setActive(i)}
              href={`/p/${tv.slug}/t/${tv.num}/discussion`} onClick={() => (returnFocus.taskId = tv.task.id)}>
              <For each={cols()}>{([key]) => <span role="cell" class={`td col-${key}`}>
                <Switch>
                  <Match when={key === "id"}><span class="mono tertiary">{tv.ref}</span></Match>
                  <Match when={key === "title"}><span class="td-title" title={tv.task.title}>{tv.task.title}</span><Show when={tv.created}><span class="mock-tag">Mock</span></Show></Match>
                  <Match when={key === "status"}><TaskChips tv={{ ...tv, wait: null, check: null, stale: false, conflict: false, responded: false }} compact /></Match>
                  <Match when={key === "wait"}><Show when={tv.wait && !tv.responded} fallback={<span class="tertiary">—</span>}><Chip tone="warn" icon={tv.wait === "permission" ? "key" : "question"}>{tv.wait === "permission" ? "Your permission" : "Your answer"}</Chip></Show></Match>
                  <Match when={key === "checks"}><Show when={tv.check} fallback={<span class="tertiary">—</span>}><Chip tone={tv.check === "failed" ? "danger" : "neutral"} icon={tv.check === "failed" ? "xCircle" : "notRun"}>{tv.check === "failed" ? "Failed check" : "Not run"}</Chip></Show></Match>
                  <Match when={key === "phase"}>{PHASE_LABEL[tv.task.phase as Phase]}</Match>
                  <Match when={key === "updated"}><span class="tertiary" title={absTime(tv.updatedAt)}>{tv.updatedAt ? relTime(tv.updatedAt) : "no events"}</span></Match>
                </Switch>
              </span>}</For>
            </A>} />
        </Match>
      </Switch>
    </div>
  </div>;
}

