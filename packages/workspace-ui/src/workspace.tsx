import { createEffect, createMemo, createSignal, Match, on, onCleanup, Show, Switch, type JSX } from "solid-js";
import { capabilityOf, FEATURES, type WorkspaceCommand, type WorkspacePort } from "@cortex/workspace-domain";
import { ProjectPage, TaskMenu } from "./board";
import { UnsavedOnClose } from "./code";
import { ReviewDialogs, type WsDialog } from "./dialogs";
import { createBuffers } from "./editor-buffers";
import { QuestionNotifications } from "./notifications";
import { NewPage } from "./newtask";
import { COPY, type CopyKey } from "./copy";
import { useWs, WsContext, type DemoSlot, type WindowChrome, type Ws } from "./ctx";
import { FrameMenus, Inspector, Nav, Terminal, TitleBar } from "./frame";
import { Home } from "./home";
import { I, place } from "./kit";
import { Archive } from "./archive";
import { MachineDialogs } from "./machine-dialogs";
import { Inbox, NotFound } from "./pages";
import { Help, Palette } from "./palette";
import { createPanels } from "./prefs";
import { createPress } from "./press";
import { needsOverride } from "./review-model";
import { createRouter, type Route } from "./router";
import { TaskPage } from "./session";
import { routeLabel, sessionModel } from "./session-model";
import { SettingsPage } from "./settings";
import { createSettingsState } from "./settings-state";
import { SprintPage } from "./sprint";
import { createWorkspaceStore, type StoreOptions } from "./store";
import { CloseDialog, ExternalDoneDialog } from "./task-chrome";
import { createTheme } from "./theme";
import "./mockup-styles.css";
import "./workspace.css";
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
  return <Show when={ws.store.ready()} fallback={<div class="page" role="status" aria-live="polite">
    <Show when={ws.store.readIssue() || ws.store.connection().state === "unavailable"} fallback={<span>Loading workspace…</span>}>
      <div class="row" style={{ gap: "12px", "flex-wrap": "wrap" }}><span>The workspace could not be loaded.</span><button class="btn btn-line" onClick={() => void ws.store.reload()}>Try again</button></div>
    </Show>
  </div>}><Show when={knownMachine()} fallback={<NotFound message="This machine is not connected to this Workspace" />}>
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
  return <Show when={lost()}><div class="strip ws-conn" role="alert"><I n="wifi-off" s={14} class="faint" /><span>{ws.t("disconnected")}</span><span class="acts"><button class="btn btn-sm" onClick={() => void ws.store.reload()}>Retry</button></span></div></Show>;
}

export type WorkspaceProps = StoreOptions & { notify?: (label: string) => Promise<void>; port: WorkspacePort; demo?: DemoSlot; chrome?: WindowChrome; terminal?: TerminalPort; voice?: import("./voice-client").VoicePort;
  /** A host notice, shown with the connection notice at the top of the main area so it never takes a frame row. */
  notice?: JSX.Element };
/** The approved Workspace frame, on whatever port it is given. It has no backend of its own and no way to pick one. */
export function Workspace(props: WorkspaceProps) {
  const press = createPress();
  onCleanup(press.dispose);
  const store = createWorkspaceStore(props.port, { now: props.now, initial: props.initial, track: press.track });
  const router = createRouter();
  const panels = createPanels();
  const theme = createTheme();
  const demo = props.demo;
  const drafts = new Map<string, string>();
  const unique = () => demo?.unique() ?? (globalThis.crypto?.randomUUID?.() ?? `${Date.now().toString(36)}${Math.random().toString(36).slice(2)}`).slice(0, 8);
  const [closing, setClosing] = createSignal<string | null>(null);
  const [externalClose, setExternalClose] = createSignal<"external" | "already_in_target" | null>(null);
  const [selected, setSelected] = createSignal<string | null>(null);
  const [paletteOpen, setPaletteOpen] = createSignal(false);
  const [helpOpen, setHelpOpen] = createSignal(false);
  const [menuTask, setMenuTask] = createSignal<string | null>(null);
  const [dialog, setDialog] = createSignal<WsDialog | null>(null);
  const [codeLeft, setCodeLeft] = createSignal<"files" | "search">("files");
  const [codeMode, setCodeMode] = createSignal<"file" | "inline" | "split">("file");
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

  const route = () => router.route();
  const machine = createMemo(() => {
    const r = route(), machines = store.state().machines;
    const wanted = "machine" in r && r.machine ? r.machine : panels.lastMachine();
    return machines.find(m => m.id === wanted) ?? machines.find(m => m.id === panels.lastMachine()) ?? machines.find(m => m.kind === "local") ?? machines[0];
  });
  const terminalAvailable = () => (props.terminal !== undefined || store.can(FEATURES.terminal)) && (route().name === "project" || route().name === "task");
  const toggleTerminal = () => { if (terminalAvailable()) panels.toggleTerminal(); };
  const inspectable = () => !["sprint", "archive", "settings"].includes(route().name);
  // A section with unsaved changes asks first. Browser Back still goes, and the draft waits in the section for its return.
  const go: Ws["go"] = (to, opts) => {
    if (!opts?.force && route().name === "settings" && settings.leaving(to)) { settings.setPending(to); return; }
    router.go(to, opts);
  };
  const t = (key: CopyKey) => demo?.copy[key] ?? COPY[key];
  const gate = (type: WorkspaceCommand["type"]) => store.can(capabilityOf(type)) ? undefined : ["pair_machine", "set_remote_access", "revoke_device"].includes(type) ? "Remote pairing is unavailable until the owner selects a relay server. No connection is attempted." : t("notOffered");
  const ws: Ws = {
    notify: props.notify, store, router, panels, theme, buffers, demo, chrome, drafts, machine, selected, go, terminal: props.terminal, voice: props.voice, terminalAvailable, toggleTerminal, settings, inspectable, t, gate,
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
    select: setSelected,
    stopTask(taskId) {
      void store.dispatch({ type: "stop_task", taskId, confirmed: demo?.stopConfirmed() ?? true, now: store.clock() });
    },
    closeTask: (id, how) => { setExternalClose(how ?? null); setClosing(id); },
    run: (command: WorkspaceCommand) => store.dispatch(command),
    dialog, open: setDialog, closeDialog: () => setDialog(null), codeLeft, setCodeLeft, codeMode, setCodeMode,
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
    newItem(kind, project = null, sprint = null) {
      const r = route();
      const here = project ?? (r.name === "project" || r.name === "sprint" ? r.project : null);
      // From a sprint's own pages a new task starts in that sprint; the owner can still pick No sprint.
      const inSprint = sprint ?? (r.name === "sprint" ? r.sprint : r.name === "project" && r.scope.kind === "sprint" ? r.scope.id : null);
      go({ name: "new", machine: machine().id, kind, project: here, ...(inSprint && kind === "task" ? { sprint: inSprint } : {}) });
    },
  };

  // Redirects replace the entry, so Back never lands on a page that bounces forward again.
  createEffect(() => {
    const r = route();
    if (r.name === "root") go({ name: "home", machine: machine().id }, { replace: true });
    else if (r.name === "project" && r.view === null) go({ ...r, view: "board" }, { replace: true });
  });
  createEffect(() => { const r = route(); if ("machine" in r && r.machine && store.state().machines.some(m => m.id === r.machine)) panels.setLastMachine(r.machine); });
  demo?.attach(ws);
  // A preview belongs to the view it was made in. Keyboard focus never falls out to <body> when a view replaces itself.
  createEffect(on(() => router.path(), () => {
    setSelected(null);
    queueMicrotask(() => { if (document.activeElement === document.body) main?.focus({ preventScroll: true }); });
  }, { defer: true }));
  const modal = () => paletteOpen() || helpOpen();
  createEffect(() => shell.toggleAttribute("inert", modal()));

  const onKey = (e: KeyboardEvent) => {
    if (document.querySelector("dialog[open]")) return;
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
      if (field) field.focus(); else document.querySelector<HTMLElement>('.pstrip [aria-label="Filter"]')?.click();
    });
    if (e.key === "Escape" && selected()) return act(() => setSelected(null));
    if (e.key === "Escape" && !panels.wide() && panels.overlayOpen()) return act(panels.closeOverlay);
  };
  document.addEventListener("keydown", onKey);
  onCleanup(() => document.removeEventListener("keydown", onKey));
  // The quiet focus line belongs to keyboard movement alone, so the last way of moving decides it, and any pointer press clears it.
  const keyboard = (on: boolean) => document.documentElement.toggleAttribute("data-keyboard", on);
  const onMoveKey = (e: KeyboardEvent) => { if (e.key === "Tab" || e.key.startsWith("Arrow")) keyboard(true); };
  const onPointer = () => keyboard(false);
  window.addEventListener("keydown", onMoveKey, true);
  window.addEventListener("pointerdown", onPointer, true);
  onCleanup(() => { window.removeEventListener("keydown", onMoveKey, true); window.removeEventListener("pointerdown", onPointer, true); keyboard(false); });

  return <WsContext.Provider value={ws}>
    <div class="ws-app" data-sidebar={panels.sidebarHidden() ? "hidden" : "shown"} data-inspector={panels.inspectorHidden() || !inspectable() ? "hidden" : "shown"}
      data-overlay={panels.overlayOpen() && inspectable() ? "open" : "closed"} data-terminal={panels.terminalOpen() ? "open" : "closed"} style={{ "--sidebar-w": `${panels.sidebarWidth()}px` }}>
      <div class="ws-shell" ref={shell}>
        <TitleBar />
        <div class="workspace">
          <Nav />
          <main class="main" ref={main} tabindex="-1">
            <ConnectionNotice />
            <Show when={store.ready() && store.readIssue() && store.connection().state === "connected"}>
              <div class="strip warn" role="alert"><span>Workspace refresh failed. Showing the last loaded view.</span>
                <button class="lnk" onClick={() => void store.reload()}>Retry</button>
              </div>
            </Show>
            <QuestionNotifications />
            {props.notice}
            <Content />
            <Show when={terminalAvailable()}><Terminal /></Show>
          </main>
          <Show when={store.ready()}><Inspector /></Show>
        </div>
        <FrameMenus />
        <TaskMenu taskId={menuTask()} ref={el => { taskMenu = el; }} />
      </div>
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
  </WsContext.Provider>;
}
export default Workspace;
