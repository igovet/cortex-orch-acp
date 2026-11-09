import { capabilityOf, FEATURES, type WorkspaceCommand } from "@cortex/workspace-domain";
import { createMemo, createSignal, For, Show } from "solid-js";
import { createBoardMotion } from "./board-motion";
import { CodeView } from "./code";
import { useWs } from "./ctx";
import { CheckoutButton, GitView } from "./git";
import { A, Count, FactLine, I, MenuItem, Pop, VirtualList } from "./kit";
import { defaultCheckout, repoOf, taskCheckout } from "./review-model";
import type { ProjectView, Route, Scope } from "./router";
import { age, awaitsOwner, bySprint, clock, columnsOf, COLUMNS, decisionRoute, inScope, rowIcon, sameBranch, sprintRoute, sprintTitle, sprintView, type ColumnId, type GroupedRow, type ProjectSummary, type TaskView } from "./selectors";
import { closeBlocked, sessionModel } from "./session-model";
import { KnowledgePage, ProjectSettingsPage } from "./project-pages";
import { QueueButton, QueueNote, RunMode, SprintTarget } from "./sprint";

const CARD_ESTIMATE = 112, CARD_GAP = 8, ROW = 40, GROUP_HEAD = 42, SPRINT_HEAD = 26;
const SECONDARY: { id: ProjectView; label: string }[] = [
  { id: "code", label: "Code" }, { id: "git", label: "Git" }, { id: "worktrees", label: "Worktrees" },
  { id: "knowledge", label: "Knowledge and skills" }, { id: "settings", label: "Settings" },
];

export const taskRoute = (v: TaskView): Route => ({ name: "task", machine: v.machine.id, task: v.id });

/** What the card's fixed action slot offers. Offline and unknown tasks offer nothing until their state is confirmed. */
function primaryAction(v: TaskView, startable: boolean): { label: string; kind: "go" | "start" | "stop" | "resume"; line?: boolean } | null {
  if (v.unknown) return null;
  switch (v.column) {
    case "needs": return { label: v.task.conflict !== "none" ? "Resolve" : "Answer", kind: "go" };
    case "todo": return startable ? { label: "Start", kind: "start" } : null;
    case "work": return v.task.status === "paused" ? { label: "Resume", kind: "resume" } : v.task.status === "working" ? { label: "Stop", kind: "stop", line: true } : null;
    case "review": return { label: "Review", kind: "go" };
    case "merge": return { label: v.task.integrationFacts?.alreadyInTarget ? "Close task…" : sameBranch(v.task, v.target) ? v.task.integrationFacts?.commitAvailable ? `Commit to ${v.target}…` : "Review task" : "Merge…", kind: "go" };
    default: return null;
  }
}
const startable = (v: TaskView) => v.task.status === "draft" && !v.unknown && v.fact.icon !== "hourglass";

function Card(props: { view: TaskView; named: boolean }) {
  const ws = useWs();
  const v = () => props.view;
  const action = () => primaryAction(v(), startable(v()));
  const cls = () => v().task.conflict === "conflict" || v().task.conflict === "failed" ? "card bad" : awaitsOwner(v()) ? "card need-you" : "card";
  const needed = () => ({ start: "start_task", stop: "stop_task", resume: "continue_task", go: null })[action()?.kind ?? "go"] as WorkspaceCommand["type"] | null;
  const [error, setError] = createSignal("");
  const act = async (e: MouseEvent) => {
    e.preventDefault(); e.stopPropagation(); setError("");
    const a = action()!, task = v().id;
    if (a.kind === "start") { const receipt = await ws.run({ type: "start_task", taskId: task, now: ws.store.clock() }); if (!receipt.ok) setError(receipt.message); }
    else if (a.kind === "stop") ws.stopTask(task);
    else if (a.kind === "resume") ws.run({ type: "continue_task", taskId: task, now: ws.store.clock() });
  };
  return <article class={cls()} draggable={!v().unknown && v().task.status !== "done" && ws.can(capabilityOf("close_task"))}
    onDragStart={e => {
      if ((e.target as Element).closest("button")) { e.preventDefault(); return; }
      e.dataTransfer?.setData("application/x-workspace-task", v().id);
      if (e.dataTransfer) {
        e.dataTransfer.effectAllowed = "move";
        const card = e.currentTarget, box = card.getBoundingClientRect();
        const ghost = card.cloneNode(true) as HTMLElement;
        ghost.removeAttribute("data-task"); ghost.setAttribute("aria-hidden", "true");
        ghost.classList.add("ws-drag-card");
        Object.assign(ghost.style, { width: `${box.width}px`, height: `${box.height}px`, font: getComputedStyle(card).font });
        card.parentElement!.append(ghost);
        e.dataTransfer.setDragImage?.(ghost, Math.max(0, e.clientX - box.left), Math.max(0, e.clientY - box.top));
        setTimeout(() => ghost.remove(), 0);
      }
    }} aria-current={ws.selected() === v().id ? "true" : undefined} data-task={v().id}>
    <A class="card-link" to={taskRoute(v())} onKeyDown={e => { if (e.key === " ") { e.preventDefault(); ws.select(v().id); } }}>
      <div class="card-m">
        <div class="card-t"><span>{v().task.title}</span></div>
        <Show when={v().task.checkout.branch}><div class="card-b"><I n={v().task.checkout.kind === "worktree" ? "branch" : "folder"} s={12} /><span class="mono trunc">{v().task.checkout.branch}</span><span class="type">{v().task.checkout.kind === "worktree" ? "Worktree" : "Main checkout"}</span></div></Show>
        <div class="card-s"><FactLine fact={v().fact} /></div>
      </div>
    </A>
    <div class="card-f">
      <span class="age ws-card-age"><Show when={props.named && v().sprint}>{s => <><I n="layers" s={12} /><span class="trunc">{sprintTitle(s())}</span></>}</Show><span class="ws-none">{age(v().at, ws.store.clock())}</span></span>
      <span class="card-a">
        <Show when={action()}>{a => a().kind === "go"
          ? <A class="btn btn-primary btn-sm" to={decisionRoute(v())}>{a().label}</A>
          : <button class={`btn ${a().line ? "btn-line" : "btn-primary"} btn-sm`} {...(needed() ? ws.needs(needed()!) : {})} onClick={act}>{a().label}</button>}</Show>
        <button class="ib" aria-label={`Actions for ${v().task.title}`} onClick={e => ws.openTaskMenu(v().id, e.currentTarget)}><I n="more" s={14} /></button>
      </span>
    </div>
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
  const columns = [...document.querySelectorAll(".board-wrap .col:not(.strip):not(.archive)")];
  focusCard(columns[columns.indexOf(column) + dir / 2]?.querySelector(".card"));
}

const rowKey = (r: GroupedRow) => r.kind === "sprint" ? `s:${r.sprint.id}` : r.view.id;
/** A sprint's name over its tasks in a column of mixed work. It leads to that sprint. */
const SprintHead = (props: { row: Extract<GroupedRow, { kind: "sprint" }>; machine: string }) => <A class="col-grp" to={sprintRoute(props.machine, props.row.sprint)}>
  <I n="layers" s={12} /><span class="trunc">{sprintTitle(props.row.sprint)}</span><span class="n">{props.row.count}</span>
</A>;

function Board(props: { summary: ProjectSummary; views: TaskView[]; onQuick: () => void; machine: string; grouped: boolean }) {
  const ws = useWs();
  let wrap: HTMLDivElement | undefined;
  const [dropHint, setDropHint] = createSignal("");
  const columns = createMemo(() => columnsOf(props.views));
  const columnLabel = (id: ColumnId, label: string) => id === "merge" && columns().merge.some(v => sameBranch(v.task, v.target)) ? "Ready to finish" : label;
  createBoardMotion(() => wrap, createMemo(() => props.views.map(v => `${v.id}:${v.column}`).join("|")));
  const rows = (id: ColumnId): GroupedRow[] => props.grouped ? bySprint(columns()[id]) : columns()[id].map(view => ({ kind: "task", view }));
  return <><Show when={dropHint()}><div class="strip info" role="status">{dropHint()}</div></Show><div class="board-wrap" ref={wrap} onKeyDown={moveOnBoard}>
    <For each={COLUMNS}>{col => {
      const open = () => columns()[col.id].length > 0 || (col.id === "todo" && !props.summary.offline);
      // One element whether it holds cards or sits folded, so a column opening around a card can be animated.
      return <section class={`col${open() ? "" : " strip"}`} data-col={col.id}
        onDragOver={e => { if (e.dataTransfer?.types.includes("application/x-workspace-task")) { e.preventDefault(); e.dataTransfer.dropEffect = col.id === "done" ? "move" : "none"; } }}
        onDrop={e => {
          const id = e.dataTransfer?.getData("application/x-workspace-task");
          if (!id || !props.views.some(v => v.id === id && !v.unknown && v.task.status !== "done")) return;
          e.preventDefault();
          if (col.id !== "done") { setDropHint("Only Done accepts manual moves. Other columns follow the task's actual progress."); return; }
          setDropHint(""); ws.closeTask(id, "external");
        }} aria-label={columnLabel(col.id, col.label)}>
        <Show when={open()} fallback={<><I n={col.icon} s={14} class="faint" /><span class="v">{columnLabel(col.id, col.label)}</span></>}>
          <div class={`col-h ${col.tone}`}><I n={col.icon} s={14} class={col.tone ? undefined : "faint"} />{columnLabel(col.id, col.label)}<Count n={columns()[col.id].length} /></div>
          <VirtualList class="col-b" label={columnLabel(col.id, col.label)} items={rows(col.id)} keyOf={rowKey} estimate={r => r.kind === "sprint" ? SPRINT_HEAD : CARD_ESTIMATE} gap={CARD_GAP}
            render={r => <Show when={r().kind === "sprint"} fallback={<Card view={(r() as Extract<GroupedRow, { kind: "task" }>).view} named={props.grouped} />}><SprintHead row={r() as Extract<GroupedRow, { kind: "sprint" }>} machine={props.machine} /></Show>}
            header={col.id === "todo" && !props.summary.offline ? <button class="quick" onClick={() => props.onQuick()}><I n="plus" s={14} />Describe the task…</button> : undefined} />
        </Show>
      </section>;
    }}</For>
    <Show when={ws.store.mode === "demo" || props.summary.archived > 0}><A class="col archive" to={{ name: "archive", machine: props.machine }}><I n="archive" s={14} /><span class="v">Archive{props.summary.archived ? ` ${props.summary.archived}` : ""}</span></A></Show>
  </div></>;
}

type ListRow = { kind: "head"; id: ColumnId; count: number } | GroupedRow;
function List(props: { views: TaskView[]; grouped: boolean; machine: string }) {
  const ws = useWs();
  const rows = createMemo(() => {
    const columns = columnsOf(props.views);
    return COLUMNS.flatMap((col): ListRow[] => columns[col.id].length ? [{ kind: "head", id: col.id, count: columns[col.id].length }, ...(props.grouped ? bySprint(columns[col.id]).map(r => r.kind === "sprint" ? { ...r, column: col.id } : r) : columns[col.id].map((view): GroupedRow => ({ kind: "task", view })))] : []);
  });
  const onKey = (e: KeyboardEvent) => {
    const row = (e.target as Element).closest?.(".lrow");
    if (!row || (e.key !== "ArrowDown" && e.key !== "ArrowUp")) return;
    e.preventDefault();
    const all = [...document.querySelectorAll<HTMLElement>(".ws-list .lrow")].sort((a, b) => a.getBoundingClientRect().top - b.getBoundingClientRect().top);
    all[all.indexOf(row as HTMLElement) + (e.key === "ArrowDown" ? 1 : -1)]?.focus();
  };
  return <VirtualList class="main-body ws-list" label="Tasks" items={rows()} keyOf={r => r.kind === "head" ? `h:${r.id}` : r.kind === "sprint" ? `s:${r.column}:${r.sprint.id}` : r.view.id} estimate={r => r.kind === "head" ? GROUP_HEAD : r.kind === "sprint" ? SPRINT_HEAD : ROW} onKeyDown={onKey}
    render={r => {
      const row = r();
      if (row.kind === "head") { const col = COLUMNS.find(c => c.id === row.id)!; return <h2 class={`sec-h ws-list-h ${col.tone}`}><I n={col.icon} s={14} class={col.tone ? undefined : "faint"} />{col.id === "merge" && props.views.some(v => v.column === "merge" && sameBranch(v.task, v.target)) ? "Ready to finish" : col.label} <span class="n">{(r() as typeof row).count}</span></h2>; }
      if (row.kind === "sprint") return <SprintHead row={r() as typeof row} machine={props.machine} />;
      const v = () => (r() as typeof row).view;
      return <A class={`lrow${awaitsOwner(v()) ? " need-you" : ""}`} to={taskRoute(v())} aria-current={ws.selected() === v().id ? "true" : undefined} onKeyDown={e => { if (e.key === " ") { e.preventDefault(); ws.select(v().id); } }}>
        <I n={rowIcon(v())} s={14} class={awaitsOwner(v()) ? "st-warn" : v().fact.tone || "faint"} /><span class="trunc w500">{v().task.title}</span>
        <span class="cap faint trunc">{v().sprint ? sprintTitle(v().sprint!) : "Standalone"}</span>
        <span class="mono trunc faint">{!v().task.checkout.branch ? "" : v().task.checkout.kind === "worktree" ? v().task.checkout.branch : `${v().task.checkout.branch} (main checkout)`}</span>
        <span class={`st trunc ${v().fact.tone}`}>{v().fact.text}</span><span class="cap faint">{age(v().at, ws.store.clock())}</span>
      </A>;
    }} />;
}

function SprintBar(props: { summary: ProjectSummary; sprintId: string }) {
  const ws = useWs();
  const sprint = () => ws.store.state().sprints.find(s => s.id === props.sprintId)!;
  const view = createMemo(() => sprintView(ws.store.index(), sprint()));
  const tab = (tab: "planning" | "summary"): Route => ({ name: "sprint", machine: props.summary.machine.id, project: props.summary.project.id, sprint: props.sprintId, tab });
  return <>
    <div class="sprint-bar">
      <RunMode sprint={sprint()} />
      <Show when={view().now}>{v => <span><span class="lab">Now running</span><b>{v().task.title}</b></span>}</Show>
      <Show when={view().next}>{v => <span><span class="lab">Next</span><b>{v().task.title}</b></span>}</Show>
      <Show when={view().blocked > 0}><span><span class="lab">Blocked</span><b>{view().blocked}</b></span></Show>
      <span class="grow"></span>
      <SprintTarget sprint={sprint()} summary={props.summary} />
      <A class="ib" to={tab("planning")} aria-label="Sprint planning"><I n="pencil" s={14} /></A>
      <A class="ib" to={tab("summary")} aria-label="Sprint summary"><I n="list" s={14} /></A>
      <QueueButton sprint={sprint()} line small />
    </div>
    <QueueNote sprint={sprint()} />
  </>;
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
    <button class="scope-btn" popovertarget="m-scope"><I n="layers" s={14} class="faint" /><span>{label()}</span><I n="chevron-down" s={14} /></button>
    <Pop id="m-scope" class="pop-anchor" align="left" label="Board scope">
      <MenuItem icon="board" label="All work" keys={String(props.summary.views.length)} onSelect={() => pick({ kind: "all" })} />
      <MenuItem icon="list" label="Standalone" keys={String(standalone())} onSelect={() => pick({ kind: "standalone" })} />
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

/** One project: strip, scope, Board or List, and its secondary areas. */
export function ProjectPage(props: { summary: ProjectSummary; route: Extract<Route, { name: "project" }> }) {
  const ws = useWs();
  const [filter, setFilter] = createSignal("");
  const [filtering, setFiltering] = createSignal(false);
  const view = () => props.route.view ?? "board";
  const sprintId = () => props.route.scope.kind === "sprint" ? props.route.scope.id : null;
  const knownSprint = () => { const id = sprintId(); return id === null || props.summary.sprints.some(s => s.id === id); };
  const missing = () => {
    const sprint = props.summary.sprints.find(s => s.id === sprintId());
    return sprint ? sprintView(ws.store.index(), sprint).missing : 0;
  };
  const scoped = createMemo(() => inScope(props.summary, props.route.scope));
  const views = createMemo(() => {
    const q = filter().trim().toLowerCase();
    return q ? scoped().filter(v => v.task.title.toLowerCase().includes(q)) : scoped();
  });
  const at = (v: ProjectView): Route => ({ ...props.route, view: v, scope: v === "board" || v === "list" ? props.route.scope : { kind: "all" }, file: undefined, line: undefined });
  // On a sprint's board a new task joins that sprint, already selected; it is saved as a draft and starts nothing.
  const addSprint = () => { const id = sprintId(); return id && props.summary.sprints.some(s => s.id === id && s.planning === "created") ? id : null; };
  const onQuick = () => ws.newItem("task", props.summary.project.id, addSprint());
  const isWork = () => view() === "board" || view() === "list";
  // Only mixed work needs its sprints named. On a sprint's own board the scope already says it.
  const grouped = () => props.route.scope.kind === "all" && views().some(v => v.sprint);
  const isRepo = () => view() === "code" || view() === "git" || view() === "worktrees";
  const repo = createMemo(() => repoOf(ws.store.state(), props.summary.project.id));
  const checkout = () => { const r = repo(); return r ? defaultCheckout(r, props.route.checkout) : null; };
  const pickCheckout = (id: string) => ws.go({ ...props.route, checkout: id, file: undefined, line: undefined });
  const codeAt = (file: string, line?: number): Route => ({ ...props.route, view: "code", scope: { kind: "all" }, checkout: checkout()?.id, file, line });
  return <>
    <Show when={ws.store.mode === "real" && props.summary.project.activated === false}><div class="strip info"><span>This project is not activated. Activating lets tasks run in this folder; it starts no task and keeps everything already here.</span><button class="btn btn-primary btn-sm" {...ws.needs("activate_project")} onClick={() => ws.run({type:"activate_project",projectId:props.summary.project.id})}>Activate</button></div></Show>
    <Show when={props.summary.project.issue}><div class="strip ws-conn" role="alert"><I n="help" s={14} class="faint" /><span>{props.summary.project.issue}</span><span class="acts"><button class="btn btn-sm" {...ws.needs("activate_project")} onClick={() => ws.run({type:"activate_project",projectId:props.summary.project.id})}>Try again</button></span></div></Show>
    <div class="pstrip">
      <Show when={isRepo() && repo() && checkout()} fallback={<ScopeMenu summary={props.summary} route={props.route} />}><CheckoutButton repo={repo()!} checkout={checkout()!} onPick={pickCheckout} /></Show>
      <div class="seg" role="tablist" aria-label="View">
        <A role="tab" to={at("board")} aria-selected={view() === "board" ? "true" : "false"}><I n="board" s={14} />Board</A>
        <A role="tab" to={at("list")} aria-selected={view() === "list" ? "true" : "false"}><I n="list" s={14} />List</A>
      </div>
      <div class="ptabs" role="tablist" aria-label="Project areas">
        <For each={SECONDARY.filter(t => ws.store.mode === "demo" || ws.can(capabilityOf("git_fetch")) || (t.id === "code" ? ws.can(FEATURES.editFiles) : !["git", "worktrees"].includes(t.id)))}>{t => <A class="ptab" role="tab" to={at(t.id)} aria-selected={view() === t.id ? "true" : "false"}>{t.label}</A>}</For>
      </div>
      <span class="grow"></span>
      <Show when={isWork()}>
        <Show when={filtering()}><input id="ws-filter" class="inp ws-filter" placeholder="Filter tasks" aria-label="Filter tasks" value={filter()} onInput={e => setFilter(e.currentTarget.value)}
          onKeyDown={e => { if (e.key === "Escape") { setFilter(""); setFiltering(false); } }} ref={el => queueMicrotask(() => el.focus())} /></Show>
        <button class="ib" aria-label="Filter" aria-pressed={filtering()} onClick={() => { if (filtering()) setFilter(""); setFiltering(!filtering()); }}><I n="search" /></button>
      </Show>
    </div>
    <Show when={isWork() && props.route.scope.kind === "all"}>
      <For each={props.summary.sprints.filter(s => s.planning !== "created" && s.planning !== "cancelled")}>{s =>
        <div class="strip info"><I n="layers" s={14} /><span class="trunc">Sprint: {sprintTitle(s)} · {s.planning === "question" ? "Question waiting" : s.planning === "ready" ? "Plan ready" : "Planning"} · No tasks created yet</span><A class="lnk" to={sprintRoute(props.summary.machine.id, s)}>Open planning</A></div>
      }</For>
    </Show>
    <Show when={props.summary.offline && isWork()}><div class="strip info"><I n="help" s={14} /><span>Last known state. {props.summary.machine.name} is offline, last confirmed {clock(props.summary.machine.lastConfirmedAt)}. Actions are unavailable until it reconnects.</span></div></Show>
    <Show when={isWork() && props.summary.project.activated !== false} fallback={<Show when={isRepo()} fallback={<Show when={view() === "knowledge"} fallback={<ProjectSettingsPage summary={props.summary} />}><KnowledgePage summary={props.summary} /></Show>}>
      <Show when={view() === "code"} fallback={<GitView summary={props.summary} repo={repo()} checkout={checkout()} right={view() === "worktrees" ? "branches" : "history"} onRight={right => ws.go(at(right === "branches" ? "worktrees" : "git"))} onPick={id => ws.go({ ...props.route, view: "git", checkout: id })} codeAt={path => ws.go(codeAt(path))} />}>
        <Show when={repo() && checkout()} fallback={<div class="note-panel"><span class="w500">No files for this project</span><span>{ws.t(props.summary.project.hasGit === false ? "noCodeWithoutGit" : "noCode")}</span></div>}><CodeView repo={repo()!} checkout={checkout()!} file={props.route.file} line={props.route.line} at={codeAt} /></Show>
      </Show>
    </Show>}>
      <Show when={knownSprint()} fallback={<div class="note-panel"><span class="w500">Sprint not found</span><A class="lnk" to={{ ...props.route, scope: { kind: "all" } }}>Show all work</A></div>}>
        <Show when={sprintId()}>{id => <SprintBar summary={props.summary} sprintId={id()} />}</Show>
        <Show when={missing() > 0}><div class="note-panel" role="status"><span class="w500">Task details unavailable</span><span>{missing()} sprint task{missing() === 1 ? "" : "s"} could not be loaded. The sprint membership is retained.</span><button class="btn btn-line btn-sm" onClick={() => ws.store.reload()}>Retry</button></div></Show>
        <Show when={scoped().length > 0} fallback={<div class="note-panel"><span class="w500">{sprintId() ? missing() ? "Waiting for sprint task details" : "No active tasks in this sprint" : props.route.scope.kind === "standalone" ? "No standalone tasks" : "No tasks in this project yet"}</span>
          <Show when={!props.summary.offline && (!sprintId() || (addSprint() && !missing()))}><button class="btn btn-line btn-sm" onClick={onQuick}><I n="plus" s={14} />New task</button></Show></div>}>
          <Show when={views().length > 0} fallback={<div class="note-panel"><span class="w500">No tasks match this filter</span><button class="btn btn-sm" onClick={() => setFilter("")}>Clear filter</button></div>}>
            <div class="main-body">
              <Show when={view() === "board"} fallback={<List views={views()} grouped={grouped()} machine={props.summary.machine.id} />}><Board summary={props.summary} views={views()} onQuick={onQuick} machine={props.summary.machine.id} grouped={grouped()} /></Show>
            </div>
          </Show>
        </Show>
      </Show>
    </Show>
  </>;
}

/** One shared task menu: rendering a popover per card would multiply with long columns. */
export function TaskMenu(props: { taskId: string | null; ref: (el: HTMLDivElement) => void }) {
  const ws = useWs();
  const view = () => props.taskId ? ws.store.index().byId.get(props.taskId) ?? null : null;
  const blocked = () => view()?.unknown ? "Unavailable until this task's state is confirmed" : undefined;
  const onOwnPage = () => { const r = ws.router.route(); return r.name === "task" && r.task === props.taskId; };
  const closeReason = () => { const m = props.taskId ? sessionModel(ws.store.index(), props.taskId) : null; return m ? closeBlocked(m) ?? undefined : undefined; };
  return <Pop id="m-task" class="pop-right" label="Task actions" ref={props.ref}>
    <Show when={view()}>{v => <>
      <Show when={!onOwnPage()}>
        <MenuItem icon="arrow-right" label="Open task" onSelect={() => ws.go(taskRoute(v()))} />
        <MenuItem icon="eye" label="Preview in Inspector" keys="Space" onSelect={() => ws.select(v().id)} />
      </Show>
      <Show when={v().task.status === "draft"}><MenuItem icon="play" label="Start" disabled={blocked() ?? (startable(v()) ? undefined : "Start is controlled by its dependencies or the sprint queue")} needs="start_task" onSelect={() => ws.run({ type: "start_task", taskId: v().id, now: ws.store.clock() })} /></Show>
      <Show when={["working", "needs_you"].includes(v().task.status)}><MenuItem icon="stop" label="Stop" keys="Ctrl+." disabled={blocked()} needs="stop_task" onSelect={() => ws.stopTask(v().id)} /></Show>
      <Show when={v().task.status === "paused"}><MenuItem icon="play" label="Resume" disabled={blocked()} needs="continue_task" onSelect={() => ws.run({ type: "continue_task", taskId: v().id, now: ws.store.clock() })} /></Show>
      <Show when={v().task.status === "stopping" && v().machine.connection === "connected"}><MenuItem icon="check" label={ws.t("confirmStop")} needs="confirm_stop" onSelect={() => ws.run({ type: "confirm_stop", taskId: v().id, now: ws.store.clock() })} /></Show>
      <MenuItem icon="x" label="Close task…" disabled={closeReason()} needs="close_task" onSelect={() => ws.closeTask(v().id)} />
      <MenuItem icon="archive" label="Archive" sub={v().task.status === "done" ? undefined : "Available when the task is done"} disabled={v().task.status === "done" && !v().task.archive ? undefined : "Available when the task is done"} needs="archive_task" onSelect={() => ws.archiveTask(v().id)} />
      <MenuItem icon="pin" label={ws.panels.pins().includes(v().id) ? "Unpin" : "Pin"} onSelect={() => ws.panels.togglePin(v().id)} />
      <div class="mi-sep"></div>
      <MenuItem icon="external" label={ws.externalLabel()} sub={ws.noOpener("workspace.open_editor", "noExternal") ?? (v().task.checkout.kind === "worktree" && !taskCheckout(ws.store.state(), v().task) ? "This task’s worktree is no longer available. Refresh the project." : undefined)} disabled={ws.noOpener("workspace.open_editor", "noExternal") ?? (v().task.checkout.kind === "worktree" && !taskCheckout(ws.store.state(), v().task) ? "Worktree unavailable" : undefined)} onSelect={() => { const c = taskCheckout(ws.store.state(), v().task); ws.run({ type: "open_editor", projectId: v().project.id, ...(c ? { checkoutId: c.id } : {}) }); }} />
      <MenuItem icon="branch" label="Copy branch" onSelect={() => { void navigator.clipboard?.writeText(v().task.checkout.branch); }} />
      <MenuItem icon="help" label="Diagnostics" onSelect={() => ws.go({ name: "settings", section: "diagnostics", machine: v().machine.id })} />
    </>}</Show>
  </Pop>;
}
