import { createEffect, createMemo, createSignal, For, Match, on, onCleanup, Show, Switch, untrack, type Accessor, type JSX } from "solid-js";
import { capabilityOf, FEATURES, type WorkspaceCommand, type WorkspacePort, type WorkspaceState } from "@cortex/workspace-domain";
import { ProjectPage, TaskMenu } from "./board";
import { UnsavedOnClose } from "./code";
import { ReviewDialogs, type WsDialog } from "./dialogs";
import { createBuffers } from "./editor-buffers";
import { QuestionNotifications } from "./notifications";
import { NewPage } from "./newtask";
import { COPY, type CopyKey } from "./copy";
import { useWs, WsContext, type DemoSlot, type WindowChrome, type Ws } from "./ctx";
import { FrameMenus, Inspector, Nav, Terminal, TitleBar } from "./frame";
import { onScreen, ScreenActive, UnsentRegistry } from "./freshness";
import { Home } from "./home";
import { I, place } from "./kit";
import { ReasonTip, Strip } from "./layout";
import { StartupScreen } from "./startup";
import { Archive } from "./archive";
import { MachineDialogs } from "./machine-dialogs";
import { Inbox, NotFound } from "./pages";
import { Help, Palette } from "./palette";
import { createPanels } from "./prefs";
import { createPress } from "./press";
import { needsOverride } from "./review-model";
import { parseRoute, type Route, type Router } from "./router";
import { TaskPage } from "./session";
import { routeLabel, sessionModel } from "./session-model";
import { SettingsPage } from "./settings";
import { createSettingsState } from "./settings-state";
import { SprintPage } from "./sprint";
import { createWorkspaceStore, type StoreOptions } from "./store";
import { CloseDialog, ExternalDoneDialog } from "./task-chrome";
import { sprintView } from "./selectors";
import { tabTitle } from "./tabs";
import { createTheme } from "./theme";
import { createTabs, parkPane, TabStrip } from "./workspace-tabs";
import "./fonts.css";
import "./mockup-styles.css";
import "./workspace.css";
import "./layout.css";
import type { TerminalPort } from "./terminal-client";

const typing = (el: Element | null) => el instanceof HTMLTextAreaElement || el instanceof HTMLSelectElement
  || (el instanceof HTMLInputElement && !["checkbox", "radio", "button"].includes(el.type)) || (el as HTMLElement | null)?.isContentEditable === true;

function Content() {
  const ws = useWs();
  const route = () => ws.router.route();
  const knownMachine = () => { const r = route(); return !("machine" in r) || r.machine === null || ws.store.state().machines.some(m => m.id === r.machine); };
  const asProject = () => { const r = route(); return r.name === "project" ? r : null; };
  const asSprint = () => { const r = route(); return r.name === "sprint" ? r : null; };
  const summary = createMemo(() => {
    const r = asProject() ?? asSprint(), s = r ? ws.store.index().summaries.get(r.project) : undefined;
    return s && s.machine.id === r!.machine ? s : null;
  });
  const task = createMemo(() => { const r = route(); return r.name === "task" ? ws.store.index().byId.get(r.task) ?? null : null; });
  // Offline from the start nothing about the machine is known yet: no empty project list and no wrong-page error stand in for it.
  return <Show when={ws.store.ready()} fallback={<NotLoaded />}><Show when={knownMachine()} fallback={<NotFound message="This machine is not connected to this Workspace" />}>
    <Switch fallback={<NotFound message="This page does not exist" />}>
      <Match when={route().name === "home"}><Home /></Match>
      <Match when={route().name === "inbox"}><Inbox /></Match>
      <Match when={route().name === "archive"}><Archive /></Match>
      <Match when={route().name === "new" && (route() as Extract<Route, { name: "new" }>)}>{r => <NewPage route={r()} />}</Match>
      <Match when={route().name === "settings" && (route() as Extract<Route, { name: "settings" }>)}>{r => <SettingsPage route={r()} />}</Match>
      <Match when={asProject()}>
        <Show when={summary()?.project.id} keyed fallback={<NotFound message="This project is not on this machine" />}>
          {_ => <ProjectPage summary={summary()!} route={asProject()!} />}
        </Show>
      </Match>
      <Match when={asSprint()}>
        <Show when={summary()?.project.id} keyed fallback={<NotFound message="This project is not on this machine" />}>
          {_ => <Show when={asSprint()?.sprint} keyed>{__ => <SprintPage route={asSprint()!} summary={summary()!} />}</Show>}
        </Show>
      </Match>
      <Match when={route().name === "task"}>
        <Show when={task()?.id} keyed fallback={<NotFound message="This task is not on this machine" />}>{id => <TaskPage id={id} />}</Show>
      </Match>
    </Switch>
  </Show></Show>;
}

function NotLoaded() {
  const ws = useWs();
  return <div class="note-panel" role="status" aria-live="polite"><span class="w500">The workspace could not be loaded.</span>
    <Show when={ws.store.readIssue() ?? ws.store.connection().reason}>{why => <span>{why()}</span>}</Show>
    <button class="btn btn-line ws-mt8" onClick={() => void ws.store.reload()}>Try again</button>
  </div>;
}

function Receipt() {
  const ws = useWs();
  let timer: ReturnType<typeof setTimeout> | undefined;
  // Every message leaves on its own, and any click, key or move to another screen takes it away sooner: nothing stays stuck.
  createEffect(on(ws.store.receipt, r => {
    clearTimeout(timer);
    if (r) timer = setTimeout(() => ws.store.clearReceipt(), r.ok ? 3000 : 8000);
  }));
  createEffect(on(() => ws.router.route(), () => { if (ws.store.receipt()?.ok === false) ws.store.clearReceipt(); }, { defer: true }));
  const dismiss = () => { if (ws.store.receipt()) ws.store.clearReceipt(); };
  const onKey = (e: KeyboardEvent) => { if (e.key === "Escape") dismiss(); };
  const onPointer = () => { if (ws.store.receipt()?.ok === false) dismiss(); };
  document.addEventListener("pointerdown", onPointer, true);
  document.addEventListener("keydown", onKey, true);
  onCleanup(() => { clearTimeout(timer); document.removeEventListener("pointerdown", onPointer, true); document.removeEventListener("keydown", onKey, true); });
  return <>
    <div class="ws-live" role="status" aria-live="polite">{ws.store.receipt()?.message ?? ""}</div>
    <Show when={ws.store.receipt()?.ok === false}><div class="ws-receipt">{ws.store.receipt()!.message}</div></Show>
  </>;
}

/** A real service that cannot be reached is named, with a way to try again. There is nothing to fall back to. */
function ConnectionNotice() {
  const ws = useWs();
  const lost = () => { const s = ws.store.connection().state; return s === "unavailable" || s === "disconnected"; };
  return <Show when={lost()}><Strip class="ws-conn" role="alert" tone="info" icon={<I n="wifi-off" s={14} class="faint" />} action={<button class="btn btn-sm" onClick={() => void ws.store.reload()}>Retry</button>}>{ws.t("disconnected")}</Strip></Show>;
}

export type WorkspaceProps = StoreOptions & { notify?: (label: string) => Promise<void>; port: WorkspacePort; demo?: DemoSlot; chrome?: WindowChrome; terminal?: TerminalPort; voice?: import("./voice-client").VoicePort;
  /** A host notice, shown with the connection notice at the top of the main area so it never takes a frame row. */
  notice?: JSX.Element;
  /** The logo shown until the first coherent read. A host that already painted it hands over that element, so nothing is drawn twice. */
  logo?: JSX.Element };
/** What a tab keeps to itself besides its route. */
type TabLocal = {
  selected: Accessor<string | null>; select: (taskId: string | null) => void;
  codeLeft: Accessor<"files" | "search">; setCodeLeft: (left: "files" | "search") => void;
  codeMode: Accessor<"file" | "inline" | "split">; setCodeMode: (mode: "file" | "inline" | "split") => void;
};
function createTabLocal(): TabLocal {
  const [selected, select] = createSignal<string | null>(null);
  const [codeLeft, setCodeLeft] = createSignal<"files" | "search">("files");
  const [codeMode, setCodeMode] = createSignal<"file" | "inline" | "split">("file");
  return { selected, select, codeLeft, setCodeLeft, codeMode, setCodeMode };
}
type TabScoped = Pick<Ws, "router" | "machine" | "go" | "terminalAvailable" | "toggleTerminal" | "inspectable" | "selected" | "select" | "codeLeft" | "setCodeLeft" | "codeMode" | "setCodeMode" | "newItem">;

/** One screen's content. Its body runs only once the tab has been shown, so a restored tab costs nothing until it is opened. */
function TabBody(props: { make: () => Ws; shown: Accessor<boolean>; unsent: (check: () => boolean) => () => void }) {
  const ws = props.make();
  // Redirects replace the entry, so Back never lands on a page that bounces forward again.
  createEffect(() => {
    const r = ws.router.route();
    if (r.name === "root") ws.go({ name: "home", machine: ws.machine().id }, { replace: true });
    else if (r.name === "project" && r.view === null) ws.go({ ...r, view: "board" }, { replace: true });
  });
  // A parked Git tab must reread when shown again, not only when its component first mounts.
  createEffect(on(() => {
    const r = ws.router.route();
    return props.shown() && r.name === "project" && (r.view === "git" || r.view === "worktrees") ? r.project : null;
  }, projectId => { if (projectId) void ws.store.refreshProjectFacts(projectId); }));
  // A preview belongs to the view it was made in.
  createEffect(on(() => ws.router.path(), () => ws.select(null), { defer: true }));
  return <WsContext.Provider value={ws}><ScreenActive.Provider value={props.shown}><UnsentRegistry.Provider value={props.unsent}>
    <Content />
    <Show when={ws.terminalAvailable()}><Terminal /></Show>
  </UnsentRegistry.Provider></ScreenActive.Provider></WsContext.Provider>;
}

/**
 * Stays mounted while another tab is on view, so typed text, scroll and expanded rows are simply still there.
 * Parked means out of sight with its work stopped: no ticks, no reads of the store, no animation. It keeps its size, so nothing inside it re-measures.
 */
function TabPane(props: { id: number; shown: Accessor<boolean>; make: () => Ws; unsent: (check: () => boolean) => () => void; keepFocus: () => boolean; home: () => HTMLElement | undefined }) {
  let pane!: HTMLDivElement, last: HTMLElement | null = null;
  const [mounted, setMounted] = createSignal(untrack(props.shown));
  createEffect(on(props.shown, shown => {
    if (shown) setMounted(true);
    parkPane(pane, !shown);
    if (shown && !props.keepFocus()) queueMicrotask(() => (last?.isConnected ? last : props.home())?.focus({ preventScroll: true }));
  }, { defer: true }));
  return <div ref={pane} class="ws-pane" role="tabpanel" aria-labelledby={`ws-tab-${props.id}`} data-parked={props.shown() ? undefined : ""} onFocusIn={e => { last = e.target as HTMLElement; }}>
    <Show when={mounted()}><TabBody make={props.make} shown={props.shown} unsent={props.unsent} /></Show>
  </div>;
}

/** The approved Workspace frame, on whatever port it is given. It has no backend of its own and no way to pick one. */
export function Workspace(props: WorkspaceProps) {
  const press = createPress();
  onCleanup(press.dispose);
  const store = createWorkspaceStore(props.port, { now: props.now, initial: props.initial, track: press.track });
  const tabs = createTabs();
  const router = tabs.active;
  const locals = new Map<number, TabLocal>();
  const localOf = (id: number) => { let held = locals.get(id); if (!held) { held = createTabLocal(); locals.set(id, held); } return held; };
  const panels = createPanels();
  // A window narrower than a reading column plus the sidebar holds the sidebar away for now, on every page, so each page keeps
  // its width down to phone size. The owner's choice is untouched, and asking for the sidebar back brings it back.
  const narrowQuery = typeof window.matchMedia === "function" ? window.matchMedia("(max-width: 819.98px)") : undefined;
  const [narrow, setNarrow] = createSignal(!!narrowQuery?.matches), [insisted, setInsisted] = createSignal(false);
  const onNarrow = () => { setNarrow(!!narrowQuery?.matches); if (!narrowQuery?.matches) setInsisted(false); };
  narrowQuery?.addEventListener("change", onNarrow);
  const sidebarOwner = {};
  createEffect(() => panels.yieldSidebar(sidebarOwner, narrow() && !insisted() && !panels.sidebarHidden() ? "room" : null, () => setInsisted(true)));
  onCleanup(() => { narrowQuery?.removeEventListener("change", onNarrow); panels.yieldSidebar(sidebarOwner, null); });
  const theme = createTheme();
  const demo = props.demo;
  const drafts = new Map<string, string>();
  const unique = () => demo?.unique() ?? (globalThis.crypto?.randomUUID?.() ?? `${Date.now().toString(36)}${Math.random().toString(36).slice(2)}`).slice(0, 8);
  const [closing, setClosing] = createSignal<string | null>(null);
  const [externalClose, setExternalClose] = createSignal<"external" | "already_in_target" | null>(null);
  const [paletteOpen, setPaletteOpen] = createSignal(false);
  const [helpOpen, setHelpOpen] = createSignal(false);
  const [menuTask, setMenuTask] = createSignal<string | null>(null);
  const [dialog, setDialog] = createSignal<WsDialog | null>(null);
  const settings = createSettingsState();
  const buffers = createBuffers({ read: store.readFile, save: store.saveFile, remember: store.rememberSource });
  const [closingWindow, setClosingWindow] = createSignal(false);
  createEffect(() => buffers.setAutosave(panels.autosave()));
  const chrome = props.chrome;
  chrome?.guardClose(() => { if (!buffers.unsaved().length) return false; setClosingWindow(true); return true; });
  // A browser has no asking dialog of its own to offer, only this.
  const leaving = (e: BeforeUnloadEvent) => { if (buffers.unsaved().length) e.preventDefault(); };
  window.addEventListener("beforeunload", leaving);
  onCleanup(() => window.removeEventListener("beforeunload", leaving));
  let taskMenu: HTMLDivElement | undefined, shell!: HTMLDivElement, main!: HTMLElement;

  /** Everything that follows the route, for the frame (the active tab) and for each tab's own content. */
  const scopedTo = (tabRouter: Router, local: () => TabLocal, state: Accessor<WorkspaceState>): TabScoped => {
    const route = () => tabRouter.route();
    const machine = createMemo(() => {
      const r = route(), machines = state().machines;
      const wanted = "machine" in r && r.machine ? r.machine : panels.lastMachine();
      return machines.find(m => m.id === wanted) ?? machines.find(m => m.id === panels.lastMachine()) ?? machines.find(m => m.kind === "local") ?? machines[0];
    });
    const terminalAvailable = () => (props.terminal !== undefined || store.can(FEATURES.terminal)) && (route().name === "project" || route().name === "task");
    // A section with unsaved changes asks first. Browser Back still goes, and the draft waits in the section for its return.
    const go: Ws["go"] = (to, opts) => {
      if (!opts?.force && route().name === "settings" && settings.leaving(to)) { settings.setPending(to); return; }
      tabRouter.go(to, opts);
    };
    return {
      router: tabRouter, machine, go, terminalAvailable,
      toggleTerminal() { if (terminalAvailable()) panels.toggleTerminal(); },
      inspectable: () => !["sprint", "archive", "settings"].includes(route().name),
      selected: () => local().selected(), select: id => local().select(id),
      codeLeft: () => local().codeLeft(), setCodeLeft: left => local().setCodeLeft(left),
      codeMode: () => local().codeMode(), setCodeMode: mode => local().setCodeMode(mode),
      newItem(kind, project = null, sprint = null) {
        const r = route();
        const here = project ?? (r.name === "project" || r.name === "sprint" ? r.project : null);
        // From a sprint's own pages a new task starts in that sprint; the owner can still pick No sprint.
        const inSprint = sprint ?? (r.name === "sprint" ? r.sprint : r.name === "project" && r.scope.kind === "sprint" ? r.scope.id : null);
        go({ name: "new", machine: machine().id, kind, project: here, ...(inSprint && kind === "task" ? { sprint: inSprint } : {}) });
      },
    };
  };
  const t = (key: CopyKey) => demo?.copy[key] ?? COPY[key];
  const gate = (type: WorkspaceCommand["type"]) => store.can(capabilityOf(type)) ? undefined : ["pair_machine", "set_remote_access", "revoke_device"].includes(type) ? "Remote pairing is unavailable until the owner selects a relay server. No connection is attempted." : t("notOffered");
  const ws: Ws = {
    ...scopedTo(router, () => localOf(tabs.activeId()), store.state),
    notify: props.notify, store, panels, theme, buffers, demo, chrome, drafts, terminal: props.terminal, voice: props.voice, settings, t, gate,
    can: store.can,
    needs(type) { const reason = gate(type); return reason ? { "aria-disabled": "true", "data-why": reason } : {}; },
    noOpener: (capability, reason) => store.can(capability) ? undefined : store.connection().state === "connected" ? t(reason) : t("notOffered"),
    externalLabel: () => store.can("workspace.open_editor") && !store.can("workspace.open_vscode") ? "Open in default app" : "Open in VS Code",
    route: value => routeLabel(value, store.state().options),
    newId: kind => demo?.newId(kind) ?? `${kind}-${unique()}`,
    unique,
    ok(command, then) { void store.dispatch(command).then(receipt => { if (receipt.ok) then(receipt); }); },
    busy: (work, control) => press.track(work, control),
    archiveTask(taskId) {
      ws.ok({ type: "archive_task", taskId, now: store.clock() }, () => {
        if (store.state().tasks.find(t => t.id === taskId)?.archiveQuestion) setDialog({ kind: "unmerged", taskId });
      });
    },
    stopTask(taskId) {
      void store.dispatch({ type: "stop_task", taskId, confirmed: demo?.stopConfirmed() ?? true, now: store.clock() });
    },
    closeTask: (id, how) => { setExternalClose(how ?? null); setClosing(id); },
    run: (command: WorkspaceCommand) => store.dispatch(command),
    dialog, open: setDialog, closeDialog: () => setDialog(null),
    accept(taskId) {
      const result = store.state().tasks.find(t => t.id === taskId)?.result;
      if (!result) return;
      if (needsOverride(result.checks) && !(result.kind === "answer" && result.acceptanceAvailable && result.checks.length === 0)) setDialog({ kind: "accept", taskId });
      else void store.dispatch({ type: "accept_result", taskId, digest: result.digest, now: store.clock() });
    },
    openPalette: () => setPaletteOpen(true),
    openHelp: () => setHelpOpen(true),
    openTaskMenu(taskId, trigger) {
      setMenuTask(taskId);
      if (!taskMenu) return;
      press.invokedBy(taskMenu, trigger);
      place(taskMenu, trigger, "right");
      taskMenu.showPopover?.();
    },
  };
  const { machine, go, inspectable, selected, select: setSelected, toggleTerminal } = ws;
  const route = () => router.route();

  // A parked tab keeps the state it last showed, and reads the machine again the moment it is on view.
  const paneWs = (id: number, shown: Accessor<boolean>): Ws => {
    const held = <T,>(read: Accessor<T>): Accessor<T> => { let last = untrack(read); return createMemo(() => { if (shown()) last = read(); return last; }); };
    const state = held(store.state), index = held(store.index);
    return { ...ws, ...scopedTo(tabs.routerOf(id), () => localOf(id), state), store: { ...store, state, index } };
  };
  const unsent = new Map<number, Set<() => boolean>>();
  const unsentIn = (id: number) => (check: () => boolean) => {
    const set = unsent.get(id) ?? new Set<() => boolean>();
    unsent.set(id, set.add(check));
    return () => set.delete(check);
  };
  const titleOf = (id: number) => {
    const r = tabs.routerOf(id), index = store.index();
    return tabTitle(parseRoute(r.path(), r.search()), {
      project: p => index.summaries.get(p)?.project.name, task: t => index.byId.get(t)?.task.title,
      sprint: s => { const found = store.state().sprints.find(x => x.id === s); return found && sprintView(index, found).title; },
    });
  };
  const closeNow = (id: number) => { tabs.close(id); locals.delete(id); unsent.delete(id); };
  const askClose = (id: number) => {
    if (![...(unsent.get(id) ?? [])].some(check => check())) return closeNow(id);
    setDialog({ kind: "confirm", title: `Close ${titleOf(id)}?`, note: "This tab has text you have not sent yet.", action: "Close tab", danger: true, go: () => { closeNow(id); return true; } });
  };
  let keepStripFocus = false;
  const selectTab = (id: number, keepFocus = false) => { keepStripFocus = keepFocus; tabs.select(id); keepStripFocus = false; };

  createEffect(() => { const r = route(); if ("machine" in r && r.machine && store.state().machines.some(m => m.id === r.machine)) panels.setLastMachine(r.machine); });
  demo?.attach(ws);
  // Keyboard focus never falls out to <body> when a view replaces itself.
  createEffect(on(() => router.path(), () => {
    queueMicrotask(() => { if (document.activeElement === document.body) main?.focus({ preventScroll: true }); });
  }, { defer: true }));
  const modal = () => paletteOpen() || helpOpen();
  createEffect(() => shell?.toggleAttribute("inert", modal()));

  const onKey = (e: KeyboardEvent) => {
    if (!store.ready() || document.querySelector("dialog[open]")) return;
    const ctrl = e.ctrlKey || e.metaKey, key = e.key.toLowerCase();
    const act = (run: () => void) => { e.preventDefault(); run(); };
    if (e.altKey && !ctrl && !e.shiftKey && e.key === "ArrowLeft") return act(() => { if (router.canBack()) router.back(); });
    if (e.altKey && !ctrl && !e.shiftKey && e.key === "ArrowRight") return act(() => { if (router.canForward()) router.forward(); });
    if (ctrl && e.altKey && e.code === "KeyB") return act(() => { if (inspectable()) panels.toggleInspector(); });
    if (ctrl && !e.altKey && !e.shiftKey && key === "b") return act(panels.toggleSidebar);
    if (ctrl && !e.altKey && !e.shiftKey && key === "k") return act(ws.openPalette);
    if (ctrl && !e.altKey && !e.shiftKey && key === "j") return act(toggleTerminal);
    if (ctrl && !e.altKey && !e.shiftKey && key === "n") return act(() => ws.newItem("task"));
    if (ctrl && e.shiftKey && !e.altKey && key === "n") return act(() => ws.newItem("sprint"));
    if (ctrl && !e.altKey && !e.shiftKey && key === "i") return act(() => go({ name: "inbox", machine: machine().id }));
    if (ctrl && !e.altKey && e.key === ",") return act(() => go({ name: "settings", section: "appearance", machine: null }));
    if (ctrl && !e.altKey && !e.shiftKey && e.key === ".") {
      const r = route();
      if (r.name === "task" && sessionModel(store.index(), r.task)?.mode === "live") act(() => ws.stopTask(r.task));
      return;
    }
    if (ctrl || e.altKey || typing(document.activeElement)) return;
    if (e.key === "?") return act(ws.openHelp);
    if (e.key === "/" && route().name === "project") return act(() => {
      const field = document.getElementById("ws-filter");
      if (field) field.focus(); else onScreen().querySelector<HTMLElement>('.pstrip [aria-label="Filter"]')?.click();
    });
    if (e.key === "Escape" && selected()) return act(() => setSelected(null));
    if (e.key === "Escape" && !panels.wide() && panels.overlayOpen()) return act(panels.closeOverlay);
  };
  document.addEventListener("keydown", onKey);
  onCleanup(() => document.removeEventListener("keydown", onKey));
  // Before the terminal and the editor see the key. Ctrl+T and Ctrl+W stay the shell's own while typing there.
  const onTabKey = (e: KeyboardEvent) => {
    if (!store.ready() || !(e.ctrlKey || e.metaKey) || e.altKey || document.querySelector("dialog[open]")) return;
    const act = (run: () => void) => { e.preventDefault(); e.stopPropagation(); run(); };
    if (e.key === "Tab") return act(() => tabs.cycle(e.shiftKey ? -1 : 1));
    if (e.shiftKey) return;
    if (/^[1-9]$/.test(e.key)) return act(() => tabs.jump(Number(e.key)));
    if ((document.activeElement as Element | null)?.closest?.(".terminal")) return;
    if (e.code === "KeyT" || e.key === "t") return act(tabs.add);
    if (e.code === "KeyW" || e.key === "w") return act(() => askClose(tabs.activeId()));
  };
  window.addEventListener("keydown", onTabKey, true);
  onCleanup(() => window.removeEventListener("keydown", onTabKey, true));
  // The quiet focus line belongs to keyboard movement alone, so the last way of moving decides it, and any pointer press clears it.
  const keyboard = (on: boolean) => document.documentElement.toggleAttribute("data-keyboard", on);
  const onMoveKey = (e: KeyboardEvent) => { if (e.key === "Tab" || e.key.startsWith("Arrow")) keyboard(true); };
  const onPointer = () => keyboard(false);
  window.addEventListener("keydown", onMoveKey, true);
  window.addEventListener("pointerdown", onPointer, true);
  onCleanup(() => { window.removeEventListener("keydown", onMoveKey, true); window.removeEventListener("pointerdown", onPointer, true); keyboard(false); });

  // The logo stands only while the machine has not been heard from, or answered without a view to show. A service that cannot be
  // reached is the offline workspace: the frame with its Offline notice and Retry, as on any later disconnect. Once built, the
  // frame stays, so a retry never drops back to the logo.
  const offline = () => { const s = store.connection().state; return s === "unavailable" || s === "disconnected"; };
  const framed = createMemo<boolean>(was => was || store.ready() || offline(), false);
  // The frame is built whole, in the same update that ends the logo, so no partial interface is ever painted.
  return <WsContext.Provider value={ws}><Show when={framed()} fallback={<StartupScreen logo={props.logo} notice={props.notice} />}>
    <div class="ws-app" data-sidebar={panels.sidebarShown() ? "shown" : "hidden"} data-inspector={panels.inspectorHidden() || !inspectable() || !store.ready() ? "hidden" : "shown"}
      data-overlay={panels.overlayOpen() && inspectable() ? "open" : "closed"} data-terminal={panels.terminalOpen() ? "open" : "closed"} style={{ "--sidebar-w": `${panels.sidebarWidth()}px` }}>
      <div class="ws-shell" ref={shell}>
        <TitleBar />
        <div class="workspace">
          <Nav />
          <main class="main" ref={main} tabindex="-1">
            <TabStrip tabs={tabs} title={titleOf} onSelect={selectTab} onClose={askClose} />
            <ConnectionNotice />
            <Show when={store.ready() && store.readIssue() && store.connection().state === "connected"}>
              <div class="strip warn" role="alert"><span>Workspace refresh failed. Showing the last loaded view.</span>
                <button class="lnk" onClick={() => void store.reload()}>Retry</button>
              </div>
            </Show>
            <QuestionNotifications />
            {props.notice}
            {/* In a fixed order: moving a mounted pane in the document would lose its scroll positions. */}
            <div class="ws-panes"><For each={[...tabs.ids()].sort((a, b) => a - b)}>{id => {
              const shown = createMemo(() => tabs.activeId() === id);
              return <TabPane id={id} shown={shown} make={() => paneWs(id, shown)} unsent={unsentIn(id)} keepFocus={() => keepStripFocus} home={() => main} />;
            }}</For></div>
          </main>
          <Show when={store.ready()}><Inspector /></Show>
        </div>
        <FrameMenus />
        <TaskMenu taskId={menuTask()} ref={el => { taskMenu = el; }} />
      </div>
      <ReasonTip />
      <Palette open={paletteOpen()} onClose={() => setPaletteOpen(false)} />
      <Help open={helpOpen()} onClose={() => setHelpOpen(false)} />
      <Show when={externalClose()} fallback={<CloseDialog taskId={closing()} onClose={() => setClosing(null)} />}>
        <ExternalDoneDialog taskId={closing()} how={externalClose()!} onClose={() => setClosing(null)} />
      </Show>
      <UnsavedOnClose open={closingWindow()} onClose={() => setClosingWindow(false)} />
      <ReviewDialogs />
      <MachineDialogs />
      <Receipt />
    </div>
  </Show></WsContext.Provider>;
}
export default Workspace;
