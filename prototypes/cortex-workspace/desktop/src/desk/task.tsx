import { createEffect, createMemo, createSignal, For, Match, on, onMount, Show, Switch, untrack } from "solid-js";
import { useDesk } from "./ctx";
import { Icon } from "./icons";
import { AS_OF_SHORT, EXEC_LABEL, hhmm, PHASE_LABEL, relTime, type Decision, type Phase, type ProjectSummary, type TaskView } from "./model";
import { GitPanel, TerminalPanel } from "./panels";
import { returnFocus } from "./project";
import { navState, TASK_TABS, type TaskTab } from "./router";
import { A, Btn, Chip, Group, IconBtn, Loading, Notice, Reason, Receipt, Row, TabOverflow, TaskChips, VirtualList, type VirtualApi } from "./ui";

const TAB_LABEL: Record<TaskTab, string> = { discussion: "Discussion", decisions: "Decisions", result: "Result", workers: "Workers", code: "Code", git: "Git" };

export function TaskPage(props: { ps: ProjectSummary; tv: TaskView; tab: TaskTab }) {
  const desk = useDesk();
  let heading!: HTMLHeadingElement;
  const taskId = createMemo(() => props.tv.task.id);
  createEffect(on(taskId, id => {
    if (untrack(desk.store.state).selectedTaskId !== id) desk.store.selectTask(id);
    queueMicrotask(() => { if (navState.userNavigated && !document.querySelector("dialog[open]")) heading?.focus(); });
  }));
  const decisions = () => desk.data.decisionsFor(props.tv.task.id).filter(d => d.state === "open" && !(d.id in desk.store.state().responses));
  // Rename (mock): the generated or fixture title can be replaced at any time; kept in memory only.
  const [renaming, setRenaming] = createSignal(false);
  const [name, setName] = createSignal("");
  const blocked = () => desk.blocked(props.tv.project.id);
  const startRename = () => { setName(props.tv.task.title); setRenaming(true); queueMicrotask(() => document.querySelector<HTMLInputElement>("#rename-input")?.select()); };
  const endRename = () => { setRenaming(false); queueMicrotask(() => document.querySelector<HTMLElement>(".rename-btn")?.focus()); };
  const saveRename = () => {
    const next = name().trim(), id = props.tv.task.id;
    if (!next) return;
    if (next !== props.tv.task.title) {
      desk.setLocal(l => ({ ...l, titles: { ...l.titles, [id]: next } }));
      desk.receipt(`task:${id}`, "Renamed in memory only (mock). Nothing was sent.", `Renamed ${props.tv.ref} to “${next}” (mock)`);
    }
    endRename();
  };
  const onEsc = (e: KeyboardEvent) => {
    if (e.key !== "Escape" || e.defaultPrevented) return;
    const t = e.target as HTMLElement;
    if (t.closest(".popover, dialog")) return;
    e.preventDefault();
    const row = document.querySelector<HTMLElement>(`[data-list-task="${props.tv.task.id}"]`);
    (row ?? heading).focus();
  };
  return <div class={`task-layout ${desk.prefs.listPaneHidden() ? "no-list" : ""}`}>
    <Show when={!desk.prefs.listPaneHidden()}><TaskListPane ps={props.ps} current={props.tv} tab={props.tab} /></Show>
    <section class="workspace" id="task-main" tabindex="-1" aria-labelledby="view-title" onKeyDown={onEsc}>
      <div class="ws-header">
        <div class="ws-title-row">
          <h1 id="view-title" ref={heading} tabindex="-1" class={`ws-title ${renaming() ? "sr-only" : ""}`}>{props.tv.task.title}</h1>
          <Show when={renaming()} fallback={<IconBtn icon="pencil" class="sm rename-btn" disabled={Boolean(blocked())}
            label={blocked() ? `Rename task (unavailable: ${blocked()})` : "Rename task"} onClick={startRename} />}>
            <form class="rename-form" onSubmit={e => { e.preventDefault(); saveRename(); }}>
              <label class="sr-only" for="rename-input">Task title</label>
              <input id="rename-input" class="input sm" value={name()} onInput={e => setName(e.currentTarget.value)}
                onKeyDown={e => { if (e.key === "Escape") { e.preventDefault(); e.stopPropagation(); endRename(); } }} />
              <Btn type="submit" variant="primary" class="sm" mock disabled={!name().trim()}>Save</Btn>
              <Btn class="sm" onClick={endRename}>Cancel</Btn>
            </form>
          </Show>
        </div>
        <div class="ws-meta">
          <Chip icon="board">Phase: {PHASE_LABEL[props.tv.task.phase as Phase]}</Chip>
          <TaskChips tv={props.tv} />
          <button type="button" class="ref-copy mono" aria-label={`Copy task reference ${props.tv.ref}`} title="Copy reference"
            onClick={() => { void navigator.clipboard?.writeText(props.tv.ref).catch(() => undefined); desk.announce(`Copied ${props.tv.ref}`); }}>
            {props.tv.ref} <Icon name="copy" size={12} /></button>
          <Show when={props.tv.created}><span class="mock-tag">Mock</span></Show>
          <span class="ws-prov tertiary" title="Observed: Cortex saw this state itself. Reported: an agent said so; it is not verified.">
            {props.tv.disconnected ? `Last known ${props.tv.lastKnown ? EXEC_LABEL[props.tv.lastKnown] : "state"} at 00:00 · current state unknown` : `Observed · last confirmed ${AS_OF_SHORT} (fixture)`}</span>
        </div>
        <Receipt id={`task:${props.tv.task.id}`} />
      </div>
      <TaskTabs tv={props.tv} tab={props.tab} decisionCount={decisions().length} />
      <div class="ws-body" role="tabpanel" id="task-panel" aria-labelledby={`tab-${props.tab}`}>
        <Switch>
          <Match when={desk.data.status() === "loading"}><div class="pad"><Loading label="Loading task" rows={6} h="1.25rem" /></div></Match>
          <Match when={props.tab === "discussion"}><Discussion tv={props.tv} /></Match>
          <Match when={props.tab === "decisions"}><div class="tab-scroll"><div class="pad reading">
            <Show when={desk.data.decisionsFor(props.tv.task.id).length} fallback={<p class="empty-row">No decisions for this task.</p>}>
              <For each={desk.data.decisionsFor(props.tv.task.id)}>{d => <DecisionPanel decision={d} tv={props.tv} />}</For>
            </Show>
          </div></div></Match>
          <Match when={props.tab === "result"}><ResultTab tv={props.tv} /></Match>
          <Match when={props.tab === "workers"}><WorkersTab tv={props.tv} /></Match>
          <Match when={props.tab === "code"}><CodeTab tv={props.tv} /></Match>
          <Match when={props.tab === "git"}><div class="tab-split"><div class="tab-scroll"><div class="pad reading"><GitPanel ps={props.ps} tv={props.tv} /></div></div>
            <Show when={desk.prefs.terminalOpen()}><TerminalPanel tv={props.tv} /></Show></div></Match>
        </Switch>
      </div>
    </section>
  </div>;
}

function TaskTabs(props: { tv: TaskView; tab: TaskTab; decisionCount: number }) {
  let list!: HTMLDivElement;
  const onKey = (e: KeyboardEvent) => {
    const tabs = [...list.querySelectorAll<HTMLElement>("[role=tab]")];
    const i = tabs.indexOf(document.activeElement as HTMLElement);
    const next = e.key === "ArrowRight" ? tabs[(i + 1) % tabs.length] : e.key === "ArrowLeft" ? tabs[(i - 1 + tabs.length) % tabs.length]
      : e.key === "Home" ? tabs[0] : e.key === "End" ? tabs[tabs.length - 1] : undefined;
    if (next) { e.preventDefault(); tabs.forEach(t => t.setAttribute("tabindex", t === next ? "0" : "-1")); next.focus(); }
    if (e.key === " " && i >= 0) { e.preventDefault(); tabs[i].click(); }
  };
  return <div class="ws-tabs"><div class="tabs-wrap"><div role="tablist" aria-label="Task views" class="tabstrip" ref={list} onKeyDown={onKey}>
    <For each={TASK_TABS}>{t => <A role="tab" id={`tab-${t}`} class="tab" aria-selected={props.tab === t} aria-controls="task-panel" tabindex={props.tab === t ? 0 : -1}
      href={`/p/${props.tv.slug}/t/${props.tv.num}/${t}`} aria-label={t === "decisions" ? `Decisions, ${props.decisionCount} open` : undefined}>
      {TAB_LABEL[t]}<Show when={t === "decisions" && props.decisionCount > 0}><span class="count-chip tone-warn" aria-hidden="true">{props.decisionCount}</span></Show>
    </A>}</For>
  </div>
  <TabOverflow strip={() => list} label="All task views" items={TASK_TABS.map(t => ({
    label: `${TAB_LABEL[t]}${t === "decisions" && props.decisionCount > 0 ? ` (${props.decisionCount} open)` : ""}`, href: `/p/${props.tv.slug}/t/${props.tv.num}/${t}`, selected: props.tab === t }))} />
  </div></div>;
}

function TaskListPane(props: { ps: ProjectSummary; current: TaskView; tab: TaskTab }) {
  const desk = useDesk();
  const tasks = () => desk.data.tasksByProject().get(props.ps.project.id) ?? [];
  const currentIndex = () => tasks().findIndex(t => t.task.id === props.current.task.id);
  const [active, setActive] = createSignal(-1);
  const act = () => active() >= 0 && active() < tasks().length ? active() : Math.max(0, currentIndex());
  let api: VirtualApi | undefined, wrap!: HTMLDivElement;
  // The list's api arrives after this component's own onMount, so the initial scroll runs when it is registered.
  // Keep the open task visible in the pane, including a task created a moment ago at the end of the list.
  createEffect(on(() => props.current.task.id, () => queueMicrotask(() => api?.scrollToIndex(Math.max(0, currentIndex()))), { defer: true }));
  const focusRow = (i: number) => {
    const n = Math.max(0, Math.min(tasks().length - 1, i));
    setActive(n); api?.scrollToIndex(n);
    queueMicrotask(() => wrap.querySelector<HTMLElement>(`[data-idx="${n}"]`)?.focus());
  };
  const onKey = (e: KeyboardEvent) => {
    if (e.key === "ArrowDown") focusRow(act() + 1); else if (e.key === "ArrowUp") focusRow(act() - 1);
    else if (e.key === "Home") focusRow(0); else if (e.key === "End") focusRow(tasks().length - 1); else return;
    e.preventDefault();
  };
  const statusText = (tv: TaskView) => tv.needsYou ? (tv.wait === "question" ? "Your answer" : "Your permission") : tv.check === "failed" ? "Failed check"
    : tv.stale ? "Result stale" : tv.conflict ? "Conflict" : tv.exec === "not_started" ? "Not started" : EXEC_LABEL[tv.exec];
  return <section class="list-pane" role="region" aria-label="Task list" ref={wrap}>
    <div class="lp-head">
      <A href={`/p/${props.ps.slug}/board`} class="lp-title" title={`Back to ${props.ps.project.name} board`}>{props.ps.project.name} · <span class="tabular">{tasks().length}</span></A>
      <IconBtn icon="panelList" label="Hide task list (Ctrl+\)" class="sm" onClick={() => desk.prefs.setListPaneHidden(true)} />
    </div>
    <VirtualList items={tasks()} rem={2.75} class="lp-body" role="listbox" label={`${props.ps.project.name} tasks`} pin={act()} api={a => { api = a; queueMicrotask(() => a.scrollToIndex(Math.max(0, currentIndex()))); }} onKeyDown={onKey}
      render={(tv, i) => <A href={`/p/${tv.slug}/t/${tv.num}/${tv.needsYou ? "decisions" : "discussion"}`} class={`lp-row ${tv.task.id === props.current.task.id ? "selected" : ""}`}
        role="option" aria-selected={tv.task.id === props.current.task.id} tabindex={act() === i ? 0 : -1} data-idx={i} data-list-task={tv.task.id}
        onFocus={() => setActive(i)} onClick={() => (returnFocus.taskId = tv.task.id)}>
        <span class="lp-row-title" title={tv.task.title}>{tv.task.title}</span>
        <span class={`lp-row-status ${tv.needsYou ? "tone-warn-fg" : tv.check === "failed" || tv.conflict ? "tone-danger-fg" : ""}`}>{tv.ref} · {statusText(tv)}</span>
      </A>} />
  </section>;
}

function planFor(tv: TaskView) {
  if (tv.capability) return { unavailable: true, steps: [] as { text: string; state: string }[] };
  if (tv.exec === "not_started") return { unavailable: false, steps: [] };
  const done = tv.exec === "complete" ? 5 : tv.exec === "failed" ? 4 : 3;
  const names = ["Read the synthetic task", "Inspect affected files", "Draft the change", "Run the mock checks", "Report the result"];
  return { unavailable: false, steps: names.map((text, i) => ({ text, state: i < done ? "reported done" : i === done ? (tv.exec === "failed" ? "reported failed" : "in progress (reported)") : "not started" })) };
}

function Plan(props: { tv: TaskView }) {
  const plan = () => planFor(props.tv);
  return <Switch>
    <Match when={plan().unavailable}><p class="tertiary">Plan updates unavailable: this agent did not negotiate plan reporting.</p></Match>
    <Match when={plan().steps.length === 0}><p class="tertiary">No plan yet.</p></Match>
    <Match when={true}><ol class="plan-steps"><For each={plan().steps}>{(s, i) => <li class={`plan-step ${s.state.startsWith("reported done") ? "done" : ""}`}>
      <span class="plan-num tabular">{i() + 1}</span><span class="plan-text">{s.text}</span><span class="plan-state">{s.state}</span></li>}</For></ol>
      <Show when={props.tv.disconnected}><p class="tertiary">as of 00:00 · current plan state unknown</p></Show></Match>
  </Switch>;
}

function Discussion(props: { tv: TaskView }) {
  const desk = useDesk();
  const [toolsOpen, setToolsOpen] = createSignal(false);
  const [fullMsg, setFullMsg] = createSignal(false);
  const [draft, setDraft] = createSignal("");
  const [newCount, setNewCount] = createSignal(0);
  let scroller!: HTMLDivElement;
  const events = () => desk.data.eventsFor(props.tv.task.id);
  const sent = () => desk.local().messages[props.tv.task.id] ?? [];
  const decision = () => desk.data.decisionsFor(props.tv.task.id)[0];
  const lines = () => Math.ceil(props.tv.task.description.length / 90);
  const long = () => lines() > 12;
  const blocked = () => desk.blocked(props.tv.project.id);
  const plan = () => planFor(props.tv);
  const done = () => plan().steps.filter(s => s.state === "reported done").length;
  const atBottom = () => scroller.scrollTop + scroller.clientHeight >= scroller.scrollHeight - 8;
  const send = () => {
    const text = draft().trim();
    if (!text || blocked()) return;
    const bottom = atBottom();
    desk.setLocal(l => ({ ...l, messages: { ...l.messages, [props.tv.task.id]: [...(l.messages[props.tv.task.id] ?? []), text] } }));
    desk.receipt(`msg:${props.tv.task.id}`, "Mock message kept in memory only. No agent received it.", `Message in ${props.tv.ref} (mock)`);
    setDraft("");
    if (!bottom) setNewCount(n => n + 1);
  };
  const empty = () => events().length === 0 && sent().length === 0 && props.tv.created;
  return <div class={`discussion ${desk.wide() ? "with-rail" : ""}`}>
    <div class="disc-main">
      <div class="tab-scroll" ref={scroller} onScroll={() => { if (atBottom()) setNewCount(0); }}>
        <div class="timeline">
          <Show when={!desk.wide()}>
            <div class="plan-disclosure">
              <button type="button" class="disclosure" aria-expanded={desk.prefs.planOpen()} aria-controls="plan-body" onClick={() => desk.prefs.setPlanOpen(!desk.prefs.planOpen())}>
                <Icon name={desk.prefs.planOpen() ? "chevronDown" : "chevronRight"} size={14} />
                <span>Plan{plan().steps.length ? ` · ${done()} of ${plan().steps.length} steps reported done` : ""}</span>
              </button>
              <Show when={desk.prefs.planOpen()}><div id="plan-body" class="plan-body"><Plan tv={props.tv} /></div></Show>
            </div>
          </Show>
          <Show when={props.tv.disconnected}><Notice tone="danger" icon="plugOff">Output paused at 00:00. The machine is disconnected; current state unknown.</Notice></Show>
          <Show when={empty()}><p class="empty-row">No discussion yet. Write the first message below.</p></Show>
          <Show when={!empty()}>
            <div class="msg user"><div class="bubble">
              <p class={long() && !fullMsg() ? "clamp-12" : ""}>{props.tv.task.description}</p>
              <Show when={long()}><button type="button" class="link-btn" aria-expanded={fullMsg()} onClick={() => setFullMsg(!fullMsg())}>
                {fullMsg() ? "Show less" : `Show full message (${lines()} lines)`}</button></Show>
            </div><div class="msg-meta">You · {props.tv.created ? "just now (mock)" : "01:00 (fixture)"}</div></div>
            <Show when={events().length}>
              <div class="msg agent"><p>I'll work through this synthetic task and report each step. Statements here are reported by the agent, not verified.</p>
                <div class="msg-meta">Agent · reported</div></div>
              <div class="tool-group">
                <button type="button" class="disclosure" aria-expanded={toolsOpen()} onClick={() => setToolsOpen(!toolsOpen())}>
                  <Icon name={toolsOpen() ? "chevronDown" : "chevronRight"} size={14} /><Icon name="terminal" size={14} />
                  <span>{events().length} tool events · reported · {toolsOpen() ? "Hide" : "Show"}</span></button>
                <Show when={toolsOpen()}><ul class="tool-list" role="list"><For each={events()}>{e => <li><span class="mono tertiary">{e.occurredAt.slice(11, 19)}</span> <span class="tertiary">{e.kind}</span> {e.text}</li>}</For></ul></Show>
              </div>
            </Show>
            <Show when={decision()}>{d => <p class="msg agent decision-link"><Icon name={d().kind === "permission" ? "key" : "question"} size={14} class="tone-warn-fg" />
              {d().kind === "permission" ? "Permission requested" : "Question asked"}: {d().title} <A class="inline-link" href={`/p/${props.tv.slug}/t/${props.tv.num}/decisions`}>Open in Decisions</A></p>}</Show>
            <Show when={props.tv.capability}><Notice icon="circleSlash">{props.tv.capability} is unavailable: Intentionally unavailable in the mock prototype.</Notice></Show>
            <For each={sent()}>{m => <div class="msg user"><div class="bubble"><p>{m}</p></div><div class="msg-meta">You · <span class="tone-mock-fg">Mock · not delivered</span></div></div>}</For>
          </Show>
        </div>
      </div>
      <Show when={newCount() > 0}><button type="button" class="jump-pill" onClick={() => { scroller.scrollTop = scroller.scrollHeight; setNewCount(0); }}>
        {newCount()} new · Jump to latest</button></Show>
      <div class="composer-wrap">
        <div class="composer">
          <label class="sr-only" for={`composer-${props.tv.num}`}>Reply in this task's discussion</label>
          <textarea id={`composer-${props.tv.num}`} rows="3" placeholder="Reply in this task's discussion" value={draft()} disabled={Boolean(blocked())}
            aria-describedby={`composer-r-${props.tv.num}`} onInput={e => setDraft(e.currentTarget.value)}
            onKeyDown={e => { if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) { e.preventDefault(); send(); } }} />
          <div class="composer-bar"><span class="tertiary sm-text">Ctrl+Enter sends</span>
            <Btn variant="primary" mock icon="send" disabled={Boolean(blocked()) || !draft().trim()} onClick={send}>Send</Btn></div>
        </div>
        <Reason id={`composer-r-${props.tv.num}`} text={blocked()} />
        <Receipt id={`msg:${props.tv.task.id}`} />
      </div>
    </div>
    <Show when={desk.wide()}><aside class="plan-rail" aria-label="Plan"><h2 class="rail-title">Plan</h2><Plan tv={props.tv} /></aside></Show>
  </div>;
}

export function DecisionPanel(props: { decision: Decision; tv: TaskView }) {
  const desk = useDesk();
  const d = () => props.decision;
  const response = () => desk.store.state().responses[d().id];
  const elsewhere = () => desk.local().elsewhere[d().id];
  const blocked = () => desk.blocked(props.tv.project.id);
  const [answer, setAnswer] = createSignal("");
  const rid = () => `decision:${d().id}`;
  const options = () => "options" in d() ? (d() as { options: string[] }).options : [];
  const respond = (value: string) => {
    desk.receipt(rid(), desk.store.decide(d().id, value).message, `${d().kind === "permission" ? "Permission" : "Answer"} · ${d().title} · ${value} (mock)`);
    // The action buttons are replaced by the recorded status: move focus there instead of losing it.
    queueMicrotask(() => document.getElementById(`dec-status-${d().id}`)?.focus());
  };
  return <article class="decision" aria-labelledby={`dec-${d().id}`}>
    <div class="dec-type"><Icon name={d().kind === "permission" ? "key" : "question"} size={14} class="tone-warn-fg" />{d().kind === "permission" ? "Permission" : "Question"}</div>
    <h2 class="dec-title" id={`dec-${d().id}`}>{d().title}</h2>
    <p class="dec-meta">{props.tv.ref} {props.tv.task.title} · requested by agent · requested at: unknown · decision v{d().version}</p>
    <div class="group dec-rows">
      <div class="row"><div class="dec-k">Effect</div><div class="dec-v">{d().effect}</div></div>
      <div class="row"><div class="dec-k">Scope</div><div class="dec-v">{d().scope} · entity version v{d().version}</div></div>
      <div class="row"><div class="dec-k">If you reject</div><div class="dec-v">{d().rejection} Nothing is reverted.</div></div>
    </div>
    <Switch>
      <Match when={elsewhere()}><p class="dec-status" role="status"><Icon name="phone" size={14} /> Answered on Phone at {elsewhere()!.at}: {elsewhere()!.answer} (mock). Your choice was not applied.</p></Match>
      {/* One status line per response; focus moves here, so it is not also a live region (no double announcement). */}
      <Match when={response()}><p class="dec-status" tabindex="-1" id={`dec-status-${d().id}`}><Icon name="flask" size={14} class="tone-mock-fg" />
        <span>{d().kind === "permission" ? "Response" : "Answer"} recorded (mock): {response()}.{d().kind === "permission" ? " No authorization was granted." : ""} The runtime has not confirmed it; execution facts are unchanged.</span></p></Match>
      <Match when={d().kind === "question"}>
        <fieldset class="dec-answer" disabled={Boolean(blocked())}>
          <legend>Your answer</legend>
          <For each={options()}>{o => <label class="radio-row"><input type="radio" name={`ans-${d().id}`} value={o} checked={answer() === o} onChange={() => setAnswer(o)} /> {o}</label>}</For>
        </fieldset>
        <div class="btn-row end"><Btn variant="primary" mock disabled={Boolean(blocked()) || !answer()} onClick={() => respond(answer())}>Send answer</Btn></div>
        <Reason text={blocked() ? `${blocked()}. Last confirmed 00:00.` : !answer() ? "Choose an answer to send it." : null} />
      </Match>
      <Match when={d().kind === "permission"}>
        <div class="btn-row end">
          <Btn mock disabled={Boolean(blocked())} onClick={() => respond("Denied")}>Deny</Btn>
          <Btn variant="primary" mock disabled={Boolean(blocked())} onClick={() => respond("Approved once")}>Approve once</Btn>
        </div>
        <Reason text={blocked() ? `${blocked()}. Last confirmed 00:00.` : null} />
      </Match>
    </Switch>
  </article>;
}

const middle = (s: string) => s.length > 22 ? `${s.slice(0, 10)}…${s.slice(-9)}` : s;
function Digest(props: { value: string }) {
  const desk = useDesk();
  return <span class="digest"><span class="mono" title={props.value} aria-label={props.value}>{middle(props.value)}</span>
    <button type="button" class="icon-btn sm" aria-label={`Copy ${props.value}`} title="Copy" onClick={() => { void navigator.clipboard?.writeText(props.value).catch(() => undefined); desk.announce("Copied digest"); }}><Icon name="copy" size={12} /></button></span>;
}

function ResultTab(props: { tv: TaskView }) {
  const desk = useDesk();
  const result = () => desk.data.resultFor(props.tv.task.id);
  const checks = createMemo(() => {
    const order = { failed: 0, not_run: 1, unverified: 2, passed: 3 } as Record<string, number>;
    return [...desk.data.checksFor(props.tv.task.id)].sort((a, b) => order[a.outcome] - order[b.outcome]);
  });
  const count = (o: string) => checks().filter(c => c.outcome === o).length;
  const accepted = () => result() && desk.store.state().acceptedResultIds.includes(result()!.id);
  const rid = () => `result:${props.tv.task.id}`;
  const blocked = () => desk.blocked(props.tv.project.id);
  const [unified, setUnified] = createSignal<boolean | null>(null);
  const autoUnified = () => !desk.wide() || desk.prefs.textSize() >= 150;
  const isUnified = () => unified() ?? autoUnified();
  const acceptReason = () => result()?.state === "stale" ? `This result changed after it was requested (digest v1 → v2). Acceptance is disabled.` : blocked();
  const checkIcon = { failed: "xCircle", not_run: "notRun", unverified: "help", passed: "checkCircle" } as const;
  const checkLabel = { failed: "Failed", not_run: "Not run", unverified: "Unverified", passed: "Passed" } as const;
  return <div class="tab-scroll"><div class="pad reading wide-ok">
    <Show when={result()} fallback={<p class="empty-row">No result yet for this task.</p>}>{r => <>
      <Show when={r().state === "stale"}><Notice tone="warn" icon="history" role="status">Result changed after review: {r().artifactDigest} → {r().workspaceDigest.replace(/.*-/, "")}. Acceptance is disabled.</Notice></Show>
      <Group label="Result" id="res-h">
        <Row label="Result">{r().id} · v{r().version}</Row>
        <Row label="State">{r().state === "stale" ? "Stale: changed after review" : "Complete"}</Row>
        <Row label="Artifact digest"><Digest value={r().artifactDigest} /></Row>
        <Row label="Workspace digest"><Digest value={r().workspaceDigest} /></Row>
      </Group>
    </>}</Show>
    <Group label={<>Checks <span class="tertiary tabular">{checks().length}</span></>} id="checks-h">
      <Show when={checks().length} fallback={<p class="empty-row">No check records for this task.</p>}>
        <For each={checks()}>{c => <div class="row">
          <div class="row-text"><div class={`row-label check-${c.outcome}`}><Icon name={checkIcon[c.outcome]} size={14} /> {checkLabel[c.outcome]} · {c.name}
            <Show when={c.outcome === "not_run"}><span class="req-tag">Required</span></Show></div>
            <div class="row-desc">{c.detail}{c.outcome === "not_run" ? " Not run is not passed." : ""}</div></div>
        </div>}</For>
      </Show>
    </Group>
    <Show when={result()}>{r => <>
      <Group label="Changes" id="chg-h">
        <Row label={<span class="mono"><span class="git-letter">M</span> synthetic/layout.ts</span>}><span class="tabular"><span class="diff-plus">+1</span> <span class="diff-minus">−1</span></span></Row>
      </Group>
      <section class="group-wrap" aria-labelledby="diff-h">
        <div class="group-label"><h2 id="diff-h">Diff</h2>
          <button type="button" class="link-btn" onClick={() => setUnified(!isUnified())}>{isUnified() ? "Side by side" : "Unified"}</button></div>
        <div class="group diff">
          <div class="diff-file mono">synthetic/layout.ts</div>
          <Show when={!isUnified()} fallback={<pre class="code unified"><span class="dl del"><span class="gut">−</span>layout = compact</span><span class="dl add"><span class="gut">+</span>layout = {r().state === "stale" ? "focused" : "comfortable"}</span></pre>}>
            <div class="split"><pre class="code"><span class="dl del"><span class="gut">−</span>layout = compact</span></pre><pre class="code"><span class="dl add"><span class="gut">+</span>layout = {r().state === "stale" ? "focused" : "comfortable"}</span></pre></div>
          </Show>
        </div>
      </section>
      <section class="group-wrap acceptance" aria-labelledby="acc-h">
        <div class="group-label"><h2 id="acc-h">Acceptance</h2></div>
        <div class="group">
          <div class="row"><div class="dec-k">Evidence</div><div class="dec-v evidence">
            <span class="check-passed"><Icon name="checkCircle" size={14} /> Passed {count("passed")}</span>
            <span class="check-failed"><Icon name="xCircle" size={14} /> Failed {count("failed")}</span>
            <span><Icon name="notRun" size={14} /> Not run {count("not_run")}</span>
            <span><Icon name="help" size={14} /> Unverified {count("unverified")}</span>
          </div></div>
          <Show when={count("not_run")}><div class="row"><div class="dec-k">Required, not run</div>
            <div class="dec-v">{checks().filter(c => c.outcome === "not_run").map(c => c.name).join(", ")}</div></div></Show>
          <div class="row column">
            <Show when={accepted()} fallback={<>
              <div class="btn-row end">
                <Btn variant="danger-ghost" mock disabled={Boolean(blocked())} onClick={async () => {
                  if (await desk.confirm({ title: `Reject ${r().id}?`, body: "The result is marked rejected in memory only. Nothing is reverted.", confirm: "Reject", danger: true, mock: true }))
                    desk.receipt(rid(), "Mock rejection recorded in memory only. Nothing is reverted.", `Rejected ${r().id} (mock)`);
                }}>Reject</Btn>
                <Btn mock disabled={Boolean(blocked())} onClick={() => desk.receipt(rid(), "Mock change request recorded in memory only. No agent received it.", `Requested changes on ${r().id} (mock)`)}>Request changes</Btn>
                <Btn variant="primary" mock disabled={Boolean(acceptReason())} aria-describedby="accept-reason"
                  onClick={() => desk.receipt(rid(), desk.store.acceptResult(r().id).message, `Accepted (mock) · ${r().id}`)}>Accept {r().id}</Btn>
              </div>
              <Reason id="accept-reason" text={acceptReason()} icon={r().state === "stale" ? "history" : "info"} />
            </>}>
              <p class="dec-status" role="status"><Icon name="flask" size={14} class="tone-mock-fg" /> Accepted (mock) · {r().id} · digest {r().artifactDigest}</p>
            </Show>
            <p class="tertiary sm-text">Accepting does not commit, push or deploy.</p>
          </div>
        </div>
        <Receipt id={rid()} />
      </section>
    </>}</Show>
  </div></div>;
}

function WorkersTab(props: { tv: TaskView }) {
  const desk = useDesk();
  const [detail, setDetail] = createSignal(false);
  const exec = () => desk.data.fixture().executions.find(e => e.id === props.tv.task.executionId);
  return <div class="tab-scroll"><div class="pad reading">
    <Show when={exec()} fallback={<p class="empty-row">No workers yet. This task has not started an execution.</p>}>{ex => <Show when={!detail()} fallback={<>
      <button type="button" class="link-btn back-link" onClick={() => setDetail(false)}><Icon name="arrowLeft" size={14} /> All workers</button>
      <Group label="Worker" id="wk-h">
        <Row label="Role">Frontend implementer (synthetic)</Row>
        <Row label="Engine / model">Not exposed by this agent</Row>
        <Row label="State"><TaskChips tv={{ ...props.tv, wait: null, check: null, stale: false, conflict: false }} /></Row>
        <Row label="Session"><span class="mono">{ex().sessionId}</span></Row>
        <Row label="Last event">{props.tv.updatedAt ? `${relTime(props.tv.updatedAt)} · reported` : "unknown ?"}</Row>
      </Group></>}>
      <Group label="Execution" id="ex-h">
        <Row label="Execution"><span class="mono">{ex().id}</span></Row>
        <Row label="State">{props.tv.exec === "unknown" ? `Unknown (last known ${EXEC_LABEL[props.tv.lastKnown ?? "running"]})` : EXEC_LABEL[props.tv.exec]} · observed</Row>
        <Row label="Attempts">1</Row>
      </Group>
      <section class="group-wrap" aria-labelledby="workers-h"><div class="group-label"><h2 id="workers-h">Workers</h2></div>
        <div class="group">
          <button type="button" class="worker-row" onClick={() => setDetail(true)}>
            <span>Frontend implementer</span><span class="tertiary">Not exposed</span>
            <span>{props.tv.exec === "unknown" ? "Unknown ?" : EXEC_LABEL[props.tv.exec]}</span><span class="tertiary">observed</span>
            <span class="tertiary">{props.tv.updatedAt ? hhmm(props.tv.updatedAt) : "—"}</span><Icon name="chevronRight" class="row-chev" />
          </button>
        </div>
      </section>
    </Show>}</Show>
  </div></div>;
}

const FILES: Record<string, string> = {
  "synthetic/layout.ts": "export const layout = \"compact\";\n",
  "synthetic/tokens.ts": "export const tokens = { gap: 8, radius: 6 };\n",
  "src/index.ts": "import { layout } from \"../synthetic/layout\";\nconsole.log(layout);\n",
  "README.md": "# Synthetic project\n\nFixture content for the mock editor.\n",
};
function CodeTab(props: { tv: TaskView }) {
  const desk = useDesk();
  const [file, setFile] = createSignal("synthetic/layout.ts");
  const [query, setQuery] = createSignal("");
  const [notice, setNotice] = createSignal(false);
  const key = () => `${props.tv.task.id}:${file()}`;
  const base = () => props.tv.conflict && file() === "synthetic/layout.ts" ? "layout = comfortable\n" : FILES[file()];
  const value = () => desk.local().buffers[key()] ?? base();
  const dirty = () => value() !== base();
  const blocked = () => desk.blocked(props.tv.project.id);
  const files = () => Object.keys(FILES).filter(f => f.toLowerCase().includes(query().toLowerCase().replace(/\*/g, "")));
  const letter = (f: string) => props.tv.conflict && f === "synthetic/layout.ts" ? "U" : f === "synthetic/layout.ts" && props.tv.exec !== "not_started" ? "M" : "";
  const rid = () => `code:${props.tv.task.id}`;
  let gutter!: HTMLPreElement;
  return <div class="tab-split">
    <div class="code-area">
      <div class="file-tree" role="region" aria-label="Files">
        <label class="sr-only" for="file-filter">Go to file</label>
        <input id="file-filter" class="input sm" placeholder="Go to file (Ctrl+P)" value={query()} onInput={e => setQuery(e.currentTarget.value)} />
        <Show when={files().length} fallback={<p class="empty-row">No files match <code>{query()}</code>.</p>}>
          <ul role="list"><For each={files()}>{f => <li><button type="button" class={`tree-row ${file() === f ? "selected" : ""}`} aria-current={file() === f ? "true" : undefined} onClick={() => setFile(f)}>
            <Icon name="file" size={14} /><span class="mono wrap-any">{f}</span><Show when={letter(f)}><span class={`git-letter ${letter(f) === "U" ? "tone-danger-fg" : ""}`}>{letter(f)}</span></Show>
          </button></li>}</For></ul>
        </Show>
      </div>
      <div class="editor">
        <div class="editor-head">
          <span class="editor-file"><Icon name="file" size={14} /><span class="mono">{file()}</span><Show when={dirty()}><span class="tone-warn-fg sm-text">Unsaved</span></Show></span>
          <span class="lease tertiary sm-text">{blocked() ? "Read-only: " + blocked() : "Lease: this tab (mock)"}</span>
          <Btn variant="primary" class="sm" mock disabled={!dirty() || Boolean(blocked())} onClick={() => {
            if (props.tv.conflict || desk.local().diskChanged) { setNotice(true); return; }
            desk.receipt(rid(), `Mock save: ${file()} kept in memory only. No file was written.`, `Saved ${file()} (mock)`);
          }}>Save</Btn>
          <Btn icon="terminal" variant="ghost" class="sm" aria-pressed={desk.prefs.terminalOpen()} onClick={() => desk.prefs.setTerminalOpen(!desk.prefs.terminalOpen())}>Terminal</Btn>
        </div>
        <div class="editor-notes">
          <Receipt id={rid()} />
          <Show when={blocked()}><Notice icon="lock">Read-only. {blocked()}.</Notice></Show>
        <Show when={notice()}><Notice tone="warn" icon="alert" role="status" action={<div class="btn-row">
          <Btn onClick={() => desk.receipt(rid(), "Compare opened: disk version differs only in the fixture.")}>Compare</Btn>
          <Btn onClick={() => { desk.setLocal(l => { const b = { ...l.buffers }; delete b[key()]; return { ...l, buffers: b }; }); setNotice(false); desk.receipt(rid(), "Reloaded the fixture version; your edits were discarded from memory."); }}>Reload</Btn>
          <Btn mock onClick={() => { setNotice(false); desk.receipt(rid(), `Mock: saved as ${file()}.copy in memory only.`, `Saved ${file()} as a copy (mock)`); }}>Save as…</Btn>
        </div>}>This file changed on disk since you opened it. <span class="mono wrap-any">{file()}</span></Notice></Show>
        </div>
        <div class="editor-body">
          <pre class="gutter" aria-hidden="true" ref={gutter}>{Array.from({ length: value().split("\n").length }, (_, i) => i + 1).join("\n")}</pre>
          <label class="sr-only" for="editor-text">Editor: {file()}</label>
          <textarea id="editor-text" class="code-input" spellcheck={false} wrap="off" value={value()} readOnly={Boolean(blocked())}
            onScroll={e => { gutter.scrollTop = e.currentTarget.scrollTop; }}
            onInput={e => desk.setLocal(l => ({ ...l, buffers: { ...l.buffers, [key()]: e.currentTarget.value } }))} />
        </div>
      </div>
    </div>
    <Show when={desk.prefs.terminalOpen()}><TerminalPanel tv={props.tv} /></Show>
  </div>;
}

