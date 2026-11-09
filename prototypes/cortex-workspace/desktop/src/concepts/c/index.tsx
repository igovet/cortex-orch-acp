import { createEffect, createMemo, createSignal, For, Match, onCleanup, onMount, Show, Switch } from "solid-js";
import { FIXTURE_VERSION } from "../../fixtures";
import { SCENARIO_SCRIPTS } from "../../scenarios";
import type { ConceptProps } from "../../store";
import "./c.css";
import { Ctx, type ConceptContext, type ConfirmRequest, type DeepSurface } from "./ctx";
import { DeepView } from "./Deep";
import { SECTION_TITLES, TaskView } from "./Focus";
import { blockReason, buildQueue, clock, connectionOf, decisionFor, resultFor, stamp } from "./model";
import { AllTasks, ConnectionPage, CreatePage, EmptyProject, ErrorState, SettingsPage } from "./Pages";
import { Queue } from "./Queue";
import { Modal } from "./ui";

type View = "attention" | "all" | "deep";
type Page = "task" | "create" | "connection" | "settings";
type Dialog = null | { kind: "help" } | { kind: "palette" } | { kind: "confirm"; request: ConfirmRequest; resolve: (ok: boolean) => void };

export default function ConceptC(props: ConceptProps) {
  const store = props.store;
  const st = () => store.state();
  const [view, setView] = createSignal<View>("attention");
  const [page, setPage] = createSignal<Page>("task");
  const [surface, setSurface] = createSignal<DeepSurface>("discussion");
  const [allMode, setAllMode] = createSignal<"list" | "board">("list");
  const [allReturn, setAllReturn] = createSignal<View>("attention");
  const [quietOpen, setQuietOpen] = createSignal(false);
  const [loading, setLoading] = createSignal(false);
  const [receipts, setReceipts] = createSignal<Record<string, string>>({});
  const [announcement, setAnnouncement] = createSignal("");
  const [scale, setScale] = createSignal(1);
  const [shortcuts, setShortcuts] = createSignal(true);
  const [sections, setSections] = createSignal<Record<string, number[]>>({});
  const [terminalOpen, setTerminalOpen] = createSignal(false);
  const [dialog, setDialog] = createSignal<Dialog>(null);
  const [projectMenu, setProjectMenu] = createSignal(false);
  const [script, setScript] = createSignal(0);
  const [step, setStep] = createSignal(0);
  let wrap!: HTMLDivElement;
  let topbar!: HTMLElement;
  let focusPane!: HTMLElement;
  let status!: HTMLElement;
  let railBack: HTMLButtonElement | undefined;
  let focusQueue: () => void = () => undefined;

  const conn = createMemo(() => connectionOf(st().scenario));
  const blocked = createMemo(() => blockReason(conn(), loading()));
  const queue = createMemo(() => buildQueue(st(), store.visibleTasks(), conn()));
  const needCount = createMemo(() => queue().filter(i => i.group === "decision").length);
  const selectedTask = createMemo(() => {
    const id = st().selectedTaskId;
    return id ? store.visibleTasks().find(t => t.id === id) ?? null : null;
  });
  const asOf = () => conn() === "connected" ? null : stamp(st().fixture.machines[0].lastConfirmedAt);

  const focusEntry = () => {
    const target = focusPane.querySelector<HTMLElement>("#c-focus-heading, #c-deep-heading, #c-all-heading");
    (target ?? focusPane).focus();
  };
  const afterRender = (fn: () => void) => queueMicrotask(fn);
  const announce = (message: string) => { setAnnouncement(""); afterRender(() => setAnnouncement(message)); };
  const openTask = (taskId: string) => {
    store.selectTask(taskId);
    setView("attention"); setPage("task");
    afterRender(focusEntry);
  };
  const openDeep = (s: DeepSurface) => { setSurface(s); setView("deep"); afterRender(focusEntry); };
  const openAll = () => { if (view() !== "all") setAllReturn(view()); setView("all"); afterRender(focusEntry); };
  const openCreate = () => { setView("attention"); setPage("create"); afterRender(() => focusPane.querySelector<HTMLElement>("textarea")?.focus()); };
  const openPage = (p: "connection" | "settings") => { setProjectMenu(false); setView("attention"); setPage(p); afterRender(focusEntry); };
  const goAttention = () => { setView("attention"); setPage("task"); afterRender(() => focusQueue()); };
  const back = () => {
    if (view() === "deep") goAttention();
    else if (view() === "all") { const to = allReturn(); setView(to); afterRender(to === "attention" ? () => focusQueue() : focusEntry); }
    else if (page() !== "task") { setPage("task"); afterRender(() => focusQueue()); }
  };
  const isAcceptance = (taskId: string) => !decisionFor(st().fixture, taskId) && !!resultFor(st().fixture, taskId);
  const sectionOpen = (taskId: string, n: number) => {
    const rec = sections()[taskId];
    return rec ? rec.includes(n) : n === 3 && isAcceptance(taskId);
  };
  const toggleSection = (taskId: string, n: number, open?: boolean) => {
    const current = SECTION_TITLES.map((_, i) => i + 1).filter(i => sectionOpen(taskId, i));
    const want = open ?? !current.includes(n);
    setSections({ ...sections(), [taskId]: want ? [...new Set([...current, n])] : current.filter(i => i !== n) });
  };
  const confirm = (request: ConfirmRequest) => new Promise<boolean>(resolve => setDialog({ kind: "confirm", request, resolve }));
  const closeDialog = (ok = false) => {
    const d = dialog();
    setDialog(null);
    if (d?.kind === "confirm") d.resolve(ok);
  };

  const ctx: ConceptContext = {
    store, conn, blocked, receipts, announce, openDeep, openTask, openCreate, openAll, openPage, confirm,
    setReceipt: (key, message) => setReceipts({ ...receipts(), [key]: message }),
    textScale: scale, sectionOpen, toggleSection, terminalOpen, setTerminalOpen,
  };

  // Text-size preset: rem-based sizes follow the root font size. Restored when the concept unmounts.
  const rootFont = document.documentElement.style.fontSize;
  createEffect(() => { document.documentElement.style.fontSize = scale() === 1 ? rootFont : `${16 * scale()}px`; });
  onCleanup(() => { document.documentElement.style.fontSize = rootFont; });

  const regions = () => [
    () => topbar.querySelector<HTMLElement>("button")?.focus(),
    ...(view() === "attention" ? [() => focusQueue()] : view() === "deep" ? [() => railBack?.focus()] : []),
    focusEntry,
    () => status.focus(),
  ];
  const regionEls = () => [topbar, ...(view() === "all" ? [] : [wrap.querySelector<HTMLElement>(".c-queue, .c-rail")]), focusPane, status];
  const cycleRegion = (dir: number) => {
    const els = regionEls();
    const active = document.activeElement;
    const i = els.findIndex(el => !!el && el.contains(active));
    const fns = regions();
    fns[(i + dir + fns.length) % fns.length]();
  };
  const moveHeading = (dir: number) => {
    const heads = Array.from(focusPane.querySelectorAll<HTMLElement>("[data-l2]"));
    if (!heads.length) return;
    const i = heads.indexOf(document.activeElement as HTMLElement);
    const next = i < 0 ? (dir > 0 ? 0 : heads.length - 1) : Math.max(0, Math.min(heads.length - 1, i + dir));
    heads[next].focus();
  };

  let gPending = false;
  const onKey = (e: KeyboardEvent) => {
    const t = e.target as HTMLElement | null;
    if (!t || !(t === document.body || wrap.contains(t))) return;
    if (dialog()) return;
    const key = e.key;
    const lower = key.length === 1 ? key.toLowerCase() : key;
    const typing = !!t.closest("input, textarea, select, [contenteditable='true']");
    const inFocus = focusPane.contains(t);
    if ((e.ctrlKey || e.metaKey) && !e.shiftKey && lower === "k") { e.preventDefault(); setDialog({ kind: "palette" }); return; }
    if (((e.ctrlKey && !e.shiftKey && !e.altKey) || (e.altKey && !e.ctrlKey)) && lower === "n") { e.preventDefault(); openCreate(); return; }
    if (e.ctrlKey && e.shiftKey && lower === "g") { e.preventDefault(); openDeep("git"); return; }
    if (e.ctrlKey && e.shiftKey && lower === "e") {
      e.preventDefault();
      const id = st().selectedTaskId;
      if (id && view() === "attention" && page() === "task") setSections({ ...sections(), [id]: [1, 2, 3, 4, 5, 6] });
      return;
    }
    if (e.ctrlKey && (key === "`" || e.code === "Backquote")) {
      e.preventDefault();
      if (view() === "deep" && surface() === "code") setTerminalOpen(!terminalOpen());
      else { setTerminalOpen(true); openDeep("code"); }
      return;
    }
    if (e.altKey && key === "ArrowLeft") { e.preventDefault(); back(); return; }
    if (key === "F6") { e.preventDefault(); cycleRegion(e.shiftKey ? -1 : 1); return; }
    if (key === "F1") { e.preventDefault(); setDialog({ kind: "help" }); return; }
    if (key === "Escape" && inFocus) {
      e.preventDefault();
      if (view() === "deep") railBack?.focus(); else if (view() === "attention") focusQueue();
      return;
    }
    if (e.altKey && !e.ctrlKey && /^Digit[1-6]$/.test(e.code) && inFocus && view() === "attention" && page() === "task") {
      const id = st().selectedTaskId;
      if (!id) return;
      e.preventDefault();
      const n = Number(e.code.slice(5));
      toggleSection(id, n);
      focusPane.querySelector<HTMLElement>(`[data-l2="${n}"]`)?.focus();
      return;
    }
    if (typing || e.ctrlKey || e.metaKey || e.altKey || !shortcuts()) { gPending = false; return; }
    if (key === "?") { e.preventDefault(); setDialog({ kind: "help" }); return; }
    if (gPending) {
      gPending = false;
      if (lower === "a") goAttention(); else if (lower === "t") openAll(); else if (lower === "c") openDeep("code"); else return;
      e.preventDefault();
      return;
    }
    if (lower === "g") { gPending = true; return; }
    if ((key === "[" || key === "]") && inFocus) { e.preventDefault(); moveHeading(key === "]" ? 1 : -1); }
  };
  onMount(() => document.addEventListener("keydown", onKey));
  onCleanup(() => document.removeEventListener("keydown", onKey));

  const scripts = SCENARIO_SCRIPTS;
  const currentScript = () => scripts[script()];
  const currentStep = () => currentScript().steps[step()];
  const connLabel = () => ({ connected: "Connected (mock)", offline: `Offline · as of ${clock(st().fixture.machines[0].lastConfirmedAt)}`, disconnected: "Disconnected" })[conn()];
  const connGlyph = () => ({ connected: "⏚", offline: "◷", disconnected: "⊗" })[conn()];

  const navItem = (label: string, active: boolean, onClick: () => void, hint: string) =>
    <button type="button" class="c-navbtn" aria-current={active ? "page" : undefined} aria-keyshortcuts={hint} onClick={onClick}>{label}</button>;

  return <Ctx.Provider value={ctx}>
    <div class="cfw-wrap" ref={wrap}>
      <div class="cfw">
        <a class="c-skip" href="#c-focus" onClick={e => { e.preventDefault(); focusEntry(); }}>Skip to main work area</a>
        <div class="c-strip" role="note" aria-label="Prototype status">
          <span><strong>PROTOTYPE</strong> · Fixture fx-{FIXTURE_VERSION} · Scenario: {st().scenario}, step {step() + 1} of {currentScript().steps.length} ({currentScript().id}) · All actions are mock and change only in-memory state</span>
          <span class="c-strip-controls">
            <label>Script <select value={script()} onChange={e => { setScript(Number(e.currentTarget.value)); setStep(0); }}>
              <For each={scripts}>{(s, i) => <option value={i()}>{s.id}</option>}</For>
            </select></label>
            <button type="button" class="c-mini" disabled={step() === 0} onClick={() => setStep(step() - 1)}>Previous step</button>
            <button type="button" class="c-mini" disabled={step() >= currentScript().steps.length - 1} onClick={() => setStep(step() + 1)}>Next step</button>
            <button type="button" class="c-mini" aria-pressed={loading()} onClick={() => setLoading(!loading())}>Preview loading</button>
          </span>
          <span class="c-strip-step">Scenario step {step() + 1} of {currentScript().steps.length} (fixture): {currentStep().action} <span class="c-mono">{currentStep().target}</span> — expected: {currentStep().expected}</span>
        </div>
        <header class="c-topbar" ref={topbar}>
          <h1 class="c-product">Focus Workspace</h1>
          <div class="c-project">
            <button type="button" class="c-navbtn" aria-expanded={projectMenu()} aria-controls="c-project-menu" onClick={() => setProjectMenu(!projectMenu())}
              onKeyDown={e => { if (e.key === "Escape") setProjectMenu(false); }}>{st().fixture.projects[0].name} <span aria-hidden="true">▾</span></button>
            <Show when={projectMenu()}>
              <ul id="c-project-menu" class="c-popover c-plain" onKeyDown={e => { if (e.key === "Escape") { setProjectMenu(false); (topbar.querySelector(".c-project button") as HTMLElement)?.focus(); } }}>
                <li><span class="c-muted">Current project: {st().fixture.projects[0].name} (fixture)</span></li>
                <li><button type="button" class="c-link" onClick={() => openPage("connection")}>Connection settings</button></li>
                <li><button type="button" class="c-link" onClick={() => openPage("settings")}>Settings: keyboard and text size</button></li>
              </ul>
            </Show>
          </div>
          <nav class="c-mainnav" aria-label="Views">
            {navItem("Attention", view() === "attention" && page() === "task", goAttention, "G A")}
            {navItem("All tasks", view() === "all", openAll, "G T")}
            {navItem("Code", view() === "deep" && (surface() === "code" || surface() === "git"), () => openDeep("code"), "G C")}
          </nav>
          <button type="button" class="c-navbtn c-search" aria-keyshortcuts="Control+K" onClick={() => setDialog({ kind: "palette" })}>Search <span class="c-kbd" aria-hidden="true">Ctrl+K</span></button>
          <button type="button" class={`c-navbtn c-conn conn-${conn()}`} aria-current={page() === "connection" && view() === "attention" ? "page" : undefined}
            onClick={() => openPage("connection")}><span aria-hidden="true">{connGlyph()} </span>{connLabel()}</button>
          <button type="button" class="c-btn c-primary" aria-label="New task (mock)" aria-keyshortcuts="Alt+N" onClick={openCreate}>New task<span class="c-mock-tag" aria-hidden="true"> · Mock</span></button>
          <button type="button" class="c-navbtn" aria-keyshortcuts="?" onClick={() => setDialog({ kind: "help" })}>Help</button>
        </header>
        <div class="c-body" classList={{ deep: view() === "deep", all: view() === "all" }}>
          <Switch>
            <Match when={view() === "attention"}>
              <Queue items={queue()} selectedId={st().selectedTaskId} loading={loading()} error={store.error()} asOf={asOf()}
                quietOpen={quietOpen()} onToggleQuiet={() => setQuietOpen(!quietOpen())} onOpen={openTask} onAllTasks={openAll}
                shortcutsOn={shortcuts()} register={fn => { focusQueue = fn; }} />
            </Match>
            <Match when={view() === "deep"}>
              <nav class="c-rail" aria-label="Attention queue (collapsed)">
                <p class="c-rail-count"><strong>{needCount()}</strong> need you</p>
                <button type="button" ref={railBack} class="c-railback" aria-keyshortcuts="Alt+ArrowLeft"
                  aria-label={`Back to attention queue, ${needCount()} decisions need you`} onClick={goAttention}>
                  <span aria-hidden="true">←</span> Back
                </button>
              </nav>
            </Match>
          </Switch>
          <section id="c-focus" ref={focusPane} class="c-focus" tabindex="-1"
            aria-label={view() === "all" ? "All tasks" : view() === "deep" ? "Deep mode" : "Focus pane"}>
            <Show when={conn() === "disconnected"}>
              <div class="c-banner danger" role="status">
                <p><strong>Machine disconnected.</strong> Last confirmed {asOf()}. Current states are unknown. Actions are unavailable.</p>
                <div class="c-actions">
                  <button type="button" class="c-btn" aria-label="Reconnect (mock)" onClick={() => setReceipts({ ...receipts(), banner: "Mock outcome: no reconnect was attempted. Choose the Default scenario to leave the disconnected fixture." })}>Reconnect<span class="c-mock-tag" aria-hidden="true"> · Mock</span></button>
                  <button type="button" class="c-link" onClick={() => openPage("connection")}>Connection</button>
                </div>
                <Show when={receipts().banner}><p class="c-receipt">{receipts().banner}</p></Show>
              </div>
            </Show>
            <Show when={conn() === "offline"}>
              <div class="c-banner neutral" role="status"><p><strong>As of {asOf()}.</strong> Showing the last confirmed state from {asOf()}. Read-only; actions are unavailable.</p></div>
            </Show>
            <div class="c-focus-inner" classList={{ wide: view() !== "attention" }}>
              <Switch>
                <Match when={view() === "all"}><AllTasks mode={allMode()} setMode={setAllMode} loading={loading()} /></Match>
                <Match when={view() === "deep"}><DeepView surface={surface()} task={selectedTask()} onSurface={s => { setSurface(s); afterRender(focusEntry); }} /></Match>
                <Match when={page() === "create"}><CreatePage onCreated={id => { setPage("task"); store.selectTask(id); afterRender(focusEntry); }} /></Match>
                <Match when={page() === "connection"}><ConnectionPage /></Match>
                <Match when={page() === "settings"}><SettingsPage scale={scale()} setScale={setScale} shortcuts={shortcuts()} setShortcuts={setShortcuts} /></Match>
                <Match when={store.error()}><ErrorState /></Match>
                <Match when={loading()}>
                  <div class="c-loading" role="status"><p class="c-title-lg">Loading tasks…</p>
                    <div class="c-skel" aria-hidden="true"><span /><span /></div><div class="c-skel" aria-hidden="true"><span /><span /></div></div>
                </Match>
                <Match when={!store.visibleTasks().length}><EmptyProject /></Match>
                <Match when={!selectedTask()}>
                  <div class="c-empty"><h2 id="c-focus-heading" class="c-title-lg" tabindex="-1">Choose an item from the attention queue</h2>
                    <p>{needCount()} decision(s) need you. Press Enter on a queue row to show it here.</p></div>
                </Match>
                <Match when={selectedTask()} keyed>{task => <TaskView task={task} />}</Match>
              </Switch>
            </div>
          </section>
        </div>
        <footer class="c-status" ref={status} tabindex="-1" aria-label="Status bar">
          <span><span aria-hidden="true">{connGlyph()} </span>Connection: {connLabel()}</span>
          <span>Fixture {FIXTURE_VERSION} · {st().fixture.tasks.length} tasks · {st().fixture.events.length.toLocaleString("en-US")} events</span>
          <span class="c-mocktext">Fixture data, not runtime evidence · Mock</span>
        </footer>
        <div class="c-sr" aria-live="polite">{announcement()}</div>
        <Show when={dialog()}>{d => <Switch>
          <Match when={d().kind === "confirm" && d() as Extract<Dialog, { kind: "confirm" }>}>{c => <Modal label={c().request.title} initialFocus=".c-safe" onClose={() => closeDialog(false)}>
            <h2 class="c-title-lg">{c().request.title}</h2>
            <p><strong>Effect:</strong> {c().request.effect}</p>
            <p><strong>Scope:</strong> {c().request.scope}</p>
            <div class="c-actions">
              <button type="button" class="c-btn c-safe" onClick={() => closeDialog(false)}>Cancel</button>
              <button type="button" class="c-btn c-danger-btn" aria-label={`${c().request.confirmLabel} (mock)`} onClick={() => closeDialog(true)}>{c().request.confirmLabel}<span class="c-mock-tag" aria-hidden="true"> · Mock</span></button>
            </div>
          </Modal>}</Match>
          <Match when={d().kind === "help"}><Modal label="Keyboard reference" onClose={() => closeDialog()}><Help onClose={() => closeDialog()} /></Modal></Match>
          <Match when={d().kind === "palette"}><Modal label="Search commands and tasks" initialFocus="input" onClose={() => closeDialog()}>
            <Palette onRun={action => { closeDialog(); action(); }} commands={[
              ["Attention", goAttention], ["All tasks", openAll], ["All tasks: Board", () => { setAllMode("board"); openAll(); }],
              ["Code", () => openDeep("code")], ["Git", () => openDeep("git")], ["New task (mock)", openCreate],
              ["Connection settings", () => openPage("connection")], ["Settings: keyboard and text size", () => openPage("settings")],
              ["Keyboard help", () => setDialog({ kind: "help" })],
            ]} />
          </Modal></Match>
        </Switch>}</Show>
      </div>
    </div>
  </Ctx.Provider>;

  function Palette(p: { onRun: (action: () => void) => void; commands: [string, () => void][] }) {
    const [q, setQ] = createSignal("");
    const commands = () => {
      const needle = q().toLowerCase();
      const hits = p.commands.filter(([label]) => label.toLowerCase().includes(needle));
      return [...hits.filter(([l]) => l.toLowerCase().startsWith(needle)), ...hits.filter(([l]) => !l.toLowerCase().startsWith(needle))];
    };
    const tasks = createMemo(() => {
      const needle = q().trim().toLowerCase();
      if (needle.length < 2) return [];
      return store.visibleTasks().filter(t => t.id.includes(needle) || t.title.toLowerCase().includes(needle)).slice(0, 20);
    });
    const first = () => commands()[0]?.[1] ?? (tasks()[0] ? () => openTask(tasks()[0].id) : null);
    return <div class="c-palette">
      <label for="c-palette-input" class="c-label">Search commands and tasks</label>
      <input id="c-palette-input" value={q()} onInput={e => setQ(e.currentTarget.value)}
        onKeyDown={e => { if (e.key === "Enter") { const run = first(); if (run) { e.preventDefault(); p.onRun(run); } } }} />
      <h2 class="c-h4">Commands</h2>
      <ul class="c-plain"><For each={commands()}>{([label, action]) => <li><button type="button" class="c-link" onClick={() => p.onRun(action)}>{label}</button></li>}</For></ul>
      <Show when={q().trim().length >= 2}>
        <h2 class="c-h4">Tasks ({tasks().length}{tasks().length === 20 ? "+" : ""})</h2>
        <ul class="c-plain"><For each={tasks()}>{t => <li><button type="button" class="c-link" onClick={() => p.onRun(() => openTask(t.id))}>{t.title} <span class="c-mono c-muted">{t.id}</span></button></li>}</For></ul>
      </Show>
      <Show when={!commands().length && !tasks().length}><p role="status">No commands or tasks match. Try fewer characters.</p></Show>
    </div>;
  }
}

function Help(p: { onClose: () => void }) {
  const rows: [string, string][] = [
    ["J / K or ↓ / ↑", "Queue: next / previous item (focus only; selection does not change)"],
    ["Enter", "Queue row: show in the focus pane and move focus to the decision heading"],
    ["[ / ]", "Focus pane: previous / next detail section heading"],
    ["Alt+1 … Alt+6", "Focus pane: toggle detail section n and focus it"],
    ["Ctrl+Shift+E", "Focus pane: expand all detail sections"],
    ["G A / G T / G C", "Attention / All tasks / Code (character shortcuts can be turned off in Settings)"],
    ["Ctrl+N or Alt+N", "New task (Ctrl+N is reserved by most browsers; Alt+N works in the browser)"],
    ["Ctrl+K", "Search commands and tasks"],
    ["Ctrl+Shift+G / Ctrl+`", "Git / terminal in Code deep mode"],
    ["Alt+←", "Back: from deep mode to the queue, or from All tasks to the previous view"],
    ["Esc", "Focus pane: return focus to the queue row"],
    ["F6 / Shift+F6", "Cycle regions: top bar, queue, focus pane, status bar"],
    ["? or F1", "This keyboard reference"],
  ];
  return <div>
    <h2 class="c-title-lg">Keyboard reference</h2>
    <p class="c-muted">Decision actions are reached with Tab only; there are no single-key decision shortcuts and no drag-and-drop.</p>
    <table class="c-table"><thead><tr><th scope="col">Keys</th><th scope="col">Action</th></tr></thead>
      <tbody><For each={rows}>{([k, a]) => <tr><th scope="row" class="c-mono">{k}</th><td>{a}</td></tr>}</For></tbody></table>
    <div class="c-actions"><button type="button" class="c-btn" onClick={() => p.onClose()}>Close</button></div>
  </div>;
}
