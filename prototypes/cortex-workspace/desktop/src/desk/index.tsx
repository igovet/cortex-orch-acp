import { createEffect, createMemo, createSignal, Match, on, onCleanup, Show, Switch } from "solid-js";
import type { ConceptProps } from "../store";
import { DeskContext, initialLocal, useDesk, type ConfirmOpts, type Desk, type PaletteMode } from "./ctx";
import { Dashboard, NeedsYouPage } from "./dashboard";
import { ConfirmDialog, CreateTaskDialog, HelpDialog, Palette } from "./dialogs";
import { createDeskData } from "./model";
import { createPrefs, createTheme } from "./prefs";
import { ProjectPage } from "./project";
import { createRouter, hrefOf, PROJECT_VIEWS, TASK_TABS, type ProjectViewId, type Route } from "./router";
import { SettingsPage } from "./settings";
import { Sidebar } from "./sidebar";
import { TaskPage } from "./task";
import { TitleBand } from "./titleband";
import { A, ErrorState } from "./ui";
import "./desk.css";

const isTextField = (el: Element | null) => !!el && (el instanceof HTMLTextAreaElement || el instanceof HTMLSelectElement
  || (el instanceof HTMLInputElement && !["checkbox", "radio", "button", "submit"].includes(el.type)) || (el as HTMLElement).isContentEditable);

/** Project Desk: the Paseo-informed candidate direction (pending Gate D). Mock data only. */
export default function Desk(props: ConceptProps) {
  const store = props.store;
  const router = createRouter();
  const theme = createTheme();
  const prefs = createPrefs();
  const [local, setLocal] = createSignal(initialLocal());
  const data = createDeskData(store, () => local().loading, () => local().titles);
  const [wide, setWide] = createSignal(window.innerWidth >= 1600);
  const onResize = () => setWide(window.innerWidth >= 1600);
  window.addEventListener("resize", onResize);
  onCleanup(() => window.removeEventListener("resize", onResize));

  const [createFor, setCreateFor] = createSignal<string | null>(null);
  const [palette, setPalette] = createSignal<PaletteMode | null>(null);
  const [help, setHelp] = createSignal(false);
  const [confirmState, setConfirmState] = createSignal<{ opts: ConfirmOpts; resolve: (ok: boolean) => void } | null>(null);
  const [announcement, setAnnouncement] = createSignal("");
  let announceTimer: ReturnType<typeof setTimeout> | undefined;

  const currentProjectId = () => {
    const r = router.route();
    const slug = r.name === "project" || r.name === "task" ? r.slug : undefined;
    return slug ? data.projectBySlug().get(slug)?.project.id : undefined;
  };
  const desk: Desk = {
    store, data, router, prefs, theme, local, setLocal, wide,
    receipt(id, text, change) {
      setLocal(l => ({ ...l, receipts: { ...l.receipts, [id]: text }, changes: change ? [...l.changes, { text: change, at: "now" }] : l.changes }));
    },
    blocked(projectId) {
      if (data.offline()) return "Offline; actions unavailable";
      const ps = projectId ? data.projects().find(p => p.project.id === projectId) : undefined;
      return ps?.disconnected ? "Disconnected; actions unavailable" : null;
    },
    go(to, opts) {
      const href = typeof to === "string" ? to : hrefOf(to);
      if (href.startsWith("/settings") && !location.pathname.startsWith("/settings")) sessionStorage.setItem("cw.beforeSettings", location.pathname);
      router.go(href, opts);
    },
    openCreate(projectId) {
      const first = data.projects().find(p => !p.disconnected) ?? data.projects()[0];
      setCreateFor(projectId ?? currentProjectId() ?? first?.project.id ?? "");
    },
    openPalette: mode => { setPalette(null); queueMicrotask(() => setPalette(mode)); },
    openHelp: () => setHelp(true),
    confirm: opts => new Promise<boolean>(resolve => setConfirmState({ opts, resolve })),
    announce(text) {
      clearTimeout(announceTimer);
      setAnnouncement("");
      announceTimer = setTimeout(() => setAnnouncement(text), 50);
    },
    reset() { store.reset(); setLocal(initialLocal()); desk.announce("Prototype reset to the fixture baseline"); },
  };

  // Incoming data never moves focus or the route: new decisions only update one polite status region.
  createEffect(on(() => data.needs().length, (n, prev) => { if (prev !== undefined && n > prev) desk.announce(`${n} decision${n === 1 ? "" : "s"} need you`); }));

  // Default width scales with the text-size preset (capped at 30% of the window); a user-dragged width is kept as chosen.
  const sidebarWidth = () => prefs.sidebarWidth() ? `${prefs.sidebarWidth()}px` : `clamp(220px, ${wide() ? 17.5 : 16.25}rem, 30vw)`;
  const route = () => router.route();
  // Project landing: last-used view, Board on first visit. Replaces the URL so Back is not trapped.
  createEffect(() => {
    const r = route();
    if (r.name === "project" && r.view === null && data.projectBySlug().get(r.slug)) {
      const last = prefs.lastView()[r.slug];
      const view = (PROJECT_VIEWS as readonly string[]).includes(last) ? last as ProjectViewId : "board";
      router.go({ name: "project", slug: r.slug, view }, { replace: true });
    }
  });

  const onKey = (e: KeyboardEvent) => {
    if (document.querySelector("dialog[open]")) return;
    const r = route();
    const ctrl = e.ctrlKey || e.metaKey;
    const digit = /^Digit([1-6])$/.exec(e.code)?.[1];
    const act = (fn: () => void) => { e.preventDefault(); fn(); };
    if (ctrl && !e.altKey && !e.shiftKey && e.key.toLowerCase() === "k") return act(() => desk.openPalette("all"));
    if (ctrl && !e.altKey && !e.shiftKey && e.key.toLowerCase() === "o") return act(() => desk.openPalette("projects"));
    if (ctrl && e.altKey && e.code === "KeyN") return act(() => desk.openCreate());
    if (ctrl && e.altKey && e.code === "KeyD") return act(() => desk.go("/needs-you"));
    if (ctrl && e.altKey && e.code === "KeyH") return act(() => desk.go("/"));
    if (ctrl && !e.altKey && e.key.toLowerCase() === "b") return act(() => prefs.setSidebarHidden(!prefs.sidebarHidden()));
    if (ctrl && e.key === "\\" && r.name === "task") return act(() => prefs.setListPaneHidden(!prefs.listPaneHidden()));
    if (ctrl && e.key === ",") return act(() => desk.go("/settings/appearance"));
    if (ctrl && (e.key === "`" || e.code === "Backquote") && r.name === "task" && (r.tab === "code" || r.tab === "git")) return act(() => prefs.setTerminalOpen(!prefs.terminalOpen()));
    if (ctrl && !e.shiftKey && e.key.toLowerCase() === "p" && r.name === "task" && r.tab === "code") return act(() => document.getElementById("file-filter")?.focus());
    if (ctrl && e.shiftKey && e.code === "KeyF" && r.name === "task" && r.tab === "code") return act(() => document.getElementById("file-filter")?.focus());
    if (ctrl && e.altKey && digit) {
      const i = Number(digit) - 1;
      if (r.name === "project") return act(() => desk.go(`/p/${r.slug}/${PROJECT_VIEWS[i]}`));
      if (r.name === "task") return act(() => desk.go(`/p/${r.slug}/t/${r.num}/${TASK_TABS[i]}`));
    }
    if ((ctrl && e.key === "/") || (e.key === "?" && !ctrl && !e.altKey && !isTextField(document.activeElement))) return act(() => setHelp(true));
  };
  document.addEventListener("keydown", onKey);
  onCleanup(() => document.removeEventListener("keydown", onKey));
  // Input modality: programmatic focus targets (view headings) show their ring after keyboard input, not after a pointer click.
  const root = document.documentElement;
  root.dataset.input = "keyboard";
  const onPointer = () => { root.dataset.input = "pointer"; };
  const onAnyKey = (e: KeyboardEvent) => { if (!["Shift", "Control", "Alt", "Meta"].includes(e.key)) root.dataset.input = "keyboard"; };
  document.addEventListener("pointerdown", onPointer, true);
  document.addEventListener("keydown", onAnyKey, true);
  onCleanup(() => { document.removeEventListener("pointerdown", onPointer, true); document.removeEventListener("keydown", onAnyKey, true); });
  // If a state change removes the focused element, focus moves to the view heading (visible ring), never to <body>.
  // Browsers do not reliably fire blur on removal, so watch DOM mutations for a disconnected last-focused element.
  let lastFocused: Element | null = null;
  const onFocusIn = (e: FocusEvent) => { lastFocused = e.target as Element; };
  document.addEventListener("focusin", onFocusIn);
  const focusGuard = new MutationObserver(() => {
    if (!lastFocused || lastFocused.isConnected || document.querySelector("dialog[open]")) return;
    if (document.activeElement && document.activeElement !== document.body) return;
    lastFocused = null;
    queueMicrotask(() => { if (document.activeElement === document.body) document.getElementById("view-title")?.focus(); });
  });
  queueMicrotask(() => { const app = document.getElementById("app"); if (app) focusGuard.observe(app, { childList: true, subtree: true }); });
  onCleanup(() => { document.removeEventListener("focusin", onFocusIn); focusGuard.disconnect(); });

  const viewKey = createMemo(() => {
    const r = route();
    return r.name === "project" ? `p:${r.slug}` : r.name === "task" ? `t:${r.slug}:${r.num}` : r.name;
  });
  const skipTo = (id: string) => (e: MouseEvent) => { e.preventDefault(); document.getElementById(id)?.focus(); };

  return <DeskContext.Provider value={desk}>
    <div id="app" class={`desk ${prefs.sidebarHidden() ? "sidebar-hidden" : ""} ${prefs.textSize() >= 150 ? "text-lg" : ""}`} style={{ "--sb-w": sidebarWidth() }}>
      <a class="skip-link" href="#sidebar-nav" onClick={skipTo("sidebar-nav")}>Skip to navigation</a>
      <a class="skip-link" href="#main" onClick={skipTo("main")}>Skip to main content</a>
      <Show when={route().name === "task"}><a class="skip-link" href="#task-main" onClick={skipTo("task-main")}>Skip to task</a></Show>
      <Show when={!prefs.sidebarHidden()}><Sidebar /></Show>
      <div class="main-col">
        <TitleBand />
        <main id="main" tabindex="-1" aria-labelledby="view-title">
          <Show when={viewKey()} keyed>{_key => <RouteView route={route()} />}</Show>
        </main>
      </div>
      <div class="sr-only" role="status" aria-live="polite">{announcement()}</div>
    </div>
    <Show when={createFor() !== null}><CreateTaskDialog open projectId={createFor()!} onClose={() => setCreateFor(null)} /></Show>
    <Show when={palette()}>{mode => <Palette open mode={mode()} onClose={() => setPalette(null)} />}</Show>
    <HelpDialog open={help()} onClose={() => setHelp(false)} />
    <ConfirmDialog opts={confirmState()?.opts ?? null} onDone={ok => { const s = confirmState(); setConfirmState(null); s?.resolve(ok); }} />
  </DeskContext.Provider>;
}

function RouteView(props: { route: Route }) {
  const desk = useDesk();
  return <Switch fallback={<NotFound />}>
    <Match when={props.route.name === "home"}><Dashboard /></Match>
    <Match when={props.route.name === "needs"}><NeedsYouPage /></Match>
    <Match when={props.route.name === "project" ? props.route : undefined}>{r => {
      const ps = () => desk.data.projectBySlug().get(r().slug);
      return <Show when={ps()} fallback={<NotFound />}>{p => <Show when={r().view}>{v => <ProjectPage ps={p()} view={v()} />}</Show>}</Show>;
    }}</Match>
    <Match when={props.route.name === "task" ? props.route : undefined}>{r => {
      const ps = () => desk.data.projectBySlug().get(r().slug);
      const tv = () => { const t = desk.data.taskByNum().get(r().num); return t && t.slug === r().slug ? t : undefined; };
      return <Show when={ps()} fallback={<NotFound />}>{p => <Show when={tv()} fallback={<TaskUnavailable slug={r().slug} name={p().project.name} />}>
        {t => <TaskPage ps={p()} tv={t()} tab={r().tab} />}
      </Show>}</Show>;
    }}</Match>
    <Match when={props.route.name === "settings" ? props.route : undefined}>{r => <SettingsPage section={r().section} slug={r().slug} />}</Match>
  </Switch>;
}

function NotFound() {
  return <div class="scroll-pane"><div class="page narrow">
    <h1 id="view-title" tabindex="-1">Nothing at this address</h1>
    <p class="subline">This prototype has no view here. <A class="inline-link" href="/">Go to all projects</A></p>
  </div></div>;
}
function TaskUnavailable(props: { slug: string; name: string }) {
  const desk = useDesk();
  return <div class="scroll-pane"><div class="page narrow">
    <h1 id="view-title" tabindex="-1">Task unavailable</h1>
    <Switch>
      <Match when={desk.data.status() === "error"}><ErrorState text="Couldn't load this task (fixture error E_PAGE). Nothing was changed." retry={() => desk.store.selectScenario("default")} /></Match>
      <Match when={desk.data.status() === "loading"}><p class="subline">Loading…</p></Match>
      <Match when={true}><p class="subline">This task isn't in the current view of {props.name}. <A class="inline-link" href={`/p/${props.slug}/board`}>Open the board</A></p></Match>
    </Switch>
  </div></div>;
}
