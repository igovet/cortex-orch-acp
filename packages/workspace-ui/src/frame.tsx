import { createMemo, For, Match, Show, Switch } from "solid-js";
import { LiveTerminal } from "./terminal";
import { FEATURES } from "@cortex/workspace-domain";
import { ArchiveFacts, DeleteArchive } from "./archive";
import { useWs, type WindowChrome } from "./ctx";
import { Confirmed } from "./freshness";
import { CodePanel } from "./code";
import { GitPanel } from "./git";
import { A, FactLine, I, Keyed, MenuItem, Pop } from "./kit";
import { SIDEBAR_DEFAULT, SIDEBAR_MAX, SIDEBAR_MIN } from "./prefs";
import { APP_SECTIONS, MACHINE_SECTIONS, ORCHESTRATION_PAGES, type OrchestrationPage, type Route, type SettingsSection } from "./router";
import { defaultCheckout, repoOf } from "./review-model";
import { awaitsOwner, checkCounts, clock, dayClock, engineLimits, homeView, inboxItems, projectsOn, rowIcon, sameBranch, sideRows, sprintRoute, sprintView, type EngineLimit, type ProjectSummary, type SideRow, type TaskView } from "./selectors";
import { workerFace } from "./session-model";
import { SprintActions, SprintCrumb } from "./sprint";
import { BranchChip, TaskActions, TaskInspector, TaskTitle } from "./task-chrome";

const tile = (name: string, n: number) => ({ letter: name[0]?.toUpperCase() ?? "?", cls: `t${(n % 6) + 1}` });
const project = (machine: string, id: string): Route => ({ name: "project", machine, project: id, view: "board", scope: { kind: "all" } });

/** The route's project, on the machine in view. */
function useRouteContext() {
  const ws = useWs();
  const view = createMemo(() => {
    const r = ws.router.route();
    return r.name === "task" ? ws.store.index().byId.get(r.task) ?? null : null;
  });
  const summary = createMemo(() => {
    const r = ws.router.route();
    const id = r.name === "project" || r.name === "sprint" ? r.project : view()?.project.id;
    return id ? ws.store.index().summaries.get(id) ?? null : null;
  });
  const sprint = createMemo(() => { const r = ws.router.route(); return r.name === "sprint" ? ws.store.state().sprints.find(s => s.id === r.sprint) ?? null : null; });
  return { view, summary, sprint };
}

export const Tile = (props: { name: string; n: number }) => <span class={`mono-tile ${tile(props.name, props.n).cls}`}>{tile(props.name, props.n).letter}</span>;

function Facts() {
  const ws = useWs();
  const home = createMemo(() => homeView(ws.store.index(), ws.machine().id));
  const offline = () => ws.machine().connection !== "connected";
  const known = () => offline() ? " last known" : "";
  return <div class="row sec muted" style={{ gap: "14px", "margin-left": "10px" }}>
    <Show when={home().projects.length > 0}><span class="trunc">{home().projects.length} project{home().projects.length === 1 ? "" : "s"}</span></Show>
    <span class="trunc">{home().working.length} working{known()}</span>
    <span class={`trunc${home().needs.length ? " st-warn" : ""}`}>{home().needs.length} need{home().needs.length === 1 ? "s" : ""} you{known()}</span>
    <span class="trunc faint">{ws.store.mode === "demo" ? (offline() ? `Offline, last confirmed ${clock(ws.machine().lastConfirmedAt)}` : `as of ${clock(ws.store.now)}`) : ""}</span>
  </div>;
}

/** The band moves the window from anywhere that is not a control. Moving a few pixels starts the drag, so a double click stays a double click. */
const BAND_CONTROLS = "a, button, input, textarea, select, label, summary, [popover], [role='button'], [role='separator'], [contenteditable='true']";
const DRAG_SLOP = 3;
function bandHandlers(chrome: WindowChrome | undefined) {
  if (!chrome) return {};
  const free = (e: MouseEvent) => e.button === 0 && !(e.target as Element).closest(BAND_CONTROLS);
  return {
    onMouseDown(down: MouseEvent) {
      if (!free(down)) return;
      const stop = () => { document.removeEventListener("mousemove", move); document.removeEventListener("mouseup", stop); };
      const move = (e: MouseEvent) => {
        if (e.buttons !== 1) return stop();
        if (Math.hypot(e.clientX - down.clientX, e.clientY - down.clientY) < DRAG_SLOP) return;
        stop();
        chrome.startDragging();
      };
      document.addEventListener("mousemove", move);
      document.addEventListener("mouseup", stop);
    },
    onDblClick(e: MouseEvent) { if (free(e)) chrome.toggleMaximize(); },
  };
}

function WindowControls() {
  const ws = useWs();
  return <Show when={ws.chrome} fallback={<Show when={ws.store.mode === "demo"}><div class="win" aria-hidden="true">
    <button tabindex="-1"><I n="minus" s={14} /></button>
    <button tabindex="-1"><I n="square" s={12} /></button>
    <button tabindex="-1"><I n="x" s={14} /></button>
  </div></Show>}>{chrome => <div class="win" role="group" aria-label="Window">
    <button aria-label="Minimize" onClick={() => chrome().minimize()}><I n="minus" s={14} /></button>
    <button aria-label={chrome().maximized() ? "Restore" : "Maximize"} onClick={() => chrome().toggleMaximize()}><I n={chrome().maximized() ? "copy" : "square"} s={12} /></button>
    <button aria-label="Close window" onClick={() => chrome().close()}><I n="x" s={14} /></button>
  </div>}</Show>;
}

export function TitleBar() {
  const ws = useWs();
  const ctx = useRouteContext();
  const route = () => ws.router.route();
  const crumb = (): string => {
    const r = route();
    return r.name === "home" ? "Home" : r.name === "inbox" ? "Inbox" : r.name === "archive" ? "Archive" : r.name === "new" ? (r.kind === "sprint" ? "New sprint" : "New task") : r.name === "settings" ? "Settings" : r.name === "notfound" ? "Not found" : "";
  };
  const areaName = () => { const r = route(); return r.name !== "project" ? null : r.view === "code" ? "Code" : r.view === "git" || r.view === "worktrees" ? "Git" : null; };
  const connection = () => !ws.store.ready() && ws.store.connection().state === "connecting"
    ? { text: "Connecting", tone: "st-idle" } : ws.machine().connection === "connected"
    ? { text: "Connected", tone: "st-ok" } : { text: "Offline", tone: "st-idle" };
  return <header class="titlebar" {...bandHandlers(ws.chrome)}>
    <button class="ib panel-t tb-toggle" aria-label="Sidebar" aria-pressed={!ws.panels.sidebarHidden()} onClick={() => ws.panels.toggleSidebar()}><I n="panel-left" /></button>
    <div class="tb-side">
      <button class="machine" popovertarget="m-machine">
        <I n={ws.machine().kind === "local" ? "laptop" : "server"} /><span class="trunc">{ws.machine().name}</span>
        <span class={`st ${connection().tone}`} style={{ "margin-left": "auto" }}>{connection().text}</span><I n="chevrons-v" s={12} class="faint" />
      </button>
    </div>
    <div class="tb-main">
      <button class="ib" classList={{ "ws-hide": !ws.router.canBack() }} aria-label="Back" onClick={() => ws.router.back()}><I n="arrow-left" /></button>
      <button class="ib" classList={{ "ws-hide": !ws.router.canForward() }} aria-label="Forward" onClick={() => ws.router.forward()}><I n="arrow-right" /></button>
      <div class="crumbs">
        <Switch fallback={<span class="cur">{crumb()}</span>}>
          <Match when={route().name === "project" && ctx.summary()}>{s => <>
            <Show when={areaName()} fallback={<span class="cur">{s().project.name}</span>}><A to={project(s().machine.id, s().project.id)}>{s().project.name}</A><span class="sep">/</span><span class="cur">{areaName()}</span></Show>
          </>}</Match>
          <Match when={ctx.view()}>{v => <>
            <A to={project(v().machine.id, v().project.id)}>{v().project.name}</A><span class="sep">/</span>
            <TaskTitle view={v()} />
          </>}</Match>
          <Match when={ctx.sprint() && ctx.summary()}>{s => <>
            <A to={project(s().machine.id, s().project.id)}>{s().project.name}</A><span class="sep">/</span>
            <SprintCrumb sprint={ctx.sprint()!} />
          </>}</Match>
        </Switch>
      </div>
      <Show when={ctx.view()}>{v => <BranchChip view={v()} />}</Show>
      <Show when={ws.store.ready() && route().name === "home"}><Facts /></Show>
      <Show when={route().name === "archive"}><ArchiveFacts /></Show>
      <Show when={route().name === "project" && !areaName() && ctx.summary()}>{s => <>
        <span class="mono faint trunc" style={{ "margin-left": "4px" }}>{s().project.repository}</span>
        <Show when={s().project.defaultBranch}><span class="chip"><I n="branch" s={12} /><span class="mono">{s().project.defaultBranch}</span></span></Show>
      </>}</Show>
      <span class="grow"></span>
      <div class="tb-actions"><Show when={ws.store.ready()}>
        <Show when={ctx.view()} fallback={<Show when={ctx.sprint()} fallback={<Show when={route().name === "archive"} fallback={<>
          <Show when={areaName() === "Code"}><button class="btn btn-line" onClick={() => ws.setCodeLeft("search")}><I n="search" s={14} />Search</button></Show>
          <Show when={areaName() === "Git" && ctx.summary()}>{s => <button class="btn btn-line" {...ws.needs("create_branch")} onClick={() => ws.open({ kind: "branch", projectId: s().project.id })}><I n="branch" s={14} />New branch</button>}</Show>
          <Show when={route().name !== "new" && route().name !== "settings" && !areaName()}><span class="split">
            <button class="btn btn-primary" onClick={() => ws.newItem("task")}><I n="plus" s={14} />New task</button>
            <button class="btn btn-primary" popovertarget="m-new" aria-label="New task or sprint"><I n="chevron-down" s={14} /></button>
          </span></Show>
          <Show when={route().name === "project"}><button class="ib" popovertarget="m-project" aria-label="Project actions"><I n="more" /></button></Show>
        </>}><DeleteArchive /></Show>}>{s => <SprintActions sprint={s()} tab={(route() as Extract<Route, { name: "sprint" }>).tab} />}</Show>}>{v => <TaskActions view={v()} onMenu={el => ws.openTaskMenu(v().id, el)} />}</Show>
      </Show></div>
      <div class="tb-fixed">
        {ws.demo?.chip()}
        <button class="ib panel-t" classList={{ "ws-hide": !ws.terminalAvailable() }} aria-label="Terminal"
          aria-pressed={ws.terminalAvailable() && ws.panels.terminalOpen()} onClick={() => ws.toggleTerminal()}><I n="panel-bottom" /></button>
        <button class="ib panel-t" classList={{ "ws-hide": !ws.inspectable() }} aria-label="Inspector" aria-pressed={ws.inspectable() && ws.panels.inspectorVisible()} onClick={() => { if (ws.inspectable()) ws.panels.toggleInspector(); }}><I n="panel-right" /></button>
      </div>
      <WindowControls />
    </div>
  </header>;
}

function ProjectRow(props: { summary: ProjectSummary; n: number; openByDefault: boolean }) {
  const ws = useWs();
  const id = () => props.summary.project.id;
  const route = () => ws.router.route();
  const current = () => { const r = route(); return r.name === "project" && r.project === id(); };
  const hasLive = () => sideRows(ws.store.index(), props.summary).length > 0;
  // Never tied to the route: opening a project must not push the rows below it, or they would move under the pointer.
  const open = () => ws.panels.expanded()[id()] ?? props.openByDefault;
  return <>
    <div class="proj-wrap">
      <A class="proj" to={project(props.summary.machine.id, id())} aria-current={current() ? "page" : undefined}>
        <Tile name={props.summary.project.name} n={props.n} /><span class="trunc">{props.summary.project.name}</span>
        <Show when={props.summary.signal}>{s => <span class={`sig ${s().tone}`}>{s().text}</span>}</Show>
      </A>
      <button class="ib proj-x" aria-label={`${open() ? "Hide" : "Show"} tasks of ${props.summary.project.name}`}
        aria-expanded={open()} classList={{ "ws-hide": !hasLive() }} onClick={() => { if (hasLive()) ws.panels.setExpanded(id(), !open()); }}>
        <I n={open() ? "chevron-down" : "chevron-right"} s={12} />
      </button>
    </div>
    <Show when={open() && hasLive()}>
      <div class="tasks">
        <Keyed each={sideRows(ws.store.index(), props.summary)} by={r => r.kind === "task" ? r.view.id : `s:${r.sprint.id}`}>{r => {
          const sprint = () => r() as Extract<SideRow, { kind: "sprint" }>;
          return r().kind === "task" ? <TaskRow view={(r() as Extract<SideRow, { kind: "task" }>).view} />
            : <A class="task-row" to={sprintRoute(props.summary.machine.id, sprint().sprint)}>
              <I n="layers" s={14} class="faint" /><span class="trunc">Sprint: {sprintView(ws.store.index(), sprint().sprint).title}</span><span class={`sig tnum${sprint().sprint.planning === "question" ? " st-warn" : ""}`}>{sprint().sprint.planning === "created" ? `${sprint().started}/${sprint().total}` : sprint().sprint.planning === "question" ? "Needs you" : "Planning"}</span>
            </A>;
        }}</Keyed>
        <Show when={props.summary.views.length > 0}><A class="task-row all" to={project(props.summary.machine.id, id())}>All {props.summary.views.length}</A></Show>
      </div>
    </Show>
  </>;
}

function TaskRow(props: { view: TaskView }) {
  const ws = useWs();
  const route = () => ws.router.route();
  const word = () => props.view.column === "needs" ? "Needs you" : props.view.column === "work" ? "Working" : props.view.column === "review" ? "Review" : props.view.project.hasGit === false || sameBranch(props.view.task, props.view.target) ? "Finish" : "Merge";
  return <A class="task-row" to={{ name: "task", machine: props.view.machine.id, task: props.view.id }} aria-current={(() => { const r = route(); return r.name === "task" && r.task === props.view.id ? "page" : undefined; })()}>
    <I n={rowIcon(props.view)} s={14} class={awaitsOwner(props.view) ? "st-warn" : props.view.fact.tone} /><span class="trunc">{props.view.task.title}</span><span class={`sig${awaitsOwner(props.view) ? " st-warn" : ""}`}>{word()}</span>
  </A>;
}

function UsageBar(props: { row: EngineLimit }) {
  return <span class="um"><span>{props.row.label}</span>
    <span class="bar"><i style={{ width: `${props.row.usedPercent ?? 0}%` }}></i></span>
    <span class="tnum">{props.row.usedPercent}%</span>
  </span>;
}
/** Only what the provider reported. An unknown limit stays off the sidebar and reads Unavailable in the popover. */
function SidebarUsage() {
  const ws = useWs();
  const known = createMemo(() => engineLimits(ws.store.state(), ws.machine()).rows.filter(r => r.usedPercent !== null));
  const label = () => `Engine limits: ${known().map(r => `${r.label} ${r.usedPercent}%`).join(", ")} used this week`;
  return <Show when={known().length > 0}>
    <button class="sb-usage" popovertarget="m-usage" aria-label={label()}><For each={known()}>{r => <UsageBar row={r} />}</For></button>
  </Show>;
}
function SidebarFoot() {
  const ws = useWs();
  const route = () => ws.router.route();
  return <div class="sb-foot">
    <button class="btn btn-sm" {...ws.needs("add_project")} onClick={() => ws.open({ kind: "add-project" })}><I n="folder-plus" s={14} />Add project</button>
    <span class="grow"></span>
    <A class="ib" to={{ name: "settings", section: "appearance", machine: null }} aria-label="Settings" aria-current={route().name === "settings" ? "page" : undefined}><I n="settings" /></A>
    <button class="ib" aria-label="Help and shortcuts" onClick={() => ws.openHelp()}><I n="help" /></button>
  </div>;
}

export function Limit(props: { row: EngineLimit }) {
  return <div class="u-eng">
    <div class="u-h"><span class="w500">{props.row.label}</span><span class="st st-ok"><I n="check-circle" s={12} />Connected</span></div>
    <div class="u-win"><span>This week</span>
      <Show when={props.row.usedPercent !== null} fallback={<span class="na">Unavailable</span>}>
        <span class="bar"><i style={{ width: `${props.row.usedPercent}%` }}></i></span><span class="v">{props.row.usedPercent}% used</span>
        <span class="rs">{props.row.resetAt ? `Resets ${dayClock(props.row.resetAt)}` : ""}</span>
      </Show>
    </div>
    <div class="u-src">{props.row.observedAt ? `Observed ${clock(props.row.observedAt)}, reported by the provider` : "Not reported by the provider"}</div>
  </div>;
}

function Sidebar() {
  const ws = useWs();
  const index = () => ws.store.index();
  const m = () => ws.machine().id;
  const route = () => ws.router.route();
  const inbox = createMemo(() => inboxItems(index()).length);
  const projects = createMemo(() => {
    const list = projectsOn(index(), m()), mode = ws.panels.sort();
    if (mode === "name") return [...list].sort((a, b) => a.project.name.localeCompare(b.project.name));
    if (mode === "activity") return [...list].sort((a, b) => (b.at ?? -1) - (a.at ?? -1));
    return list;
  });
  const firstLive = createMemo(() => projects().find(s => sideRows(index(), s).length > 0)?.project.id);
  const pinned = createMemo(() => ws.panels.pins().flatMap(id => { const v = index().byId.get(id); return v && v.machine.id === m() ? [v] : []; }));
  let dragging = false;
  const onMove = (e: PointerEvent) => { if (dragging) ws.panels.setSidebarWidth(e.clientX); };
  return <nav class="sidebar" aria-label="Workspace">
    <div class="sb-top">
      <A class="nav" to={{ name: "new", machine: m(), kind: "task", project: null }} aria-current={route().name === "new" ? "page" : undefined}><I n="plus" />New task<span class="kbd">Ctrl+N</span></A>
      <A class="nav" to={{ name: "inbox", machine: m() }} aria-current={route().name === "inbox" ? "page" : undefined}><I n="inbox" />Inbox<Show when={inbox() > 0}><span class="count">{inbox()}</span></Show></A>
      <button class="nav ws-nav-btn" onClick={() => ws.openPalette()}><I n="search" />Search<span class="kbd">Ctrl+K</span></button>
      <A class="nav" to={{ name: "home", machine: m() }} aria-current={route().name === "home" ? "page" : undefined}><I n="home" />Home</A>
      <A class="nav" to={{ name: "archive", machine: m() }} aria-current={route().name === "archive" ? "page" : undefined}><I n="archive" />Archive<Show when={index().archive.length > 0}><span class="count">{index().archive.length}</span></Show></A>
    </div>
    <div class="sb-scroll">
      <Show when={pinned().length > 0}>
        <div class="sb-h"><span>Pinned</span></div>
        <Keyed each={pinned()} by={v => v.id}>{v => <A class="task-row" style={{ "padding-left": "8px" }} to={{ name: "task", machine: v().machine.id, task: v().id }}><I n="pin" s={14} class="faint" /><span class="trunc">{v().task.title}</span></A>}</Keyed>
      </Show>
      <div class="sb-h"><span>Projects</span><button class="ib" style={{ width: "22px", height: "22px" }} popovertarget="m-sort" aria-label="Sort projects"><I n="chevrons-v" s={14} /></button></div>
      <Show when={ws.store.ready()}><Show when={projects().length > 0} fallback={<p class="cap faint ws-side-empty">No projects on this machine.</p>}>
        <Keyed each={projects()} by={s => s.project.id}>{(s, i) => <ProjectRow summary={s()} n={i()} openByDefault={s().project.id === firstLive()} />}</Keyed>
      </Show></Show>
    </div>
    <SidebarUsage />
    <SidebarFoot />
    <div class="ws-resize" role="separator" aria-orientation="vertical" aria-label="Sidebar width" tabindex="0"
      aria-valuemin={SIDEBAR_MIN} aria-valuemax={SIDEBAR_MAX} aria-valuenow={ws.panels.sidebarWidth()}
      onPointerDown={e => { dragging = true; e.currentTarget.setPointerCapture?.(e.pointerId); }} onPointerMove={onMove} onPointerUp={() => { dragging = false; }}
      onDblClick={() => ws.panels.setSidebarWidth(SIDEBAR_DEFAULT)}
      onKeyDown={e => {
        const step = e.key === "ArrowLeft" ? -16 : e.key === "ArrowRight" ? 16 : 0;
        if (step) { e.preventDefault(); ws.panels.setSidebarWidth(ws.panels.sidebarWidth() + step); }
      }}></div>
  </nav>;
}

const SECTION_LABEL: Record<SettingsSection, string> = {
  appearance: "Appearance", updates: "Updates", voice: "Voice input", shortcuts: "Shortcuts", machines: "All machines", overview: "Overview",
  orchestration: "Orchestration", connections: "Engine connections", worktrees: "Worktrees", storage: "Storage and archive", diagnostics: "Diagnostics", remote: "Remote access",
};
const PAGE_LABEL: Record<OrchestrationPage, string> = { defaults: "Coordinator", models: "Models", routing: "Routing", effective: "Route preview", policy: "Limits and fallback", tools: "Tools and MCP", profiles: "Profiles", files: "Files and presets" };
const SECTION_ICON: Record<SettingsSection, string> = {
  appearance: "palette", updates: "download", voice: "mic", shortcuts: "keyboard", machines: "server", overview: "laptop",
  orchestration: "sliders", connections: "plug", worktrees: "branch", storage: "database", diagnostics: "activity", remote: "cloud",
};

function SettingsNav() {
  const ws = useWs();
  const r = () => { const x = ws.router.route(); return x.name === "settings" ? x : null; };
  // A component, so each machine read leaves the links in place: a call with the machine's id redrew all of them
  // under a resting pointer whenever the machine's record moved.
  const Item = (p: { section: SettingsSection; machine: string | null }) => <A class="snav" role="tab" to={{ name: "settings", section: p.section, machine: p.machine }} aria-selected={r()?.section === p.section && !r()?.page ? "true" : "false"}>
    <I n={SECTION_ICON[p.section]} /><span class="trunc">{SECTION_LABEL[p.section]}</span>
    <Show when={p.section === "machines"}><span class="r">{ws.store.state().machines.length}</span></Show>
  </A>;
  const page = (id: OrchestrationPage) => <A class="snav sub" role="tab" to={{ name: "settings", section: "orchestration", machine: ws.machine().id, page: id }} aria-selected={r()?.page === id ? "true" : "false"}><span class="trunc">{PAGE_LABEL[id]}</span></A>;
  const switchTo = (id: string) => { const x = r(); ws.go(x?.machine ? { ...x, machine: id } : { name: "settings", section: "overview", machine: id }); };
  const connection = (m: { connection: string; lastConfirmedAt: string | null; kind: string }) => m.connection === "connected" ? `${m.kind === "local" ? "Local" : "Remote"}, connected` : `Offline, read-only, last seen ${clock(m.lastConfirmedAt)}`;
  return <nav class="sidebar" aria-label="Settings">
    <div class="sb-top" style={{ "padding-bottom": "4px" }}><A class="nav" to={{ name: "home", machine: ws.machine().id }}><I n="arrow-left" />Back to workspace</A></div>
    <div class="sb-scroll">
      <div class="snav-h">App</div>
      <For each={APP_SECTIONS.filter(s => s !== "machines")}>{s => <Item section={s} machine={null} />}</For>
      <div class="snav-h">Machines</div>
      <Item section="machines" machine={null} />
      <button class="snav-h ws-set-machine" popovertarget="m-set-machine"><I n={ws.machine().kind === "local" ? "laptop" : "server"} s={14} />{ws.machine().name}<I n="chevrons-v" s={12} class="faint" />
        <Show when={ws.machine().connection === "connected"} fallback={<span class="st st-idle ws-ml-auto">Offline</span>}><span class="st st-ok ws-ml-auto">Connected</span></Show></button>
      <Pop id="m-set-machine" class="pop-anchor" align="left" label="Settings for" style={{ width: "250px" }}>
        <div class="mi-h">Settings for</div>
        <For each={ws.store.state().machines}>{m => <MenuItem icon={m.kind === "local" ? "laptop" : "server"} label={m.name} sub={connection(m)} check={m.id === ws.machine().id} onSelect={() => switchTo(m.id)} />}</For>
      </Pop>
      <For each={MACHINE_SECTIONS}>{s => <><Item section={s} machine={ws.machine().id} /><Show when={s === "orchestration"}><For each={ORCHESTRATION_PAGES}>{page}</For></Show></>}</For>
    </div>
    <SidebarUsage />
    <SidebarFoot />
  </nav>;
}
export const Nav = () => {
  const ws = useWs();
  return <Show when={ws.router.route().name === "settings"} fallback={<Sidebar />}><SettingsNav /></Show>;
};

function Checks(props: { view: TaskView }) {
  const counts = () => checkCounts(props.view.task.result?.checks ?? []);
  return <div class="chk-sum">
    <Show when={counts().passed}><span class="st st-ok"><I n="check-circle" s={12} />{counts().passed} passed</span></Show>
    <Show when={counts().failed}><span class="st st-bad"><I n="x-circle" s={12} />{counts().failed} failed</span></Show>
    <Show when={counts().not_run}><span class="st st-idle"><I n="circle" s={12} />{counts().not_run} not run</span></Show>
    <Show when={counts().unverified}><span class="st st-idle"><I n="help" s={12} />{counts().unverified} unverified</span></Show>
  </div>;
}

export function TaskPanel(props: { view: TaskView; heading: string }) {
  const task = () => props.view.task;
  return <>
    <div class="ins-sec">
      <div class="ins-h"><span>{props.heading}</span></div>
      <div class="w500 selectable" style={{ "font-size": "14px", "line-height": "20px" }}>{task().title}</div>
      <div style={{ "margin-top": "6px" }}><FactLine fact={props.view.fact} /></div>
    </div>
    <div class="ins-sec">
      <div class="ins-h"><span>Checkout</span></div>
      <dl class="kv">
        <dt>Type</dt><dd>{task().checkout.kind === "worktree" ? "Worktree" : "Main checkout"}</dd>
        <Show when={task().checkout.branch}><dt>Branch</dt><dd class="mono">{task().checkout.branch}</dd></Show>
        <Show when={task().checkout.kind === "worktree"}><dt>Path</dt><dd class="mono">{(task().checkout as { path: string }).path}</dd></Show>
        <Show when={props.view.target}><dt>{sameBranch(task(), props.view.target) ? "Target branch" : "Merge into"}</dt><dd class="mono">{props.view.target}</dd></Show>
      </dl>
    </div>
    <Show when={props.view.workers.length > 0}>
      <div class="ins-sec">
        <div class="ins-h"><span>Workers</span><span>{props.view.running} of {props.view.workers.length} running</span></div>
        <For each={props.view.workers}>{w => <div class="wins"><I n={workerFace(w.status).icon} s={14} class="faint" /><span class="trunc">{w.role}</span><span class="st st-idle">{workerFace(w.status).label}</span></div>}</For>
      </div>
    </Show>
    <Show when={task().result}>
      <div class="ins-sec"><div class="ins-h"><span>Checks</span></div><Checks view={props.view} /></div>
    </Show>
  </>;
}

function SprintPanel(props: { sprintId: string }) {
  const ws = useWs();
  const sprint = createMemo(() => ws.store.state().sprints.find(s => s.id === props.sprintId));
  const view = createMemo(() => { const s = sprint(); return s ? sprintView(ws.store.index(), s) : null; });
  return <Show when={view()}>{v => <>
    <div class="ins-sec">
      <div class="ins-h"><span>Sprint</span></div>
      <div class="w500 selectable" style={{ "font-size": "14px", "line-height": "20px" }}>{v().title}</div>
      <div class="st st-idle" style={{ "margin-top": "6px" }}>{v().state}</div>
    </div>
    <div class="ins-sec">
      <div class="ins-h"><span>Run</span></div>
      <dl class="kv">
        <dt>Mode</dt><dd>{v().sprint.runMode === "automatic" ? "Automatic, one task at a time" : "Manual"}</dd>
        <dt>Target branch</dt><dd class="mono">{v().sprint.targetBranch ?? ws.store.index().summaries.get(v().sprint.projectId)?.project.defaultBranch}</dd>
        <dt>Finishing</dt><dd>Each task waits for your confirmation</dd>
        <Show when={v().sprint.pauseReason}><dt>Paused</dt><dd>{v().sprint.pauseReason}</dd></Show>
      </dl>
    </div>
    <Show when={v().views.length > 0}>
      <div class="ins-sec">
        <div class="ins-h"><span>Tasks</span></div>
        <Keyed each={v().views} by={t => t.id}>{t => <div class="wins"><I n={rowIcon(t())} s={14} class={t().fact.tone} /><span class="trunc">{t().task.title}</span></div>}</Keyed>
      </div>
    </Show>
  </>}</Show>;
}

export function Inspector() {
  const ws = useWs();
  const ctx = useRouteContext();
  const route = () => ws.router.route();
  const selected = () => { const id = ws.selected(); return id ? ws.store.index().byId.get(id) ?? null : null; };
  const scopeSprint = () => { const r = route(); return r.name === "project" && r.scope.kind === "sprint" ? r.scope.id : null; };
  const firstNeed = () => homeView(ws.store.index(), ws.machine().id).needs[0] ?? null;
  const repoArea = () => { const r = route(); return r.name === "project" && ["code", "git", "worktrees"].includes(r.view ?? "") && !!repoOf(ws.store.state(), r.project); };
  // A component: called with the task, the button was redrawn every time the task moved on.
  const Open = (p: { v: TaskView }) => <div class="ins-sec"><A class="btn btn-line ws-wide" to={{ name: "task", machine: p.v.machine.id, task: p.v.id }}>Open task<I n="arrow-right" s={14} /></A></div>;
  return <aside class="inspector" aria-label="Inspector">
    <Switch fallback={<MachinePanel />}>
      <Match when={ctx.view() && route().name === "task"}><TaskInspector id={ctx.view()!.id} pick={(route() as { worker?: string }).worker ?? null} /></Match>
      <Match when={selected()}>{v => <><TaskPanel view={v()} heading="Selected task" /><Open v={v()} /></>}</Match>
      <Match when={scopeSprint()}>{id => <SprintPanel sprintId={id()} />}</Match>
      <Match when={repoArea() && ctx.summary()}>{s => {
        const repo = () => repoOf(ws.store.state(), s().project.id)!;
        const r = () => route() as Extract<Route, { name: "project" }>;
        return <Show when={r().view === "code"} fallback={<GitPanel summary={s()} repo={repo()} checkout={defaultCheckout(repo(), r().checkout)} />}><CodePanel repo={repo()} checkout={defaultCheckout(repo(), r().checkout)} file={r().file} /></Show>;
      }}</Match>
      <Match when={route().name === "project" && ctx.summary()}>{s => <ProjectPanel summary={s()} />}</Match>
      <Match when={route().name === "home" && firstNeed()}>{v => <><TaskPanel view={v()} heading="First decision" /><Open v={v()} /></>}</Match>
    </Switch>
  </aside>;
}

function ProjectPanel(props: { summary: ProjectSummary }) {
  return <>
    <div class="ins-sec">
      <div class="ins-h"><span>Project</span></div>
      <div class="w500 selectable" style={{ "font-size": "14px", "line-height": "20px" }}>{props.summary.project.name}</div>
      <p class="cap muted" style={{ "margin-top": "4px" }}>Press Space on a card, or use its menu, to preview it here.</p>
    </div>
    <div class="ins-sec">
      <dl class="kv">
        <dt>Repository</dt><dd class="mono">{props.summary.project.repository}</dd>
        <Show when={props.summary.project.openBranch}><dt>Open branch</dt><dd class="mono">{props.summary.project.openBranch}</dd></Show>
        <Show when={props.summary.project.defaultBranch}><dt>Default branch</dt><dd class="mono">{props.summary.project.defaultBranch}</dd></Show>
        <dt>Machine</dt><dd>{props.summary.machine.name}</dd>
      </dl>
    </div>
  </>;
}

function MachinePanel() {
  const ws = useWs();
  const m = () => ws.machine();
  return <div class="ins-sec">
    <div class="ins-h"><span>Machine</span></div>
    <dl class="kv">
      <dt>Name</dt><dd>{m().name}</dd>
      <dt>Kind</dt><dd>{m().kind === "local" ? "Local" : "Remote"}</dd>
      <dt>Connection</dt><dd>{m().connection === "connected" ? "Connected" : `Offline, last confirmed ${clock(m().lastConfirmedAt)}`}</dd>
      <dt>Projects</dt><dd>{projectsOn(ws.store.index(), m().id).length}</dd>
      <dt>Archive kept</dt><dd>{m().retentionDays} days</dd>
    </dl>
  </div>;
}

/** Belongs to the checkout in view. It says so when no shell is connected. */
export function Terminal() {
  const ws = useWs();
  const ctx = useRouteContext();
  const projectId = () => ctx.view()?.project.id ?? ctx.summary()?.project.id;
  if (ws.terminal) return <Show when={projectId()} keyed>{id =>
    <LiveTerminal projectId={id} repository={ws.store.state().projects.find(p => p.id === id)?.repository ?? ""} port={ws.terminal!} />
  }</Show>;
  const where = () => {
    const v = ctx.view(), s = ctx.summary(), r = ws.router.route();
    const repo = !v && s ? repoOf(ws.store.state(), s.project.id) : null;
    if (repo && r.name === "project" && r.checkout) {
      const c = defaultCheckout(repo, r.checkout);
      return `${c.path} (${c.kind === "main" ? "main checkout" : "worktree"}, ${c.branch})`;
    }
    if (v) return `${v.task.checkout.kind === "worktree" ? `${(v.task.checkout as { path: string }).path} (worktree` : `${v.project.repository} (main checkout`}, ${v.task.checkout.branch})`;
    return s ? `${s.project.repository} (main checkout, ${s.project.openBranch})` : "";
  };
  return <section class="terminal" aria-label="Terminal">
    <div class="term-h">
      <span class="term-tab" aria-selected="true"><I n="terminal" s={12} />bash</span>
      <button class="ib" style={{ width: "24px", height: "24px" }} data-why={ws.t("notAvailable")} aria-label="New shell in this checkout" aria-disabled="true"><I n="plus" s={14} /></button>
      <span class="mono faint" style={{ "margin-left": "6px" }}>{where()}</span>
    </div>
    <div class="term-body">{ws.t("terminalBody")}</div>
  </section>;
}

/** Menus that belong to the frame. They stay mounted so their triggers can open them natively. */
export function FrameMenus() {
  const ws = useWs();
  const machines = () => ws.store.state().machines;
  const current = () => ws.machine();
  const limits = createMemo(() => engineLimits(ws.store.state(), current()));
  const projectId = () => { const r = ws.router.route(); return r.name === "project" ? r.project : null; };
  const sub = (m: typeof machines extends () => (infer T)[] ? T : never) => `${m.kind === "local" ? "Local" : "Remote"}, ${m.connection === "connected" ? "connected" : `offline, last seen ${clock(m.lastConfirmedAt)}`}`;
  return <>
    <Pop id="m-usage" class="pop-usage">
      <div class="mi-h">Engine limits on {current().name}</div>
      <For each={limits().rows}>{(r, i) => <><Show when={i() > 0}><div class="mi-sep"></div></Show><Limit row={r} /></>}</For>
      <div class="mi-sep"></div>
      <MenuItem icon="gauge" label="All limits and windows" onSelect={() => ws.go({ name: "settings", section: "connections", machine: current().id })} />
    </Pop>
    <Pop id="m-machine" class="pop-tl" label="Machines">
      <div class="mi-h">Machines</div>
      <For each={machines()}>{m => <MenuItem icon={m.kind === "local" ? "laptop" : "server"} label={m.name} sub={sub(m)} check={m.id === current().id} onSelect={() => ws.go({ name: "home", machine: m.id })} />}</For>
      <div class="mi-sep"></div>
      <MenuItem icon="plus" label="Add remote machine…" needs="pair_machine" onSelect={() => ws.open({ kind: "add-machine" })} />
      <MenuItem icon="settings" label="Machine settings" onSelect={() => ws.go({ name: "settings", section: "overview", machine: current().id })} />
    </Pop>
    <Pop id="m-new" class="pop-right" align="right" label="New">
      <MenuItem icon="plus" label="New task" keys="Ctrl+N" onSelect={() => ws.newItem("task")} />
      <MenuItem icon="layers" label="New sprint" keys="Ctrl+Shift+N" needs="create_sprint" onSelect={() => ws.newItem("sprint")} />
    </Pop>
    <Pop id="m-project" class="pop-right" align="right" label="Project actions">
      <MenuItem icon="terminal" label="Open in terminal" disabled={ws.terminalAvailable() ? undefined : ws.t("notAvailable")} onSelect={() => { if (!ws.panels.terminalOpen()) ws.toggleTerminal(); }} />
      <MenuItem icon="external" label={ws.externalLabel()} disabled={ws.noOpener("workspace.open_editor", "noExternal")} onSelect={() => { const id = projectId(); if (id) ws.run({type:"open_editor",projectId:id}); }} />
      <MenuItem icon="settings" label="Project settings" onSelect={() => { const id = projectId(); if (id) ws.go({ name: "project", machine: current().id, project: id, view: "settings", scope: { kind: "all" } }); }} />
      <div class="mi-sep"></div>
      <MenuItem icon="x" label="Remove project…" danger needs="remove_project" onSelect={() => { const id=projectId(); if (id) ws.open({kind:"confirm",title:"Remove this project?",danger:true,action:"Remove",note:"Stops its work and removes this project from the list. Files, branches and task history are kept. You can add the folder again.",go:async()=>{const result=await ws.run({type:"remove_project",projectId:id,confirmed:true});if(result.ok)ws.go({name:"home",machine:current().id});return result.ok;}}); }} />
    </Pop>
    <Pop id="m-sort" class="pop-anchor" align="left" label="Sort projects">
      <MenuItem label="Default order" check={ws.panels.sort() === "default"} onSelect={() => ws.panels.setSort("default")} />
      <MenuItem label="Name" check={ws.panels.sort() === "name"} onSelect={() => ws.panels.setSort("name")} />
      <MenuItem label="Recent activity" check={ws.panels.sort() === "activity"} onSelect={() => ws.panels.setSort("activity")} />
    </Pop>
    {ws.demo?.menu()}
  </>;
}
