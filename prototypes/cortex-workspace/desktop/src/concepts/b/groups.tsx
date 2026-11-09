import { createEffect, createMemo, createSignal, For, Match, onCleanup, onMount, Show, Switch } from "solid-js";
import { changeCounts, fmtUtc, RESULT_CHANGES, shortDigest, splitRows, TERMINAL_TRANSCRIPT, unifiedRows } from "./data";
import type { BModel, BottomTab, Check, Decision, FixtureEvent, G2Tab, Result, Sub, Task } from "./model";
import { TaskCell } from "./side";
import { ActionRow, Badge, ConnectionBanner, Glyph, MockButton, Receipts, TaskListState, VirtualListbox } from "./ui";

type TabItem = { key: string; title: string; preview?: boolean; suffix?: string; suffixTone?: string };

function TabStrip(props: { id: string; label: string; tabs: TabItem[]; active: string | null; onSelect: (key: string) => void; onClose: (key: string) => void; onPin?: (key: string) => void }) {
  const refs = new Map<string, HTMLButtonElement>();
  // Keep the active tab visible in a horizontally scrolling strip.
  createEffect(() => { const el = props.active ? refs.get(props.active) : undefined; el?.scrollIntoView?.({ block: "nearest", inline: "nearest" }); });
  const onKey = (e: KeyboardEvent, i: number) => {
    const n = props.tabs.length;
    let j = -1;
    if (e.key === "ArrowRight") j = (i + 1) % n;
    else if (e.key === "ArrowLeft") j = (i - 1 + n) % n;
    else if (e.key === "Home") j = 0;
    else if (e.key === "End") j = n - 1;
    else if (e.key === "Delete") { e.preventDefault(); props.onClose(props.tabs[i].key); return; }
    else if (e.key === "Enter" && props.onPin) { props.onPin(props.tabs[i].key); return; }
    if (j < 0) return;
    e.preventDefault();
    props.onSelect(props.tabs[j].key);
    refs.get(props.tabs[j].key)?.focus();
  };
  return <div class="cb-tabs" role="tablist" aria-label={props.label}>
    <For each={props.tabs}>{(t, i) =>
      <div class="cb-tab" classList={{ active: t.key === props.active, preview: !!t.preview }}>
        <button type="button" role="tab" ref={el => refs.set(t.key, el)} id={`${props.id}-tab-${i()}`} class="cb-tab-btn"
          aria-selected={t.key === props.active} aria-controls={`${props.id}-panel`} tabindex={t.key === props.active ? 0 : -1}
          title={t.preview ? `${t.title} (preview; Enter or double-click pins)` : t.title}
          onClick={() => props.onSelect(t.key)} onDblClick={() => props.onPin?.(t.key)} onKeyDown={e => onKey(e, i())}>
          <span class="cb-tab-title">{t.title}</span>
          <Show when={t.suffix}><span class={`cb-tab-suffix ${t.suffixTone ?? ""}`}> · {t.suffix}</span></Show>
          <Show when={t.preview}><span class="cb-sr"> (preview)</span></Show>
        </button>
        <button type="button" class="cb-tab-x" tabindex="-1" aria-label={`Close ${t.title}`} title="Close (Ctrl+W or Delete)" onClick={() => props.onClose(t.key)}><Glyph name="close" /></button>
      </div>}
    </For>
  </div>;
}

// ---------------------------------------------------------------- group 1 (code)
export function Group1(props: { m: BModel }) {
  const m = props.m;
  const tabs = (): TabItem[] => m.ui.g1.map(t => ({
    key: t.path, title: t.path.split("/").pop()!,
    suffix: m.isConflicted(t.path) ? "Conflict" : m.dirty(t.path) ? "Unsaved" : undefined,
    suffixTone: m.isConflicted(t.path) ? "t-warn-text" : "",
  }));
  return <section id="cb-g1" class="cb-group" aria-label="Editor group 1 (code)" tabindex="-1">
    <ConnectionBanner m={m} />
    <TabStrip id="cb-g1" label="Open files" tabs={tabs()} active={m.ui.g1Active} onSelect={k => m.setUi("g1Active", k)} onClose={k => m.closeG1(k)} />
    <div id="cb-g1-panel" role="tabpanel" class="cb-tabpanel" aria-label={m.ui.g1Active ?? "No file open"}>
      <Show when={m.ui.g1Active} fallback={<div class="cb-empty cb-pad"><p class="cb-empty-h">No file open.</p><p>Open a file from Files (Ctrl+Shift+E) or quick open (Ctrl+P).</p></div>}>
        {path => <Editor m={m} path={path()} />}
      </Show>
    </div>
  </section>;
}

function Editor(props: { m: BModel; path: string }) {
  const m = props.m;
  let ta!: HTMLTextAreaElement;
  const text = () => m.fileText(props.path);
  const lines = () => text().split("\n");
  const longest = () => Math.max(20, ...lines().map(l => l.length));
  const saveBlock = () => m.block() ?? (m.ui.conflictBanner === props.path ? "Save disabled: the file changed on disk" : null);
  const fromSelection = () => {
    const before = ta.value.slice(0, ta.selectionStart).split("\n").length;
    const after = ta.value.slice(0, ta.selectionEnd).split("\n").length;
    m.openNewTask(`${props.path}:${before}${after !== before ? `-${after}` : ""}`);
  };
  return <div class="cb-editor-wrap">
    <div class="cb-crumbs">
      <span class="cb-mono" aria-label={`Path ${props.path}`}>{props.path.split("/").join(" › ")}</span>
      <span class="cb-muted">· Lease: none (fixture)</span>
      <span class="cb-crumb-actions">
        <MockButton label="Save" disabled={!!saveBlock()} onClick={() => m.save(props.path)} />
        <button type="button" class="cb-btn" onClick={fromSelection} title="Proposal for owner review">New task from selection… (proposal)</button>
      </span>
    </div>
    <Show when={m.ui.conflictBanner === props.path}>
      <div class="cb-banner t-warn" role="note"><Glyph name="warn" />
        <p>This file changed on disk since you opened it (agent edit recorded in the fixture). Save is disabled; nothing will be overwritten.</p>
        <div class="cb-banner-actions">
          <button type="button" class="cb-btn" onClick={() => m.openMerge()}>Compare</button>
          <button type="button" class="cb-btn" onClick={() => m.reload(props.path)}>Reload</button>
          <MockButton label="Save as…" disabled={!!m.block()} onClick={() => m.saveAs(props.path)} />
        </div>
      </div>
    </Show>
    <Receipts m={m} where={`editor:${props.path}`} last={2} />
    <div class="cb-editor">
      <div class="cb-gutter" aria-hidden="true"><For each={lines()}>{(_, i) => <div>{i() + 1}</div>}</For></div>
      <textarea ref={ta} class="cb-code" spellcheck={false} wrap="off" rows={lines().length} value={text()}
        style={{ width: `max(100%, ${longest() + 4}ch)` }}
        aria-label={`Code editor: ${props.path}. Plain-text fixture editor; Tab moves focus out.`}
        onInput={e => m.edit(props.path, e.currentTarget.value)} />
    </div>
  </div>;
}

// ---------------------------------------------------------------- group 2 (orchestration)
export function Group2(props: { m: BModel }) {
  const m = props.m;
  const title = (t: G2Tab): TabItem => {
    switch (t.kind) {
      case "task": return { key: t.key, title: t.taskId!, preview: t.preview, suffix: m.execInfo(t.taskId!).text, suffixTone: m.execInfo(t.taskId!).tone === "warn" ? "t-warn-text" : "" };
      case "board": return { key: t.key, title: "Board" };
      case "new": return { key: t.key, title: "New task" };
      case "settings": return { key: t.key, title: "Settings: Connection" };
      case "terminal": return { key: t.key, title: "Terminal" };
      case "problems": return { key: t.key, title: "Problems" };
      case "output": return { key: t.key, title: "Output" };
      case "history": return { key: t.key, title: "Project history" };
      case "merge": return { key: t.key, title: `Compare: ${m.conflictFile().path.split("/").pop()}` };
      case "diff": return { key: t.key, title: `Diff: ${t.path!.split("/").pop()}` };
    }
  };
  const active = () => m.ui.g2.find(t => t.key === m.ui.g2Active) ?? null;
  return <section id="cb-g2" class="cb-group cb-g2" aria-label="Editor group 2 (orchestration)" tabindex="-1">
    <ConnectionBanner m={m} />
    <TabStrip id="cb-g2" label="Orchestration tabs" tabs={m.ui.g2.map(title)} active={m.ui.g2Active}
      onSelect={k => m.setUi("g2Active", k)} onClose={k => m.closeG2(k)} onPin={k => m.pin(k)} />
    <div id="cb-g2-panel" role="tabpanel" class="cb-tabpanel" aria-label={active() ? title(active()!).title : "Group 2"}>
      <Show when={active()} keyed fallback={<div class="cb-empty cb-pad"><p class="cb-empty-h">Group 2 is empty.</p><p>Open a task from Mission Control (Ctrl+Shift+M) or the board (Ctrl+Shift+B).</p></div>}>
        {tab => <Switch>
          <Match when={tab.kind === "task"}><TaskTab m={m} tab={tab} /></Match>
          <Match when={tab.kind === "board"}><BoardTab m={m} /></Match>
          <Match when={tab.kind === "new"}><NewTaskTab m={m} /></Match>
          <Match when={tab.kind === "settings"}><SettingsTab m={m} /></Match>
          <Match when={tab.kind === "terminal"}><TerminalPanel m={m} /></Match>
          <Match when={tab.kind === "problems"}><ProblemsPanel /></Match>
          <Match when={tab.kind === "output"}><OutputPanel m={m} /></Match>
          <Match when={tab.kind === "history"}><HistoryTab m={m} /></Match>
          <Match when={tab.kind === "merge"}><MergeTab m={m} /></Match>
          <Match when={tab.kind === "diff"}><DiffTab m={m} resultId={tab.resultId!} path={tab.path!} /></Match>
        </Switch>}
      </Show>
    </div>
  </section>;
}

const SUBS: { id: Sub; label: string }[] = [
  { id: "discussion", label: "Discussion & plan" }, { id: "decisions", label: "Decisions" },
  { id: "result", label: "Result & diff" }, { id: "workers", label: "Workers" },
];

function TaskTab(props: { m: BModel; tab: G2Tab }) {
  const m = props.m;
  const task = () => m.taskMap().get(props.tab.taskId!);
  const sub = () => m.ui.g2.find(t => t.key === props.tab.key)?.sub ?? "discussion";
  const result = () => m.resultFor(props.tab.taskId!);
  const refs = new Map<Sub, HTMLButtonElement>();
  const onKey = (e: KeyboardEvent, i: number) => {
    const j = e.key === "ArrowRight" ? (i + 1) % SUBS.length : e.key === "ArrowLeft" ? (i - 1 + SUBS.length) % SUBS.length : -1;
    if (j < 0) return;
    e.preventDefault();
    m.setSub(props.tab.key, SUBS[j].id);
    refs.get(SUBS[j].id)?.focus();
  };
  const count = (s: Sub) => s === "decisions" ? m.decisionsFor(props.tab.taskId!).filter(d => !m.st().responses[d.id]).length : null;
  return <Show when={task()} fallback={<div class="cb-pad cb-empty"><p>Task <code>{props.tab.taskId}</code> is not in the current fixture view ({m.scenario()}).</p></div>}>
    {t => <div class="cb-task">
      <header class="cb-task-h">
        <h2 class="cb-task-title">{t().title}</h2>
        <dl class="cb-meta">
          <div><dt>ID</dt><dd class="cb-mono">{t().id}</dd></div>
          <div><dt>Phase</dt><dd>{t().phase} <span class="cb-muted">(projection · fixture)</span></dd></div>
          <div><dt>Execution</dt><dd><Badge tone={m.execInfo(t().id).tone} glyph={m.execInfo(t().id).glyph} text={m.execInfo(t().id).text} prov={m.execInfo(t().id).prov} /></dd></div>
          <div><dt>Wait reason</dt><dd>{m.execInfo(t().id).wait ?? "None"}</dd></div>
          <div><dt>Last confirmed</dt><dd>{m.conn() === "connected" ? "Current · fixture" : `As of ${m.lastConfirmed()} (cached)`}</dd></div>
        </dl>
      </header>
      <Show when={result()?.state === "stale"}>
        <div class="cb-banner t-warn" role="note"><Glyph name="history" />
          <p>Result changed after review: digest {shortDigest(result()!.artifactDigest)} → {shortDigest(result()!.workspaceDigest)}. Previous acceptance (mock) is invalidated.</p>
          <button type="button" class="cb-btn" onClick={() => m.setSub(props.tab.key, "result")}>Review new result</button>
        </div>
      </Show>
      <div class="cb-subtabs">
        <div role="tablist" aria-label="Task sections" class="cb-subtablist">
          <For each={SUBS}>{(s, i) =>
            <button type="button" role="tab" ref={el => refs.set(s.id, el)} id={`cb-sub-${s.id}`} aria-selected={sub() === s.id} aria-controls="cb-sub-panel"
              tabindex={sub() === s.id ? 0 : -1} class="cb-subtab" onClick={() => m.setSub(props.tab.key, s.id)} onKeyDown={e => onKey(e, i())}>
              {s.label}<Show when={count(s.id)}> ({count(s.id)})</Show>
            </button>}
          </For>
        </div>
        <label class="cb-subselect cb-field">Section
          <select value={sub()} onChange={e => m.setSub(props.tab.key, e.currentTarget.value as Sub)}>
            <For each={SUBS}>{s => <option value={s.id}>{s.label}</option>}</For>
          </select>
        </label>
      </div>
      <div id="cb-sub-panel" class="cb-sub-panel" role="tabpanel" aria-labelledby={`cb-sub-${sub()}`}>
        <Switch>
          <Match when={sub() === "discussion"}><DiscussionPanel m={m} task={t()} /></Match>
          <Match when={sub() === "decisions"}><DecisionsPanel m={m} task={t()} /></Match>
          <Match when={sub() === "result"}><ResultPanel m={m} task={t()} /></Match>
          <Match when={sub() === "workers"}><WorkersPanel m={m} task={t()} /></Match>
        </Switch>
      </div>
    </div>}
  </Show>;
}

function LongText(props: { text: string }) {
  const [open, setOpen] = createSignal(false);
  const long = () => props.text.length > 700;
  return <div>
    <p class="cb-prose" classList={{ "cb-clamp12": long() && !open() }}>{props.text}</p>
    <Show when={long()}>
      <button type="button" class="cb-btn" aria-expanded={open()} onClick={() => setOpen(!open())}>
        {open() ? "Show less" : `Show full message (${props.text.length.toLocaleString("en-US")} characters)`}
      </button>
    </Show>
  </div>;
}

function DiscussionPanel(props: { m: BModel; task: Task }) {
  const m = props.m;
  const [planOpen, setPlanOpen] = createSignal(true);
  const evs = () => m.eventsFor(props.task.id);
  const plan = () => evs().filter(e => e.kind === "plan");
  return <div class="cb-discussion">
    <section class="cb-plan" aria-labelledby="cb-plan-h">
      <h3 id="cb-plan-h" class="cb-group-h">
        <button type="button" class="cb-mc-toggle" aria-expanded={planOpen()} onClick={() => setPlanOpen(!planOpen())}>
          <span aria-hidden="true">{planOpen() ? "▾" : "▸"}</span> Plan ({plan().length} entries · reported)
        </button>
      </h3>
      <Show when={planOpen()}>
        <Show when={plan().length} fallback={<p class="cb-muted">No plan entries in the fixture for this task.</p>}>
          <ol class="cb-plain cb-stack-s"><For each={plan()}>{e => <li><span class="cb-mono cb-muted">#{e.sequence}</span> {e.text} <span class="cb-prov">· reported · fixture</span></li>}</For></ol>
        </Show>
        <p class="cb-muted">Progress is shown only as reported entries; there are no percentages or estimates.</p>
      </Show>
    </section>
    <section class="cb-stack-s" aria-labelledby="cb-desc-h">
      <h3 id="cb-desc-h" class="cb-group-h">Description</h3>
      <LongText text={props.task.description} />
    </section>
    <section class="cb-stack-s cb-timeline-sec" aria-labelledby="cb-tl-h">
      <h3 id="cb-tl-h" class="cb-group-h">Timeline ({evs().length} events · fixture)</h3>
      <Show when={evs().length} fallback={<p class="cb-muted">No events yet.</p>}>
        <ol class="cb-plain cb-timeline">
          <For each={evs()}>{e => <li><span class="cb-mono cb-muted">#{e.sequence} · {fmtUtc(e.occurredAt, true)}</span> <span class="cb-tag">{e.kind}</span> {e.text} <span class="cb-prov">· reported · fixture</span></li>}</For>
        </ol>
      </Show>
    </section>
  </div>;
}

function DecisionsPanel(props: { m: BModel; task: Task }) {
  const m = props.m;
  const ds = () => m.decisionsFor(props.task.id);
  const r = () => m.resultFor(props.task.id);
  return <div class="cb-stack">
    <Show when={ds().length || r()} fallback={<p class="cb-empty">Nothing needs your decision for this task.</p>}>
      <For each={ds()}>{d => <DecisionView m={m} d={d} />}</For>
      <Show when={r()}>{res => <AcceptanceView m={m} r={res()} />}</Show>
    </Show>
  </div>;
}

export function DecisionView(props: { m: BModel; d: Decision }) {
  const m = props.m;
  const d = () => props.d;
  const options = () => ("options" in d() ? (d() as { options: string[] }).options : []);
  const [choice, setChoice] = createSignal(options()[0] ?? "");
  const resp = () => m.st().responses[d().id];
  const phone = () => m.ui.phoneAnswered[d().id];
  const ex = () => m.execFor(d().taskId);
  const hid = () => `cb-dec-${d().id}`;
  const reason = () => m.decisionBlock(d());
  return <section class="cb-decision" aria-labelledby={hid()}>
    <p class="cb-dec-type"><Glyph name={d().kind === "question" ? "message" : "key"} /> {d().kind === "question" ? "Question" : "Permission"}</p>
    <h2 id={hid()} class="cb-dec-title">{d().title}</h2>
    <p class="cb-muted">Task <span class="cb-mono">{d().taskId}</span> · {m.taskMap().get(d().taskId)?.title} · requested by {ex() ? <><span class="cb-mono">{ex()!.id}</span> → worker <span class="cb-mono">{ex()!.sessionId}</span></> : "unknown"} · requested at: unknown (not in fixture) · decision version {d().version}</p>
    <dl class="cb-dec-dl">
      <dt>Effect</dt><dd>{d().effect}</dd>
      <dt>Scope</dt><dd>{d().scope}</dd>
      <dt>If you reject</dt><dd>{d().rejection} Nothing is reverted.</dd>
    </dl>
    <Switch>
      <Match when={phone()}>
        <p class="cb-banner t-mock" role="status"><Glyph name="flask" /> Answered on Phone (simulated): {phone()} (mock). Your choice was not applied.</p>
      </Match>
      <Match when={resp()}>
        <p><Badge tone="mock" glyph="flask" text={`Responded (mock): ${resp()}`} prov="in memory only" /></p>
      </Match>
      <Match when={d().kind === "question"}>
        <form class="cb-stack-s" onSubmit={e => { e.preventDefault(); m.answer(d(), choice()); }}>
          <fieldset class="cb-fieldset" disabled={!!reason()}>
            <legend>Answer</legend>
            <For each={options()}>{o => <label class="cb-check"><input type="radio" name={`ans-${d().id}`} value={o} checked={choice() === o} onChange={() => setChoice(o)} /> {o}</label>}</For>
          </fieldset>
          <ActionRow reason={reason()}><MockButton type="submit" label="Send answer" primary disabled={!!reason()} /></ActionRow>
        </form>
      </Match>
      <Match when={true}>
        <ActionRow reason={reason()}>
          <MockButton label="Approve once" primary disabled={!!reason()} onClick={() => m.answer(d(), "Approved once")} />
          <MockButton label="Deny" disabled={!!reason()} onClick={() => m.answer(d(), "Denied")} />
        </ActionRow>
        <p class="cb-muted">"Always allow" is not offered: it was not negotiated in this fixture.</p>
      </Match>
    </Switch>
    <Receipts m={m} where={`decision:${d().id}`} />
  </section>;
}

const OUTCOME: Record<Check["outcome"] | "unverified", { text: string; tone: "danger" | "neutral" | "ok"; glyph: "x" | "dash" | "question" | "check"; order: number }> = {
  failed: { text: "Failed", tone: "danger", glyph: "x", order: 0 },
  not_run: { text: "Not run", tone: "neutral", glyph: "dash", order: 1 },
  unverified: { text: "Unverified", tone: "neutral", glyph: "question", order: 2 },
  passed: { text: "Passed", tone: "ok", glyph: "check", order: 3 },
};

function Evidence(props: { m: BModel; checks: Check[] }) {
  const counts = () => props.m.checkCounts(props.checks);
  const notRun = () => props.checks.filter(c => c.outcome === "not_run");
  return <div class="cb-evidence">
    <p class="cb-counts">
      <Badge tone="ok" glyph="check" text={`Passed ${counts().passed}`} /> <Badge tone="danger" glyph="x" text={`Failed ${counts().failed}`} />{" "}
      <Badge tone="neutral" glyph="dash" text={`Not run ${counts().notRun}`} /> <Badge tone="neutral" glyph="question" text={`Unverified ${counts().unverified}`} />
    </p>
    <p><strong>Required but not run:</strong> {notRun().length ? notRun().map(c => c.name).join(", ") : "none recorded in the fixture"}</p>
  </div>;
}

export function AcceptanceView(props: { m: BModel; r: Result }) {
  const m = props.m;
  const r = () => props.r;
  const checks = () => m.checksFor(r().taskId);
  const action = () => m.ui.resultActions[r().id];
  const reason = () => m.acceptBlock(r());
  const files = () => RESULT_CHANGES[r().id] ?? [];
  return <section class="cb-decision" aria-labelledby={`cb-acc-${r().id}`}>
    <p class="cb-dec-type"><Glyph name="filecheck" /> Result acceptance</p>
    <h2 id={`cb-acc-${r().id}`} class="cb-dec-title">Accept result {r().id}, revision {r().version}?</h2>
    <p class="cb-muted">Task <span class="cb-mono">{r().taskId}</span> · {m.taskMap().get(r().taskId)?.title} · requested at: unknown (not in fixture) · decision version {r().version}</p>
    <dl class="cb-dec-dl">
      <dt>Effect</dt><dd>Records a mock acceptance of this result revision in memory. A person's acceptance is separate from runtime completion.</dd>
      <dt>Scope</dt><dd>Result {r().id} · artifact <span class="cb-mono">{r().artifactId}</span> · digest <span class="cb-mono">{r().artifactDigest}</span> · {files().length} changed file{files().length === 1 ? "" : "s"}</dd>
      <dt>If you reject</dt><dd>The result stays unaccepted. Nothing is reverted.</dd>
      <dt>Evidence</dt><dd><Evidence m={m} checks={checks()} /></dd>
    </dl>
    <Switch>
      <Match when={m.accepted(r().id)}>
        <p><Badge tone="mock" glyph="flask" text={`Accepted (mock) · ${r().id} · digest ${shortDigest(r().artifactDigest)}`} prov="not runtime evidence" /></p>
      </Match>
      <Match when={action()}><p><Badge tone="mock" glyph="flask" text={action()!} prov="in memory only" /></p></Match>
      <Match when={true}>
        <ActionRow reason={reason()}>
          <MockButton label={`Accept ${r().id}`} primary disabled={!!reason()} onClick={() => m.accept(r())} />
          <MockButton label="Request changes" disabled={!!m.block()} onClick={() => m.requestChanges(r().id)} />
          <MockButton label="Reject" disabled={!!m.block()} onClick={() => m.askReject(r())} />
        </ActionRow>
        <Show when={r().state === "stale"}><p class="cb-muted">No newer result revision exists in the fixture, so there is nothing else to accept.</p></Show>
      </Match>
    </Switch>
    <p class="cb-footer-note">Accepting does not commit, push or deploy.</p>
    <Receipts m={m} where={`result:${r().id}`} />
  </section>;
}

function ChecksTable(props: { m: BModel; checks: Check[] }) {
  const sorted = () => [...props.checks].sort((a, b) => OUTCOME[a.outcome].order - OUTCOME[b.outcome].order);
  return <div class="cb-table-wrap"><table class="cb-table">
    <caption>Checks (sorted Failed, Not run, Unverified, Passed)</caption>
    <thead><tr><th scope="col">Check</th><th scope="col">Outcome</th><th scope="col">Source</th><th scope="col">Duration</th><th scope="col">Details</th></tr></thead>
    <tbody>
      <For each={sorted()}>{c => <tr>
        <th scope="row">{c.name}</th>
        <td><Badge tone={OUTCOME[c.outcome].tone} glyph={OUTCOME[c.outcome].glyph} text={OUTCOME[c.outcome].text} /></td>
        <td>{c.outcome === "passed" ? "fixture (mock outcome)" : c.outcome === "failed" ? "observed · fixture" : "fixture"}</td>
        <td>unknown</td>
        <td>{c.detail} <button type="button" class="cb-link" onClick={() => props.m.openLog(c)}>Open log</button></td>
      </tr>}</For>
    </tbody>
  </table></div>;
}

function ResultPanel(props: { m: BModel; task: Task }) {
  const m = props.m;
  const r = () => m.resultFor(props.task.id);
  const checks = () => m.checksFor(props.task.id);
  const failed = () => checks().some(c => c.outcome === "failed");
  return <div class="cb-stack">
    <Show when={r()} fallback={<>
      <p class="cb-empty">No result revision in the fixture for this task.</p>
      <Show when={checks().length}>
        <ChecksTable m={m} checks={checks()} />
        <Show when={failed()}>
          <ActionRow reason={m.block()}><MockButton label="Request changes" disabled={!!m.block() || !!m.ui.resultActions[props.task.id]} onClick={() => m.requestChanges(props.task.id)} /></ActionRow>
          <Show when={m.ui.resultActions[props.task.id]}><p><Badge tone="mock" glyph="flask" text={m.ui.resultActions[props.task.id]} /></p></Show>
          <Receipts m={m} where={`result:${props.task.id}`} />
        </Show>
      </Show>
    </>}>
      {res => <>
        <dl class="cb-meta cb-result-h">
          <div><dt>Result</dt><dd class="cb-mono">{res().id}</dd></div>
          <div><dt>Revision</dt><dd>{res().version}</dd></div>
          <div><dt>Workspace</dt><dd>Synthetic Mosaic (fixture) · digest <span class="cb-mono">{shortDigest(res().workspaceDigest)}</span></dd></div>
          <div><dt>Artifact digest</dt><dd><span class="cb-mono">{shortDigest(res().artifactDigest)}</span> <CopyButton value={res().artifactDigest} /></dd></div>
          <div><dt>Produced at</dt><dd>unknown (not in fixture)</dd></div>
          <div><dt>Freshness</dt><dd>{res().state === "stale" ? <Badge tone="warn" glyph="history" text="Stale (result changed)" /> : "Current"}</dd></div>
          <div><dt>Acceptance</dt><dd>{m.accepted(res().id) ? "Accepted (mock)" : m.ui.resultActions[res().id] ?? "Not accepted"}</dd></div>
        </dl>
        <ChecksTable m={m} checks={checks()} />
        <Evidence m={m} checks={checks()} />
        <section aria-labelledby="cb-changes-h" class="cb-stack-s">
          <h3 id="cb-changes-h" class="cb-group-h">Changes ({(RESULT_CHANGES[res().id] ?? []).length} files · concept-local diff bodies)</h3>
          <div class="cb-table-wrap"><table class="cb-table">
            <thead><tr><th scope="col">File</th><th scope="col">Change</th><th scope="col">Lines</th><th scope="col">Open</th></tr></thead>
            <tbody><For each={RESULT_CHANGES[res().id] ?? []}>{c => <tr>
              <th scope="row" class="cb-mono">{c.path}</th><td>{c.type}</td>
              <td><span class="cb-add-text">+{changeCounts(c).add}</span> <span class="cb-del-text">−{changeCounts(c).del}</span></td>
              <td class="cb-actions-btns">
                <button type="button" class="cb-btn" onClick={() => m.openDiff(res().id, c.path)}>Open diff</button>
                <button type="button" class="cb-btn" onClick={() => m.openFile(c.path)}>Open in editor</button>
              </td>
            </tr>}</For></tbody>
          </table></div>
        </section>
        <AcceptanceView m={m} r={res()} />
      </>}
    </Show>
  </div>;
}

function CopyButton(props: { value: string }) {
  const [state, setState] = createSignal("Copy");
  const copy = async () => {
    try { await navigator.clipboard.writeText(props.value); setState("Copied"); }
    catch { setState("Copy unavailable"); }
  };
  return <button type="button" class="cb-link" aria-label={`Copy digest ${props.value}`} onClick={copy}>{state()}</button>;
}

function WorkersPanel(props: { m: BModel; task: Task }) {
  const m = props.m;
  const ex = () => m.execFor(props.task.id);
  const [open, setOpen] = createSignal(false);
  return <Show when={ex()} fallback={<p class="cb-empty">No execution for this task. Execution: {m.execInfo(props.task.id).text}.</p>}>
    {e => <ul class="cb-plain cb-stack">
      <li class="cb-worker">
        <button type="button" class="cb-row-btn" aria-expanded={open()} onClick={() => setOpen(!open())}>
          <span aria-hidden="true">{open() ? "▾" : "▸"}</span> Execution <span class="cb-mono">{e().id}</span> → worker <span class="cb-mono">{e().sessionId}</span>{" "}
          <Badge tone={m.execInfo(props.task.id).tone} glyph={m.execInfo(props.task.id).glyph} text={m.execInfo(props.task.id).text} />
        </button>
        <Show when={open()}>
          <dl class="cb-dec-dl cb-pad">
            <dt>Fixture state</dt><dd>{e().state} <span class="cb-prov">· observed · fixture</span></dd>
            <dt>Wait reason</dt><dd>{e().waitReason ?? "none"}</dd>
            <dt>History events</dt><dd>{m.events().filter(x => x.executionId === e().id).length} events in the shared history</dd>
            <dt>Provenance</dt><dd>{m.execInfo(props.task.id).prov}</dd>
          </dl>
          <Show when={e().state === "running" || e().state === "blocked" || e().state === "waiting"}>
            <ActionRow reason={m.block()}><MockButton label="Cancel execution" disabled={!!m.block() || !!m.ui.cancelRequested[e().id]} onClick={() => m.askCancel(e())} /></ActionRow>
          </Show>
          <Receipts m={m} where={`exec:${e().id}`} />
        </Show>
      </li>
    </ul>}
  </Show>;
}

const PHASES = [["backlog", "Backlog"], ["working", "Working"], ["review", "Review"], ["done", "Done"]] as const;

function BoardTab(props: { m: BModel }) {
  const m = props.m;
  let el!: HTMLDivElement;
  const [width, setWidth] = createSignal(0);
  onMount(() => {
    const measure = () => setWidth(el.getBoundingClientRect().width);
    measure();
    if (typeof ResizeObserver !== "undefined") { const ro = new ResizeObserver(measure); ro.observe(el); onCleanup(() => ro.disconnect()); }
  });
  const mode = () => m.ui.boardMode !== "auto" ? m.ui.boardMode : width() >= 760 ? "board" : "list";
  const byPhase = createMemo(() => {
    const out: Record<string, Task[]> = { backlog: [], working: [], review: [], done: [] };
    for (const t of m.tasks()) out[t.phase].push(t);
    return out;
  });
  const selected = () => m.store.state().selectedTaskId;
  return <div class="cb-board" ref={el}>
    <div class="cb-board-head">
      <h2 class="cb-group-h">Board · {m.tasks().length} tasks</h2>
      <p class="cb-muted">Cards are tasks, not workers. Phase is a projection set by the runtime (fixture values); no drag and drop.</p>
      <div role="group" aria-label="Board layout" class="cb-actions-btns">
        <button type="button" class="cb-btn" aria-pressed={mode() === "board"} onClick={() => m.setUi("boardMode", "board")}>Board</button>
        <button type="button" class="cb-btn" aria-pressed={mode() === "list"} onClick={() => m.setUi("boardMode", "list")}>List</button>
        <Show when={m.ui.boardMode !== "auto"}><button type="button" class="cb-btn" onClick={() => m.setUi("boardMode", "auto")}>Automatic</button></Show>
      </div>
    </div>
    <TaskListState m={m}>
      <Show when={mode() === "board"} fallback={
        <VirtualListbox<Task> id="cb-board-list" label={`Task list (${m.tasks().length})`} items={m.tasks()} rowRem={4} rootPx={m.rootPx()}
          keyOf={t => t.id} selectedKey={selected()} onActivate={(t, pin) => m.openTask(t.id, pin)} render={t => <TaskCell m={m} t={t} />} />}>
        <div class="cb-board-cols">
          <For each={PHASES}>{([phase, label]) =>
            <section class="cb-col" aria-labelledby={`cb-col-${phase}`}>
              <h3 id={`cb-col-${phase}`} class="cb-col-h">{label} <span class="cb-muted">({byPhase()[phase].length})</span></h3>
              <Show when={byPhase()[phase].length} fallback={<p class="cb-muted cb-pad">No tasks.</p>}>
                <VirtualListbox<Task> id={`cb-col-list-${phase}`} label={`${label} tasks (${byPhase()[phase].length})`} items={byPhase()[phase]} rowRem={4.75} rootPx={m.rootPx()}
                  keyOf={t => t.id} selectedKey={selected()} onActivate={(t, pin) => m.openTask(t.id, pin)} render={t => <TaskCell m={m} t={t} />} />
              </Show>
            </section>}
          </For>
        </div>
      </Show>
    </TaskListState>
  </div>;
}

function NewTaskTab(props: { m: BModel }) {
  const m = props.m;
  const created = () => m.ui.createdId;
  return <form class="cb-pad cb-stack cb-form" onSubmit={e => { e.preventDefault(); m.createTask(); }} aria-labelledby="cb-new-h">
    <h2 id="cb-new-h" class="cb-task-title">New task</h2>
    <p class="cb-muted">Project: Synthetic Mosaic (fixture). Creating records the task in memory only; no agent is started.</p>
    <label class="cb-field">Title (required)
      <input id="cb-new-title" type="text" value={m.ui.newTask.title} onInput={e => m.setUi("newTask", "title", e.currentTarget.value)} autocomplete="off" />
    </label>
    <label class="cb-field">Description
      <textarea rows="3" value={m.ui.newTask.description} onInput={e => m.setUi("newTask", "description", e.currentTarget.value)} />
    </label>
    <label class="cb-field">Context (optional; "New task from selection…" fills it)
      <input type="text" value={m.ui.newTask.context} onInput={e => m.setUi("newTask", "context", e.currentTarget.value)} />
    </label>
    <ActionRow reason={m.createBlock()}><MockButton type="submit" label="Create task" primary disabled={!!m.createBlock()} /></ActionRow>
    <Receipts m={m} where="new" />
    <Show when={created()}>
      <p>Created (mock) <span class="cb-mono">{created()}</span> · Execution: Not started (mock).{" "}
        <Show when={m.taskMap().has(created()!)} fallback={<span class="cb-muted">Not visible in the current fixture state ({m.scenario()}).</span>}>
          <button type="button" class="cb-btn" onClick={() => m.openTask(created()!, true)}>Open task</button>
        </Show>
      </p>
    </Show>
  </form>;
}

function SettingsTab(props: { m: BModel }) {
  const m = props.m;
  const mc = () => m.machine();
  return <div class="cb-pad cb-stack cb-form">
    <h2 id="cb-settings-title" class="cb-task-title" tabindex="-1">Settings → Connection</h2>
    <dl class="cb-dec-dl">
      <dt>Machine</dt><dd>{mc().name} <span class="cb-mono cb-muted">{mc().id}</span></dd>
      <dt>State</dt><dd>
        <Switch>
          <Match when={m.conn() === "disconnected"}><Badge tone="danger" glyph="plugoff" text="Disconnected" prov={`last confirmed ${m.lastConfirmed()}`} /></Match>
          <Match when={m.conn() === "offline"}><Badge tone="neutral" glyph="clock" text={`As of ${m.lastConfirmed()} (cached)`} /></Match>
          <Match when={true}><Badge tone="neutral" glyph="plug" text="Connected (mock)" prov="fixture state" /></Match>
        </Switch>
      </dd>
      <dt>Current execution state</dt><dd>{m.conn() === "connected" ? `${m.running().length} running · fixture` : "Unknown"}</dd>
      <dt>Transport</dt><dd>None. This prototype has no network connection; Vite's dev server is tooling only.</dd>
    </dl>
    <ActionRow reason={m.ui.loading ? "Loading; actions unavailable" : null}><MockButton label="Reconnect" disabled={m.ui.loading} onClick={m.reconnect} /></ActionRow>
    <ActionRow reason={m.block()}><MockButton label="Test connection" disabled={!!m.block()} onClick={m.testConnection} /></ActionRow>
    <section class="cb-stack-s" aria-labelledby="cb-device-h">
      <h3 id="cb-device-h" class="cb-group-h">This device</h3>
      <p>Enrollment: {m.ui.enrolled ? "Enrolled (mock)" : "Not enrolled"}</p>
      <ActionRow reason={m.block()}>
        <MockButton label="Enroll" disabled={!!m.block() || m.ui.enrolled} onClick={m.enroll} />
        <MockButton label="Revoke" disabled={!!m.block() || !m.ui.enrolled} onClick={m.askRevoke} />
      </ActionRow>
    </section>
    <section class="cb-stack-s" aria-labelledby="cb-cap-h">
      <h3 id="cb-cap-h" class="cb-group-h">Capabilities (fixture)</h3>
      <ul class="cb-plain cb-stack-s">
        <For each={m.fx().capabilities}>{c => <li>
          {c.name}: <Show when={c.available} fallback={<Badge tone="neutral" glyph="slash" text={`Unavailable (${c.reason})`} />}><Badge tone="neutral" text="Available" /></Show>
        </li>}</For>
      </ul>
    </section>
    <Receipts m={m} where="settings" />
  </div>;
}

export function TerminalPanel(props: { m: BModel }) {
  const m = props.m;
  const [cmd, setCmd] = createSignal("");
  return <div class="cb-term">
    <p class="cb-muted">Terminal (fixture) · mock output only; no shell process is attached and commands are not run.</p>
    <pre class="cb-term-out" tabindex="0" aria-label="Terminal output">{TERMINAL_TRANSCRIPT.join("\n")}</pre>
    <form class="cb-term-form" onSubmit={e => { e.preventDefault(); if (m.runCommand(cmd())) setCmd(""); }}>
      <label class="cb-field cb-term-field">Command
        <input type="text" class="cb-mono" value={cmd()} disabled={!!m.block()} onInput={e => setCmd(e.currentTarget.value)} autocomplete="off" />
      </label>
      <ActionRow reason={m.block()}><MockButton type="submit" label="Run" disabled={!!m.block()} /></ActionRow>
    </form>
    <Receipts m={m} where="terminal" last={3} />
  </div>;
}

export function ProblemsPanel() {
  return <div class="cb-pad cb-stack-s">
    <p><Badge tone="neutral" glyph="slash" text="Unavailable" /> Diagnostics unavailable: language server not running (fixture).</p>
    <p class="cb-muted">The editor is a plain-text fixture editor: CodeMirror 6 is not among the shared prototype dependencies.</p>
  </div>;
}

export function OutputPanel(props: { m: BModel }) {
  return <div class="cb-pad"><pre class="cb-term-out" tabindex="0" aria-label="Output">{props.m.ui.output ?? "No output selected. Use Open log on a check."}</pre></div>;
}

function HistoryTab(props: { m: BModel }) {
  const m = props.m;
  const [shown, setShown] = createSignal(0);
  let host!: HTMLDivElement;
  onMount(() => {
    const count = () => setShown(host.querySelectorAll("#cb-history [role=option]").length);
    count();
    const mo = new MutationObserver(count);
    mo.observe(host, { childList: true, subtree: true });
    onCleanup(() => mo.disconnect());
  });
  return <div class="cb-history" ref={host}>
    <div class="cb-pad">
      <h2 class="cb-group-h">Project history · {m.events().length.toLocaleString("en-US")} events (fixture)</h2>
      <p class="cb-muted">Viewport-bounded list: {shown()} rows in the DOM. Arrow keys, Page Up/Down, Home and End move; Enter opens the event's task. Nothing advances on a timer.</p>
    </div>
    <TaskListState m={m} label="history">
      <VirtualListbox<FixtureEvent> id="cb-history" label={`Project history (${m.events().length} events)`} items={m.events()} rowRem={1.75} rootPx={m.rootPx()}
        keyOf={e => e.id} onActivate={(e, pin) => m.openTask(e.taskId, pin)}
        render={e => <span class="cb-event"><span class="cb-mono cb-muted">#{e.sequence} · {fmtUtc(e.occurredAt, true)}</span> <span class="cb-tag">{e.kind}</span> <span class="cb-mono">{e.taskId}</span> {e.text} <span class="cb-prov">· reported</span></span>} />
    </TaskListState>
  </div>;
}

function MergeTab(props: { m: BModel }) {
  const m = props.m;
  const f = () => m.conflictFile();
  return <div class="cb-pad cb-stack">
    <h2 id="cb-merge-title" class="cb-task-title" tabindex="-1">Compare: <span class="cb-mono">{f().path}</span></h2>
    <p class="cb-muted">Base, local and incoming text from the fixture. Choosing a side records a mock choice only; no file or Git state changes.</p>
    <div class="cb-merge">
      <section aria-labelledby="cb-m-base"><h3 id="cb-m-base" class="cb-group-h">Base</h3><pre class="cb-code-block">{f().base}</pre></section>
      <section aria-labelledby="cb-m-local"><h3 id="cb-m-local" class="cb-group-h">Local (yours)</h3><pre class="cb-code-block cb-conf-bg">{m.fileText(f().path).trimEnd() || f().local}</pre></section>
      <section aria-labelledby="cb-m-inc"><h3 id="cb-m-inc" class="cb-group-h">Incoming (agent edit)</h3><pre class="cb-code-block cb-conf-bg">{f().incoming}</pre></section>
    </div>
    <Show when={m.st().conflictResolution}>{c => <p><Badge tone="mock" glyph="flask" text={`Recorded (mock): keep ${c()}`} prov="in memory only" /></p>}</Show>
    <ActionRow reason={m.block()}>
      <MockButton label="Keep local" disabled={!!m.block()} onClick={() => m.resolveConflict("local")} />
      <MockButton label="Take incoming" disabled={!!m.block()} onClick={() => m.resolveConflict("incoming")} />
    </ActionRow>
    <button type="button" class="cb-btn" onClick={() => m.openFile(f().path)}>Open file in group 1</button>
    <Receipts m={m} where="merge" />
  </div>;
}

function DiffTab(props: { m: BModel; resultId: string; path: string }) {
  const m = props.m;
  const ch = () => (RESULT_CHANGES[props.resultId] ?? []).find(c => c.path === props.path);
  const [wrap, setWrap] = createSignal(false);
  return <Show when={ch()} fallback={<p class="cb-pad cb-empty">No diff for {props.path} in result {props.resultId}.</p>}>
    {c => <div class="cb-diff" classList={{ wrap: wrap() }}>
      <div class="cb-pad cb-diff-head">
        <h2 class="cb-group-h cb-mono">{c().path}</h2>
        <p class="cb-muted">{c().type} · <span class="cb-add-text">+{changeCounts(c()).add}</span> <span class="cb-del-text">−{changeCounts(c()).del}</span> · result {props.resultId} · side-by-side at 760 px or wider, otherwise unified (concept-local diff body)</p>
        <div class="cb-actions-btns">
          <label class="cb-check"><input type="checkbox" checked={wrap()} onChange={e => setWrap(e.currentTarget.checked)} /> Wrap long lines</label>
          <button type="button" class="cb-btn" onClick={() => m.openFile(c().path)}>Open in editor</button>
        </div>
      </div>
      <div class="cb-diff-body" tabindex="0" aria-label={`Diff of ${c().path}`}>
        <table class="cb-diff-split"><caption class="cb-sr">Side-by-side diff</caption>
          <colgroup><col class="cb-ln-col" /><col /><col class="cb-ln-col" /><col /></colgroup>
          <tbody><For each={splitRows(c().lines)}>{row => <tr>
            <td class="cb-ln">{row.left?.n ?? ""}</td><td class={`cb-dl ${row.left?.t === "-" ? "del" : ""}`}><span class="cb-sign">{row.left?.t === "-" ? "−" : " "}</span>{row.left?.s ?? ""}</td>
            <td class="cb-ln">{row.right?.n ?? ""}</td><td class={`cb-dl ${row.right?.t === "+" ? "add" : ""}`}><span class="cb-sign">{row.right?.t === "+" ? "+" : " "}</span>{row.right?.s ?? ""}</td>
          </tr>}</For></tbody>
        </table>
        <table class="cb-diff-unified"><caption class="cb-sr">Unified diff</caption>
          <tbody><For each={unifiedRows(c().lines)}>{row => <tr>
            <td class="cb-ln">{row.a ?? ""}</td><td class="cb-ln">{row.b ?? ""}</td>
            <td class={`cb-dl ${row.t === "+" ? "add" : row.t === "-" ? "del" : ""}`}><span class="cb-sign">{row.t === "-" ? "−" : row.t}</span>{row.s}</td>
          </tr>}</For></tbody>
        </table>
      </div>
    </div>}
  </Show>;
}

// ---------------------------------------------------------------- bottom panel (one group only)
export function BottomPanel(props: { m: BModel }) {
  const m = props.m;
  const TABS: { id: BottomTab; label: string }[] = [{ id: "terminal", label: "Terminal" }, { id: "problems", label: "Problems" }, { id: "output", label: "Output" }];
  return <section id="cb-bottom" class="cb-bottom" aria-label="Bottom panel" tabindex="-1">
    <div class="cb-bottom-head">
      <TabStrip id="cb-bt" label="Bottom panel" tabs={TABS.map(t => ({ key: t.id, title: t.label }))} active={m.ui.bottomTab}
        onSelect={k => m.setUi("bottomTab", k as BottomTab)} onClose={() => m.setUi("bottomOpen", false)} />
      <button type="button" class="cb-icon-btn" aria-label="Close bottom panel" onClick={() => { m.setUi("bottomOpen", false); document.getElementById("cb-g1")?.focus(); }}><Glyph name="close" /></button>
    </div>
    <div id="cb-bt-panel" role="tabpanel" class="cb-tabpanel" aria-label={m.ui.bottomTab}>
      <ConnectionBanner m={m} />
      <Switch>
        <Match when={m.ui.bottomTab === "terminal"}><TerminalPanel m={m} /></Match>
        <Match when={m.ui.bottomTab === "problems"}><ProblemsPanel /></Match>
        <Match when={m.ui.bottomTab === "output"}><OutputPanel m={m} /></Match>
      </Switch>
    </div>
  </section>;
}
