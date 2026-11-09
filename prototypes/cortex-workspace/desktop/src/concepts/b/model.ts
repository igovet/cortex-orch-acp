import { batch, createMemo, createSignal } from "solid-js";
import { createStore, produce } from "solid-js/store";
import type { Fixture, Scenario } from "../../fixtures";
import { SCENARIO_SCRIPTS } from "../../scenarios";
import type { MockStore } from "../../store";
import { fmtUtc, RESULT_CHANGES, START_FILE, WORKSPACE_FILES, type WsFile } from "./data";

export type Task = Fixture["tasks"][number];
export type Decision = Fixture["decisions"][number];
export type Check = Fixture["checks"][number];
export type Result = Fixture["results"][number];
export type Execution = Fixture["executions"][number];
export type FixtureEvent = Fixture["events"][number];

export type SideView = "files" | "search" | "git" | "mission" | "board" | "settings";
export type Sub = "discussion" | "decisions" | "result" | "workers";
export type G2Kind = "task" | "board" | "new" | "settings" | "terminal" | "problems" | "output" | "history" | "merge" | "diff";
export type G2Tab = { key: string; kind: G2Kind; preview: boolean; taskId?: string; sub?: Sub; resultId?: string; path?: string };
export type G1Tab = { path: string; preview: boolean };
export type BottomTab = "terminal" | "problems" | "output";
export type Overlay = null | "commands" | "files" | "keys";
export type Tone = "neutral" | "info" | "warn" | "danger" | "ok" | "mock";
export type Confirm = { title: string; effect: string; scope: string; confirmLabel: string; run: () => void };
export type NeedKind = "Question" | "Permission" | "Result acceptance";
export type NeedItem = { kind: NeedKind; id: string; taskId: string; title: string; sub: Sub };
export type Receipt = { id: number; where: string; text: string };
export type ExecInfo = { text: string; tone: Tone; glyph: "half" | "pause" | "minus" | "x" | "question" | "slash"; wait: string | null; prov: string };

const raf = (f: () => void) => (typeof requestAnimationFrame === "function" ? requestAnimationFrame(() => f()) : setTimeout(f, 16));
export const focusLater = (selector: string) => raf(() => document.querySelector<HTMLElement>(selector)?.focus());

const STEP_HINTS: Record<string, string> = {
  create: "Press Ctrl+Shift+N (or Mission Control → New task), enter a title, then Create task · Mock.",
  decide: "Alt+D focuses the next decision in Mission Control; Enter opens it in the task's Decisions sub-tab.",
  inspectResult: "Open the task from Mission Control and choose the Result & diff sub-tab.",
  acceptResult: "In Result & diff, use Accept · Mock below the evidence.",
  compare: "Use Compare in the editor conflict banner or in the Git view.",
  resolveConflict: "In the Compare tab choose Keep local · Mock, then Mark resolved · Mock in the Git view.",
  selectScenario: "Apply fixture step switches the shared fixture state.",
  setDevice: "Apply fixture step simulates switching device; the desktop view is preserved.",
  selectTask: "Apply fixture step selects the shared task without opening anything.",
};

export function createBModel(store: MockStore) {
  const st = () => store.state();
  const fx = () => st().fixture;
  const originalFontSize = document.documentElement.style.fontSize;
  const [ui, setUi] = createStore({
    side: "files" as SideView,
    sideVisible: true,
    expanded: { src: true, "src/auth": true, "src/routes": false, synthetic: true, tests: false } as Record<string, boolean>,
    g1: [{ path: START_FILE, preview: false }] as G1Tab[],
    g1Active: START_FILE as string | null,
    g2: [] as G2Tab[],
    g2Active: null as string | null,
    split: false,
    bottomOpen: true,
    bottomTab: "terminal" as BottomTab,
    buffers: {} as Record<string, string>,
    saved: {} as Record<string, string>,
    reloaded: {} as Record<string, boolean>,
    conflictBanner: null as string | null,
    textScale: 1,
    loading: false,
    scriptIndex: 0,
    step: 0,
    receipts: [] as Receipt[],
    resultActions: {} as Record<string, string>,
    cancelRequested: {} as Record<string, boolean>,
    gitResolved: {} as Record<string, boolean>,
    staged: [] as string[],
    commitMessage: "",
    pushUnknown: false,
    phoneAnswered: {} as Record<string, string>,
    collapsed: {} as Record<string, boolean>,
    boardMode: "auto" as "auto" | "board" | "list",
    query: "",
    regex: false,
    charKeys: true,
    newTask: { title: "", description: "", context: "" },
    createdId: null as string | null,
    output: null as string | null,
    enrolled: false,
  });
  const [overlay, setOverlay] = createSignal<Overlay>(null);
  const [confirm, setConfirm] = createSignal<Confirm | null>(null);
  const [rootPx, setRootPx] = createSignal(16);
  let receiptSeq = 0;

  // ---- fixture-derived state -------------------------------------------------
  const scenario = () => st().scenario;
  const conn = (): "connected" | "offline" | "disconnected" => scenario() === "disconnected" ? "disconnected" : scenario() === "offline" ? "offline" : "connected";
  const machine = () => fx().machines[0];
  const lastConfirmed = () => fmtUtc(machine().lastConfirmedAt);
  const block = (): string | null => ui.loading ? "Loading; actions unavailable"
    : conn() === "disconnected" ? "Disconnected; actions unavailable"
    : conn() === "offline" ? "Offline: showing the last confirmed state; actions unavailable" : null;
  const tasks = createMemo(() => store.visibleTasks());
  const taskMap = createMemo(() => new Map(tasks().map(t => [t.id, t])));
  const events = createMemo(() => fx().events);
  const eventsByTask = createMemo(() => {
    const out = new Map<string, FixtureEvent[]>();
    for (const e of events()) { let list = out.get(e.taskId); if (!list) out.set(e.taskId, list = []); list.push(e); }
    return out;
  });
  const eventsFor = (taskId: string) => eventsByTask().get(taskId) ?? [];
  const execFor = (taskId: string): Execution | null => fx().executions.find(e => e.taskId === taskId) ?? null;
  const resultFor = (taskId: string): Result | null => fx().results.find(r => r.taskId === taskId) ?? null;
  const accepted = (resultId: string) => st().acceptedResultIds.includes(resultId);
  const needsReview = (r: Result) => r.state === "complete" && !accepted(r.id) && !ui.resultActions[r.id];
  const checksFor = (taskId: string): Check[] => {
    const r = resultFor(taskId);
    const ids = new Set<string>(r ? r.checkIds : []);
    return fx().checks.filter(c => c.taskId === taskId || ids.has(c.id));
  };
  const checkCounts = (checks: Check[]) => ({
    passed: checks.filter(c => c.outcome === "passed").length,
    failed: checks.filter(c => c.outcome === "failed").length,
    notRun: checks.filter(c => c.outcome === "not_run").length,
    unverified: 0,
  });
  const isCreated = (taskId: string) => taskId.startsWith("mock-created-task");

  function execInfo(taskId: string): ExecInfo {
    const e = execFor(taskId);
    const r = resultFor(taskId);
    if (!e) return { text: isCreated(taskId) ? "Not started (mock)" : "Not started", tone: "neutral", glyph: "minus", wait: null, prov: isCreated(taskId) ? "mock" : "fixture" };
    const live = e.state === "running" || e.state === "blocked" || e.state === "waiting";
    if (conn() !== "connected" && live) return { text: "Unknown", tone: "neutral", glyph: "question", wait: null, prov: `last confirmed ${lastConfirmed()}` };
    switch (e.state) {
      case "running": return { text: ui.cancelRequested[e.id] ? "Running · cancel requested (mock)" : "Running", tone: "info", glyph: "half", wait: null, prov: "observed · fixture" };
      case "blocked": return { text: "Waiting", tone: "warn", glyph: "pause", wait: st().responses["mock-decision-permission"] ? null : "Your permission needed", prov: "observed · fixture" };
      case "waiting": return { text: "Waiting", tone: "warn", glyph: "pause", wait: st().responses["mock-decision-question"] ? null : "Your answer needed", prov: "observed · fixture" };
      case "failed": return { text: "Failed", tone: "danger", glyph: "x", wait: null, prov: "observed · fixture" };
      default: return { text: "Idle", tone: "neutral", glyph: "minus", wait: r && needsReview(r) ? "Result review needed" : null, prov: "execution complete · fixture" };
    }
  }

  const decisions = () => fx().decisions.filter(d => taskMap().has(d.taskId));
  const decisionsFor = (taskId: string) => decisions().filter(d => d.taskId === taskId);
  const needsYou = createMemo((): NeedItem[] => {
    if (ui.loading) return [];
    const out: NeedItem[] = [];
    for (const d of decisions()) if (!st().responses[d.id]) out.push({ kind: d.kind === "question" ? "Question" : "Permission", id: d.id, taskId: d.taskId, title: d.title, sub: "decisions" });
    for (const r of fx().results) if (taskMap().has(r.taskId) && needsReview(r)) out.push({ kind: "Result acceptance", id: r.id, taskId: r.taskId, title: `Review result ${r.id}, revision ${r.version}`, sub: "result" });
    const order: NeedKind[] = ["Question", "Permission", "Result acceptance"];
    return out.sort((a, b) => order.indexOf(a.kind) - order.indexOf(b.kind));
  });
  const running = () => fx().executions.filter(e => e.state === "running" && taskMap().has(e.taskId));
  const changed = createMemo(() => {
    const out: { key: string; taskId: string; text: string; tone: Tone; sub: Sub }[] = [];
    for (const c of fx().checks) if (c.outcome === "failed" && taskMap().has(c.taskId)) out.push({ key: c.id, taskId: c.taskId, text: `Check failed: ${c.name}`, tone: "danger", sub: "result" });
    for (const r of fx().results) if (r.state === "stale" && taskMap().has(r.taskId)) out.push({ key: r.id, taskId: r.taskId, text: `Result changed after review: ${r.id}`, tone: "warn", sub: "result" });
    for (const [id, answer] of Object.entries(ui.phoneAnswered)) {
      const d = fx().decisions.find(x => x.id === id);
      if (d && taskMap().has(d.taskId)) out.push({ key: `phone:${id}`, taskId: d.taskId, text: `Answered on Phone (simulated): ${d.title} → ${answer} (mock)`, tone: "mock", sub: "decisions" });
    }
    for (const t of tasks()) if (isCreated(t.id)) out.push({ key: t.id, taskId: t.id, text: `Created (mock): ${t.title} · not started`, tone: "mock", sub: "discussion" });
    return out;
  });

  // ---- workspace files and Git ---------------------------------------------------
  const conflictFile = () => fx().files[0];
  const files = createMemo((): WsFile[] => [...WORKSPACE_FILES, { path: conflictFile().path, text: conflictFile().local }].sort((a, b) => a.path.localeCompare(b.path)));
  const original = (path: string) => files().find(f => f.path === path)?.text ?? "";
  const fileText = (path: string) => ui.buffers[path] ?? ui.saved[path] ?? original(path);
  const dirty = (path: string) => ui.buffers[path] !== undefined;
  const gitConflicts = () => scenario() === "conflict" ? fx().git.fileIds.map(id => fx().files.find(f => f.id === id)).filter((f): f is Fixture["files"][number] => !!f) : [];
  const unresolved = () => gitConflicts().filter(f => !ui.gitResolved[f.id]);
  const isConflicted = (path: string) => ui.conflictBanner === path || unresolved().some(f => f.path === path);
  const unstaged = createMemo(() => {
    const paths = new Set<string>((RESULT_CHANGES["mock-result-complete"] ?? []).map(c => c.path));
    for (const p of Object.keys(ui.saved)) paths.add(p);
    for (const f of gitConflicts()) paths.delete(f.path);
    for (const p of ui.staged) paths.delete(p);
    return [...paths].sort();
  });
  const commitBlock = () => block()
    ?? (unresolved().length ? `Merge stopped: ${unresolved().length} conflicted file${unresolved().length === 1 ? "" : "s"} unresolved` : null)
    ?? (ui.staged.length === 0 ? "Nothing staged" : null)
    ?? (!ui.commitMessage.trim() ? "Enter a commit message" : null);

  const search = createMemo(() => {
    const q = ui.query;
    if (!q) return { error: null as string | null, hits: [] as { path: string; line: number; text: string }[] };
    let test: (s: string) => boolean;
    if (ui.regex) {
      try { const re = new RegExp(q, "i"); test = s => re.test(s); }
      catch (e) { return { error: `Invalid regular expression: ${(e as Error).message}. Nothing was searched.`, hits: [] }; }
    } else {
      const needle = q.toLowerCase().replace(/\*/g, "");
      test = s => needle.length > 0 && s.toLowerCase().includes(needle);
    }
    const hits: { path: string; line: number; text: string }[] = [];
    for (const f of files()) {
      if (test(f.path)) hits.push({ path: f.path, line: 0, text: "File name match" });
      fileText(f.path).split("\n").forEach((l, i) => { if (test(l)) hits.push({ path: f.path, line: i + 1, text: l.trim() }); });
    }
    return { error: null, hits: hits.slice(0, 200) };
  });

  // ---- receipts and guarded mock actions ----------------------------------------------
  const addReceipt = (where: string, text: string) => setUi("receipts", r => [...r, { id: ++receiptSeq, where, text }]);
  const receiptsFor = (where: string) => ui.receipts.filter(r => r.where === where);
  function guard(where: string, run: () => string, extra: string | null = null) {
    if (block() ?? extra) return false;
    addReceipt(where, run());
    // The control that was used may disappear after the response; keep focus at the inline receipt instead of the page body.
    raf(() => {
      const active = document.activeElement as HTMLButtonElement | null;
      const lost = !active || active === document.body || active.disabled === true;
      if (lost) document.querySelector<HTMLElement>(`[data-receipts="${where}"]`)?.focus();
    });
    return true;
  }
  const askConfirm = (c: Confirm) => { if (!block()) setConfirm(c); };

  // ---- navigation -------------------------------------------------------------------
  const history: string[] = [];
  let historyIndex = -1;
  let navigating = false;
  const pushHistory = (loc: string) => {
    if (navigating || history[historyIndex] === loc) return;
    history.splice(historyIndex + 1);
    history.push(loc);
    historyIndex = history.length - 1;
  };
  const go = (loc: string) => {
    navigating = true;
    const [group, key] = [loc.slice(0, 2), loc.slice(3)];
    if (group === "g1" && ui.g1.some(t => t.path === key)) setUi("g1Active", key);
    if (group === "g2" && ui.g2.some(t => t.key === key)) setUi({ g2Active: key, split: true });
    navigating = false;
  };

  function showSide(view: SideView, focus = true) {
    setUi({ side: view, sideVisible: true });
    if (focus) focusLater(view === "search" ? "#cb-search-input" : "#cb-side-title");
  }
  function toggleSide() {
    const hiding = ui.sideVisible;
    const inside = !!document.activeElement?.closest("#cb-side");
    setUi("sideVisible", !hiding);
    if (hiding && inside) focusLater("#cb-g1");
  }
  function openFile(path: string, line = 0) {
    setUi(produce(s => {
      if (!s.g1.some(t => t.path === path)) s.g1.push({ path, preview: false });
      s.g1Active = path;
    }));
    pushHistory(`g1:${path}`);
    raf(() => {
      const ta = document.querySelector<HTMLTextAreaElement>("#cb-g1 textarea");
      if (!ta) return;
      ta.focus();
      if (line > 0) {
        const offset = fileText(path).split("\n").slice(0, line - 1).reduce((n, l) => n + l.length + 1, 0);
        ta.setSelectionRange(offset, offset);
      }
    });
  }
  function closeG1(path: string) {
    setUi(produce(s => {
      const i = s.g1.findIndex(t => t.path === path);
      if (i < 0) return;
      s.g1.splice(i, 1);
      if (s.g1Active === path) s.g1Active = s.g1[Math.min(i, s.g1.length - 1)]?.path ?? null;
      if (s.conflictBanner === path) s.conflictBanner = null;
    }));
  }
  function openG2(tab: G2Tab, focusSelector?: string) {
    setUi(produce(s => {
      const existing = s.g2.find(t => t.key === tab.key);
      if (existing) { if (!tab.preview) existing.preview = false; if (tab.sub) existing.sub = tab.sub; }
      else {
        const previewIndex = tab.preview ? s.g2.findIndex(t => t.preview) : -1;
        if (previewIndex >= 0) s.g2[previewIndex] = tab; else s.g2.push(tab);
      }
      s.g2Active = tab.key;
      s.split = true;
    }));
    pushHistory(`g2:${tab.key}`);
    if (focusSelector) focusLater(focusSelector);
    else raf(() => {
      // The invoking control can be replaced by the new tab's content; never leave focus on the page body.
      if (!document.activeElement || document.activeElement === document.body) document.querySelector<HTMLElement>("#cb-g2 [role=tab][aria-selected=true]")?.focus();
    });
  }
  const openTask = (taskId: string, pin: boolean, sub: Sub = "discussion") => {
    store.selectTask(taskId);
    openG2({ key: `task:${taskId}`, kind: "task", preview: !pin, taskId, sub });
  };
  function closeG2(key: string) {
    setUi(produce(s => {
      const i = s.g2.findIndex(t => t.key === key);
      if (i < 0) return;
      s.g2.splice(i, 1);
      if (s.g2Active === key) s.g2Active = s.g2[Math.min(i, s.g2.length - 1)]?.key ?? null;
      if (s.g2.length === 0) s.split = false;
    }));
    if (!ui.split) focusLater("#cb-g1 textarea");
  }
  const setSub = (key: string, sub: Sub) => setUi("g2", t => t.key === key, "sub", sub);
  const pin = (key: string) => setUi("g2", t => t.key === key, "preview", false);
  const toggleSplit = () => setUi("split", !ui.split);
  function openTerminal() {
    if (ui.split) openG2({ key: "terminal", kind: "terminal", preview: false }, "#cb-g2 .cb-term input");
    else { setUi({ bottomOpen: true, bottomTab: "terminal" }); focusLater("#cb-bottom .cb-term input"); }
  }
  function openPanel(tab: BottomTab) {
    if (ui.split) openG2({ key: tab, kind: tab, preview: false });
    else setUi({ bottomOpen: true, bottomTab: tab });
  }
  const openNewTask = (context = "") => {
    if (context) setUi("newTask", "context", context);
    openG2({ key: "new", kind: "new", preview: false }, "#cb-new-title");
  };
  const openBoard = () => openG2({ key: "board", kind: "board", preview: false }, "#cb-g2 [role=tab][aria-selected=true]");
  const openHistory = () => openG2({ key: "history", kind: "history", preview: false });
  const openSettings = () => openG2({ key: "settings", kind: "settings", preview: false }, "#cb-settings-title");
  const openMerge = () => openG2({ key: "merge", kind: "merge", preview: false }, "#cb-merge-title");
  const openDiff = (resultId: string, path: string) => openG2({ key: `diff:${resultId}:${path}`, kind: "diff", preview: false, resultId, path });

  function cycleTab(group: 1 | 2, dir: 1 | -1) {
    if (group === 1) {
      const i = ui.g1.findIndex(t => t.path === ui.g1Active);
      const n = ui.g1.length; if (!n) return;
      setUi("g1Active", ui.g1[(i + dir + n) % n].path);
    } else {
      const i = ui.g2.findIndex(t => t.key === ui.g2Active);
      const n = ui.g2.length; if (!n) return;
      setUi("g2Active", ui.g2[(i + dir + n) % n].key);
    }
  }
  function closeActive(group: 1 | 2) {
    if (group === 2 && ui.split && ui.g2Active) closeG2(ui.g2Active);
    else if (ui.g1Active) closeG1(ui.g1Active);
  }
  function focusNextDecision(fromStart = false) {
    const items = needsYou();
    setUi(produce(s => { s.side = "mission"; s.sideVisible = true; s.collapsed.needs = false; }));
    if (!items.length) { focusLater("#cb-side-title"); return; }
    const current = items.findIndex(n => n.id === document.activeElement?.getAttribute("data-need"));
    const i = fromStart || current < 0 ? 0 : (current + 1) % items.length;
    focusLater(`[data-need="${items[i].id}"]`);
  }
  const back = () => { if (historyIndex > 0) go(history[--historyIndex]); };
  const forward = () => { if (historyIndex < history.length - 1) go(history[++historyIndex]); };

  // ---- editor --------------------------------------------------------------------------
  const edit = (path: string, text: string) => setUi("buffers", path, text);
  function save(path: string | null) {
    if (!path) return;
    const reason = block();
    if (reason) { addReceipt(`editor:${path}`, `Save unavailable: ${reason}.`); return; }
    if (ui.conflictBanner === path) { addReceipt(`editor:${path}`, "Save is disabled while the file conflict is open. Use Compare, Reload or Save as."); return; }
    if (scenario() === "conflict" && path === conflictFile().path && !ui.reloaded[path]) { setUi("conflictBanner", path); return; }
    setUi(produce(s => { s.saved[path] = fileText(path); delete s.buffers[path]; }));
    addReceipt(`editor:${path}`, `Mock outcome: ${path} saved in memory only. No file was written.`);
  }
  function reload(path: string) {
    setUi(produce(s => { s.saved[path] = conflictFile().incoming; delete s.buffers[path]; s.conflictBanner = null; s.reloaded[path] = true; }));
    addReceipt(`editor:${path}`, "Mock outcome: the buffer now shows the fixture's incoming text; your unsaved edits were discarded from memory. No file was read or written.");
  }
  const saveAs = (path: string) => guard(`editor:${path}`, () => `Mock outcome: no file was written. Save as would keep ${path} unchanged and write a copy.`);

  // ---- mock decisions, results and execution ----------------------------------------------------
  const decisionBlock = (d: Decision) => block() ?? (ui.phoneAnswered[d.id] ? "Answered on another device" : null);
  const answer = (d: Decision, value: string) => guard(`decision:${d.id}`, () => `Mock outcome: ${store.decide(d.id, value).message} Recorded in memory only; no agent was started.`, decisionBlock(d));
  const acceptBlock = (r: Result) => block() ?? (r.state !== "complete" ? "Result is stale: the workspace changed after this result" : null);
  const accept = (r: Result) => guard(`result:${r.id}`, () => `Mock outcome: ${store.acceptResult(r.id).message}`, acceptBlock(r));
  const requestChanges = (key: string) => guard(`result:${key}`, () => {
    setUi("resultActions", key, "Changes requested (mock)");
    return "Mock outcome: change request recorded in memory only. No agent was started.";
  });
  const askReject = (r: Result) => askConfirm({
    title: `Reject result ${r.id}?`,
    effect: "Records a mock rejection in memory. Nothing is reverted and no agent is started.",
    scope: `Result ${r.id}, revision ${r.version}, artifact digest ${r.artifactDigest}`,
    confirmLabel: "Reject result",
    run: () => guard(`result:${r.id}`, () => { setUi("resultActions", r.id, "Rejected (mock)"); return "Mock outcome: rejection recorded in memory only. Nothing was reverted."; }),
  });
  const askCancel = (e: Execution) => askConfirm({
    title: `Cancel execution ${e.id}?`,
    effect: "Records a mock cancel request in memory. No process is signalled; the fixture state is unchanged.",
    scope: `Execution ${e.id} · session ${e.sessionId} · task ${e.taskId}`,
    confirmLabel: "Cancel execution",
    run: () => guard(`exec:${e.id}`, () => { setUi("cancelRequested", e.id, true); return "Mock outcome: cancel request recorded in memory only. No agent or process was contacted."; }),
  });
  const openLog = (c: Check) => { setUi("output", `${c.name}: ${c.detail} (fixture log text)`); openPanel("output"); };
  const createBlock = () => block()
    ?? (store.error() ? "The task list failed to load (fixture error); reset the mock store to recover" : null)
    ?? (!ui.newTask.title.trim() ? "Enter a title to create a task" : null);
  function createTask() {
    if (createBlock()) return;
    const out = store.createTask(ui.newTask.title);
    batch(() => { setUi("createdId", store.state().selectedTaskId); setUi("newTask", { title: "", description: "", context: "" }); });
    addReceipt("new", `Mock outcome: ${out.message} No agent was started.`);
  }

  // ---- Git, connection and terminal mocks -----------------------------------------------------------------
  const resolveConflict = (choice: "local" | "incoming") => guard("merge", () => `Mock outcome: ${store.resolveConflict(choice).message}`);
  const markResolved = (fileId: string, path: string) => guard("git", () => { setUi("gitResolved", fileId, true); return `Mock outcome: ${path} marked resolved in memory only. No Git command ran.`; });
  const askAbortMerge = () => askConfirm({
    title: "Abort the merge?",
    effect: "Records a mock abort request in memory. No Git command runs and the fixture conflict stays as recorded.",
    scope: `Branch ${fx().git.branch} · ${gitConflicts().length} conflicted file(s)`,
    confirmLabel: "Abort merge",
    run: () => guard("git", () => "Mock outcome: abort request recorded in memory only. No Git command ran; the fixture merge state is unchanged."),
  });
  const stage = (path: string) => guard("git", () => { setUi("staged", s => [...s, path]); return `Mock outcome: ${path} staged in memory only. No Git command ran.`; });
  const commit = () => guard("git", () => { setUi("commitMessage", ""); return "Mock outcome: no commit was created; nothing was written to Git."; }, commitBlock());
  const pull = () => guard("git", () => "Mock outcome: no fetch or merge ran. The fixture is unchanged.");
  const askPush = () => askConfirm({
    title: `Push ${fx().git.branch} to origin?`,
    effect: "Simulates the ambiguous push outcome. Nothing is sent to any remote.",
    scope: `Branch ${fx().git.branch} → origin/${fx().git.branch}`,
    confirmLabel: "Push",
    run: () => guard("git", () => { setUi("pushUnknown", true); return "Mock outcome: nothing was sent to a remote. The fixture shows how an unconfirmed push is presented."; }),
  });
  const inspectRemote = () => guard("git", () => "Mock outcome: no remote was contacted. Check the remote yourself before trying again.");
  const reconnect = () => { if (!ui.loading) addReceipt("settings", "Mock outcome: no reconnect was attempted; the fixture state is unchanged. Choose the Default fixture state to view a connected machine."); };
  const testConnection = () => guard("settings", () => "Mock outcome: no connection test ran; no network request was made.");
  const enroll = () => guard("settings", () => { setUi("enrolled", true); return "Mock outcome: enrollment recorded in memory only. No device credential was created."; });
  const askRevoke = () => askConfirm({
    title: "Revoke this device?",
    effect: "Records a mock revocation in memory. No credential exists or is changed.",
    scope: "This desktop prototype device (synthetic)",
    confirmLabel: "Revoke device",
    run: () => guard("settings", () => { setUi("enrolled", false); return "Mock outcome: revocation recorded in memory only."; }),
  });
  const runCommand = (command: string) => guard("terminal", () => `Mock outcome: no command was run (${command || "empty input"}). There is no shell process.`);

  // ---- scenario scripts --------------------------------------------------------------------
  const script = () => SCENARIO_SCRIPTS[ui.scriptIndex];
  const currentStep = () => script().steps[ui.step];
  const stepHint = () => STEP_HINTS[currentStep().action] ?? "";
  const stepApplicable = () => {
    const a = currentStep().action;
    return a === "selectScenario" || a === "setDevice" || a === "selectTask" || (a === "decide" && st().device === "phone");
  };
  function applyStep() {
    const step = currentStep();
    if (step.action === "selectScenario") store.selectScenario(step.target as Scenario);
    else if (step.action === "setDevice") store.setDevice(step.target as "desktop" | "phone");
    else if (step.action === "selectTask") store.selectTask(step.target);
    else if (step.action === "decide" && st().device === "phone") {
      const target = step.target;
      store.decide(target, "Compact");
      setUi("phoneAnswered", target, "Compact");
    }
  }
  const setScript = (i: number) => setUi({ scriptIndex: i, step: 0 });
  const moveStep = (dir: 1 | -1) => setUi("step", s => Math.max(0, Math.min(script().steps.length - 1, s + dir)));

  // ---- text size -----------------------------------------------------------------------------
  const measureRoot = () => setRootPx(parseFloat(getComputedStyle(document.documentElement).fontSize) || 16);
  function setTextScale(scale: number) {
    document.documentElement.style.fontSize = scale === 1 ? originalFontSize : `${scale * 100}%`;
    setUi("textScale", scale);
    measureRoot();
  }
  const dispose = () => { document.documentElement.style.fontSize = originalFontSize; };

  return {
    store, ui, setUi, st, fx, scenario, conn, machine, lastConfirmed, block, tasks, taskMap, events, eventsFor,
    execFor, execInfo, resultFor, accepted, checksFor, checkCounts, isCreated, decisions, decisionsFor, needsYou, running, changed,
    files, fileText, dirty, conflictFile, gitConflicts, unresolved, isConflicted, unstaged, commitBlock, search,
    receiptsFor, overlay, setOverlay, confirm, setConfirm, rootPx, measureRoot, setTextScale, dispose,
    showSide, toggleSide, openFile, closeG1, openG2, openTask, closeG2, setSub, pin, toggleSplit, openTerminal, openPanel,
    openNewTask, openBoard, openHistory, openSettings, openMerge, openDiff, cycleTab, closeActive, focusNextDecision, back, forward,
    edit, save, reload, saveAs, decisionBlock, answer, acceptBlock, accept, requestChanges, askReject, askCancel, openLog,
    createBlock, createTask, resolveConflict, markResolved, askAbortMerge, stage, commit, pull, askPush, inspectRemote,
    reconnect, testConnection, enroll, askRevoke, runCommand,
    script, currentStep, stepHint, stepApplicable, applyStep, setScript, moveStep,
    setExpanded: (path: string, open: boolean) => setUi("expanded", path, open),
    setQuery: (q: string) => setUi("query", q),
    setRegex: (on: boolean) => setUi("regex", on),
    setLoading: (on: boolean) => setUi("loading", on),
    toggleSection: (id: string) => setUi("collapsed", id, !ui.collapsed[id]),
  };
}
export type BModel = ReturnType<typeof createBModel>;
