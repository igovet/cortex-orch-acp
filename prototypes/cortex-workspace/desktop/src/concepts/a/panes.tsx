import { createMemo, createSignal, For, Show, type Accessor } from "solid-js";
import type { Scenario } from "../../fixtures";
import { SCENARIO_SCRIPTS } from "../../scenarios";
import { useDesk } from "./ctx";
import { Glyph } from "./glyphs";
import { needsYou, stamp, type Decision } from "./model";
import { Badge, ConnBanner, Dialog, MockButton, Receipt } from "./ui";
import { EmptyBlock } from "./workspace";

export function CreatePane() {
  const desk = useDesk();
  const [title, setTitle] = createSignal("");
  const [details, setDetails] = createSignal("");
  const [files, setFiles] = createSignal<string[]>([]);
  const [mode, setMode] = createSignal<"discuss" | "draft">("discuss");
  const [error, setError] = createSignal("");
  const submit = (e: Event) => {
    e.preventDefault();
    if (desk.block()) return;
    if (!title().trim()) { setError("Enter a title."); document.getElementById("a-new-title")?.focus(); return; }
    desk.store.createTask(title());
    const id = desk.store.state().selectedTaskId;
    if (mode() === "draft") desk.addDraft(id);
    desk.setReceipt(`task:${id}`, `Mock outcome: recorded in memory only. No agent was started.${mode() === "draft" ? " Saved as draft." : " Execution: Not started (mock)."}`);
    desk.openTask(id, "discussion");
  };
  return <section class="a-ws a-pane" aria-labelledby="a-create-h">
    <ConnBanner area="create" />
    <div class="a-pane-body">
      <h2 id="a-create-h" tabindex="-1">New task</h2>
      <p class="a-muted">Creates a task in memory only. No agent is started. The shared mock store records the title; details and context stay in this form.</p>
      <form class="a-form" onSubmit={submit} novalidate>
        <label for="a-new-title">Title (required)</label>
        <input id="a-new-title" value={title()} required aria-invalid={!!error()} aria-describedby={error() ? "a-new-title-err" : undefined}
          onInput={e => { setTitle(e.currentTarget.value); if (error()) setError(""); }} />
        <Show when={error()}><p id="a-new-title-err" class="a-error-text">{error()}</p></Show>
        <label for="a-new-details">Details (optional)</label>
        <textarea id="a-new-details" rows={4} value={details()} onInput={e => setDetails(e.currentTarget.value)} />
        <fieldset>
          <legend>Context files (optional)</legend>
          <For each={desk.store.state().fixture.files}>{f => <label class="a-check">
            <input type="checkbox" checked={files().includes(f.id)} onChange={e => setFiles(e.currentTarget.checked ? [...files(), f.id] : files().filter(x => x !== f.id))} />
            <span class="a-mono">{f.path}</span>
          </label>}</For>
        </fieldset>
        <fieldset class="a-radios">
          <legend>Start mode</legend>
          <label><input type="radio" name="a-mode" checked={mode() === "discuss"} onChange={() => setMode("discuss")} /> Discuss first</label>
          <label><input type="radio" name="a-mode" checked={mode() === "draft"} onChange={() => setMode("draft")} /> Save as draft</label>
        </fieldset>
        <div class="a-actions">
          <MockButton label="Create task" tone="primary" submit reason={desk.block()} />
          <button type="button" onClick={() => desk.setPane(null)}>Cancel</button>
        </div>
      </form>
    </div>
  </section>;
}

const KIND_GLYPH = { Permission: "key", Question: "message", "Result acceptance": "filecheck" } as const;

export function NeedsPane() {
  const desk = useDesk();
  const items = createMemo(() => needsYou(desk.visible()));
  const actionable = () => items().filter(i => i.actionable);
  const other = () => items().filter(i => !i.actionable);
  return <section class="a-ws a-pane" aria-labelledby="a-needs-h">
    <ConnBanner area="needs" />
    <div class="a-pane-body">
      <h2 id="a-needs-h" tabindex="-1">Needs you</h2>
      <p class="a-muted">Across all tasks. Opening an item does not act on it.</p>
      <Show when={actionable().length} fallback={<EmptyBlock title="Nothing needs your decision." body="Checked against the current fixture state.">
        <button type="button" onClick={() => desk.setPane(null)}>Show all tasks</button>
      </EmptyBlock>}>
        <ul class="a-need-list"><For each={actionable()}>{it => <li class="a-need">
          <p class="a-dec-type"><Glyph name={KIND_GLYPH[it.kind]} /> {it.kind}</p>
          <h3 id={`a-need-${it.key}`}>{it.title}</h3>
          <p class="a-muted">{it.view.title} · <span class="a-mono">{it.view.id}</span></p>
          <button type="button" aria-describedby={`a-need-${it.key}`} onClick={() => desk.openTask(it.view.id, it.tab)}>Open in task</button>
        </li>}</For></ul>
      </Show>
      <Show when={other().length}>
        <h3>Not actionable now</h3>
        <ul class="a-need-list"><For each={other()}>{it => <li class="a-need">
          <p class="a-dec-type"><Glyph name="history" /> {it.kind}</p>
          <h4>{it.title}</h4>
          <p>{it.note}</p>
          <button type="button" onClick={() => desk.openTask(it.view.id, it.tab)}>Open result</button>
        </li>}</For></ul>
      </Show>
    </div>
  </section>;
}

export function ConnectionPane() {
  const desk = useDesk();
  const machine = () => desk.store.state().fixture.machines[0];
  const cap = () => desk.store.state().fixture.capabilities[0];
  const [relay, setRelay] = createSignal("");
  const state = () => desk.conn() === "connected" ? { label: "Connected (simulated for this scenario)", tone: "neutral" as const, glyph: "plug" as const }
    : desk.conn() === "offline" ? { label: "Offline: showing cached state", tone: "neutral" as const, glyph: "clock" as const }
    : { label: "Disconnected", tone: "danger" as const, glyph: "plugoff" as const };
  const device = () => desk.store.state().device;
  return <section class="a-ws a-pane" aria-labelledby="a-conn-h">
    <ConnBanner area="conn" />
    <div class="a-pane-body">
      <h2 id="a-conn-h" tabindex="-1">Machine & connection</h2>
      <dl class="a-facts">
        <dt>Machine</dt><dd>{machine().name} <span class="a-mono a-muted">{machine().id}</span></dd>
        <dt>State</dt><dd><Badge tone={state().tone} glyph={state().glyph}>{state().label}</Badge></dd>
        <dt>Fixture machine record</dt><dd>{machine().state}; last confirmed {stamp(machine().lastConfirmedAt)}; current execution state {machine().currentExecutionState}</dd>
        <dt>Relay</dt><dd>
          <p>Default relay: not configured in the fixture.</p>
          <label for="a-relay">Relay override (not applied)</label>
          <input id="a-relay" class="a-mono" value={relay()} onInput={e => setRelay(e.currentTarget.value)} />
        </dd>
        <dt>Push notifications</dt><dd><Badge tone="neutral" glyph="slash">Unavailable</Badge> {cap().name}: {cap().reason}</dd>
      </dl>
      <h3>Devices</h3>
      <div class="a-scroll-x">
        <table class="a-grid-table">
          <thead><tr><th scope="col">Device</th><th scope="col">Role</th><th scope="col">Status</th><th scope="col">Action</th></tr></thead>
          <tbody>
            <tr><th scope="row">This desktop (simulated)</th><td>Current device</td><td>{device() === "desktop" ? "In use" : "Inactive"}</td><td>None</td></tr>
            <tr><th scope="row">Phone (simulated)</th><td>Paired device (mock)</td><td>{device() === "phone" ? "In use" : "Inactive"}</td>
              <td><MockButton label="Revoke" tone="danger" reason={desk.block()} onClick={() => desk.confirm({
                title: "Revoke the simulated phone?", confirmLabel: "Revoke device",
                effect: "Records a mock revocation in memory. No credential or device is changed.", scope: "Phone (simulated)",
                onConfirm: () => desk.setReceipt("conn", "Mock outcome: no device was revoked. Recorded in memory only.", true),
              })} /></td></tr>
          </tbody>
        </table>
      </div>
      <div class="a-actions">
        <MockButton label="Test connection" reason={desk.block()} onClick={() => desk.setReceipt("conn", "Mock outcome: no connection was attempted.", true)} />
      </div>
      <Receipt k="conn" />
    </div>
  </section>;
}

export function Palette(props: { onClose: () => void }) {
  const desk = useDesk();
  const [q, setQ] = createSignal("");
  const matches = createMemo(() => {
    const s = q().trim().toLowerCase();
    return s ? desk.visible().filter(v => v.title.toLowerCase().includes(s) || v.id.includes(s)) : desk.visible();
  });
  const shown = () => matches().slice(0, 12);
  const open = (id: string) => { props.onClose(); desk.openTask(id); };
  return <Dialog title="Search tasks" onClose={props.onClose} class="a-palette" initial="#a-palette-input">
    <label for="a-palette-input">Title or task ID</label>
    <input id="a-palette-input" type="search" value={q()} aria-describedby="a-palette-count" onInput={e => setQ(e.currentTarget.value)}
      onKeyDown={e => { if (e.key === "Enter" && shown()[0]) { e.preventDefault(); open(shown()[0].id); } }} />
    <p id="a-palette-count" class="a-muted" role="status">{matches().length} matching tasks{matches().length > 12 ? ", showing the first 12. Enter opens the first." : ""}</p>
    <Show when={shown().length} fallback={<p>No tasks match <code>{q()}</code>. Try fewer characters.</p>}>
      <ul class="a-palette-list"><For each={shown()}>{v => <li>
        <button type="button" onClick={() => open(v.id)}><span>{v.title}</span> <span class="a-mono a-muted">{v.id}</span></button>
      </li>}</For></ul>
    </Show>
  </Dialog>;
}

const KEYS: [string, string][] = [
  ["Ctrl+K", "Search tasks"], ["Ctrl+N", "New task"], ["G then B, L or D", "Board, List or Needs you (can be turned off below)"],
  ["Ctrl+\\", "Collapse or expand the task list"], ["Arrows, J/K, H/L", "Move between cards and columns while the board has focus"],
  ["Home / End", "First or last card in a column"], ["Enter", "Open the focused task; focus moves to its title"],
  ["Alt+1 … Alt+6", "Discussion, Decisions, Result, Code, Git, Workers"], ["Ctrl+Shift+G", "Git tab"], ["Ctrl+`", "Terminal in the Code tab"],
  ["Ctrl+P / Ctrl+Shift+F", "Files / file search in the Code tab"], ["Esc", "Return focus to the task in the list; the task stays open"],
  ["F6 / Shift+F6", "Move between regions"], ["? or F1", "This keyboard reference"],
];

export function HelpDialog(props: { onClose: () => void; gseq: Accessor<boolean>; setGseq: (v: boolean) => void }) {
  return <Dialog title="Keyboard reference" onClose={props.onClose}>
    <div class="a-scroll-x">
      <table class="a-grid-table">
        <thead><tr><th scope="col">Keys</th><th scope="col">Action</th></tr></thead>
        <tbody><For each={KEYS}>{([k, a]) => <tr><th scope="row"><kbd>{k}</kbd></th><td>{a}</td></tr>}</For></tbody>
      </table>
    </div>
    <label class="a-check"><input type="checkbox" checked={props.gseq()} onChange={e => props.setGseq(e.currentTarget.checked)} /> Enable G-sequences</label>
    <p class="a-muted">Decision actions have no single-key shortcuts. A browser may reserve some keys (for example Ctrl+N); every action also has a visible button.</p>
    <div class="a-actions"><button type="button" onClick={() => props.onClose()}>Close</button></div>
  </Dialog>;
}

export function Guide(props: {
  script: number; setScript: (n: number) => void; step: number; setStep: (n: number) => void;
  loading: boolean; setLoading: (v: boolean) => void; onClose: () => void;
}) {
  const desk = useDesk();
  const s = () => SCENARIO_SCRIPTS[props.script];
  const cur = () => s().steps[props.step];
  const applicable = () => ["selectScenario", "setDevice", "selectTask"].includes(cur().action);
  // A button that becomes disabled must not swallow focus: move it to a live control.
  const go = (n: number) => {
    props.setStep(n);
    const last = n >= s().steps.length - 1;
    if (n === 0 || last) desk.focusSoon(last ? (applicable() ? "a-guide-apply" : "a-guide-prev") : "a-guide-next");
  };
  const apply = () => {
    const st = cur();
    if (st.action === "selectScenario") desk.store.selectScenario(st.target as Scenario);
    else if (st.action === "setDevice") desk.store.setDevice(st.target as "desktop" | "phone");
    else if (st.action === "selectTask") desk.openTask(st.target);
  };
  return <div class="a-guide" id="a-guide" role="group" aria-label="Scenario guide"
    onKeyDown={e => { if (e.key === "Escape") { e.preventDefault(); e.stopPropagation(); props.onClose(); } }}>
    <h2 class="a-guide-h">Scenario guide</h2>
    <label>Script <select value={String(props.script)} onChange={e => { props.setScript(Number(e.currentTarget.value)); props.setStep(0); }}>
      <For each={SCENARIO_SCRIPTS}>{(x, i) => <option value={String(i())}>{x.id}</option>}</For>
    </select></label>
    <ol class="a-guide-steps"><For each={s().steps}>{(st, i) => <li aria-current={i() === props.step ? "step" : undefined}>
      <strong>{st.device}: {st.action} {st.target}</strong> <span class="a-muted">Expected: {st.expected}</span>
    </li>}</For></ol>
    <p>Scenario step {props.step + 1} of {s().steps.length} (fixture). Steps never advance on their own: perform the step in the desk, then choose Next step.</p>
    <div class="a-actions">
      <button type="button" id="a-guide-prev" disabled={props.step === 0} onClick={() => go(props.step - 1)}>Previous step</button>
      <button type="button" id="a-guide-next" disabled={props.step >= s().steps.length - 1} onClick={() => go(props.step + 1)}>Next step</button>
      <Show when={applicable()}><button type="button" id="a-guide-apply" onClick={apply}>Apply: {cur().action} {cur().target}</button></Show>
    </div>
    <label class="a-check"><input type="checkbox" checked={props.loading} onChange={e => props.setLoading(e.currentTarget.checked)} /> Preview the loading state (stays until unchecked; no timer)</label>
    <div class="a-actions"><button type="button" onClick={() => props.onClose()}>Close guide</button></div>
  </div>;
}

export function PhoneSim() {
  const desk = useDesk();
  const open = () => desk.views().flatMap(v => v.openDecisions.map(d => ({ d, v })));
  const options = (d: Decision) => "options" in d ? d.options : ["Approve once", "Deny"];
  const answer = (d: Decision, a: string) => {
    desk.store.decide(d.id, a);
    desk.setUi("phoneAnswers", x => [...x, d.id]);
    desk.setReceipt("phone", `Mock outcome: answered on the simulated phone (${a}). Recorded in shared memory only.`, true);
  };
  const back = () => { desk.store.setDevice("desktop"); desk.focusSoon("a-guide-btn"); };
  return <Dialog title="Simulated phone session" onClose={back}>
    <p>This desk is inactive while the simulated phone is in use. Its open task and view are preserved. This is not a physical device or synchronization.</p>
    <Show when={open().length} fallback={<p class="a-muted">No open decisions.</p>}>
      <ul class="a-need-list"><For each={open()}>{({ d, v }) => <li class="a-need">
        <p class="a-dec-type"><Glyph name={d.kind === "question" ? "message" : "key"} /> {d.kind === "question" ? "Question" : "Permission"}</p>
        <h3>{d.title}</h3>
        <p class="a-muted">{v.title} · <span class="a-mono">{v.id}</span></p>
        <div class="a-actions"><For each={options(d)}>{o => <MockButton label={`Answer ${o} on phone`} reason={desk.block()} onClick={() => answer(d, o)} />}</For></div>
      </li>}</For></ul>
    </Show>
    <Receipt k="phone" />
    <div class="a-actions"><button type="button" onClick={back}>Return to desktop</button></div>
  </Dialog>;
}

export function ChangesSince() {
  const desk = useDesk();
  const items = () => desk.ui.changesSince.map(id => {
    const d = desk.store.state().fixture.decisions.find(x => x.id === id);
    return d ? { d, answer: desk.store.state().responses[id] } : null;
  }).filter(Boolean) as { d: Decision; answer: string }[];
  return <Show when={items().length}>
    <section class="a-changes" aria-labelledby="a-changes-h">
      <h2 id="a-changes-h">Changes since you were last here</h2>
      <ul><For each={items()}>{it => <li>
        {it.d.kind === "question" ? "Question" : "Permission"} “{it.d.title}” on <span class="a-mono">{it.d.taskId}</span>: Answered on Phone (mock): {it.answer}.
        {" "}<button type="button" onClick={() => desk.openTask(it.d.taskId, "decisions")}>Open</button>
      </li>}</For></ul>
      <button type="button" onClick={() => desk.setUi("changesSince", [])}>Dismiss</button>
    </section>
  </Show>;
}
