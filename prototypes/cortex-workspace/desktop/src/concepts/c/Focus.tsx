import { createMemo, createSignal, For, Match, Show, Switch, type JSX } from "solid-js";
import type { Task } from "../../fixtures";
import { useC, uid } from "./ctx";
import {
  checksForTask, clock, CONFLICT_TASK_ID, countChecks, decisionFor, eventsForTask, evidenceLine, executionFor,
  executionGlyph, executionLabel, executionTone, fileDiffs, hasChanges, isCreated, phaseLabel, resultFor, waitLabel,
  type Decision, type Result,
} from "./model";
import { Actions, Badge, CheckTable, LongText, MockButton, OutcomeBadge, Receipt } from "./ui";

/** The answered action buttons are removed; keep keyboard focus in the decision instead of the page body. */
const keepFocus = () => queueMicrotask(() => document.getElementById("c-focus-heading")?.focus());

export const SECTION_TITLES = ["Why this is asked", "Plan", "Result & evidence", "Changes", "Code & Git", "Workers"] as const;

/** L1 + L2 for one task. Keyed by task so local inputs reset when the task changes. */
export function TaskView(p: { task: Task }) {
  const c = useC();
  const f = () => c.store.state().fixture;
  return <div class="c-taskview">
    <Switch fallback={<TaskHeader task={p.task} />}>
      <Match when={decisionFor(f(), p.task.id)}>{d => <AskDecision task={p.task} d={d()} />}</Match>
      <Match when={resultFor(f(), p.task.id)}>{r => <AcceptDecision task={p.task} r={r()} />}</Match>
    </Switch>
    <Sections task={p.task} />
  </div>;
}

function RequestLine(p: { task: Task; version?: string }) {
  const c = useC();
  const e = () => executionFor(c.store.state().fixture, p.task.id);
  return <p class="c-meta">
    <span class="c-mono">{p.task.id}</span> · {p.task.title} · Requested by{" "}
    <Show when={e()} fallback="no execution">{x => <><span class="c-mono">{x().id}</span> → <span class="c-mono">{x().sessionId}</span></>}</Show>
    {" "}· Requested: unknown<Show when={p.version}> · {p.version}</Show>
  </p>;
}

function AskDecision(p: { task: Task; d: Decision }) {
  const c = useC();
  const answer = () => c.store.state().responses[p.d.id];
  const options = () => ("options" in p.d ? p.d.options : []) as readonly string[];
  const [choice, setChoice] = createSignal("");
  const [error, setError] = createSignal("");
  const errorId = uid("err");
  const group = uid("answer");
  const rkey = `decision:${p.d.id}`;
  const label = p.d.kind === "permission" ? "Permission" : "Question";
  const respond = (value: string) => {
    const outcome = c.store.decide(p.d.id, value);
    c.setReceipt(rkey, `Mock outcome: ${outcome.message} Recorded in memory only. No agent was started.`);
    c.announce(`${label} answered (mock): ${value}. Recorded in memory only.`);
    keepFocus();
  };
  const send = () => {
    if (!choice()) { setError("Choose an answer before sending."); return; }
    setError("");
    respond(choice());
  };
  return <section class="c-decision" classList={{ blocking: !answer() }} aria-labelledby="c-focus-heading">
    <p class="c-band"><span class="c-glyph" aria-hidden="true">{p.d.kind === "permission" ? "◆" : "?"}</span>{label}
      <Show when={!answer()} fallback=" · answered (mock)"> · {p.d.kind === "permission" ? "Your permission needed" : "Your answer needed"}</Show></p>
    <div class="c-decision-body">
      <h2 id="c-focus-heading" class="c-title-xl" tabindex="-1">{p.d.title}</h2>
      <RequestLine task={p.task} version={`Decision ${p.d.id} v${p.d.version}`} />
      <dl class="c-facts">
        <div><dt>Effect</dt><dd>{p.d.effect}</dd></div>
        <div><dt>Scope</dt><dd>{p.d.scope} · decision <span class="c-mono">{p.d.id}</span> version {p.d.version}</dd></div>
        <div><dt>If you reject</dt><dd>{p.d.rejection} Nothing is reverted.</dd></div>
      </dl>
      <Show when={!answer()} fallback={<p class="c-answered" role="status">Answered (mock): {answer()}. Recorded in memory only; no authorization was granted and no agent was started.</p>}>
        <Show when={p.d.kind === "question"}>
          <fieldset class="c-fieldset" aria-describedby={error() ? errorId : undefined} aria-invalid={error() ? true : undefined}>
            <legend>Your answer</legend>
            <For each={options()}>{option => <label class="c-radio">
              <input type="radio" name={group} value={option} checked={choice() === option} onChange={() => { setChoice(option); setError(""); }} /> {option}
            </label>}</For>
          </fieldset>
          <Show when={error()}><p id={errorId} class="c-field-error">{error()}</p></Show>
        </Show>
        <Actions reason={c.blocked()}>{reasonId => <Show when={p.d.kind === "permission"} fallback={
          <MockButton label="Send answer" tone="primary" reason={c.blocked()} reasonId={reasonId} onAct={send} />}>
          <MockButton label="Approve once" tone="primary" reason={c.blocked()} reasonId={reasonId} onAct={() => respond("Approved once")} />
          <MockButton label="Deny" reason={c.blocked()} reasonId={reasonId} onAct={() => respond("Denied")} />
        </Show>}</Actions>
      </Show>
      <Receipt rkey={rkey} />
    </div>
  </section>;
}

function AcceptDecision(p: { task: Task; r: Result }) {
  const c = useC();
  const f = () => c.store.state().fixture;
  const stale = () => p.r.state === "stale";
  const accepted = () => c.store.state().acceptedResultIds.includes(p.r.id);
  const checks = () => f().checks.filter(ch => (p.r.checkIds as readonly string[]).includes(ch.id));
  const counts = () => countChecks(checks());
  const staleId = uid("stale");
  const rkey = `result:${p.r.id}`;
  const staleReason = "Stale result: the workspace changed after this result was produced. Acceptance is unavailable.";
  const acceptReason = () => c.blocked() ?? (stale() ? staleReason : null);
  const accept = () => {
    if (stale()) return; // stale decisions never reach acceptance
    const outcome = c.store.acceptResult(p.r.id);
    c.setReceipt(rkey, `Mock outcome: ${outcome.message} No artifact was accepted, committed, pushed or deployed.`);
    c.announce(`Accepted (mock) ${p.r.id}. Not runtime evidence.`);
    keepFocus();
  };
  const requestChanges = () => {
    c.setReceipt(rkey, "Mock outcome: change request recorded in memory only. No agent was started.");
  };
  const reject = async () => {
    const ok = await c.confirm({
      title: `Reject result ${p.r.id}? (mock)`, effect: "Marks the result as rejected in this prototype only.",
      scope: `Result ${p.r.id} v${p.r.version}, artifact ${p.r.artifactId}. Nothing is reverted.`, confirmLabel: "Reject",
    });
    c.setReceipt(rkey, ok ? "Mock outcome: rejection recorded in memory only. Nothing was reverted; no agent was started." : "Reject cancelled. Nothing changed.");
  };
  const reviewNew = () => {
    c.toggleSection(p.task.id, 3, true);
    document.querySelector<HTMLElement>('[data-l2="3"]')?.focus();
  };
  return <section class="c-decision" classList={{ blocking: !accepted() && !stale(), stale: stale() }} aria-labelledby="c-focus-heading">
    <p class="c-band"><span class="c-glyph" aria-hidden="true">{stale() ? "↺" : "▣"}</span>Result acceptance
      {stale() ? " · Stale" : accepted() ? " · Accepted (mock)" : " · Result review needed"}</p>
    <div class="c-decision-body">
      <Show when={stale()}>
        <div class="c-banner warn" role="note">
          <p id={staleId}><strong>Result changed after review:</strong> digest <span class="c-mono">{p.r.artifactDigest}</span> → <span class="c-mono">{p.r.workspaceDigest}</span>. Previous acceptance (mock) is invalidated. Acceptance is unavailable.</p>
          <button type="button" class="c-btn" onClick={reviewNew}>Review new result</button>
        </div>
      </Show>
      <h2 id="c-focus-heading" class="c-title-xl" tabindex="-1">Accept result {p.r.id}?</h2>
      <RequestLine task={p.task} version={`Result v${p.r.version}`} />
      <dl class="c-facts">
        <div><dt>Effect</dt><dd>Records a mock acceptance of artifact <span class="c-mono">{p.r.artifactId}</span> in memory. It does not commit, push or deploy.</dd></div>
        <div><dt>Scope</dt><dd>Result <span class="c-mono">{p.r.id}</span> v{p.r.version} · artifact digest <span class="c-mono">{p.r.artifactDigest}</span> · workspace digest <span class="c-mono">{p.r.workspaceDigest}</span> · {checks().length} check record(s)</dd></div>
        <div><dt>If you reject</dt><dd>The result stays unaccepted and the task stays in review. Nothing is reverted.</dd></div>
        <div><dt>Evidence</dt><dd>
          <span class="c-evidence"><OutcomeBadge outcome="passed" /> {counts().passed} · <OutcomeBadge outcome="failed" /> {counts().failed} · <OutcomeBadge outcome="not_run" /> {counts().not_run} · <OutcomeBadge outcome="unverified" /> {counts().unverified}</span>
          <Show when={checks().filter(ch => ch.outcome === "not_run").length}>
            <span class="c-block">Required checks that did not run: {checks().filter(ch => ch.outcome === "not_run").map(ch => ch.name).join(", ")}.</span>
          </Show>
          <span class="c-block c-muted">Source: fixture check records, not runtime evidence.</span>
        </dd></div>
      </dl>
      <Show when={!accepted()} fallback={<p class="c-answered" role="status">Accepted (mock) · <span class="c-mono">{p.r.id}</span> · digest <span class="c-mono">{p.r.artifactDigest}</span>. Not runtime evidence.</p>}>
        <Actions reason={c.blocked()}>{reasonId => <>
          <MockButton label={`Accept ${p.r.id}`} tone="primary" reason={acceptReason()} reasonId={c.blocked() ? reasonId : staleId} onAct={accept} />
          <MockButton label="Request changes" reason={c.blocked()} reasonId={reasonId} onAct={requestChanges} />
          <MockButton label="Reject" tone="danger" reason={c.blocked()} reasonId={reasonId} onAct={() => void reject()} />
        </>}</Actions>
      </Show>
      <p class="c-footnote">Accepting does not commit, push or deploy.</p>
      <Receipt rkey={rkey} />
    </div>
  </section>;
}

function TaskHeader(p: { task: Task }) {
  const c = useC();
  const f = () => c.store.state().fixture;
  const e = () => executionFor(f(), p.task.id);
  const label = () => executionLabel(e(), c.conn());
  const failed = () => checksForTask(f(), p.task.id).filter(ch => ch.outcome === "failed");
  const rkey = `task:${p.task.id}`;
  return <section class="c-taskhead" aria-labelledby="c-focus-heading">
    <p class="c-type">Task</p>
    <h2 id="c-focus-heading" class="c-title-lg" tabindex="-1">{p.task.title}</h2>
    <p class="c-meta"><span class="c-mono">{p.task.id}</span> · Phase: {phaseLabel(p.task.phase)} (fixture)</p>
    <p class="c-statusline">
      <Badge tone={executionTone(label())} glyph={executionGlyph(label())} label={`${label()}${isCreated(p.task.id) ? " (mock)" : ""}`} />
      <Show when={waitLabel(e())}>{w => <>{" "}<Badge tone="warn" glyph="!" label={w()} /></>}</Show>
      <Show when={label() === "Unknown"}><span class="c-muted"> Last confirmed running; current state unknown.</span></Show>
    </p>
    <LongText text={p.task.description} label="description" />
    <Show when={failed().length}>
      <div class="c-banner danger">
        <For each={failed()}>{check => <p><OutcomeBadge outcome="failed" />{" "}<strong>{check.name}</strong> failed · fixture record · {check.detail}</p>}</For>
        <Actions reason={c.blocked()}>{reasonId => <>
          <button type="button" class="c-btn" onClick={() => { c.toggleSection(p.task.id, 3, true); document.querySelector<HTMLElement>('[data-l2="3"]')?.focus(); }}>Open log</button>
          <MockButton label="Request changes" reason={c.blocked()} reasonId={reasonId}
            onAct={() => c.setReceipt(rkey, "Mock outcome: change request recorded in memory only. No agent was started.")} />
        </>}</Actions>
      </div>
    </Show>
    <Show when={p.task.id === CONFLICT_TASK_ID}>
      <div class="c-banner danger">
        <p><Badge tone="danger" glyph="≠" label="Conflict" />{" "}Merge stopped: {f().git.fileIds.length} file has conflicts on <span class="c-mono">{f().git.branch}</span>. {f().git.detail}</p>
        <div class="c-actions">
          <button type="button" class="c-btn" onClick={() => c.openDeep("code")}>Open editor</button>
          <button type="button" class="c-btn" onClick={() => c.openDeep("git")}>Open Git</button>
        </div>
      </div>
    </Show>
    <Receipt rkey={`created:${p.task.id}`} />
    <Receipt rkey={rkey} />
  </section>;
}

function Sections(p: { task: Task }) {
  const c = useC();
  const f = () => c.store.state().fixture;
  const id = () => p.task.id;
  const summaries = createMemo(() => {
    const fx = f();
    const d = decisionFor(fx, id());
    const r = resultFor(fx, id());
    const e = executionFor(fx, id());
    const events = eventsForTask(fx, id());
    const discussion = events.filter(ev => ev.kind === "discussion").length;
    const plan = events.filter(ev => ev.kind === "plan").length;
    const checks = checksForTask(fx, id());
    return [
      d ? `${d.kind === "permission" ? "Permission" : "Question"}: ${d.title}` : r ? "A result needs review before acceptance" : `${discussion} discussion entries · fixture`,
      plan ? `${plan} plan entries reported · fixture` : e ? "No plan entries in the fixture" : "Not started; no plan reported",
      r ? `Result ${r.id} v${r.version}: ${evidenceLine(checks)} · fixture${r.state === "stale" ? " · Stale" : ""}`
        : checks.length ? `${evidenceLine(checks)} · fixture` : "No result yet",
      hasChanges(fx, id()) ? `${fx.files.length} file changed · ${fx.files.map(file => file.path).join(", ")}` : "No changes recorded",
      `Branch ${fx.git.branch} · merge conflict (${fx.git.fileIds.length} file) · not committed`,
      e ? `${e.id} · ${executionLabel(e, c.conn())}` : "No execution",
    ];
  });
  const bodies: (() => JSX.Element)[] = [
    () => <WhyBody task={p.task} />, () => <PlanBody task={p.task} />, () => <ResultBody task={p.task} />,
    () => <ChangesBody task={p.task} />, () => <CodeGitBody task={p.task} />, () => <WorkersBody task={p.task} />,
  ];
  return <div class="c-sections" role="group" aria-label="Task details">
    <For each={SECTION_TITLES}>{(title, i) => {
      const n = i() + 1;
      const bodyId = uid("l2");
      const open = () => c.sectionOpen(id(), n);
      return <div class="c-l2">
        <h3 class="c-l2-head">
          <button type="button" class="c-l2-toggle" data-l2={n} aria-expanded={open()} aria-controls={bodyId}
            aria-keyshortcuts={`Alt+${n}`} onClick={() => c.toggleSection(id(), n)}>
            <span class="c-l2-title"><span class="c-chev" aria-hidden="true">{open() ? "▾" : "▸"}</span>{title}<span class="c-kbd" aria-hidden="true">Alt+{n}</span></span>
            <span class="c-l2-summary">{summaries()[n - 1]}</span>
          </button>
        </h3>
        <div id={bodyId} class="c-l2-body" hidden={!open()}><Show when={open()}>{bodies[n - 1]()}</Show></div>
      </div>;
    }}</For>
  </div>;
}

function L3Link(p: { label: string; onClick: () => void }) {
  return <button type="button" class="c-link c-l3" onClick={() => p.onClick()}>{p.label} <span aria-hidden="true">→</span></button>;
}

function WhyBody(p: { task: Task }) {
  const c = useC();
  const f = () => c.store.state().fixture;
  const d = () => decisionFor(f(), p.task.id);
  const discussion = () => eventsForTask(f(), p.task.id).filter(e => e.kind === "discussion");
  return <div>
    <Show when={d()}>{x => <p>The worker asks: <strong>{x().title}</strong> Effect: {x().effect} Scope: {x().scope}.</p>}</Show>
    <Show when={!d() && resultFor(f(), p.task.id)}><p>A result was produced and needs your review before it can be accepted (mock).</p></Show>
    <LongText text={p.task.description} label="message" />
    <Show when={discussion().length} fallback={<p class="c-muted">No discussion entries in the fixture for this task.</p>}>
      <ol class="c-events"><For each={discussion().slice(0, 5)}>{e =>
        <li><span class="c-mono">{clock(e.occurredAt)}</span> · {e.text} <span class="c-prov">· fixture</span></li>}</For></ol>
      <p class="c-muted">Showing 5 of {discussion().length} discussion entries.</p>
    </Show>
    <L3Link label="Open full discussion" onClick={() => c.openDeep("discussion")} />
  </div>;
}

function PlanBody(p: { task: Task }) {
  const c = useC();
  const f = () => c.store.state().fixture;
  const plan = () => eventsForTask(f(), p.task.id).filter(e => e.kind === "plan");
  return <div>
    <Show when={plan().length} fallback={<p class="c-muted">{executionFor(f(), p.task.id) ? "No plan entries in the fixture for this task." : "Not started; no plan has been reported. Viewing a task never starts an agent."}</p>}>
      <p>{plan().length} plan entries reported (fixture). No percentage or ETA is estimated.</p>
      <ol class="c-events"><For each={plan().slice(0, 6)}>{(e, i) =>
        <li>Step {i() + 1}: {e.text} <span class="c-prov">· reported · {clock(e.occurredAt)}</span></li>}</For></ol>
      <Show when={plan().length > 6}><p class="c-muted">{plan().length - 6} more in the full discussion.</p></Show>
    </Show>
  </div>;
}

function ResultBody(p: { task: Task }) {
  const c = useC();
  const f = () => c.store.state().fixture;
  const r = () => resultFor(f(), p.task.id);
  const checks = () => checksForTask(f(), p.task.id);
  return <Show when={r()} fallback={<Show when={checks().length} fallback={<p class="c-muted">No result has been produced for this task (fixture).</p>}>
    <CheckTable checks={checks()} />
  </Show>}>{result => <div>
    <dl class="c-facts compact">
      <div><dt>Result</dt><dd><span class="c-mono">{result().id}</span> · revision v{result().version}</dd></div>
      <div><dt>Artifact</dt><dd><span class="c-mono">{result().artifactId}</span> · digest <span class="c-mono">{result().artifactDigest}</span></dd></div>
      <div><dt>Workspace</dt><dd>digest <span class="c-mono">{result().workspaceDigest}</span></dd></div>
      <div><dt>Produced at</dt><dd>unknown (not in the fixture)</dd></div>
      <div><dt>Acceptance</dt><dd>{c.store.state().acceptedResultIds.includes(result().id) ? "Accepted (mock)" : result().state === "stale" ? "Unavailable (stale)" : "Not accepted"}</dd></div>
    </dl>
    <Show when={result().state === "stale"}>
      <p class="c-banner warn">No newer result exists in the fixture for workspace digest <span class="c-mono">{result().workspaceDigest}</span>; acceptance stays unavailable.</p>
    </Show>
    <CheckTable checks={checks()} />
  </div>}</Show>;
}

function ChangesBody(p: { task: Task }) {
  const c = useC();
  const f = () => c.store.state().fixture;
  return <Show when={hasChanges(f(), p.task.id)} fallback={<p class="c-muted">No changes recorded for this task in the fixture.</p>}>
    <table class="c-table">
      <caption class="c-sr">Changed files</caption>
      <thead><tr><th scope="col">File</th><th scope="col">Change</th><th scope="col">Added</th><th scope="col">Removed</th></tr></thead>
      <tbody><For each={fileDiffs(f())}>{d => <tr>
        <th scope="row" class="c-mono">{d.file.path}</th><td>Modified</td>
        <td>+{d.lines.filter(l => l.kind === "add").length}</td><td>−{d.lines.filter(l => l.kind === "del").length}</td>
      </tr>}</For></tbody>
    </table>
    <p class="c-muted">Synthetic diff derived from the fixture file record (base → local).</p>
    <L3Link label="Open full diff" onClick={() => c.openDeep("diff")} />
  </Show>;
}

function CodeGitBody(p: { task: Task }) {
  const c = useC();
  const f = () => c.store.state().fixture;
  const r = () => resultFor(f(), p.task.id);
  const accepted = () => !!r() && c.store.state().acceptedResultIds.includes(r()!.id);
  return <div>
    <dl class="c-facts compact">
      <div><dt>Branch</dt><dd class="c-mono">{f().git.branch}</dd></div>
      <div><dt>Git state</dt><dd><Badge tone="danger" glyph="≠" label="Merge conflict" /> {f().git.fileIds.length} file · fixture</dd></div>
      <div><dt>Working changes</dt><dd>{hasChanges(f(), p.task.id) ? `${f().files.length} file changed, not committed` : "None recorded for this task"}{accepted() ? " · mock acceptance did not commit" : ""}</dd></div>
      <div><dt>Conflict resolution</dt><dd>{c.store.state().conflictResolution ? `Mock choice recorded: ${c.store.state().conflictResolution}. No file or Git mutation occurred.` : "None recorded"}</dd></div>
      <For each={f().capabilities}>{cap => <div><dt>{cap.name}</dt><dd><Badge tone="neutral" glyph="⊘" label={cap.available ? "Available" : "Unavailable"} /> {cap.reason}</dd></div>}</For>
    </dl>
    <div class="c-actions">
      <L3Link label="Open editor" onClick={() => c.openDeep("code")} />
      <L3Link label="Open Git" onClick={() => c.openDeep("git")} />
    </div>
  </div>;
}

function WorkersBody(p: { task: Task }) {
  const c = useC();
  const f = () => c.store.state().fixture;
  const e = () => executionFor(f(), p.task.id);
  return <Show when={e()} fallback={<p class="c-muted">No execution for this task. Creating or viewing a task in this prototype never starts an agent.</p>}>{x => <div>
    <dl class="c-facts compact">
      <div><dt>Execution</dt><dd class="c-mono">{x().id}</dd></div>
      <div><dt>Session</dt><dd class="c-mono">{x().sessionId}</dd></div>
      <div><dt>State</dt><dd><Badge tone={executionTone(executionLabel(x(), c.conn()))} glyph={executionGlyph(executionLabel(x(), c.conn()))} label={executionLabel(x(), c.conn())} /> · fixture</dd></div>
      <div><dt>Wait reason</dt><dd>{waitLabel(x()) ?? "None"}</dd></div>
    </dl>
    <L3Link label="Open worker detail" onClick={() => c.openDeep("worker")} />
  </div>}</Show>;
}
