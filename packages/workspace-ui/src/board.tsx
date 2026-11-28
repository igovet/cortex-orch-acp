import { capabilityOf, type Sprint, FEATURES, sprintRouteLabel } from "@cortex/workspace-domain";
import { createEffect, createMemo, createSignal, createUniqueId, For, mapArray, on, onCleanup, Show } from "solid-js";
import { CodeView } from "./code";
import { useWs } from "./ctx";
import { REQUESTING, RequestStatus } from "./request-feedback";
import { onScreen } from "./freshness";
import { CheckoutButton, GitView } from "./git";
import { A, Count, FactLine, I, Keyed, MenuItem, Pop, RowList } from "./kit";
import { Page, StatusLane, Strip, Toolbar, type StripItem } from "./layout";
import { defaultCheckout, repoOf, taskCheckout, unavailableCheckout } from "./review-model";
import type { ProjectView, Route, Scope } from "./router";
import { age, automaticAcceptanceHold, awaitsOwner, bySprint, cardFact, clock, columnsOf, COLUMNS, decisionRoute, inScope, nothingToCommit, planningNeedsOwner, projectsOn, rowIcon, sameBranch, sprintRoute, sprintTitle, sprintView, type ColumnId, type GroupedRow, type ProjectSummary, type TaskView } from "./selectors";
import { taskActionsFor, taskRoute, type TaskAction } from "./task-actions-model";
import { taskContextMenu, TaskMenuButton } from "./task-menu-trigger";
import { resumeState } from "./session-model";
import { KnowledgePage, ProjectSettingsPage } from "./project-pages";
import { createQueueStrip, QueueButton, QueueHelp, RunMode, SprintTarget, SprintLifecycleItems, TargetChoices } from "./sprint";
import { EmptyUntilRead, projectsPending } from "./skeleton";
import { watchLiveGraphs } from "./live-graph";
import { SprintBlockers } from "./sprint-blockers";

const CARD_GAP = 8;
const SECONDARY: { id: ProjectView; label: string }[] = [
  { id: "code", label: "Code" }, { id: "git", label: "Git" }, { id: "worktrees", label: "Worktrees" },
  { id: "knowledge", label: "Knowledge and skills" }, { id: "settings", label: "Settings" },
];

export { taskRoute } from "./task-actions-model";

/**
 * The card's one next action, in the order of the Conversation's attention rules: answer or resolve what waits, start a draft,
 * resume a pause, continue a held interruption, review a result, finish accepted work. Running work is followed in its
 * conversation, so its action is Open; Stop stays in the card's menu and on the task. Resume and Continue follow `resumeState`,
 * as everywhere else: a stopped, ended or unconfirmed task without one is opened to read why. Offline and unknown tasks offer
 * nothing until their state is confirmed.
 */
function primaryAction(v: TaskView, actions: TaskAction[]): { label: string; kind: "go" | "start" | "resume" | "continue"; line?: boolean } | null {
  if (v.unknown) return null;
  const run = resumeState(v);
  const get = (id: "start" | "resume" | "continue") => { const action = actions.find(a => a.id === id); return action ? { label: action.label, kind: id } : null; };
  switch (v.column) {
    case "needs": return automaticAcceptanceHold(v) ? { label: "Review result", kind: "go" } : run?.action === "continue_task" ? get("resume")
      : run?.action === "retry_task" ? get("continue") : run && v.task.conflict === "none" ? { label: "Open", kind: "go" }
      : { label: v.task.conflict !== "none" ? "Resolve" : /permission/i.test(v.task.issue ?? "") ? "Review" : "Answer", kind: "go" };
    case "todo": return get("start");
    case "work": return run?.action === "continue_task" ? get("resume") : v.task.status === "working" ? { label: "Open", kind: "go", line: true } : null;
    case "review": return { label: "Review", kind: "go" };
    case "merge": return { label: v.task.integrationFacts?.alreadyInTarget || nothingToCommit(v.task) ? "Close task…" : sameBranch(v.task, v.target) ? v.task.integrationFacts?.commitAvailable ? `Commit to ${v.target}…` : "Review task" : "Merge…", kind: "go" };
    default: return null;
  }
}
function Card(props: { view: TaskView; named: boolean; showProject?: boolean }) {
  const ws = useWs();
  const v = () => props.view;
  const actions = createMemo(() => taskActionsFor(v(), ws));
  const action = () => primaryAction(v(), actions());
  const descriptor = () => actions().find(a => a.id === action()?.kind);
  const cls = () => v().task.conflict === "conflict" || v().task.conflict === "failed" ? "card bad" : awaitsOwner(v()) ? "card need-you" : "card";
  // A result is reviewed inline in the conversation; Changes is one press from there.
  const goTo = () => v().column === "review" ? taskRoute(v()) : decisionRoute(v());
  const [error, setError] = createSignal("");
  const act = async (e: MouseEvent) => {
    e.preventDefault(); e.stopPropagation(); setError("");
    const a = descriptor();
    if (!a?.enabled) return;
    const receipt = await a.run();
    if (a.id === "start" && receipt && !receipt.ok) setError(receipt.message);
  };
  return <article class={cls()} {...taskContextMenu(() => v().id)} draggable={false} aria-current={ws.selected() === v().id ? "true" : undefined} data-task={v().id}>
    <A draggable={false} class="card-link" to={taskRoute(v())} onKeyDown={e => { if (e.key === " ") { e.preventDefault(); ws.select(v().id); } }}>
      <div class="card-m">
        <div class="card-t"><span data-tooltip-cut="">{v().task.title}</span></div>
        <Show when={props.showProject}><div class="card-project">{v().project.name}</div></Show>
        <Show when={v().task.checkout.branch}><div class="card-b"><I n={v().task.checkout.kind === "worktree" ? "branch" : "folder"} s={12} /><span data-tooltip-cut="" class="mono trunc">{v().task.checkout.branch}</span><span class="type">{v().task.checkout.kind === "worktree" ? "Worktree" : "Project folder"}</span></div></Show>
        <Show when={v().task.sprintRoute}>{r => <div class="card-b"><I n="cpu" s={12} /><span data-tooltip-cut="" class="trunc">{sprintRouteLabel(v().machine.config.catalogue, r())}</span></div>}</Show>
        <div class="card-s"><FactLine fact={cardFact(v())} cutHint /></div>
      </div>
    </A>
    <div class="card-f">
      <span class="age ws-card-age"><Show when={props.named && v().sprint}>{s => <><I n="layers" s={12} /><span data-tooltip-cut="" class="trunc">{sprintTitle(s())}</span></>}</Show><span class="ws-none">{age(v().at, ws.store.clock())}</span></span>
      <span class="card-a">
        <Show when={action()}>{a => a().kind === "go"
          ? <A draggable={false} class={`btn ${a().line ? "btn-line" : "btn-primary"} btn-sm`} to={goTo()}>{a().label}</A>
          : <button class={`btn ${a().line ? "btn-line" : "btn-primary"} btn-sm`} aria-disabled={descriptor()?.why ? "true" : undefined} data-why={descriptor()?.why} onClick={act}>{a().label}</button>}</Show>
        <TaskMenuButton view={v()} />
      </span>
    </div>
    <Show when={v().column === "todo"}><RequestStatus text={ws.store.pendingTask("start_task", v().id) ? REQUESTING.start_task : undefined} /></Show>
    <Show when={error()}><div class="card-error" role="alert">{error()} Try Start again after resolving this.</div></Show>
  </article>;
}

const focusCard = (el: Element | undefined | null) => el?.querySelector<HTMLElement>(".card-link")?.focus();
/** Arrow keys move between cards and columns; Tab still walks every control. */
function moveOnBoard(e: KeyboardEvent) {
  const dir = { ArrowDown: 1, ArrowUp: -1, ArrowLeft: -2, ArrowRight: 2 }[e.key];
  const card = (e.target as Element).closest?.(".card");
  if (!dir || !card) return;
  e.preventDefault();
  const column = card.closest(".col")!;
  if (Math.abs(dir) === 1) {
    const cards = [...column.querySelectorAll(".card")].sort((a, b) => a.getBoundingClientRect().top - b.getBoundingClientRect().top);
    focusCard(cards[cards.indexOf(card) + dir]);
    return;
  }
  const columns = [...onScreen().querySelectorAll(".board-wrap .col:not(.strip):not(.archive)")];
  focusCard(columns[columns.indexOf(column) + dir / 2]?.querySelector(".card"));
}

const rowKey = (r: GroupedRow) => r.kind === "sprint" ? `s:${r.sprint.id}` : r.view.id;
/** A sprint's name over its tasks in a column of mixed work. It leads to that sprint. */
const SprintHead = (props: { row: Extract<GroupedRow, { kind: "sprint" }>; machine: string }) => <A class="col-grp" to={sprintRoute(props.machine, props.row.sprint)}>
  <I n="layers" s={12} /><span data-tooltip-cut="" class="trunc">{sprintTitle(props.row.sprint)}</span><span class="n">{props.row.count}</span>
</A>;

type BoardPosition = { left: number; lanes: Map<string, number> };
const boardPositions = new WeakMap<object, Map<string, BoardPosition>>();

function Board(props: { positionKey: string; summary?: ProjectSummary; views: TaskView[]; onQuick?: () => void; machine: string; grouped?: boolean; sprint?: boolean }) {
  const ws = useWs();
  // Each sprint's tasks in the service's execution order, in the places they hold in their column.
  const columns = createMemo(() => columnsOf(props.views, ws.store.index()));
  const columnLabel = (id: ColumnId, label: string) => id === "merge" && columns().merge.some(v => sameBranch(v.task, v.target)) ? "Ready to finish" : label;
  const quick = () => props.summary && !props.summary.offline && props.onQuick;
  const rows = (id: ColumnId): GroupedRow[] => props.grouped ? bySprint(columns()[id]) : columns()[id].map(view => ({ kind: "task", view }));
  let board!: HTMLDivElement;
  let positions = boardPositions.get(ws.store);
  if (!positions) boardPositions.set(ws.store, positions = new Map());
  const memory = positions;
  createEffect(on(() => props.positionKey, key => {
    const position = memory.get(key) ?? { left: 0, lanes: new Map<string, number>() };
    memory.set(key, position);
    board.scrollLeft = position.left;
    for (const lane of board.querySelectorAll<HTMLElement>(".col-b")) {
      lane.scrollTop = position.lanes.get(lane.closest<HTMLElement>("[data-col]")!.dataset.col!) ?? 0;
    }
    const remember = (event: Event) => {
      const target = event.target;
      if (target === board) position.left = board.scrollLeft;
      else if (target instanceof HTMLElement && target.matches(".col-b")) {
        const column = target.closest<HTMLElement>("[data-col]")?.dataset.col;
        if (column) position.lanes.set(column, target.scrollTop);
      }
    };
    board.addEventListener("scroll", remember, true);
    onCleanup(() => board.removeEventListener("scroll", remember, true));
  }));
  return <div ref={board} class="board-wrap" onKeyDown={moveOnBoard}>
    <For each={COLUMNS.filter(col => col.id !== "closed_unverified" || columns()[col.id].length > 0)}>{col => {
      const open = () => columns()[col.id].length > 0 || (col.id === "todo" && !!quick());
      return <section class={`col${open() ? "" : " strip"}`} data-col={col.id} aria-label={columnLabel(col.id, col.label)}>
        <Show when={open()} fallback={<><I n={col.icon} s={14} class="faint" /><span class="v">{columnLabel(col.id, col.label)}</span></>}>
          <div class={`col-h ${col.tone}`}><I n={col.icon} s={14} class={col.tone ? undefined : "faint"} />{columnLabel(col.id, col.label)}<Count n={columns()[col.id].length} /></div>
          <RowList class="col-b" label={columnLabel(col.id, col.label)} items={rows(col.id)} keyOf={rowKey} gap={CARD_GAP}
            render={r => <Show when={r().kind === "sprint"} fallback={<Card view={(r() as Extract<GroupedRow, { kind: "task" }>).view} named={!!props.grouped} showProject={!props.summary} />}><SprintHead row={r() as Extract<GroupedRow, { kind: "sprint" }>} machine={props.machine} /></Show>}
            header={col.id === "todo" && quick() ? <button class="quick" onClick={() => props.onQuick?.()}><I n="plus" s={14} />Describe the task…</button> : undefined} />
        </Show>
      </section>;
    }}</For>
    <Show when={props.summary && (ws.store.mode === "demo" || props.summary.archived > 0) && props.summary}>{summary => <A class="col archive" to={{ name: "archive", machine: props.machine }}><I n="archive" s={14} /><span class="v">Archive{summary().archived ? ` ${summary().archived}` : ""}</span></A>}</Show>
  </div>;
}

function WorkFilter(props: { value: string; onInput: (value: string) => void }) {
  const [open, setOpen] = createSignal(false);
  const field = createUniqueId();
  const close = () => { props.onInput(""); setOpen(false); };
  let toggle!: HTMLButtonElement;
  return <>
    <Show when={open()}><input id={field} class="inp ws-filter" placeholder="Filter tasks" aria-label="Filter tasks" value={props.value}
      onInput={e => props.onInput(e.currentTarget.value)}
      onKeyDown={e => { if (e.key === "Escape") { close(); toggle.focus(); } }}
      ref={el => queueMicrotask(() => el.focus())} /></Show>
    <button ref={toggle} data-tooltip="" class="ib" aria-label="Filter" aria-expanded={open()} aria-controls={open() ? field : undefined}
      onClick={() => open() ? close() : setOpen(true)}><I n="search" /></button>
  </>;
}

function WorkViewTabs(props: { view: ProjectView; at: (view: "board" | "list") => Route; hidden?: boolean }) {
  return <div class="seg" classList={{ "l-tight-hide": props.hidden }} role="tablist" aria-label="View">
    <A role="tab" to={props.at("board")} aria-selected={props.view === "board" ? "true" : "false"}><I n="board" s={14} />Board</A>
    <A role="tab" to={props.at("list")} aria-selected={props.view === "list" ? "true" : "false"}><I n="list" s={14} />List</A>
  </div>;
}

export function MachineBoard(props: { route: Extract<Route, { name: "board" }> }) {
  const ws = useWs();
  const [filter, setFilter] = createSignal("");
  const scopeMenu = createUniqueId();
  const projects = createMemo(() => projectsOn(ws.store.index(), ws.machine().id));
  const scope = () => projects().find(p => p.project.id === props.route.project);
  const views = createMemo(() => props.route.project ? scope()?.views ?? [] : projects().flatMap(p => p.views));
  const filtered = createMemo(() => filterTasks(views(), filter()));
  const grouped = () => filtered().some(v => v.sprint);
  const at = (view: "board" | "list"): Route => ({ ...props.route, view: view === "list" ? view : undefined });
  const pick = (project?: string) => ws.go({ ...props.route, project });
  return <Page class="l-work" scroll="none"
    head={<Toolbar class="pstrip" label="Board options"
      mid={<>
        <button class="scope-btn l-scope" popovertarget={scopeMenu} aria-label={`Board scope: ${scope()?.project.name ?? (props.route.project ? "Project unavailable" : "All projects")}`}>
          <I n="folder" s={14} class="faint" /><span class="trunc">{scope()?.project.name ?? (props.route.project ? "Project unavailable" : "All projects")}</span><I n="chevron-down" s={14} />
        </button>
        <Pop id={scopeMenu} class="pop-anchor" align="left" label="Board scope">
          <MenuItem icon="board" label="All projects" check={!props.route.project} onSelect={() => pick()} />
          <For each={projects()}>{p => <MenuItem icon="folder" label={p.project.name} check={p.project.id === props.route.project} onSelect={() => pick(p.project.id)} />}</For>
        </Pop>
        <WorkViewTabs view={props.route.view ?? "board"} at={at} />
      </>}
      end={<WorkFilter value={filter()} onInput={setFilter} />} />}
    status={<Show when={ws.shell.kind !== "phone" && ws.machine().connection !== "connected"}>
      <Strip tone="info">Last known state. {ws.machine().name} is offline, last confirmed {clock(ws.machine().lastConfirmedAt)}.</Strip>
  </Show>}>
    <Show when={views().length > 0} fallback={<EmptyUntilRead name={`board|${ws.machine().id}|${props.route.project ?? ""}`} pending={() => projectsPending(ws, p => p.machineId === ws.machine().id && (!props.route.project || p.id === props.route.project))} label="Loading the board…">
      <div class="note-panel"><span class="w500">{props.route.project ? scope() ? "No tasks in this project yet" : "Project unavailable on this machine" : projects().length ? "No tasks to show on this machine" : "No projects on this machine yet"}</span>
      <Show when={props.route.project}><button class="btn btn-sm" onClick={() => pick()}>Show all projects</button></Show>
      <A class="lnk" to={{ name: "home", machine: ws.machine().id }}>Back to Home</A></div></EmptyUntilRead>}>
      <Show when={filtered().length > 0} fallback={<div class="note-panel"><span class="w500">No tasks match this filter</span><button class="btn btn-sm" onClick={() => setFilter("")}>Clear filter</button></div>}>
        <div class="main-body">
          <Show when={props.route.view !== "list"} fallback={<List views={filtered()} machine={ws.machine().id} grouped={grouped()} showProject />}>
            <Board positionKey={JSON.stringify(["machine", ws.machine().id, props.route.project])} views={filtered()} machine={ws.machine().id} grouped={grouped()} />
          </Show>
        </div>
      </Show>
    </Show>
  </Page>;
}

function filterTasks(views: TaskView[], filter: string) {
  const query = filter.trim().toLowerCase();
  return query ? views.filter(v => v.task.title.toLowerCase().includes(query)) : views;
}

type ListRow = { kind: "head"; id: ColumnId; count: number } | GroupedRow;
function List(props: { views: TaskView[]; grouped: boolean; machine: string; sprint?: boolean; showProject?: boolean }) {
  const ws = useWs();
  const rows = createMemo(() => {
    const columns = columnsOf(props.views, ws.store.index());
    return COLUMNS.flatMap((col): ListRow[] => columns[col.id].length ? [{ kind: "head", id: col.id, count: columns[col.id].length }, ...(props.grouped ? bySprint(columns[col.id]).map(r => r.kind === "sprint" ? { ...r, column: col.id } : r) : columns[col.id].map((view): GroupedRow => ({ kind: "task", view })))] : []);
  });
  const onKey = (e: KeyboardEvent) => {
    const row = (e.target as Element).closest?.(".lrow");
    if (!row || (e.key !== "ArrowDown" && e.key !== "ArrowUp")) return;
    e.preventDefault();
    const all = [...onScreen().querySelectorAll<HTMLElement>(".ws-list .lrow")].sort((a, b) => a.getBoundingClientRect().top - b.getBoundingClientRect().top);
    all[all.indexOf(row as HTMLElement) + (e.key === "ArrowDown" ? 1 : -1)]?.focus();
  };
  return <RowList class="main-body ws-list" label="Tasks" items={rows()} keyOf={r => r.kind === "head" ? `h:${r.id}` : r.kind === "sprint" ? `s:${r.column}:${r.sprint.id}` : r.view.id} onKeyDown={onKey}
    render={r => {
      const row = r();
      if (row.kind === "head") { const col = COLUMNS.find(c => c.id === row.id)!; return <h2 class={`sec-h ws-list-h ${col.tone}`}><I n={col.icon} s={14} class={col.tone ? undefined : "faint"} />{col.id === "merge" && props.views.some(v => v.column === "merge" && sameBranch(v.task, v.target)) ? "Ready to finish" : col.label} <span class="n">{(r() as typeof row).count}</span></h2>; }
      if (row.kind === "sprint") return <SprintHead row={r() as typeof row} machine={props.machine} />;
      const v = () => (r() as typeof row).view;
      return <div class="lrow-shell" {...taskContextMenu(() => v().id)}><A class={`lrow${awaitsOwner(v()) ? " need-you" : ""}`} to={taskRoute(v())} aria-current={ws.selected() === v().id ? "true" : undefined} onKeyDown={e => { if (e.key === " ") { e.preventDefault(); ws.select(v().id); } }}>
        <I n={rowIcon(v())} s={14} class={awaitsOwner(v()) ? "st-warn" : v().fact.tone || "faint"} />
        <Show when={props.showProject} fallback={<span data-tooltip-cut="" class="trunc w500">{v().task.title}</span>}>
          <span class="board-list-title"><span data-tooltip-cut="" class="trunc w500">{v().task.title}</span><span data-tooltip-cut="" class="cap faint trunc">{v().project.name}</span></span>
        </Show>
        <span class="cap faint trunc">{v().sprint ? sprintTitle(v().sprint!) : "Standalone"}</span>
        <span class="mono trunc faint">{!v().task.checkout.branch ? "" : v().task.checkout.kind === "worktree" ? v().task.checkout.branch : `${v().task.checkout.branch} (project folder)`}</span>
        <span class={`st trunc ${cardFact(v()).tone}`}><span class="trunc" data-tooltip-cut="">{cardFact(v()).text}</span></span><span class="cap faint">{age(v().at, ws.store.clock())}</span>
      </A><TaskMenuButton view={v()} /></div>;
    }} />;
}

/** A task the queue is on: its title shortens here, and the link leads to the whole of it. */
const QueueFact = (props: { view: TaskView; label: string; class: string }) => <A class={`l-fact ${props.class}`} to={taskRoute(props.view)} aria-label={`${props.label}: ${props.view.task.title}`}><span class="lab">{props.label}</span><b>{props.view.task.title}</b></A>;

/** The queue's toolbar. Its one disabled reason is said by the strip below it, named by `reason`, so nothing here can grow. */
function SprintBar(props: { summary: ProjectSummary; sprintId: string; reason: string }) {
  const ws = useWs();
  const sprint = () => ws.store.state().sprints.find(s => s.id === props.sprintId)!;
  const view = createMemo(() => sprintView(ws.store.index(), sprint()));
  const tab = (tab: "planning" | "summary"): Route => ({ name: "sprint", machine: props.summary.machine.id, project: props.summary.project.id, sprint: props.sprintId, tab });
  return <Toolbar label="Sprint queue"
    start={<><RunMode sprint={sprint()} describedBy={props.reason} /><QueueHelp sprint={sprint()} /></>}
    mid={<>
      <Show when={view().now}>{v => <QueueFact view={v()} label="Now running" class="l-tight-hide" />}</Show>
      {/* Next only when the service's order names one; a Manual sprint's order is a suggestion, never a promise. */}
      <Show when={view().next ?? (sprint().runMode === "manual" ? view().suggested : null)}>{v => <QueueFact view={v()} label={view().next ? "Next" : "Suggested"} class={view().now ? "l-roomy-only" : "l-tight-hide"} />}</Show>
      <Show when={view().blocked > 0}><span class="l-fact l-fact-n"><span class="lab">Blocked</span><b>{view().blocked}</b></span></Show>
    </>}
    end={<>
      <SprintTarget sprint={sprint()} summary={props.summary} class="l-tight-hide" />
      <Show when={sprint().planning === "created" && !sprint().archivedAt && !sprint().deletion}><SprintBlockers sprint={sprint()} /></Show>
      <A data-tooltip="" class="ib l-tight-hide" to={tab("planning")} aria-label="Sprint planning"><I n="pencil" s={14} /></A>
      <A data-tooltip="" class="ib l-tight-hide" to={tab("summary")} aria-label="Sprint summary"><I n="list" s={14} /></A>
      <button data-tooltip="" class="ib l-tight-only" popovertarget="m-queue-more" aria-label="More queue options"><I n="more" s={14} /></button>
      <Pop id="m-queue-more" class="pop-right" align="right" label="More queue options" style={{ width: "280px" }}>
        <MenuItem icon="pencil" label="Sprint planning" onSelect={() => ws.go(tab("planning"))} />
        <MenuItem icon="list" label="Sprint summary" onSelect={() => ws.go(tab("summary"))} />
        <div class="mi-sep"></div>
        <TargetChoices sprint={sprint()} summary={props.summary} />
      </Pop>
      <QueueButton sprint={sprint()} line small steady describedBy={props.reason} />
    </>} />;
}

function ScopeMenu(props: { summary: ProjectSummary; route: Extract<Route, { name: "project" }> }) {
  const ws = useWs();
  const pick = (scope: Scope) => ws.go({ ...props.route, scope, view: props.route.view === "list" ? "list" : "board" });
  const standalone = () => props.summary.views.filter(v => !v.sprint).length;
  const label = () => {
    const s = props.route.scope;
    if (s.kind === "all") return "All work";
    if (s.kind === "standalone") return "Standalone";
    const sprint = props.summary.sprints.find(x => x.id === s.id);
    return sprint ? `Sprint: ${sprintTitle(sprint)}` : "Sprint";
  };
  return <>
    <button class="scope-btn l-scope" popovertarget="m-scope" aria-label={`Board scope: ${label()}`}><I n="layers" s={14} class="faint" /><span class="trunc">{label()}</span><I n="chevron-down" s={14} /></button>
    <Pop id="m-scope" class="pop-anchor" align="left" label="Board scope">
      <MenuItem icon="board" label="All work" count={String(props.summary.views.length)} onSelect={() => pick({ kind: "all" })} />
      <MenuItem icon="list" label="Standalone" count={String(standalone())} onSelect={() => pick({ kind: "standalone" })} />
      <Show when={props.summary.sprints.length > 0}><div class="mi-h">Sprints</div></Show>
      <For each={props.summary.sprints}>{s => {
        const view = () => sprintView(ws.store.index(), s);
        return <MenuItem icon="layers" label={view().title} sub={view().state} onSelect={() => { if (s.planning === "created") pick({ kind: "sprint", id: s.id }); else ws.go(sprintRoute(props.summary.machine.id, s)); }} />;
      }}</For>
      <div class="mi-sep"></div>
      <MenuItem icon="plus" label="New sprint…" onSelect={() => ws.newItem("sprint", props.summary.project.id)} />
    </Pop>
  </>;
}

/** One project: toolbars, status lane, then Board or List or a secondary area. */
export function ProjectPage(props: { summary: ProjectSummary; route: Extract<Route, { name: "project" }> }) {
  const ws = useWs();
  const [filter, setFilter] = createSignal("");
  const view = () => props.route.view ?? "board";
  const sprintId = () => props.route.scope.kind === "sprint" ? props.route.scope.id : null;
  const knownSprint = () => { const id = sprintId(); return id === null || props.summary.sprints.some(s => s.id === id); };
  const missing = () => {
    const sprint = props.summary.sprints.find(s => s.id === sprintId());
    return sprint ? sprintView(ws.store.index(), sprint).missing : 0;
  };
  const scoped = createMemo(() => inScope(props.summary, props.route.scope));
  const views = createMemo(() => filterTasks(scoped(), filter()));
  const at = (v: ProjectView): Extract<Route, { name: "project" }> => ({ ...props.route, view: v, scope: sprintId() !== null || v === "board" || v === "list" ? props.route.scope : { kind: "all" }, file: undefined, line: undefined });
  // On a sprint's board a new task joins that sprint, already selected; it is saved as a draft and starts nothing.
  const addSprint = () => { const id = sprintId(); return id && props.summary.sprints.some(s => s.id === id && s.planning === "created") ? id : null; };
  const onQuick = () => ws.newItem("task", props.summary.project.id, addSprint());
  const isWork = () => view() === "board" || view() === "list";
  // Only mixed work needs its sprints named. On a sprint's own board the scope already says it.
  const grouped = () => props.route.scope.kind === "all" && views().some(v => v.sprint);
  const isRepo = () => view() === "code" || view() === "git" || view() === "worktrees";
  const repo = createMemo(() => repoOf(ws.store.state(), props.summary.project.id));
  const state = ws.store.state;
  const checkout = () => {
    const r = repo(), id = props.route.checkout;
    return r && !(id && unavailableCheckout(state(), r.projectId, id)) ? defaultCheckout(r, id) ?? null : null;
  };
  const missingCheckout = () => !!props.route.checkout && !checkout();
  const pickCheckout = (id: string) => ws.go({ ...props.route, checkout: id, file: undefined, line: undefined });
  const codeAt = (file: string, line?: number): Route => ({ ...at("code"), checkout: checkout()?.id, file, line });
  const areas = () => SECONDARY.filter(t => ws.store.mode === "demo" || ws.can(capabilityOf("git_fetch")) || (t.id === "code" ? ws.can(FEATURES.editFiles) : !["git", "worktrees"].includes(t.id)));
  // A narrow pane keeps Code and Git and the area in view; the others fold into a menu. A phone-width pane folds Code and
  // Git too, and away from the board it folds Board and List, so the scope, the open area and the menu always fit.
  const folds = (id: ProjectView) => id !== "code" && id !== "git";
  const queueSprint = () => { const id = sprintId(); return id && isWork() && props.summary.project.activated !== false ? ws.store.state().sprints.find(s => s.id === id) ?? null : null; };
  // The order Board, List and the queue bar show comes from each shown sprint's live graph.
  watchLiveGraphs(ws.store, () => !isWork() || props.summary.project.activated === false ? []
    : sprintId() ? props.summary.sprints.filter(s => s.id === sprintId()) : props.summary.sprints);
  const queueReason = createUniqueId();
  const queueStrip = createQueueStrip(queueSprint, queueReason);
  const activate = () => ws.run({ type: "activate_project", projectId: props.summary.project.id });
  // Each strip is one item for the life of the page, so a change of words never redraws the rest of the lane.
  const notActivated: StripItem = { id: "activate", tone: "info", text: "This project is not activated. Activating lets tasks run in this folder; it starts no task and keeps everything already here.", action: <button class="btn btn-primary btn-sm" {...ws.needs("activate_project")} onClick={activate}>Activate</button> };
  const issue: StripItem = { id: "issue", tone: "fail", role: "alert", icon: <I n="help" s={14} class="faint" />, get text() { const cause = props.summary.project.issueCause; return <>{props.summary.project.issue}{cause ? <span class="cap muted ws-block">Cause: {cause}</span> : null}</>; }, action: <button class="btn btn-sm" {...ws.needs("activate_project")} onClick={activate}>Try again</button> };
  const moving: StripItem = { id: "moving-history", tone: "info", role: "status", text: "Moving project history…" };
  const offline: StripItem = { id: "offline", tone: "info", icon: <I n="help" s={14} />, get text() { return `Last known state. ${props.summary.machine.name} is offline, last confirmed ${clock(props.summary.machine.lastConfirmedAt)}. Actions are unavailable until it reconnects.`; } };
  const planning = mapArray(() => isWork() && props.route.scope.kind === "all" ? props.summary.sprints.filter(s => s.planning !== "created" && s.planning !== "cancelled") : [], (s): StripItem => ({
    id: `planning:${s.id}`, tone: "info", icon: <I n="layers" s={14} />,
    get text() { return `Sprint: ${sprintTitle(s)} · ${planningNeedsOwner(ws.store.state(), s) ? "Question waiting" : s.planning === "ready" ? "Plan ready" : "Planning"} · No tasks created yet`; },
    action: <A class="lnk" to={sprintRoute(props.summary.machine.id, s)}>Open planning</A>,
  }));
  // A sprint deletion that stopped part-way is said where its tasks are, with the way to its retry.
  const deleting = mapArray(() => isWork() && props.route.scope.kind === "all" ? props.summary.sprints.filter(s => s.deletion) : [], (s): StripItem => ({
    id: `deleting:${s.id}`, tone: "warn", role: "status", icon: <I n="hourglass" s={14} class="st-warn" />,
    get text() { return `Sprint: ${sprintTitle(s)} · ${sprintView(ws.store.index(), s).state}`; },
    action: <A class="lnk" to={sprintRoute(props.summary.machine.id, s)}>Open sprint</A>,
  }));
  const status = createMemo<StripItem[]>(() => [
    ...(ws.store.movingProjectHistory(props.summary.project.id) ? [moving] : [
      ...(ws.store.mode === "real" && props.summary.project.activated === false ? [notActivated] : []),
      ...(props.summary.project.issue ? [issue] : []),
    ]),
    ...deleting(),
    ...planning(),
    ...(props.summary.offline && isWork() ? [offline] : []),
    ...(queueStrip() ? [queueStrip()!] : []),
  ]);
  return <Page class="l-work" scroll="none"
    head={<>
      <Toolbar class="pstrip"
        mid={<>
          <Show when={isRepo() && repo() && checkout()} fallback={<ScopeMenu summary={props.summary} route={props.route} />}><CheckoutButton repo={repo()!} checkout={checkout()!} onPick={pickCheckout} /></Show>
          <WorkViewTabs view={view()} at={at} hidden={!isWork()} />
          <div class="ptabs" role="tablist" aria-label="Project areas">
            <For each={areas()}>{t => <A class="ptab" classList={{ "l-compact-hide": folds(t.id) && view() !== t.id, "l-tight-hide": !folds(t.id) && view() !== t.id }} role="tab" to={at(t.id)} aria-selected={view() === t.id ? "true" : "false"}>{t.label}</A>}</For>
          </div>
          <Show when={areas().length > 0}>
            <button class={`ib ${areas().some(t => folds(t.id)) ? "l-compact-only" : "l-tight-only"}`} popovertarget="m-areas" aria-label="More project areas"><I n="more" s={14} /></button>
            <Pop id="m-areas" class="pop-anchor" align="left" label="More project areas">
              <div class="l-tight-block">
                <Show when={!isWork()}><MenuItem icon="board" label="Board" onSelect={() => ws.go(at("board"))} /><MenuItem icon="list" label="List" onSelect={() => ws.go(at("list"))} /></Show>
                <For each={areas().filter(t => !folds(t.id))}>{t => <MenuItem label={t.label} check={view() === t.id} onSelect={() => ws.go(at(t.id))} />}</For>
              </div>
              <For each={areas().filter(t => folds(t.id))}>{t => <MenuItem label={t.label} check={view() === t.id} onSelect={() => ws.go(at(t.id))} />}</For>
            </Pop>
          </Show>
        </>}
        end={<Show when={isWork()}><WorkFilter value={filter()} onInput={setFilter} /></Show>} />
      <Show when={queueSprint()?.id}>{id => <SprintBar summary={props.summary} sprintId={id()} reason={queueReason} />}</Show>
    </>}
    status={<StatusLane items={status()} />}>
    <Show when={isWork() && props.summary.project.activated !== false} fallback={<Show when={isRepo()} fallback={<Show when={view() === "knowledge"} fallback={<ProjectSettingsPage summary={props.summary} />}><KnowledgePage summary={props.summary} /></Show>}>
      <Show when={!missingCheckout()} fallback={<div class="note-panel" role="status"><span class="w500">Checkout unavailable</span><span>Restore the original checkout and refresh, or choose another checkout.</span><div class="row"><button class="btn btn-line btn-sm" onClick={() => void ws.store.reload()}>Refresh</button><A class="btn btn-line btn-sm" to={{ ...at(view()), checkout: undefined }}>Choose another checkout</A></div></div>}>
      <Show when={view() === "code"} fallback={<GitView summary={props.summary} repo={repo()} checkout={checkout()} right={view() === "worktrees" ? "branches" : "history"} onRight={right => ws.go(at(right === "branches" ? "worktrees" : "git"))} onPick={id => ws.go({ ...props.route, view: "git", checkout: id })} codeAt={path => ws.go(codeAt(path))} />}>
        <Show when={repo() && checkout()} fallback={<div class="note-panel"><span class="w500">No files for this project</span><span>{ws.t(props.summary.project.hasGit === false ? "noCodeWithoutGit" : "noCode")}</span></div>}><CodeView repo={repo()!} checkout={checkout()!} file={props.route.file} line={props.route.line} at={codeAt} /></Show>
      </Show></Show>
    </Show>}>
      <Show when={knownSprint()} fallback={<div class="note-panel"><span class="w500">Sprint not found</span><A class="lnk" to={{ ...props.route, scope: { kind: "all" } }}>Show all work</A></div>}>
        <Show when={missing() > 0 && !!props.summary.project.issue}><div class="note-panel" role="status"><span class="w500">Task details unavailable</span><span>{missing()} sprint task{missing() === 1 ? "" : "s"} could not be loaded. The sprint membership is retained.</span><button class="btn btn-line btn-sm" onClick={() => ws.store.reload()}>Retry</button></div></Show>
        <Show when={scoped().length > 0} fallback={<EmptyUntilRead name={`project|${props.summary.project.id}`} pending={() => projectsPending(ws, p => p.id === props.summary.project.id)} label="Loading the board…">
          <div class="note-panel"><span class="w500">{sprintId() ? missing() ? "" : "No active tasks in this sprint" : props.route.scope.kind === "standalone" ? "No standalone tasks" : "No tasks in this project yet"}</span>
          <Show when={!props.summary.offline && (!sprintId() || (addSprint() && !missing()))}><button class="btn btn-line btn-sm" onClick={onQuick}><I n="plus" s={14} />New task</button></Show></div></EmptyUntilRead>}>
          <Show when={views().length > 0} fallback={<div class="note-panel"><span class="w500">No tasks match this filter</span><button class="btn btn-sm" onClick={() => setFilter("")}>Clear filter</button></div>}>
            <div class="main-body">
              <Show when={view() === "board"} fallback={<List views={views()} grouped={grouped()} machine={props.summary.machine.id} sprint={sprintId() !== null} />}><Board positionKey={JSON.stringify(["project", props.summary.machine.id, props.summary.project.id, props.route.scope.kind, sprintId()])} summary={props.summary} views={views()} onQuick={onQuick} machine={props.summary.machine.id} grouped={grouped()} sprint={sprintId() !== null} /></Show>
            </div>
          </Show>
        </Show>
      </Show>
    </Show>
  </Page>;
}

/** One shared task menu: rendering a popover per card would multiply with long columns. */
export function TaskMenu(props: { taskId: string | null; sprint?: Sprint; focusTrigger?: () => void; ref: (el: HTMLDivElement) => void }) {
  const ws = useWs();
  const actions = createMemo(() => {
    const view = props.taskId ? ws.store.index().byId.get(props.taskId) : undefined;
    return view ? taskActionsFor(view, ws) : [];
  });
  return <Pop id="m-task" class="pop-right" role="menu" label={props.sprint ? "Sprint actions" : "Task actions"} ref={props.ref} onKeyDown={e => {
    const items = [...e.currentTarget.querySelectorAll<HTMLElement>("[role=menuitem]")];
    const index = items.indexOf(document.activeElement as HTMLElement);
    const key = e.key;
    if (key === "Escape") { e.preventDefault(); e.stopPropagation(); e.currentTarget.hidePopover?.(); props.focusTrigger?.(); return; }
    if (!items.length || !["ArrowDown", "ArrowUp", "Home", "End"].includes(key)) return;
    e.preventDefault();
    const next = key === "Home" ? 0 : key === "End" ? items.length - 1
      : key === "ArrowDown" ? (index + 1) % items.length : (index <= 0 ? items.length : index) - 1;
    items[next]?.focus();
  }}>
    <Show when={props.sprint}>{s => <SprintLifecycleItems sprint={s()} focusTrigger={props.focusTrigger} />}</Show>
    <Keyed each={actions()} by={a => a.id}>{a => <>
      <Show when={a().id === "open_editor"}><div class="mi-sep" /></Show>
      <MenuItem actionId={a().id} icon={a().icon} label={a().label} sub={a().sub} keys={a().keys} danger={a().danger} disabled={a().why} onSelect={() => void a().run()} />
    </>}</Keyed>
  </Pop>;
}
