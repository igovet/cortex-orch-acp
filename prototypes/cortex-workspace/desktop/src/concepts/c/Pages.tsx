import { createMemo, createSignal, For, Show } from "solid-js";
import type { Task } from "../../fixtures";
import { useC, uid } from "./ctx";
import { executionGlyph, executionLabel, executionTone, PHASES, phaseLabel, stamp } from "./model";
import { Actions, Badge, MockButton, Receipt, rovingFocusIn, rovingKeyDown } from "./ui";

export function ErrorState() {
  const c = useC();
  return <div class="c-banner danger" role="alert">
    <p><span aria-hidden="true">✕ </span><strong>Could not load tasks</strong> (fixture error E_FIXTURE_READ: {c.store.error()}). Nothing was changed.</p>
    <button type="button" class="c-btn" onClick={() => c.store.reset()}>Try again (reload the fixture; resets mock state)</button>
  </div>;
}

export function EmptyProject() {
  const c = useC();
  return <div class="c-empty">
    <h2 id="c-focus-heading" class="c-title-lg" tabindex="-1">No tasks in this project yet.</h2>
    <p>Create a task to start a discussion.</p>
    <button type="button" class="c-btn c-primary" aria-label="New task (mock)" onClick={() => c.openCreate()}>New task<span class="c-mock-tag" aria-hidden="true"> · Mock</span></button>
  </div>;
}

export function AllTasks(p: { mode: "list" | "board"; setMode: (m: "list" | "board") => void; loading: boolean }) {
  const c = useC();
  const tasks = () => c.store.visibleTasks();
  const f = () => c.store.state().fixture;
  const execs = createMemo(() => new Map(f().executions.map(e => [e.taskId, e] as const)));
  const needs = createMemo(() => {
    const s = c.store.state();
    const set = new Set<string>();
    for (const d of s.fixture.decisions) if (!s.responses[d.id]) set.add(d.taskId);
    for (const r of s.fixture.results) if (!s.acceptedResultIds.includes(r.id)) set.add(r.taskId);
    return set;
  });
  const state = (t: Task) => executionLabel(execs().get(t.id) ?? null, c.conn());
  return <section class="c-all" aria-labelledby="c-all-heading">
    <div class="c-all-head">
      <h2 id="c-all-heading" class="c-title-lg" tabindex="-1">All tasks ({tasks().length})</h2>
      <div class="c-seg" role="group" aria-label="Task view">
        <button type="button" class="c-btn" aria-pressed={p.mode === "list"} onClick={() => p.setMode("list")}>List</button>
        <button type="button" class="c-btn" aria-pressed={p.mode === "board"} onClick={() => p.setMode("board")}>Board</button>
      </div>
      <p class="c-muted">Selecting a task returns to the attention view with that task in the focus pane. Alt+← goes back. Phases are fixture phases; board phase names are an owner decision.</p>
    </div>
    <Show when={!c.store.error()} fallback={<ErrorState />}>
      <Show when={!p.loading} fallback={<div class="c-loading" role="status"><p>Loading tasks…</p>
        <div class="c-skel" aria-hidden="true"><span /><span /></div><div class="c-skel" aria-hidden="true"><span /><span /></div></div>}>
      <Show when={tasks().length} fallback={<EmptyProject />}>
        <Show when={p.mode === "list"} fallback={<Board tasks={tasks()} state={state} needs={needs()} />}>
          <table class="c-table c-tasklist">
            <caption class="c-sr">All tasks; arrow keys move between tasks</caption>
            <thead><tr><th scope="col">Task</th><th scope="col">ID</th><th scope="col">Phase</th><th scope="col">Execution</th><th scope="col">Needs you</th></tr></thead>
            <tbody data-roving-box onKeyDown={e => rovingKeyDown(e, true)} onFocusIn={rovingFocusIn}>
              <For each={tasks()}>{(t, i) => <tr aria-current={t.id === c.store.state().selectedTaskId ? "true" : undefined}>
                <th scope="row"><button type="button" class="c-link c-tasklink" data-roving tabindex={i() === 0 ? 0 : -1} onClick={() => c.openTask(t.id)}>{t.title}</button></th>
                <td class="c-mono">{t.id}</td><td>{phaseLabel(t.phase)}</td>
                <td><Badge tone={executionTone(state(t))} glyph={executionGlyph(state(t))} label={state(t)} /></td>
                <td>{needs().has(t.id) ? "Your decision needed" : "—"}</td>
              </tr>}</For>
            </tbody>
          </table>
        </Show>
      </Show>
      </Show>
    </Show>
  </section>;
}

function Board(p: { tasks: readonly Task[]; state: (t: Task) => string; needs: Set<string> }) {
  const c = useC();
  const [limits, setLimits] = createSignal<Record<string, number>>({});
  const STEP = 50;
  return <div class="c-board">
    <For each={PHASES}>{phase => {
      const items = createMemo(() => p.tasks.filter(t => t.phase === phase.id));
      const limit = () => limits()[phase.id] ?? STEP;
      const headId = uid("col");
      return <section class="c-col" aria-labelledby={headId}>
        <h3 id={headId} class="c-h3">{phase.label} ({items().length})</h3>
        <ul class="c-plain c-cards" data-roving-box onKeyDown={e => rovingKeyDown(e, true)} onFocusIn={rovingFocusIn}>
          <For each={items().slice(0, limit())}>{(t, i) => <li>
            <button type="button" class="c-card" data-roving tabindex={i() === 0 ? 0 : -1}
              aria-current={t.id === c.store.state().selectedTaskId ? "true" : undefined} onClick={() => c.openTask(t.id)}>
              <span class="c-card-title">{t.title}</span>
              <span class="c-card-meta"><span class="c-mono">{t.id}</span> · <Badge tone={executionTone(p.state(t))} glyph={executionGlyph(p.state(t))} label={p.state(t)} />
                {p.needs.has(t.id) ? " · Your decision needed" : ""}</span>
            </button>
          </li>}</For>
        </ul>
        <Show when={items().length > limit()}>
          <button type="button" class="c-btn" onClick={() => setLimits({ ...limits(), [phase.id]: limit() + STEP })}>
            Show {Math.min(STEP, items().length - limit())} more ({items().length - limit()} not shown)
          </button>
        </Show>
      </section>;
    }}</For>
  </div>;
}

export function CreatePage(p: { onCreated: (taskId: string) => void }) {
  const c = useC();
  const [title, setTitle] = createSignal("");
  const [details, setDetails] = createSignal(false);
  const [error, setError] = createSignal("");
  const fieldId = uid("what");
  const errorId = uid("what-err");
  const submit = () => {
    if (c.blocked()) return;
    if (!title().trim()) { setError("Describe what should be done."); return; }
    setError("");
    c.store.createTask(title());
    const id = c.store.state().selectedTaskId;
    c.setReceipt(`created:${id}`, "Mock outcome: recorded in memory only. No agent was started.");
    c.announce(`Task created (mock): ${title().trim()}. It appears under Changed since last visit as Not started.`);
    p.onCreated(id);
  };
  return <form class="c-create" onSubmit={e => { e.preventDefault(); submit(); }}>
    <h2 id="c-focus-heading" class="c-title-lg" tabindex="-1">New task</h2>
    <label for={fieldId} class="c-label c-label-lg">What should be done?</label>
    <textarea id={fieldId} class="c-composer" rows={3} value={title()} aria-invalid={error() ? true : undefined}
      aria-describedby={`${error() ? errorId + " " : ""}${fieldId}-hint`}
      onInput={e => { setTitle(e.currentTarget.value); e.currentTarget.style.height = "auto"; e.currentTarget.style.height = `${e.currentTarget.scrollHeight}px`; }}
      onKeyDown={e => { if (e.key === "Enter" && !e.shiftKey) { e.preventDefault(); submit(); } }} />
    <p id={`${fieldId}-hint`} class="c-muted">Enter creates; Shift+Enter adds a new line.</p>
    <Show when={error()}><p id={errorId} class="c-field-error">{error()}</p></Show>
    <button type="button" class="c-link" aria-expanded={details()} aria-controls="c-create-details" onClick={() => setDetails(!details())}>Add details</button>
    <div id="c-create-details" hidden={!details()}>
      <Show when={details()}>
        <label class="c-label" for="c-create-desc">Details</label>
        <textarea id="c-create-desc" rows={4} />
        <label class="c-label" for="c-create-files">Context files</label>
        <input id="c-create-files" class="c-mono" placeholder="synthetic/layout.ts" />
      </Show>
    </div>
    <Actions reason={c.blocked()}>{reasonId => <MockButton label="Create task" tone="primary" reason={c.blocked()} reasonId={reasonId} onAct={submit} />}</Actions>
    <p class="c-footnote">Creating a task records it in memory only. It does not start an agent.</p>
  </form>;
}

export function ConnectionPage() {
  const c = useC();
  const f = () => c.store.state().fixture;
  const machine = () => f().machines[0];
  const status = () => ({ connected: "Connected (mock; no real connection is made)", offline: "Offline · showing the last confirmed state", disconnected: "Disconnected" })[c.conn()];
  const revoke = async () => {
    const ok = await c.confirm({ title: "Revoke this device? (mock)", effect: "Would remove this device's access to the machine.", scope: `${machine().name} (${machine().id}). This is a mock; nothing is revoked.`, confirmLabel: "Revoke" });
    c.setReceipt("connection", ok ? "Mock outcome: revocation recorded in memory only. Nothing was revoked." : "Revoke cancelled. Nothing changed.");
  };
  return <section aria-labelledby="c-focus-heading">
    <h2 id="c-focus-heading" class="c-title-lg" tabindex="-1">Connection settings</h2>
    <dl class="c-facts">
      <div><dt>Prototype status</dt><dd>{status()}</dd></div>
      <div><dt>Fixture machine</dt><dd>{machine().name} · <span class="c-mono">{machine().id}</span></dd></div>
      <div><dt>Fixture record</dt><dd>{machine().state} · last confirmed {stamp(machine().lastConfirmedAt)} · current execution state {machine().currentExecutionState}</dd></div>
    </dl>
    <Actions reason={c.blocked()}>{reasonId => <>
      <MockButton label="Test connection" reason={c.blocked()} reasonId={reasonId} onAct={() => c.setReceipt("connection", "Mock outcome: no connection was attempted.")} />
      <MockButton label="Reconnect" onAct={() => c.setReceipt("connection", "Mock outcome: no reconnect was attempted. Choose the Default scenario to leave the disconnected fixture.")} />
      <MockButton label="Enroll this device" reason={c.blocked()} reasonId={reasonId} onAct={() => c.setReceipt("connection", "Mock outcome: no enrolment was made.")} />
      <MockButton label="Revoke device" tone="danger" reason={c.blocked()} reasonId={reasonId} onAct={() => void revoke()} />
    </>}</Actions>
    <Receipt rkey="connection" />
    <h3 class="c-h3">Capabilities</h3>
    <ul class="c-plain"><For each={f().capabilities}>{cap => <li>
      <Badge tone="neutral" glyph="⊘" label={cap.available ? "Available" : "Unavailable"} /> {cap.name}: {cap.reason}
    </li>}</For></ul>
  </section>;
}

export function SettingsPage(p: { scale: number; setScale: (n: number) => void; shortcuts: boolean; setShortcuts: (on: boolean) => void }) {
  const name = uid("scale");
  return <section aria-labelledby="c-focus-heading">
    <h2 id="c-focus-heading" class="c-title-lg" tabindex="-1">Settings</h2>
    <fieldset class="c-fieldset"><legend>Keyboard</legend>
      <label class="c-check"><input type="checkbox" checked={p.shortcuts} onChange={e => p.setShortcuts(e.currentTarget.checked)} /> Character-key shortcuts (J/K, G A/G T/G C, [ and ], ?)</label>
      <p class="c-muted">Modifier shortcuts (Ctrl+K, Ctrl+N or Alt+N, Alt+←, Alt+1…6, F6) stay available.</p>
    </fieldset>
    <fieldset class="c-fieldset"><legend>Text size</legend>
      <For each={[1, 1.5, 2]}>{n => <label class="c-radio"><input type="radio" name={name} checked={p.scale === n} onChange={() => p.setScale(n)} /> {n * 100}%</label>}</For>
      <p class="c-muted">Sets the root text size; all concept sizes are in rem.</p>
    </fieldset>
  </section>;
}
