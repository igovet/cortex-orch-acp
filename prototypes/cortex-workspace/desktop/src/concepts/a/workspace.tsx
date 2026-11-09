import { createEffect, createSignal, For, Match, on, Show, Switch, type JSX } from "solid-js";
import { useDesk } from "./ctx";
import { Glyph } from "./glyphs";
import { CHECK_STYLE, columnName, decisionCount, hhmm, stamp, TABS, type Check, type Decision, type Result, type TabId, type TaskView } from "./model";
import { CodeTab, GitTab } from "./code";
import { Badge, CheckBadge, ConnBanner, ExecBadge, LongText, MockButton, Receipt, WaitBadge } from "./ui";
import { VirtualList, type VirtualApi } from "./virtual";

export function EmptyBlock(props: { title: string; body?: string; children?: JSX.Element }) {
  return <div class="a-empty"><p class="a-empty-title">{props.title}</p><Show when={props.body}><p>{props.body}</p></Show>{props.children}</div>;
}

export function TaskWorkspace(props: { v: TaskView; tab: TabId }) {
  const desk = useDesk();
  let tablist!: HTMLDivElement;
  const machine = () => desk.store.state().fixture.machines[0];
  const onTabKey = (e: KeyboardEvent) => {
    const i = TABS.findIndex(t => t.id === props.tab), n = TABS.length;
    const next = e.key === "ArrowRight" ? (i + 1) % n : e.key === "ArrowLeft" ? (i - 1 + n) % n : e.key === "Home" ? 0 : e.key === "End" ? n - 1 : -1;
    if (next < 0) return;
    e.preventDefault();
    desk.setTab(TABS[next].id);
    tablist.querySelector<HTMLElement>(`#a-tab-${TABS[next].id}`)?.focus();
  };
  return <section class="a-ws" aria-labelledby="a-task-title">
    <ConnBanner area="ws" />
    <Receipt k={`task:${props.v.id}`} />
    <header class="a-ws-head">
      <h2 id="a-task-title" tabindex="-1" class="a-ws-title">{props.v.title}</h2>
      <p class="a-ws-meta">
        <span class="a-mono">{props.v.id}</span>
        <span>Phase: {columnName(props.v.column)} <span class="a-muted">· fixture projection</span></span>
        <span>Execution: <ExecBadge v={props.v} /> <span class="a-muted">
          {props.v.lastKnown ? `· Last known ${props.v.lastKnown} at ${hhmm(machine().lastConfirmedAt)} · current state unknown` : "· fixture"}
        </span></span>
        <Show when={props.v.wait || props.v.answered || props.v.stale}><span>Wait reason: <WaitBadge v={props.v} /></span></Show>
        <span class="a-muted">Last confirmed: {desk.conn() === "connected" ? "fixture state, no live connection" : stamp(machine().lastConfirmedAt)}</span>
      </p>
    </header>
    <div role="tablist" aria-label="Task views" class="a-tabs" ref={tablist} onKeyDown={onTabKey}>
      <For each={TABS}>{t => <button type="button" role="tab" id={`a-tab-${t.id}`} aria-selected={props.tab === t.id} aria-controls="a-tabpanel"
        tabindex={props.tab === t.id ? 0 : -1} onClick={() => desk.setTab(t.id)}>
        {t.label}{t.id === "decisions" ? ` (${decisionCount(props.v)})` : ""}
      </button>}</For>
    </div>
    <div role="tabpanel" id="a-tabpanel" class="a-tabpanel" aria-labelledby={`a-tab-${props.tab}`} tabindex="0">
      <Switch>
        <Match when={props.tab === "discussion"}><DiscussionTab v={props.v} /></Match>
        <Match when={props.tab === "decisions"}><DecisionsTab v={props.v} /></Match>
        <Match when={props.tab === "result"}><ResultTab v={props.v} /></Match>
        <Match when={props.tab === "code"}><CodeTab taskId={props.v.id} /></Match>
        <Match when={props.tab === "git"}><GitTab v={props.v} /></Match>
        <Match when={props.tab === "workers"}><WorkersTab v={props.v} /></Match>
      </Switch>
    </div>
  </section>;
}

const KIND: Record<string, string> = { discussion: "Message", plan: "Plan update", check: "Check event", snapshot: "Snapshot" };

function DiscussionTab(props: { v: TaskView }) {
  const desk = useDesk();
  const [scope, setScope] = createSignal<"task" | "project">("task");
  const [atEnd, setAtEnd] = createSignal(true);
  let api: VirtualApi | undefined;
  const events = () => desk.store.state().fixture.events;
  const items = () => scope() === "task" ? desk.events().byTask.get(props.v.id) ?? [] : desk.events().all;
  const plan = () => (desk.events().byTask.get(props.v.id) ?? []).filter(i => events()[i].kind === "plan");
  const jump = () => { api?.scrollToIndex(items().length - 1, "end"); setAtEnd(true); };
  createEffect(on([scope, () => props.v.id], () => queueMicrotask(jump)));
  return <div class="a-disc">
    <div class="a-disc-main">
      <section aria-labelledby="a-desc-h" class="a-desc">
        <h3 id="a-desc-h">Task description <span class="a-muted">· fixture</span></h3>
        <LongText text={props.v.description} id="a-desc-text" />
      </section>
      <div class="a-disc-tools">
        <fieldset class="a-radios">
          <legend>Timeline</legend>
          <label><input type="radio" name="a-scope" checked={scope() === "task"} onChange={() => setScope("task")} /> This task ({desk.events().byTask.get(props.v.id)?.length ?? 0} events)</label>
          <label><input type="radio" name="a-scope" checked={scope() === "project"} onChange={() => setScope("project")} /> Whole project ({desk.events().all.length.toLocaleString("en-US")} events)</label>
        </fieldset>
        <Show when={!atEnd()}><button type="button" onClick={jump}>Jump to latest</button></Show>
      </div>
      <Show when={items().length} fallback={<EmptyBlock title="No events yet." body={props.v.created ? "This mock task has not started. No agent was started." : "The fixture has no history for this task."} />}>
        <VirtualList items={items()} rowU={3.5} class="a-timeline" listRole="list" listLabel="Timeline events" rowRole="listitem"
          api={a => { api = a; }} onScrollChange={setAtEnd}
          render={i => {
            const e = () => events()[i()];
            return <div class="a-event">
              <span class="a-event-line"><span class="a-event-kind">{KIND[e().kind] ?? e().kind}</span> {e().text}</span>
              <span class="a-muted a-event-meta">#{e().sequence} · {e().occurredAt.slice(11, 19)} UTC · fixture{scope() === "project" ? ` · ${e().taskId}` : ""}</span>
            </div>;
          }} />
      </Show>
    </div>
    <section class="a-plan" aria-labelledby="a-plan-h">
      <h3 id="a-plan-h">Plan</h3>
      <Show when={plan().length} fallback={<p class="a-muted">No plan steps are recorded for this task in the fixture.</p>}>
        <p class="a-muted">{plan().length} plan updates · fixture. Step state is not in the fixture, so every step reads Unknown. No percentages or estimates.</p>
        <ol class="a-steps">
          <For each={plan().slice(0, 8)}>{i => <li><span>{events()[i].text}</span> <Badge tone="neutral" glyph="question">Unknown</Badge></li>}</For>
        </ol>
        <Show when={plan().length > 8}><p class="a-muted">{plan().length - 8} more plan updates are in the timeline.</p></Show>
      </Show>
    </section>
  </div>;
}

function DecisionsTab(props: { v: TaskView }) {
  return <div class="a-stack">
    <For each={props.v.decisions}>{d => <DecisionPanel d={d} v={props.v} />}</For>
    <Show when={props.v.result}>{r => <AcceptancePanel v={props.v} r={r()} />}</Show>
    <Show when={!props.v.decisions.length && !props.v.result}>
      <EmptyBlock title="Nothing needs your decision for this task." body="Questions, permission requests and results for review appear here." />
    </Show>
  </div>;
}

export function DecisionPanel(props: { d: Decision; v: TaskView }) {
  const desk = useDesk();
  const [choice, setChoice] = createSignal("");
  const answer = () => desk.store.state().responses[props.d.id];
  const viaPhone = () => desk.ui.phoneAnswers.includes(props.d.id);
  const isQuestion = () => props.d.kind === "question";
  const options = () => "options" in props.d ? props.d.options : [];
  const decide = (a: string) => {
    const out = desk.store.decide(props.d.id, a);
    desk.setReceipt(`dec:${props.d.id}`, `Mock outcome: ${out.message} Recorded in memory only; no agent was started and the fixture's execution facts are unchanged.`, true);
  };
  const hid = `a-dec-${props.d.id}`;
  return <section class="a-decision" aria-labelledby={hid}>
    <p class="a-dec-type"><Glyph name={isQuestion() ? "message" : "key"} /> {isQuestion() ? "Question" : "Permission"}</p>
    <h3 id={hid}>{props.d.title}</h3>
    <p class="a-muted">{props.v.id} · {props.v.title} · requested by {props.v.execution?.id ?? "unknown execution"} → {props.v.execution?.sessionId ?? "unknown worker"} · requested at: not in fixture · decision v{props.d.version}</p>
    <dl class="a-facts">
      <dt>Effect</dt><dd>{props.d.effect}</dd>
      <dt>Scope</dt><dd>{props.d.scope}</dd>
      <dt>If you reject</dt><dd>{props.d.rejection} Nothing is reverted.</dd>
    </dl>
    <Switch fallback={
      <Show when={isQuestion()} fallback={<>
        <div class="a-actions">
          <MockButton label="Approve once" tone="primary" reason={desk.block()} onClick={() => decide("Approve once")} />
          <MockButton label="Deny" reason={desk.block()} onClick={() => decide("Deny")} />
        </div>
        <p class="a-muted">Always allow is not offered: the fixture does not mark it as negotiated. There is no approve-all.</p>
      </>}>
        <fieldset class="a-radios">
          <legend>Your answer</legend>
          <For each={options()}>{o => <label><input type="radio" name={`a-ans-${props.d.id}`} value={o} checked={choice() === o} onChange={() => setChoice(o)} /> {o}</label>}</For>
        </fieldset>
        <div class="a-actions">
          <MockButton label="Send answer" tone="primary" reason={desk.block() ?? (choice() ? null : "Choose an answer first.")} onClick={() => decide(choice())} />
        </div>
      </Show>
    }>
      <Match when={answer() && viaPhone()}>
        <p class="a-state" role="status">Answered on Phone (mock): {answer()}. Your choice on this desktop was not applied.</p>
      </Match>
      <Match when={answer()}>
        <p class="a-state">Answered (mock): {answer()}. Execution facts are unchanged (the fixture still reports {props.v.execution?.state ?? "no execution"}).</p>
      </Match>
    </Switch>
    <Receipt k={`dec:${props.d.id}`} />
  </section>;
}

export function evidenceFor(fixtureChecks: readonly Check[], v: TaskView) {
  const ids = new Set([...(v.result?.checkIds ?? []), ...v.checks.map(c => c.id)]);
  return fixtureChecks.filter(c => ids.has(c.id)).sort((a, b) => CHECK_STYLE[a.outcome].order - CHECK_STYLE[b.outcome].order);
}

export function AcceptancePanel(props: { v: TaskView; r: Result }) {
  const desk = useDesk();
  const checks = () => evidenceFor(desk.store.state().fixture.checks, props.v);
  const count = (o: Check["outcome"]) => checks().filter(c => c.outcome === o).length;
  const request = () => desk.ui.requests[props.v.id];
  const stale = () => props.r.state === "stale";
  const acceptReason = () => stale() ? `Result is stale: the workspace changed after it (digest ${props.r.artifactDigest} → ${props.r.workspaceDigest}).`
    : props.v.accepted ? "Already accepted (mock) for this digest." : desk.block();
  const key = `acc:${props.r.id}`;
  const accept = () => desk.setReceipt(key, `Mock outcome: ${desk.store.acceptResult(props.r.id).message} No artifact was accepted for real.`, true);
  return <section class="a-decision" aria-labelledby={`a-acc-${props.r.id}`}>
    <p class="a-dec-type"><Glyph name="filecheck" /> Result acceptance</p>
    <h3 id={`a-acc-${props.r.id}`}>Review result {props.r.id} revision {props.r.version}</h3>
    <p class="a-muted">{props.v.id} · {props.v.title} · artifact {props.r.artifactId} · produced at: not in fixture</p>
    <dl class="a-facts">
      <dt>Effect</dt><dd>Records acceptance of exactly this result digest in memory (mock). It is not runtime evidence.</dd>
      <dt>Scope</dt><dd>Result {props.r.id} v{props.r.version}; artifact digest <code class="a-mono">{props.r.artifactDigest}</code>; workspace digest <code class="a-mono">{props.r.workspaceDigest}</code>; 1 file in the fixture.</dd>
      <dt>If you reject</dt><dd>The result stays unaccepted and the task stays in Result review. Nothing is reverted.</dd>
      <dt>Evidence</dt><dd class="a-counts">
        <CheckBadge outcome="passed" /> {count("passed")} · <CheckBadge outcome="failed" /> {count("failed")} · <CheckBadge outcome="not_run" /> {count("not_run")} · <Badge tone="neutral" glyph="question">Unverified</Badge> 0
        <Show when={count("not_run")}><span class="a-block-line">Did not run: {checks().filter(c => c.outcome === "not_run").map(c => c.name).join(", ")}.</span></Show>
      </dd>
    </dl>
    <Show when={stale()}>
      <p class="a-state a-state-warn"><Glyph name="history" /> This result changed after it was requested (digest {props.r.artifactDigest} → {props.r.workspaceDigest}). Review the new result.</p>
    </Show>
    <Switch>
      <Match when={props.v.accepted}><p class="a-state a-state-ok"><Glyph name="flask" /> Accepted (mock) · {props.r.id} · digest {props.r.artifactDigest}</p></Match>
      <Match when={request() === "changes"}><p class="a-state">Changes requested (mock). No agent was started.</p></Match>
      <Match when={request() === "rejected"}><p class="a-state">Rejected (mock). Nothing was reverted.</p></Match>
    </Switch>
    <div class="a-actions">
      <MockButton label={`Accept ${props.r.id}`} tone="primary" reason={acceptReason()} onClick={accept} />
      <MockButton label="Request changes" reason={desk.block()} onClick={() => {
        desk.setUi("requests", props.v.id, "changes");
        desk.setReceipt(key, "Mock outcome: change request recorded in memory only. No agent was started.", true);
      }} />
      <MockButton label="Reject" tone="danger" reason={desk.block()} onClick={() => desk.confirm({
        title: `Reject result ${props.r.id}?`, confirmLabel: "Reject result",
        effect: "Records a mock rejection in memory. Nothing is reverted, and no agent is started.",
        scope: `Result ${props.r.id} v${props.r.version}, digest ${props.r.artifactDigest}`,
        onConfirm: () => { desk.setUi("requests", props.v.id, "rejected"); desk.setReceipt(key, "Mock outcome: rejection recorded in memory only. Nothing was reverted; not runtime evidence.", true); },
      })} />
    </div>
    <p class="a-footnote">Accepting does not commit, push or deploy.</p>
    <Receipt k={key} />
  </section>;
}

function ResultTab(props: { v: TaskView }) {
  const desk = useDesk();
  const checks = () => evidenceFor(desk.store.state().fixture.checks, props.v);
  const file = () => desk.store.state().fixture.files[0];
  const notRun = () => checks().filter(c => c.outcome === "not_run");
  return <div class="a-stack">
    <Show when={!props.v.result && !checks().length}>
      <EmptyBlock title="No result yet." body="Results and their evidence appear here after an execution reports one. Nothing is shown as passed without a check record." />
    </Show>
    <Show when={props.v.result}>{r => <section class="a-res-head" aria-labelledby="a-res-h">
      <h3 id="a-res-h">Result {r().id}</h3>
      <dl class="a-facts a-facts--grid">
        <dt>Revision</dt><dd>v{r().version}</dd>
        <dt>Workspace</dt><dd>Shared fixture worktree (path not in fixture)</dd>
        <dt>Artifact digest</dt><dd><code class="a-mono">{r().artifactDigest}</code></dd>
        <dt>Workspace digest</dt><dd><code class="a-mono">{r().workspaceDigest}</code></dd>
        <dt>Produced at</dt><dd>Not in fixture</dd>
        <dt>Acceptance</dt><dd>{props.v.accepted ? `Accepted (mock) for digest ${r().artifactDigest}` : r().state === "stale" ? "Not accepted; acceptance disabled because the result is stale" : "Not reviewed"}</dd>
      </dl>
      <Show when={r().state === "stale"}>
        <div class="a-banner a-banner-warn" role="group" aria-label="Stale result">
          <Glyph name="history" />
          <p><strong>Result changed after review:</strong> digest {r().artifactDigest} → {r().workspaceDigest}. {props.v.accepted ? "Previous acceptance (mock) is invalidated." : "No acceptance exists for it, and acceptance is disabled."}</p>
          <span class="a-mock-action">
            <button type="button" disabled aria-describedby="a-stale-why">Review new result</button>
            <span class="a-reason" id="a-stale-why">The fixture has no newer result revision yet.</span>
          </span>
        </div>
      </Show>
    </section>}</Show>
    <Show when={checks().length}>
      <section aria-labelledby="a-checks-h">
        <h3 id="a-checks-h">Checks</h3>
        <div class="a-scroll-x">
          <table class="a-grid-table">
            <thead><tr><th scope="col">Check</th><th scope="col">Outcome</th><th scope="col">Source</th><th scope="col">Duration</th><th scope="col">Details</th></tr></thead>
            <tbody><For each={checks()}>{c => <tr>
              <th scope="row">{c.name}</th>
              <td><CheckBadge outcome={c.outcome} /> <span class="a-muted">· fixture</span></td>
              <td>Fixture record; no receipt</td><td>Not in fixture</td><td>{c.detail}</td>
            </tr>}</For></tbody>
          </table>
        </div>
        <h4>Did not run</h4>
        <Show when={notRun().length} fallback={<p class="a-muted">No check for this result is marked not run in the fixture.</p>}>
          <ul><For each={notRun()}>{c => <li>{c.name} not run. Required flag is not in the fixture.</li>}</For></ul>
        </Show>
        <Show when={!props.v.result && props.v.failedCheck}>
          <p>No result revision exists in the fixture for this task; the failed check is listed first.</p>
          <div class="a-actions">
            <MockButton label="Request changes" reason={desk.block()} onClick={() => {
              desk.setUi("requests", props.v.id, "changes");
              desk.setReceipt(`chk:${props.v.id}`, "Mock outcome: change request recorded in memory only. No agent was started.", true);
            }} />
          </div>
          <Receipt k={`chk:${props.v.id}`} />
        </Show>
      </section>
    </Show>
    <Show when={props.v.result}>{r => <>
      <section aria-labelledby="a-chg-h">
        <h3 id="a-chg-h">Changes</h3>
        <p class="a-muted">The shared fixture carries one file, shown for every result (fixture limitation). Lines come from its base and local text.</p>
        <ul class="a-files"><li><span class="a-mono">{file().path}</span> · Modified · <span class="a-add-text">+1</span> <span class="a-del-text">−1</span></li></ul>
        <div class="a-diff" role="group" aria-label={`Diff of ${file().path}`}>
          <div class="a-diff-split">
            <div><p class="a-diff-cap">Before (base)</p><pre class="a-code"><span class="a-line a-del"><span class="a-gut">−</span>{file().base}</span></pre></div>
            <div><p class="a-diff-cap">After (local)</p><pre class="a-code"><span class="a-line a-add"><span class="a-gut">+</span>{file().local}</span></pre></div>
          </div>
          <pre class="a-code a-diff-unified"><span class="a-line a-del"><span class="a-gut">−</span>{file().base}</span>{"\n"}<span class="a-line a-add"><span class="a-gut">+</span>{file().local}</span></pre>
        </div>
      </section>
      <AcceptancePanel v={props.v} r={r()} />
    </>}</Show>
  </div>;
}

function WorkersTab(props: { v: TaskView }) {
  const desk = useDesk();
  const [open, setOpen] = createSignal(false);
  const live = () => props.v.exec === "Running" || props.v.exec === "Waiting";
  return <Show when={props.v.execution} fallback={
    <EmptyBlock title={props.v.created ? "Not started (mock)" : "No executions"} body="Nothing has started for this task, so no worker exists." />}>{ex =>
    <div class="a-stack">
      <div class="a-scroll-x">
        <table class="a-grid-table">
          <caption>Execution {ex().id} → attempts → workers</caption>
          <thead><tr><th scope="col">Worker (session)</th><th scope="col">Attempt</th><th scope="col">State · provenance</th><th scope="col">Last event</th><th scope="col">Detail</th></tr></thead>
          <tbody><tr>
            <th scope="row" class="a-mono">{ex().sessionId}</th>
            <td>1 (attempts not modelled in the fixture)</td>
            <td><ExecBadge v={props.v} /> <span class="a-muted">· fixture</span></td>
            <td>{props.v.updatedAt ? `${hhmm(props.v.updatedAt)} · fixture` : "Unknown"}</td>
            <td><button type="button" aria-expanded={open()} aria-controls="a-worker-detail" onClick={() => setOpen(!open())}>{open() ? "Hide worker detail" : "Show worker detail"}</button></td>
          </tr></tbody>
        </table>
      </div>
      <Show when={open()}>
        <section id="a-worker-detail" aria-labelledby="a-wd-h" class="a-panel">
          <h3 id="a-wd-h">Worker {ex().sessionId}</h3>
          <dl class="a-facts">
            <dt>Role</dt><dd>Not exposed in the fixture</dd>
            <dt>Engine and model</dt><dd>Not exposed in the fixture</dd>
            <dt>State</dt><dd>{props.v.exec}{props.v.lastKnown ? ` (last known ${props.v.lastKnown})` : ""} · fixture</dd>
            <dt>Tool events</dt><dd>Not in fixture</dd>
            <dt>Permissions requested</dt><dd>{props.v.decisions.filter(d => d.kind === "permission").map(d => d.title).join("; ") || "None"}</dd>
          </dl>
        </section>
      </Show>
      <Show when={live()}>
        <div class="a-actions">
          <MockButton label="Cancel execution" tone="danger" reason={desk.block()} onClick={() => desk.confirm({
            title: `Cancel execution ${ex().id}?`, confirmLabel: "Cancel execution",
            effect: "Records a mock cancellation in memory. No process is stopped and no agent is contacted.",
            scope: `Execution ${ex().id}, worker ${ex().sessionId}`,
            onConfirm: () => desk.setReceipt(`cancel:${ex().id}`, `Mock outcome: no execution was cancelled. Recorded in memory only; the fixture still reports ${ex().state}.`, true),
          })} />
        </div>
        <Receipt k={`cancel:${ex().id}`} />
      </Show>
    </div>}
  </Show>;
}
