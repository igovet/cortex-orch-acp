import { createEffect, createMemo, createSignal, Match, on, onCleanup, onMount, Show, Switch } from "solid-js";
import { createStore, reconcile } from "solid-js/store";
import { FIXTURE_VERSION } from "../../fixtures";
import { SCENARIO_SCRIPTS } from "../../scenarios";
import type { ConceptProps } from "../../store";
import { Board, TaskTable, type Pos, type TableApi } from "./board";
import { CodeTab } from "./code";
import { DeskContext, receiptId, type ConfirmOpts, type Desk, type Pane, type Ui } from "./ctx";
import { Glyph } from "./glyphs";
import {
  applyFilters, buildEventIndex, COLUMNS, connOf, deriveViews, EXEC_LABELS, hhmm, needsYou, stamp, TABS, WAIT_LABELS,
  type Filters, type TabId,
} from "./model";
import { ChangesSince, ConnectionPane, CreatePane, Guide, HelpDialog, NeedsPane, Palette, PhoneSim } from "./panes";
import { Badge, ConfirmDialog, ConnBanner, MockTag } from "./ui";
import { EmptyBlock, TaskWorkspace } from "./workspace";
import "./a.css";

const initialUi = (): Ui => ({
  editor: {}, saveConflict: false, compare: false, reloaded: false, saveAs: false, markedResolved: false,
  terminalOpen: true, terminal: [], codeMode: "files", query: "", regex: false,
  requests: {}, push: {}, phoneAnswers: [], changesSince: [],
});
const SCALES = [1, 1.25, 1.5, 2];

/** Concept A: Task Desk. Board-first; a task opens beside a compact list with tabs for every task surface. */
export default function ConceptA(props: ConceptProps) {
  performance.mark?.("a-desk-start");
  const store = props.store;
  const [home, setHome] = createSignal<"board" | "list">("board");
  const [pane, setPaneSig] = createSignal<Pane | null>(null);
  const [collapsed, setCollapsed] = createSignal(false);
  const [scale, setScale] = createSignal(1);
  // Measured text unit (rem × preset) and viewport height: large text switches to the scrolling layout
  // whether it comes from the preset, a browser text-size setting or zoom.
  const [unitPx, setUnitPx] = createSignal(16);
  const [viewH, setViewH] = createSignal(typeof innerHeight === "number" ? innerHeight : 800);
  const large = () => scale() >= 1.5 || unitPx() >= 24 || viewH() / unitPx() < 45;
  const [filters, setFilters] = createStore<Filters>({ phase: "all", wait: "all", exec: "all" });
  const [pos, setPos] = createSignal<Pos>({ col: 1, idx: 0 });
  const [restore, setRestore] = createSignal(false);
  const [drafts, setDrafts] = createSignal<ReadonlySet<string>>(new Set());
  const [receipts, setReceipts] = createStore<Record<string, string>>({});
  const [confirmOpts, setConfirmOpts] = createSignal<ConfirmOpts | null>(null);
  const [dialog, setDialog] = createSignal<"palette" | "help" | null>(null);
  const [guideOpen, setGuideOpen] = createSignal(false);
  const [script, setScript] = createSignal(0);
  const [step, setStep] = createSignal(0);
  const [loading, setLoading] = createSignal(false);
  const [gseq, setGseq] = createSignal(true);
  const [menu, setMenu] = createSignal(false);
  const [ui, setUi] = createStore<Ui>(initialUi());
  let topEl!: HTMLElement, filterEl!: HTMLDivElement, statusEl!: HTMLElement, menuBtn!: HTMLButtonElement;
  let primaryEl: HTMLElement | undefined, wsEl: HTMLDivElement | undefined;
  let listApi: TableApi | undefined, homeListApi: TableApi | undefined;

  const conn = () => connOf(store.state().scenario);
  const eventsRef = createMemo(() => store.state().fixture.events);
  const eventIndex = createMemo(() => buildEventIndex(eventsRef()));
  const allViews = createMemo(() => deriveViews(store.state(), eventIndex(), drafts(), conn()));
  const visible = createMemo(() => store.visibleTasks().length ? allViews() : []);
  const filtered = createMemo(() => applyFilters(visible(), filters));
  const byId = createMemo(() => new Map(allViews().map(v => [v.id, v] as const)));
  const needs = createMemo(() => needsYou(visible()).filter(n => n.actionable));
  const block = () => loading() ? "Loading; actions unavailable." : conn() === "disconnected" ? "Disconnected; actions unavailable."
    : conn() === "offline" ? "Offline (cached state); actions unavailable." : store.error() ? "Fixture read error; actions unavailable." : null;
  const focusSoon = (id: string) => { setTimeout(() => document.getElementById(id)?.focus(), 0); };

  const posOf = (id: string): Pos | null => {
    const v = byId().get(id);
    if (!v) return null;
    const idx = filtered().filter(x => x.column === v.column).findIndex(x => x.id === id);
    return idx < 0 ? null : { col: COLUMNS.findIndex(c => c.id === v.column), idx };
  };
  const openTask = (id: string, tab: TabId = "discussion") => {
    store.selectTask(id);
    setRestore(false);
    setPaneSig({ kind: "task", tab });
    focusSoon("a-task-title");
  };
  const goHome = (view: "board" | "list") => {
    const p = posOf(store.state().selectedTaskId);
    if (p) setPos(p);
    setHome(view);
    setRestore(true);
    setPaneSig(null);
    if (view === "list") setTimeout(() => homeListApi?.focusTask(store.state().selectedTaskId), 0);
  };
  const setPane = (p: Pane | null) => {
    if (!p) return goHome(home());
    setPaneSig(p);
    focusSoon(p.kind === "create" ? "a-new-title" : p.kind === "needs" ? "a-needs-h" : p.kind === "connection" ? "a-conn-h" : p.kind === "code" ? "a-ed-h" : "a-task-title");
  };
  const setTab = (tab: TabId) => { if (pane()?.kind === "task") setPaneSig({ kind: "task", tab }); };
  const goCode = () => { if (pane()?.kind === "task") setTab("code"); else setPaneSig({ kind: "code" }); };
  const closeGuide = () => { setGuideOpen(false); focusSoon("a-guide-btn"); };
  const toggleList = () => {
    if (!pane()) return;
    setCollapsed(!collapsed());
    focusSoon(collapsed() ? "a-rail" : "a-list-toggle");
  };

  const desk: Desk = {
    store, views: allViews, visible, byId, events: eventIndex, conn, block, scale, pane, setPane, openTask, setTab,
    receipt: k => receipts[k],
    setReceipt: (k, m, f) => { setReceipts(k, m); if (f) focusSoon(receiptId(k)); },
    confirm: o => setConfirmOpts(o), ui, setUi,
    addDraft: id => setDrafts(new Set([...drafts(), id])), focusSoon,
  };

  // The shell's reset creates a new fixture; drop concept-local mock state with it.
  createEffect(on(eventsRef, () => {
    setUi(reconcile(initialUi()));
    setReceipts(reconcile({}));
    setDrafts(new Set<string>());
    setPaneSig(null);
  }, { defer: true }));
  // Simulated phone session: answers given there are listed on return, nothing auto-opens.
  let phoneStart = 0;
  createEffect(on(() => store.state().device, (device, prev) => {
    if (device === "phone") phoneStart = ui.phoneAnswers.length;
    else if (prev === "phone") setUi("changesSince", ui.phoneAnswers.slice(phoneStart));
  }, { defer: true }));

  const regions = () => [topEl, filterEl, primaryEl, wsEl, statusEl].filter((x): x is HTMLElement => !!x && x.isConnected);
  const cycle = (dir: 1 | -1) => {
    const list = regions(), active = document.activeElement;
    const cur = list.findIndex(r => r.contains(active));
    list[(cur + dir + list.length) % list.length]?.focus();
  };
  const returnToOrigin = () => {
    if (collapsed()) return document.getElementById("a-rail")?.focus();
    listApi?.focusTask(store.state().selectedTaskId);
  };
  let gPending = false;
  const editable = (t: EventTarget | null) => !!(t as HTMLElement | null)?.closest?.("input, textarea, select, [contenteditable='true']");
  const onKey = (e: KeyboardEvent) => {
    if (confirmOpts() || dialog() || store.state().device === "phone") return;
    const k = e.key, ctrl = e.ctrlKey || e.metaKey, lower = k.toLowerCase(), taskPane = pane()?.kind === "task";
    if (ctrl && !e.shiftKey && !e.altKey && lower === "k") { e.preventDefault(); setDialog("palette"); return; }
    if (ctrl && !e.shiftKey && !e.altKey && lower === "n") { e.preventDefault(); setPane({ kind: "create" }); return; }
    if (ctrl && k === "\\") { e.preventDefault(); toggleList(); return; }
    if (ctrl && e.shiftKey && lower === "g" && taskPane) { e.preventDefault(); setTab("git"); focusSoon("a-tab-git"); return; }
    if (ctrl && e.code === "Backquote") { e.preventDefault(); goCode(); setUi("terminalOpen", true); focusSoon("a-term-input"); return; }
    if (ctrl && !e.shiftKey && lower === "p") { e.preventDefault(); goCode(); setUi("codeMode", "files"); focusSoon("a-tree-file"); return; }
    if (ctrl && e.shiftKey && lower === "f") { e.preventDefault(); goCode(); setUi("codeMode", "search"); focusSoon("a-search-input"); return; }
    const digit = /^Digit([1-6])$/.exec(e.code);
    if (e.altKey && !ctrl && digit && taskPane) { e.preventDefault(); const t = TABS[Number(digit[1]) - 1].id; setTab(t); focusSoon(`a-tab-${t}`); return; }
    if (k === "F6") { e.preventDefault(); cycle(e.shiftKey ? -1 : 1); return; }
    if (k === "F1") { e.preventDefault(); setDialog("help"); return; }
    if (k === "Escape") {
      if (menu()) { setMenu(false); menuBtn.focus(); return; }
      if (guideOpen()) { closeGuide(); return; }
      if (taskPane && wsEl?.contains(document.activeElement)) { e.preventDefault(); returnToOrigin(); }
      return;
    }
    if (ctrl || e.altKey || e.defaultPrevented || editable(e.target)) return;
    if (k === "?") { e.preventDefault(); setDialog("help"); return; }
    if (!gseq()) return;
    if (gPending) {
      gPending = false;
      if (lower === "b" || lower === "l") { e.preventDefault(); goHome(lower === "b" ? "board" : "list"); }
      else if (lower === "d") { e.preventDefault(); setPane({ kind: "needs" }); }
      return;
    }
    if (lower === "g") gPending = true;
  };
  let probe!: HTMLSpanElement;
  onMount(() => {
    const measure = () => { setUnitPx(probe.getBoundingClientRect().height || 16); setViewH(innerHeight); };
    measure();
    if (typeof ResizeObserver !== "undefined") {
      const ro = new ResizeObserver(measure);
      ro.observe(probe);
      onCleanup(() => ro.disconnect());
    }
    window.addEventListener("resize", measure);
    onCleanup(() => window.removeEventListener("resize", measure));
    document.addEventListener("keydown", onKey);
    const done = () => {
      performance.mark?.("a-desk-painted");
      try { performance.measure?.("a-desk-first-render", "a-desk-start", "a-desk-painted"); } catch { /* marks unavailable */ }
    };
    if (typeof requestAnimationFrame === "function") requestAnimationFrame(() => requestAnimationFrame(done)); else done();
  });
  onCleanup(() => document.removeEventListener("keydown", onKey));

  const machine = () => store.state().fixture.machines[0];
  const scriptDef = () => SCENARIO_SCRIPTS[script()];

  const PrimaryState = (p: { compact: boolean }) => <Switch fallback={
    <Show when={p.compact || home() === "list"} fallback={
      <Board views={filtered()} loading={loading()} pos={pos()} setPos={setPos} restoreFocus={restore()} onOpen={id => openTask(id)} />}>
      <TaskTable views={filtered()} compact={p.compact} loading={loading()} onOpen={id => openTask(id)}
        api={a => { if (p.compact) listApi = a; else homeListApi = a; }} />
    </Show>}>
    <Match when={store.error()}>
      <div class="a-empty a-error" role="alert">
        <p class="a-empty-title"><Glyph name="x" /> Could not load tasks.</p>
        <p>Fixture error E_FIXTURE_READ: {store.error()}. Nothing was changed.</p>
        <button type="button" onClick={() => store.selectScenario("default")}>Try again (restores the Default scenario)</button>
      </div>
    </Match>
    <Match when={!loading() && !visible().length}>
      <EmptyBlock title="No tasks in this project yet." body="Create a task to start a discussion.">
        <button type="button" class="a-btn-primary" aria-label="New task (mock)" onClick={() => setPane({ kind: "create" })}>New task<MockTag /></button>
      </EmptyBlock>
    </Match>
    <Match when={!loading() && !filtered().length}>
      <EmptyBlock title="No tasks match these filters.">
        <button type="button" onClick={() => setFilters({ phase: "all", wait: "all", exec: "all" })}>Clear filters</button>
      </EmptyBlock>
    </Match>
  </Switch>;

  return <DeskContext.Provider value={desk}>
    <div class="a-desk" classList={{ "a-scale-large": large() }} style={{ "--s": String(scale()) }}>
      <span class="a-vmeasure" ref={probe} aria-hidden="true" />
      <a class="a-skip" href="#a-primary" onClick={e => { e.preventDefault(); (primaryEl ?? wsEl)?.focus(); }}>Skip to main work area</a>
      <div class="a-strip" role="note" aria-label="Prototype status">
        <Glyph name="flask" />
        <p><strong>PROTOTYPE</strong> · Fixture {FIXTURE_VERSION} · Scenario: {store.state().scenario}, step {step() + 1} of {scriptDef().steps.length} ({scriptDef().id}) · All actions are mock and change only in-memory state</p>
        <button type="button" class="a-strip-btn" id="a-guide-btn" aria-expanded={guideOpen()} aria-controls="a-guide" onClick={() => setGuideOpen(!guideOpen())}>Scenario guide</button>
        <Show when={guideOpen() && store.state().device !== "phone"}>
          <Guide script={script()} setScript={setScript} step={step()} setStep={setStep} loading={loading()} setLoading={setLoading} onClose={closeGuide} />
        </Show>
      </div>
      <nav class="a-top" aria-label="Task Desk" ref={topEl} tabindex="-1">
        <h1 class="a-brand">Task Desk</h1>
        <div class="a-menu-wrap">
          <button type="button" ref={menuBtn} aria-expanded={menu()} aria-controls="a-project-menu" onClick={() => setMenu(!menu())}>
            Project: {store.state().fixture.projects[0].name} ▾
          </button>
          <Show when={menu()}>
            <ul id="a-project-menu" class="a-menu" aria-label="Project">
              <li><button type="button" onClick={() => { setMenu(false); setPane({ kind: "connection" }); }}>Machine & connection</button></li>
              <li><button type="button" onClick={() => { setMenu(false); setPaneSig({ kind: "code" }); focusSoon("a-ed-h"); }}>Code (project)</button></li>
              <li><button type="button" onClick={() => { setMenu(false); setDialog("help"); }}>Keyboard reference</button></li>
            </ul>
          </Show>
        </div>
        <div class="a-seg" role="group" aria-label="Home view">
          <button type="button" aria-pressed={home() === "board"} onClick={() => goHome("board")}>Board</button>
          <button type="button" aria-pressed={home() === "list"} onClick={() => goHome("list")}>List</button>
        </div>
        <button type="button" onClick={() => setDialog("palette")}>Search tasks <span class="a-key">Ctrl+K</span></button>
        <span class="a-top-spacer" />
        <button type="button" class="a-needs" classList={{ "a-needs--on": needs().length > 0 }} aria-pressed={pane()?.kind === "needs"} onClick={() => setPane({ kind: "needs" })}>
          <Glyph name="message" /> Needs you <strong>{needs().length}</strong>
        </button>
        <span class="a-sr" role="status">{needs().length} decisions need you</span>
        <button type="button" class="a-btn-primary" aria-label="New task (mock)" onClick={() => setPane({ kind: "create" })}>New task<MockTag /></button>
        <label class="a-textsize">Text size
          <select value={String(scale())} onChange={e => setScale(Number(e.currentTarget.value))}>
            {SCALES.map(s => <option value={String(s)}>{s * 100}%</option>)}
          </select>
        </label>
        <button type="button" onClick={() => setDialog("help")}>Help</button>
      </nav>
      <div class="a-body">
        <ChangesSince />
        <div class="a-filters" ref={filterEl} tabindex="-1" role="group" aria-label="Filters">
          <label>Phase <select value={filters.phase} onChange={e => setFilters("phase", e.currentTarget.value as Filters["phase"])}>
            <option value="all">All</option>{COLUMNS.map(c => <option value={c.id}>{c.name}</option>)}
          </select></label>
          <label>Wait reason <select value={filters.wait} onChange={e => setFilters("wait", e.currentTarget.value as Filters["wait"])}>
            <option value="all">All</option><option value="none">Not waiting on you</option>{WAIT_LABELS.map(w => <option value={w}>{w}</option>)}
          </select></label>
          <label>Execution <select value={filters.exec} onChange={e => setFilters("exec", e.currentTarget.value as Filters["exec"])}>
            <option value="all">All</option>{EXEC_LABELS.map(x => <option value={x}>{x}</option>)}
          </select></label>
          <p class="a-muted">{loading() ? "Loading tasks…" : `Showing ${filtered().length.toLocaleString("en-US")} of ${visible().length.toLocaleString("en-US")} tasks`}</p>
          <Show when={loading()}><p role="status" class="a-sr">Loading tasks…</p></Show>
        </div>
        <Show when={pane()} fallback={
          <section class="a-home" id="a-primary" ref={el => { primaryEl = el; wsEl = undefined; }} tabindex="-1" aria-label={home() === "board" ? "Board" : "List"}>
            <ConnBanner area="home" />
            <PrimaryState compact={false} />
          </section>}>
          <div class="a-split" classList={{ "a-split--collapsed": collapsed() }}>
            <section class="a-listpane" id="a-primary" ref={el => { primaryEl = el; }} tabindex="-1" aria-label="Task list">
              <Show when={!collapsed()} fallback={
                <button type="button" class="a-rail" id="a-rail" aria-expanded="false" onClick={toggleList}>Tasks <span class="a-sr">(expand list, Ctrl+\)</span></button>}>
                <div class="a-listpane-head">
                  <h2 class="a-pane-h">Tasks</h2>
                  <button type="button" id="a-list-toggle" aria-expanded="true" onClick={toggleList}>Collapse list <span class="a-key">Ctrl+\</span></button>
                </div>
                <PrimaryState compact />
              </Show>
            </section>
            <div class="a-wsarea" ref={el => { wsEl = el; }} tabindex="-1">
              <Switch>
                <Match when={pane()?.kind === "task"}>
                  <Show when={byId().get(store.state().selectedTaskId)} fallback={
                    <section class="a-ws a-pane"><div class="a-pane-body"><EmptyBlock title="No task selected." body="Choose a task from the list." /></div></section>}>
                    {v => <TaskWorkspace v={v()} tab={(pane() as { tab: TabId }).tab} />}
                  </Show>
                </Match>
                <Match when={pane()?.kind === "needs"}><NeedsPane /></Match>
                <Match when={pane()?.kind === "create"}><CreatePane /></Match>
                <Match when={pane()?.kind === "connection"}><ConnectionPane /></Match>
                <Match when={pane()?.kind === "code"}>
                  <section class="a-ws a-pane" aria-label="Project code"><div class="a-pane-body"><CodeTab taskId={null} /></div></section>
                </Match>
              </Switch>
            </div>
          </div>
        </Show>
      </div>
      <footer class="a-status" ref={statusEl} tabindex="-1" aria-label="Status">
        <button type="button" class="a-status-conn" onClick={() => setPane({ kind: "connection" })}>
          <Show when={conn() === "connected"} fallback={<Badge tone={conn() === "disconnected" ? "danger" : "neutral"} glyph={conn() === "disconnected" ? "plugoff" : "clock"}>
            {conn() === "disconnected" ? "Disconnected" : `Offline · As of ${hhmm(machine().lastConfirmedAt)} (cached)`}</Badge>}>
            <Badge tone="neutral" glyph="plug">Connected (simulated)</Badge>
          </Show>
        </button>
        <span>{conn() === "connected" ? "Fixture state, no live connection" : `Last confirmed ${stamp(machine().lastConfirmedAt)}`}</span>
        <span>Fixture {FIXTURE_VERSION}</span>
        <span>Fixture data, not runtime evidence</span>
        <Badge tone="mock" glyph="flask">Mock</Badge>
      </footer>
      <Show when={confirmOpts()}>{o => <ConfirmDialog opts={o()} onClose={() => setConfirmOpts(null)} />}</Show>
      <Show when={dialog() === "palette"}><Palette onClose={() => setDialog(null)} /></Show>
      <Show when={dialog() === "help"}><HelpDialog onClose={() => setDialog(null)} gseq={gseq} setGseq={setGseq} /></Show>
      <Show when={store.state().device === "phone"}><PhoneSim /></Show>
    </div>
  </DeskContext.Provider>;
}
