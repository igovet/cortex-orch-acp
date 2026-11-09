import { createMemo, createSignal, For, Show } from "solid-js";
import { FIXTURE_VERSION, SCENARIOS, type Concept, type Scenario } from "../../fixtures";
import { SCENARIO_SCRIPTS } from "../../scenarios";
import type { BModel, Confirm } from "./model";
import { Glyph, Modal } from "./ui";

export const KEYMAP: { keys: string; action: string }[] = [
  { keys: "Ctrl+Shift+M", action: "Mission Control view" },
  { keys: "Ctrl+Shift+E / F / G / B", action: "Files / Search / Git / Board views" },
  { keys: "Ctrl+P", action: "Quick open file" },
  { keys: "Ctrl+K", action: "Command palette (tasks, decisions, commands)" },
  { keys: "Ctrl+B", action: "Toggle side panel" },
  { keys: "Ctrl+\\", action: "Split or unsplit the editor" },
  { keys: "Ctrl+`", action: "Terminal: bottom panel with one group, group-2 tab with two groups" },
  { keys: "Ctrl+Tab / Ctrl+PageDown, Ctrl+Shift+Tab / Ctrl+PageUp", action: "Next / previous tab in the focused group" },
  { keys: "Ctrl+W or Ctrl+F4, Delete on a tab", action: "Close the tab" },
  { keys: "Alt+D", action: "Focus the next pending decision in Mission Control (focus only, no action)" },
  { keys: "Ctrl+Shift+N", action: "New task" },
  { keys: "Ctrl+S", action: "Save the file in group 1 (mock)" },
  { keys: "Ctrl+1 / Ctrl+2", action: "Focus editor group 1 / group 2" },
  { keys: "F6 / Shift+F6", action: "Cycle regions: activity bar, side panel, group 1, group 2 or bottom panel, status bar" },
  { keys: "Alt+← / Alt+→", action: "Back / forward through opened tabs" },
  { keys: "Arrows, Enter, → / ←", action: "Move in trees and lists; open; expand or collapse" },
  { keys: "F1 or ?", action: "This keyboard reference (? only outside text fields; can be turned off in Settings)" },
];

function goConcept(concept: Concept) {
  history.pushState(null, "", `/${concept}`);
  window.dispatchEvent(new PopStateEvent("popstate"));
}

export function PrototypeStrip(props: { m: BModel }) {
  const m = props.m;
  const [guide, setGuide] = createSignal(false);
  let toggle!: HTMLButtonElement;
  const close = () => { setGuide(false); toggle.focus(); };
  return <header class="cb-strip" aria-label="Prototype strip">
    <h1 class="cb-strip-title">IDE with Mission Control</h1>
    <p class="cb-strip-text"><strong>PROTOTYPE</strong> · Fixture {FIXTURE_VERSION} · State: {m.scenario()}{m.ui.loading ? " (loading)" : ""} · Scenario: {m.script().id}, step {m.ui.step + 1} of {m.script().steps.length} · All actions are mock and change only in-memory state · target <span class="cb-mono">{m.st().selectedTaskId || "none"}</span></p>
    <button ref={toggle} type="button" class="cb-btn" aria-expanded={guide()} aria-controls="cb-guide" onClick={() => setGuide(!guide())}>Prototype controls</button>
    <Show when={guide()}>
      <div id="cb-guide" class="cb-guide" role="region" aria-label="Prototype controls" onKeyDown={e => { if (e.key === "Escape") { e.preventDefault(); close(); } }}>
        <div class="cb-strip-controls">
          <label class="cb-field">Fixture state <select value={m.scenario()} onChange={e => m.store.selectScenario(e.currentTarget.value as Scenario)}>
            <For each={SCENARIOS}>{s => <option value={s}>{s}</option>}</For>
          </select></label>
          <label class="cb-check"><input type="checkbox" checked={m.ui.loading} onChange={e => m.setLoading(e.currentTarget.checked)} /> Loading (simulated)</label>
          <button type="button" class="cb-btn" onClick={() => m.store.reset()}>Reset mock store</button>
          <nav aria-label="Concepts" class="cb-concepts">
            <a href="/a" onClick={e => { e.preventDefault(); goConcept("a"); }}>Concept A</a>
            <a href="/b" aria-current="page">Concept B</a>
            <a href="/c" onClick={e => { e.preventDefault(); goConcept("c"); }}>Concept C</a>
          </nav>
        </div>
        <label class="cb-field">Script <select value={m.ui.scriptIndex} onChange={e => m.setScript(Number(e.currentTarget.value))}>
          <For each={SCENARIO_SCRIPTS}>{(s, i) => <option value={i()}>{s.id}</option>}</For>
        </select></label>
        <ol class="cb-guide-steps">
          <For each={m.script().steps}>{(s, i) => <li aria-current={i() === m.ui.step ? "step" : undefined} classList={{ current: i() === m.ui.step }}>
            <span class="cb-mono">{s.action}</span> → <span class="cb-mono">{s.target}</span> ({s.device}): {s.expected}
          </li>}</For>
        </ol>
        <p><strong>Scenario step {m.ui.step + 1} of {m.script().steps.length} (fixture).</strong> {m.stepHint()}</p>
        <div class="cb-actions-btns">
          <button type="button" class="cb-btn" disabled={m.ui.step === 0} onClick={() => m.moveStep(-1)}>Previous step</button>
          <button type="button" class="cb-btn" disabled={!m.stepApplicable()} onClick={() => m.applyStep()}>Apply fixture step</button>
          <button type="button" class="cb-btn" disabled={m.ui.step >= m.script().steps.length - 1} onClick={() => m.moveStep(1)}>Next step</button>
          <button type="button" class="cb-btn" onClick={close}>Close (Esc)</button>
        </div>
        <p class="cb-muted">Steps advance only through these controls. Nothing advances on a timer. Fixture data is not runtime evidence.</p>
      </div>
    </Show>
  </header>;
}

export function StatusBar(props: { m: BModel }) {
  const m = props.m;
  const [roving, setRoving] = createSignal(0);
  let bar!: HTMLDivElement;
  const onKey = (e: KeyboardEvent) => {
    const items = [...bar.querySelectorAll<HTMLButtonElement>("button")];
    const i = items.indexOf(document.activeElement as HTMLButtonElement);
    if (i < 0) return;
    const j = e.key === "ArrowRight" ? (i + 1) % items.length : e.key === "ArrowLeft" ? (i - 1 + items.length) % items.length : e.key === "Home" ? 0 : e.key === "End" ? items.length - 1 : -1;
    if (j < 0) return;
    e.preventDefault();
    setRoving(j);
    items[j].focus();
  };
  const tab = (i: number) => (roving() === i ? 0 : -1);
  const n = () => m.needsYou().length;
  const conflicts = () => m.unresolved().length;
  return <div id="cb-status" class="cb-status" role="toolbar" aria-label="Status bar" tabindex="-1" ref={bar} onKeyDown={onKey}>
    <button type="button" class="cb-sb" tabindex={tab(0)} onFocus={() => setRoving(0)} onClick={() => m.showSide("git")} aria-label={`Branch ${m.fx().git.branch}, open Git view`}>
      <Glyph name="branch" /> {m.fx().git.branch}
    </button>
    <Show when={conflicts() > 0}>
      <button type="button" class="cb-sb t-warn-text" tabindex={tab(1)} onFocus={() => setRoving(1)} onClick={() => m.showSide("git")}>
        <Glyph name="warn" /> {conflicts()} conflict{conflicts() === 1 ? "" : "s"}
      </button>
    </Show>
    <button type="button" class="cb-sb" classList={{ "t-warn-text": n() > 0 }} tabindex={tab(2)} onFocus={() => setRoving(2)} onClick={() => m.focusNextDecision(true)}>
      Needs you {m.ui.loading ? "…" : n()}
    </button>
    <button type="button" class="cb-sb" tabindex={tab(3)} onFocus={() => setRoving(3)} onClick={() => { m.showSide("mission"); }}>
      Running {m.ui.loading ? "…" : m.conn() === "connected" ? m.running().length : "Unknown"}
    </button>
    <button type="button" class="cb-sb" classList={{ "t-danger-text": m.conn() === "disconnected" }} tabindex={tab(4)} onFocus={() => setRoving(4)} onClick={() => m.openSettings()}>
      <Glyph name={m.conn() === "disconnected" ? "plugoff" : m.conn() === "offline" ? "clock" : "plug"} />{" "}
      {m.conn() === "disconnected" ? `Disconnected · last confirmed ${m.lastConfirmed()}` : m.conn() === "offline" ? `As of ${m.lastConfirmed()} (cached)` : "Connected (mock)"}
    </button>
    <span class="cb-sb-text">Fixture data, not runtime evidence</span>
    <span class="cb-sb-text t-mock-text"><Glyph name="flask" /> Mock</span>
    <Show when={m.st().device === "phone"}><span class="cb-sb-text t-mock-text">Simulated: phone active; desktop view preserved</span></Show>
  </div>;
}

type Pick = { label: string; detail?: string; run: () => void };

function PickList(props: { m: BModel; label: string; placeholder: string; items: (q: string) => Pick[] }) {
  const [q, setQ] = createSignal("");
  const [active, setActive] = createSignal(0);
  const list = createMemo(() => props.items(q()).slice(0, 40));
  const close = () => props.m.setOverlay(null);
  const run = (p: Pick | undefined) => {
    if (!p) return;
    close();
    requestAnimationFrame(() => p.run());
  };
  const onKey = (e: KeyboardEvent) => {
    if (e.key === "ArrowDown") setActive(a => Math.min(list().length - 1, a + 1));
    else if (e.key === "ArrowUp") setActive(a => Math.max(0, a - 1));
    else if (e.key === "Enter") run(list()[active()]);
    else return;
    e.preventDefault();
  };
  return <Modal label={props.label} onClose={close}>
    <h2 class="cb-group-h">{props.label}</h2>
    <input data-autofocus type="text" class="cb-pick-input" role="combobox" aria-expanded="true" aria-controls="cb-pick-list" aria-autocomplete="list"
      aria-activedescendant={list().length ? `cb-pick-${active()}` : undefined} aria-label={props.placeholder} placeholder={props.placeholder}
      value={q()} onInput={e => { setQ(e.currentTarget.value); setActive(0); }} onKeyDown={onKey} />
    <ul id="cb-pick-list" role="listbox" class="cb-pick-list" aria-label={props.label}>
      <For each={list()} fallback={<li class="cb-muted">No matches.</li>}>{(p, i) =>
        <li id={`cb-pick-${i()}`} role="option" aria-selected={i() === active()} classList={{ active: i() === active() }} onClick={() => run(p)}>
          <span>{p.label}</span><Show when={p.detail}><span class="cb-muted"> · {p.detail}</span></Show>
        </li>}
      </For>
    </ul>
    <p class="cb-muted">Arrow keys move, Enter runs, Escape closes.</p>
  </Modal>;
}

export function Overlays(props: { m: BModel }) {
  const m = props.m;
  const match = (q: string) => (s: string) => s.toLowerCase().includes(q.toLowerCase());
  const commands = (): Pick[] => [
    { label: "View: Mission Control", detail: "Ctrl+Shift+M", run: () => m.showSide("mission") },
    { label: "View: Files", detail: "Ctrl+Shift+E", run: () => m.showSide("files") },
    { label: "View: Search", detail: "Ctrl+Shift+F", run: () => m.showSide("search") },
    { label: "View: Git", detail: "Ctrl+Shift+G", run: () => m.showSide("git") },
    { label: "View: Board", detail: "Ctrl+Shift+B", run: () => m.showSide("board") },
    { label: "View: Settings", run: () => m.showSide("settings") },
    { label: "Open board tab", run: () => m.openBoard() },
    { label: "New task", detail: "Ctrl+Shift+N", run: () => m.openNewTask() },
    { label: "Project history (10,000 events)", run: () => m.openHistory() },
    { label: "Focus next pending decision", detail: "Alt+D", run: () => m.focusNextDecision() },
    { label: "Toggle side panel", detail: "Ctrl+B", run: () => m.toggleSide() },
    { label: "Split or unsplit editor", detail: "Ctrl+\\", run: () => m.toggleSplit() },
    { label: "Terminal", detail: "Ctrl+`", run: () => m.openTerminal() },
    { label: "Problems", run: () => m.openPanel("problems") },
    { label: "Output", run: () => m.openPanel("output") },
    { label: "Connection settings", run: () => m.openSettings() },
    { label: "Save file (mock)", detail: "Ctrl+S", run: () => m.save(m.ui.g1Active) },
    { label: "Focus editor group 1", detail: "Ctrl+1", run: () => document.querySelector<HTMLElement>("#cb-g1 textarea, #cb-g1")?.focus() },
    { label: "Focus editor group 2", detail: "Ctrl+2", run: () => document.querySelector<HTMLElement>("#cb-g2 [role=tab][aria-selected=true], #cb-g2")?.focus() },
    { label: "Next tab in group 1", detail: "Ctrl+PageDown", run: () => m.cycleTab(1, 1) },
    { label: "Next tab in group 2", run: () => m.cycleTab(2, 1) },
    { label: "Close tab in group 2", run: () => m.closeActive(2) },
    { label: "Back", detail: "Alt+←", run: () => m.back() },
    { label: "Forward", detail: "Alt+→", run: () => m.forward() },
    { label: "Keyboard reference", detail: "F1", run: () => m.setOverlay("keys") },
  ];
  const paletteItems = (q: string): Pick[] => {
    const cmds = commands().filter(c => match(q)(c.label));
    const decisions = m.needsYou().filter(n => match(q)(n.title) || match(q)(n.taskId))
      .map(n => ({ label: `Decision: ${n.title}`, detail: `${n.kind} · ${n.taskId}`, run: () => m.openTask(n.taskId, true, n.sub) }));
    const tasks = q.length >= 2 ? m.tasks().filter(t => match(q)(t.title) || match(q)(t.id)).slice(0, 20)
      .map(t => ({ label: `Task: ${t.title}`, detail: t.id, run: () => m.openTask(t.id, true) })) : [];
    return [...cmds, ...decisions, ...tasks];
  };
  const fileItems = (q: string): Pick[] => m.files().filter(f => match(q)(f.path)).map(f => ({ label: f.path, run: () => m.openFile(f.path) }));
  return <>
    <Show when={m.overlay() === "commands"}><PickList m={m} label="Command palette" placeholder="Type a command, task or decision" items={paletteItems} /></Show>
    <Show when={m.overlay() === "files"}><PickList m={m} label="Quick open" placeholder="Type a file name" items={fileItems} /></Show>
    <Show when={m.overlay() === "keys"}>
      <Modal label="Keyboard reference" wide onClose={() => m.setOverlay(null)}>
        <h2 class="cb-group-h">Keyboard reference</h2>
        <table class="cb-table"><thead><tr><th scope="col">Keys</th><th scope="col">Action</th></tr></thead>
          <tbody><For each={KEYMAP}>{k => <tr><th scope="row"><kbd>{k.keys}</kbd></th><td>{k.action}</td></tr>}</For></tbody></table>
        <p class="cb-muted">Decision actions have no single-key shortcuts. Browsers reserve some combinations (for example Ctrl+W, Ctrl+Tab and Ctrl+Shift+N in Chrome); use the alternatives or the command palette.</p>
        <button type="button" class="cb-btn" data-autofocus onClick={() => m.setOverlay(null)}>Close</button>
      </Modal>
    </Show>
    <Show when={m.confirm()}>{c => <ConfirmDialog m={m} c={c()} />}</Show>
  </>;
}

function ConfirmDialog(props: { m: BModel; c: Confirm }) {
  const close = () => props.m.setConfirm(null);
  return <Modal label={props.c.title} onClose={close}>
    <h2 class="cb-group-h">{props.c.title}</h2>
    <dl class="cb-dec-dl">
      <dt>Effect</dt><dd>{props.c.effect}</dd>
      <dt>Scope</dt><dd>{props.c.scope}</dd>
    </dl>
    <div class="cb-actions-btns">
      <button type="button" class="cb-btn" data-autofocus onClick={close}>Cancel</button>
      <button type="button" class="cb-btn danger" aria-label={`${props.c.confirmLabel} (mock)`} onClick={() => { const run = props.c.run; close(); run(); }}>
        {props.c.confirmLabel} <span class="cb-mock-tag" aria-hidden="true">· Mock</span>
      </button>
    </div>
  </Modal>;
}
