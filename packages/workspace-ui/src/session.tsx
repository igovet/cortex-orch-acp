import { createEffect, createMemo, createSignal, For, Match, onCleanup, Show, Switch, useContext, type Accessor } from "solid-js";
import { capabilityOf, FEATURES, isOpen, type Activity, type Decision, type TimelineEntry, type Worker } from "@cortex/workspace-domain";
import { CodeView } from "./code";
import { archiveRow, daysLeft } from "./archive-model";
import type { AttachmentInput } from "@cortex/workspace-domain";
import { AttachChips, Composer, RouteBar, TaskAttachments } from "./composer";
import { useWs } from "./ctx";
import { Confirmed, createBodyMark, createTicker, ScreenActive } from "./freshness";
import { A, Count, createFollow, I, MenuItem, Pop } from "./kit";
import { ConflictPanel, ReadyPanel } from "./merge";
import { Answered, QuestionTabs, type Ask } from "./questions";
import { ChangesPanel, ResultCard } from "./review";
import { ActionBar, Page, StatusLane, Toolbar, Whole, type StripItem } from "./layout";
import { taskWhyId, titleBlock } from "./task-chrome";
import { inConflict, repoOf, taskCheckout } from "./review-model";
import type { Route, TaskTab } from "./router";
import { age, clock } from "./selectors";
import { contextOf, duration, elapsedOf, isFinished, phases, recordedCoordinatorRoute, sessionModel, taskSettings, tokens, verificationFace, workerFace, workerState, type SessionModel } from "./session-model";
import { CORTEX, Pill, StopCommand, VERIFICATION, WorkerPane, WorkingIndicator } from "./worker";
import { createOpenState, Fold, TextSteps } from "./feed";

import { ResultText, resultPreview } from "./result-text";
import { viewOfText } from "./tool-names";

type TaskRoute = Extract<Route, { name: "task" }>;
const useTicker = () => {
  const ws = useWs();
  const [now, setNow] = createSignal(ws.store.clock());
  createTicker(() => setNow(ws.store.clock()), 1000);
  return now;
};

const workersNote = (m: SessionModel) => {
  if (m.active.length || m.finished.length !== m.workers.length) return `${m.active.length} active of ${m.workers.length}`;
  const cancelled = m.workers.filter(w => w.status === "cancelled").length, done = m.workers.length - cancelled;
  return `${done} finished${cancelled ? `, ${cancelled} cancelled` : ""}`;
};

/** What the task is doing, in words. No percentage is made up: a phase or the last recorded fact, and the plan's real count. */
function progressLine(m: SessionModel) {
  if (m.view.unknown) return "Last recorded activity";
  const now = phases(m).find(p => p.state === "now")?.label.replace(/\s+\d+\/\d+$/, "");
  const plan = m.session.plan && m.planTotal ? `${m.planDone} of ${m.planTotal} steps done` : "";
  return [now ?? m.view.fact.text, plan].filter(Boolean).join(", ");
}

/**
 * The conversation is the task's default view. Work details, Changes, Code and Terminal stay one activation away, each
 * with its own route, and a detail view has one obvious way back. On a narrow pane the entries fold into one Details menu.
 */
function TaskBar(props: { m: SessionModel; tab: TaskTab | undefined; route: TaskRoute }) {
  const ws = useWs();
  const at = (tab?: TaskTab): Route => ({ name: "task", machine: props.route.machine, task: props.route.task, tab });
  const changes = () => props.m.session.changes;
  // Each area needs its own capability, never Git fetch: Work details is the runtime's record, Changes the result review, Code the files.
  const entries = () => ([{ id: "workers", label: "Work details" }, { id: "changes", label: "Changes" }, { id: "code", label: "Code" }, { id: "terminal", label: "Terminal" }] as { id: TaskTab; label: string }[])
    .filter(t => t.id === "workers" || ws.store.mode === "demo"
      || (t.id === "changes" ? ws.can(capabilityOf("accept_result")) || ws.can(capabilityOf("request_changes"))
        : t.id === "terminal" ? ws.can(FEATURES.terminal) : ws.can(capabilityOf("git_fetch")) || ws.can(FEATURES.editFiles)));
  const menu = `m-task-details-${props.route.task}`;
  return <Toolbar small class="taskbar" label="Task progress and details"
    start={<Show when={props.tab}><A class="btn td-back" to={at()}><I n="arrow-left" />Conversation</A></Show>}
    mid={<>
      <Whole class="td-progress" text={progressLine(props.m)} />
      <Show when={ws.store.mode === "real"}><Confirmed at={props.m.view.task.lastConfirmedAt} /></Show>
    </>}
    end={<>
      <nav class="td-links" aria-label="Task details"><For each={entries()}>{t => <A class="btn td-link" to={at(t.id)} aria-current={props.tab === t.id ? "page" : undefined}>{t.label}
        <Show when={t.id === "workers" && props.m.workers.length > 0}><span class="n">{workersNote(props.m)}</span></Show>
        <Show when={t.id === "changes" && changes()}>{c => <span class="tnum n"><span class="add">+{c().add}</span> <span class="del">−{c().del}</span></span>}</Show>
      </A>}</For></nav>
      <button type="button" class="btn td-more" popovertarget={menu} aria-label="Task details"><I n="list" />Details<I n="chevron-down" s={12} /></button>
      <Pop id={menu} class="pop-anchor" align="right" label="Task details" style={{ width: "220px" }}>
        <Show when={props.tab}><MenuItem icon="message" label="Conversation" onSelect={() => ws.go(at())} /></Show>
        <For each={entries()}>{t => <MenuItem label={t.label} check={props.tab === t.id} onSelect={() => ws.go(at(t.id))} />}</For>
      </Pop>
    </>} />;
}

/** A factual one-line summary of a team update, opened in place for the full list. */
function teamLine(m: SessionModel, title: string, ids: string[]) {
  const list = m.workers.filter(w => ids.includes(w.id));
  const working = list.filter(w => !isFinished(w)).length;
  return `${title}: ${working ? `${working} of ${list.length} working` : `${list.length} finished`}`;
}

/** Why a recorded result is not offered as the current one right now, or null when it is. */
function resultHeld(m: SessionModel, pending: boolean): string | null {
  if (m.mode === "offline" || m.mode === "unconfirmed") return "State needs confirmation";
  if (pending || m.open.length > 0) return "A decision is pending";
  if (m.mode === "stopping") return "The stop is not confirmed yet";
  if (m.mode === "live" || m.active.length > 0) return "Work is running";
  return null;
}

function Brief(props: { m: SessionModel }) {
  const ws = useWs();
  const t = () => props.m.view.task;
  const s = () => props.m.session;
  return <div class="brief">
    <ResultText breaks text={t().description} />
    <TaskAttachments taskId={t().id} />
    <div class="meta">
      <Show when={s().createdAt !== null}><span>Created {clock(s().createdAt)}</span></Show>
      <Show when={s().base}>{b => <span>Base <span class="mono">{b().branch}</span> at <span class="mono">{b().commit}</span></span>}</Show>
      <Show when={ws.t("sessionBadge")}><span>{ws.t("sessionBadge")}</span></Show>
      <Show when={t().titleState === "pending" && ws.store.mode === "demo"}><span>Naming…</span></Show>
      <Show when={t().titleState === "failed"}><span class="st-warn">Title not generated <button class="lnk" {...ws.needs("generate_title")} onClick={() => ws.run({ type: "generate_title", taskId: t().id })}>Retry</button></span></Show>
    </div>
  </div>;
}

function Plan(props: { m: SessionModel }) {
  const worker = (id: string | null) => props.m.workers.find(w => w.id === id);
  const plan = () => props.m.session.plan!;
  const label = (w: Worker | undefined) => {
    if (!w) return { text: "Pending", tone: "st-idle", icon: "hourglass" };
    const f = workerFace(w.status);
    return { text: w.status === "completed" ? "Done" : w.status === "pending" || w.status === "not_started" ? "Pending" : w.status === "working" ? "Running" : f.label, tone: f.tone, icon: f.icon, spin: f.spin };
  };
  return <div class="surface">
    <div class="plan-h"><I n="list" s={14} class="faint" />Plan r{plan().revision}<span class="r">Executing {props.m.planDone} of {props.m.planTotal} done, {plan().note}</span></div>
    <For each={plan().nodes}>{n => {
      const f = () => label(worker(n.workerId));
      return <div class="node"><span class="s"><span class={`st ${f().tone}`}><I n={f().icon} s={14} class={f().spin ? "spin" : undefined} />{f().text}</span></span>{n.title}<span class="who">{n.who}</span></div>;
    }}</For>
  </div>;
}

function WorkerRow(props: { w: Worker; now: number; selected: boolean; onPick: () => void }) {
  const ws = useWs();
  const f = () => workerState(props.w);
  const goal = () => props.w.waitsFor ? `Waits for ${props.w.waitsFor}` : props.w.now ?? props.w.goal;
  return <button class="wrow" role="tab" aria-selected={props.selected ? "true" : "false"} onClick={() => props.onPick()}>
    <I n={f().icon} class={`${f().spin ? "spin " : ""}${f().tone === "st-idle" ? "faint" : f().tone}`} />
    <span class="trunc"><span class="role">{props.w.role}</span><br /><span class="goal">{goal()}</span></span>
    <span class={`route${props.w.route ? "" : " faint"}`}>{ws.route(props.w.route)}</span><span class={`st ${f().tone}`}>{f().label}</span><span class="el">{duration(elapsedOf(props.w, props.now))}</span>
  </button>;
}

function Workers(props: { m: SessionModel; entry: Extract<TimelineEntry, { kind: "workers" }>; last: boolean; now: number; pick: string; onPick: (key: string) => void }) {
  const ws = useWs();
  const ids = () => props.entry.workerIds.filter(id => props.m.workers.some(w => w.id === id));
  const worker = (id: string) => props.m.workers.find(w => w.id === id)!;
  const others = () => props.m.finished.filter(w => !props.entry.workerIds.includes(w.id));
  const where = () => { const c = props.m.view.task.checkout; return c.kind === "worktree" ? "Same worktree" : "Project folder"; };
  const [stopping, setStopping] = createSignal<{ worker: Worker; item: Activity } | null>(null);
  const runningCommand = (w: Worker) => w.status === "working" ? w.activity.find(a => a.command?.state === "running") : undefined;
  return <div class="surface" role="tablist" aria-orientation="vertical" aria-label={props.entry.title}>
    <div class="wt-h"><I n="users" s={14} class="faint" />{props.entry.title}<span class="r">{where()}<Show when={props.m.view.task.checkout.branch}>, <span class="mono">{props.m.view.task.checkout.branch}</span></Show></span></div>
    <For each={ids()}>{id => <>
      <WorkerRow w={worker(id)} now={props.now} selected={props.pick === id} onPick={() => props.onPick(id)} />
      <Show when={runningCommand(worker(id))}>{c => <div class="wexp"><div class="cmd"><I n="terminal" s={12} class="faint" /><code>{c().text}</code><span class="st st-run"><I n="loader" s={12} class="spin" />Running</span>
        <button class="btn btn-sm" {...ws.needs("stop_command")} onClick={() => { if (!ws.gate("stop_command")) setStopping({ worker: worker(id), item: c() }); }}>Stop…</button></div></div>}</Show>
    </>}</For>
    <Show when={props.last && others().length > 0}>
      <button class="wrow ws-wrow-sum" role="tab" aria-selected={others().some(w => w.id === props.pick) ? "true" : "false"} onClick={() => props.onPick(others()[0].id)}>
        <I n="check-circle" class="faint" /><span class="goal trunc">{others().length} finished: {others().map(w => w.role).join(", ")}</span><span></span><span class="st st-idle">View results</span><span class="el"></span>
      </button>
    </Show>
    <StopCommand target={stopping()} onClose={() => setStopping(null)} />
  </div>;
}

const decisionBlocked = (m: SessionModel, d: Decision) => d.unavailable ?? (d.suspended ? "Resume the task first" : undefined) ?? (m.mode === "paused" ? "Resume the task first" : m.mode === "live" ? undefined : "This task cannot take an answer now");

/** A permission asks one thing at a time. A settled decision reads as what was asked and what was said. */
function DecisionCard(props: { m: SessionModel; d: Decision; now: number }) {
  const ws = useWs();
  const blocked = () => decisionBlocked(props.m, props.d);
  const answer = (text: string) => { if (!blocked()) ws.run({ type: "answer_decision", taskId: props.m.view.id, decisionId: props.d.id, answer: text, optionId: null, now: ws.store.clock() }); };
  const waited = () => props.d.at === null ? "" : `waiting ${age(props.d.at, props.now)}`;
  return <Switch>
    <Match when={props.d.withdrawn}><div class="answered"><I n="x" s={14} /><span class="grow">Withdrawn: {props.d.text}</span></div></Match>
    <Match when={props.d.answer}>{a => <Answered question={props.d.text} answer={a().text} at={a().at} icon={props.d.kind === "permission" ? "shield" : "question"} />}</Match>
    <Match when={true}><div class="dc" id={`decision-${props.d.id}`}>
      <div class="dc-h"><I n="shield" s={14} />Permission<span class="r">{waited()}</span></div>
      <div class="dc-q">{props.d.text}</div>
      <Show when={props.d.unavailable}><div class="cap st-warn ws-dc-note">{props.d.unavailable}</div></Show>
      <Show when={blocked() && props.m.mode === "paused"}><div class="cap muted ws-dc-note">Preserved while paused. Resume the task to answer.</div></Show>
      <dl class="kv ws-dc-kv"><dt>Effect</dt><dd>{props.d.effect}</dd><dt>Scope</dt><dd>{props.d.scope}</dd><dt>If you deny</dt><dd>{props.d.ifDenied}</dd></dl>
      <div class="ws-dc-acts">
        <button class="btn btn-line btn-sm" aria-disabled={blocked() ? "true" : undefined} data-why={blocked()} {...ws.needs("answer_decision")} onClick={() => answer("You denied this")}>Deny</button>
        <button class="btn btn-primary btn-sm" aria-disabled={blocked() ? "true" : undefined} data-why={blocked()} {...ws.needs("answer_decision")} onClick={() => answer("You allowed once")}>Allow once</button>
      </div>
    </div></Match>
  </Switch>;
}

/** Every open question of the task, as tabs of one card. */
function Questions(props: { m: SessionModel; asks: Decision[]; now: number }) {
  const ws = useWs();
  const decision = (ask: Ask) => props.asks.find(d => d.id === ask.id)!;
  const blocked = (ask: Ask) => decisionBlocked(props.m, decision(ask));
  const answer = (ask: Ask, text: string, optionId: string | null, attachments?: AttachmentInput[]) => blocked(ask) ? undefined : ws.run({ type: "answer_decision", taskId: props.m.view.id, decisionId: ask.id, answer: text, optionId, attachments, now: ws.store.clock() });
  // The same model and effort row as the task's own composer, setting the same task: an answer box is the standard composer.
  // Access and language stay on the task's composer: they are not part of an answer, and the card has less room.
  const route = (ask: Ask) => <RouteBar id={`answer-${ask.id}`} up compact needs="set_task_settings" settings={taskSettings(props.m)}
    disabled={blocked(ask) || props.m.mode === "stopping" ? "Unavailable for this task now" : undefined}
    onChange={settings => ws.run({ type: "set_task_settings", taskId: props.m.view.id, settings: { ...taskSettings(props.m), ...settings } })} />;
  return <QuestionTabs asks={props.asks} needs="answer_decision" blocked={blocked} route={route} onAnswer={answer} waiting={ask => ask.at === null ? "" : `waiting ${age(ask.at, props.now)}`}
    note={ask => <Show when={blocked(ask) && props.m.mode === "paused"}><div class="cap muted ws-dc-note">Preserved while paused. Resume the task to answer.</div></Show>} />;
}

/** Waiting for the owner with no card, or a conflict: still reads as a decision, with the way out on it. */
function Waiting(props: { m: SessionModel }) {
  const ws = useWs();
  const t = () => props.m.view.task;
  const conflict = (): Route => ({ name: "task", machine: props.m.view.machine.id, task: t().id, tab: "changes", sub: "conflict" });
  return <div class="dc" id="decision-issue">
    <div class="dc-h"><I n={t().conflict !== "none" ? "alert" : "pause"} s={14} />{t().conflict !== "none" ? "Conflict" : "Waiting for you"}</div>
    <div class="dc-q">{t().conflict !== "none" ? `Merge conflict with ${props.m.view.target}` : t().issue ?? "Waiting for a decision"}</div>
    {/* A card's action bar, so a way out the machine does not offer says why beside it. */}
    <ActionBar place="card">
      <Show when={t().conflict === "none"} fallback={<><A class="btn btn-line btn-sm" to={conflict()}>Open the conflict</A><button class="btn btn-primary btn-sm" {...ws.needs("resolve_with_cortex")} onClick={() => ws.open({ kind: "cortex", taskId: t().id })}>Resolve automatically…</button></>}>
        <button class="btn btn-line btn-sm" {...ws.needs("clear_issue")} onClick={() => ws.run({ type: "clear_issue", taskId: t().id })}>{ws.t("clearWait")}</button>
      </Show>
    </ActionBar>
  </div>;
}

function Conversation(props: { model: Accessor<SessionModel>; now: number; pick: string; onPick: (key: string) => void }) {
  const ws = useWs();
  const m = () => props.model();
  const opened = createOpenState();
  const [dock, setDock] = createSignal(124);
  let scroller: HTMLDivElement | undefined;
  const ids = createMemo(() => m().session.timeline.map(e => e.id));
  const entries = createMemo(() => new Map(m().session.timeline.map(e => [e.id, e])));
  const entry = (id: string) => entries().get(id)!;
  const lastWorkers = () => { const list = m().session.timeline.filter(e => e.kind === "workers"); return list.at(-1)?.id; };
  const follow = createFollow(() => scroller, () => ids().length);
  // The dock's height is the timeline's bottom clearance, which resizes neither the timeline nor its rows: when the dock grows
  // (its reason takes a second line, a face arrives), a reader at the end stays at the end instead of under the dock.
  let lastDock = dock();
  createEffect(() => { const h = dock(); if (h !== lastDock) { lastDock = h; if (follow.following()) requestAnimationFrame(() => follow.jump()); } });
  const decision = (id: string) => m().session.decisions.find(d => d.id === id)!;
  const message = (id: string) => m().messages.find(x => x.id === id)!;
  const stranded = () => m().view.column === "needs" && !m().view.unknown && m().open.length === 0;
  // Questions asked together stay together until the last answer goes, so a held answer keeps its tab.
  const asks = createMemo(() => m().session.decisions.filter(d => d.kind === "question" && (isOpen(d) || d.held)));
  const inAsks = (id: string) => asks().some(d => d.id === id);
  // While a decision waits or work runs, no completed result and no Accept stand beside it: the last one stays as history.
  const held = () => m().view.task.result ? resultHeld(m(), asks().length > 0) : null;
  const changesAt = (): Route => ({ name: "task", machine: m().view.machine.id, task: m().view.id, tab: "changes" });
  return <section class="task-panel" aria-label="Conversation">
    <div class="timeline" ref={scroller} style={{ "--dock-clear": `${dock() + 16}px` }} onScroll={follow.onScroll}>
      <div class="tl-in">
        <Brief m={m()} />
        <For each={ids()}>{id => <Switch>
          <Match when={entry(id).kind === "cortex"}><div class="ai"><ResultText breaks text={(entry(id) as Extract<TimelineEntry, { kind: "cortex" }>).text} /></div></Match>
          <Match when={entry(id).kind === "said"}>{(() => {
            const text = () => (entry(id) as Extract<TimelineEntry, { kind: "said" }>).text;
            return <div class="mine"><ResultText breaks text={text()} /><AttachChips files={m().messages.find(x => x.text === text())?.attachments} open /></div>;
          })()}</Match>
          <Match when={entry(id).kind === "fold"}>{(() => {
            const f = () => entry(id) as Extract<TimelineEntry, { kind: "fold" }>;
            return <Fold text={f().text} open={opened.is(id)} onToggle={() => opened.toggle(id)}>
              {() => <TextSteps steps={f().steps} taskId={m().view.id} view={viewOfText} state={opened} scope={id} />}
            </Fold>;
          })()}</Match>
          <Match when={entry(id).kind === "plan" && m().session.plan}>
            <Fold text={`Plan r${m().session.plan!.revision}: ${m().planDone} of ${m().planTotal} done`} open={opened.is(id)} onToggle={() => opened.toggle(id)}>{() => <Plan m={m()} />}</Fold>
          </Match>
          <Match when={entry(id).kind === "workers"}>{(() => {
            const e = () => entry(id) as Extract<TimelineEntry, { kind: "workers" }>;
            return <Fold text={teamLine(m(), e().title, e().workerIds)} open={opened.is(id)} onToggle={() => opened.toggle(id)}>
              {() => <Workers m={m()} entry={e()} last={lastWorkers() === id} now={props.now} pick={props.pick} onPick={props.onPick} />}
            </Fold>;
          })()}</Match>
          <Match when={entry(id).kind === "message"}>{(() => {
            const x = () => message((entry(id) as Extract<TimelineEntry, { kind: "message" }>).messageId);
            return <>
              <div class="mine"><ResultText breaks text={x().text} /><AttachChips files={x().attachments} open /></div>
              <div class="mine-meta"><I n="check" s={12} />{m().mode === "paused" ? "Steer received, kept for when you resume" : "Steer received, applies after the current step"}<Show when={x().at !== null}><span>{clock(x().at)}</span></Show></div>
            </>;
          })()}</Match>
          <Match when={entry(id).kind === "decision"}>{(() => {
            const d = () => decision((entry(id) as Extract<TimelineEntry, { kind: "decision" }>).decisionId);
            return <Show when={inAsks(d().id)} fallback={<DecisionCard m={m()} d={d()} now={props.now} />}><Show when={asks()[0]?.id === d().id}><Questions m={m()} asks={asks()} now={props.now} /></Show></Show>;
          })()}</Match>
          <Match when={entry(id).kind === "line"}>{(() => {
            const l = () => entry(id) as Extract<TimelineEntry, { kind: "line" }>;
            return <div class={`answered${l().rule ? " ws-rule" : ""}`}><I n={l().icon} s={14} /><span>{l().text}</span><Show when={l().at !== null}><span class="faint">{clock(l().at)}</span></Show></div>;
          })()}</Match>
        </Switch>}</For>
        <Show when={stranded()}><Waiting m={m()} /></Show>
        <Show when={m().view.task.result && m().active.length === 0 && m().workers.length > 0 && !lastWorkers()}>
          <div class="answered"><I n="users" s={14} />{workersNote(m())} <A class="lnk ws-ml4" to={{ name: "task", machine: m().view.machine.id, task: m().view.id, tab: "workers" }}>Work details</A></div>
        </Show>
        <Show when={m().view.task.result && !held()}><ResultCard m={m()} /></Show>
        <Show when={m().view.task.result && held()}>
          <div class="answered ws-prev-result" role="note"><I n="eye" s={14} /><span>Previous result v{m().view.task.result!.version}, read-only. {held()}.</span> <A class="lnk ws-ml4" to={changesAt()}>Open</A></div>
        </Show>
        <Show when={m().mode === "live"}><WorkingIndicator activity={m().active.flatMap(w => w.activity)} /></Show>
      </div>
    </div>
    <Show when={ws.can(capabilityOf("send_message"))}><Composer model={props.model} onHeight={setDock} pill={<Pill follow={follow} />} folded={m().open.some(d => d.kind === "question")} /></Show>
  </section>;
}

function WorkerCard(props: { w: Worker; now: number; to: Route; selected: boolean }) {
  const ws = useWs();
  const f = () => workerState(props.w);
  const line = () => props.w.waitsFor ? `Waits for ${props.w.waitsFor}` : isFinished(props.w) ? resultPreview(props.w.result?.summary ?? props.w.goal) : props.w.now ?? props.w.goal;
  const elapsed = () => duration(elapsedOf(props.w, props.now));
  return <A class="wcard" role="tab" to={props.to} aria-selected={props.selected ? "true" : "false"}>
    <span class="l1"><span class="ic"><I n={f().icon} s={14} class={`${f().spin ? "spin " : ""}${f().tone === "st-idle" ? "faint" : f().tone}`} /></span>{props.w.role}<span class={`st ${f().tone}`}>{f().label}</span></span>
    <span class="l2 trunc" style={{ display: "block" }}>{line()}</span>
    <span class="l3"><Show when={ws.route(props.w.route)}>{route => <span>{route()}</span>}</Show><Show when={elapsed()}><span>{elapsed()}</span></Show>
      <Show when={contextOf(props.w.context)}>{c => <span>Context {c().percent}%</span>}</Show><Show when={props.w.usage !== null}><span>Usage {tokens(props.w.usage!)}</span></Show></span>
  </A>;
}

function WorkersList(props: { m: SessionModel; now: number; pick: string; route: TaskRoute }) {
  const at = (worker: string): Route => ({ name: "task", machine: props.route.machine, task: props.route.task, tab: "workers", worker });
  const lane = (name: "execution" | "review") => props.m.workers.filter(w => w.lane === name && !isFinished(w));
  const coordinator = () => props.m.session.coordinator;
  const ws = useWs();
  const route = () => ws.route(recordedCoordinatorRoute(props.m, ws.store.mode === "demo"));
  const check = () => verificationFace(props.m);
  // A component, not a call inside the list: a call would redraw every card whenever the task moved on, and a
  // card replaced between press and release swallows the click.
  const Group = (g: { title: string; list: Worker[] }) => <Show when={g.list.length > 0}>
    <div class="wg">{g.title}<Count n={g.list.length} /></div>
    <For each={g.list.map(w => w.id)}>{id => <WorkerCard w={props.m.workers.find(w => w.id === id)!} now={props.now} to={at(id)} selected={props.pick === id} />}</For>
  </Show>;
  return <div class="wlist" role="tablist" aria-orientation="vertical" aria-label="Workers">
    <div class="wg">Coordinator</div>
    <A class="wcard" role="tab" to={at(CORTEX)} aria-selected={props.pick === CORTEX ? "true" : "false"}>
      <span class="l1"><span class="ic"><I n="bot" s={14} /></span>Coordinator<span class={`st ${props.m.view.fact.tone}`}>{props.m.mode === "live" ? "Coordinating" : props.m.view.fact.text}</span></span>
      <Show when={coordinator().note}><span class="l2 trunc" style={{ display: "block" }}>{coordinator().note}</span></Show>
      <span class="l3"><Show when={route()}><span>{route()}</span></Show><Show when={coordinator().elapsedMs !== null}><span>{duration(coordinator().elapsedMs)}</span></Show><Show when={contextOf(props.m.session.context)}>{c => <span>Context {c().percent}%</span>}</Show><Show when={coordinator().usage !== null}><span>Usage {tokens(coordinator().usage!)}</span></Show></span>
    </A>
    <Group title="Execution" list={lane("execution")} />
    <Group title="Review" list={lane("review")} />
    <div class="wg">Verification</div>
    <A class="wcard lane" role="tab" to={at(VERIFICATION)} aria-selected={props.pick === VERIFICATION ? "true" : "false"}>
      <span class="l1"><span class="ic"><I n={check().icon} s={14} class={check().spin ? `spin ${check().tone}` : check().tone === "st-idle" ? "faint" : check().tone} /></span>Runtime verification<span class={`st ${check().tone}`}>{check().label}</span></span>
      <span class="l2 trunc" style={{ display: "block" }}>Automatic, not a worker</span>
    </A>
    <Group title="Finished" list={props.m.finished} />
  </div>;
}

/** The same Changes tab carries the diff review, Ready to merge and the conflict, so a task's state decides what it opens on. */
function ChangesTab(props: { m: SessionModel; route: TaskRoute }) {
  const t = () => props.m.view.task;
  const merging = () => inConflict(t()) || t().status === "ready_to_merge" || (t().integration?.method === "squash" || t().integration?.method === "commit");
  return <Switch fallback={<ChangesPanel m={props.m} route={props.route} />}>
    <Match when={props.route.sub === "conflict"}><ConflictPanel m={props.m} route={props.route} /></Match>
    <Match when={props.route.sub === undefined && merging()}><ReadyPanel m={props.m} route={props.route} /></Match>
  </Switch>;
}

function CodeTab(props: { m: SessionModel; route: TaskRoute }) {
  const ws = useWs();
  const repo = () => repoOf(ws.store.state(), props.m.view.project.id);
  const checkout = () => taskCheckout(ws.store.state(), props.m.view.task);
  const at = (file: string, line?: number): Route => ({ name: "task", machine: props.route.machine, task: props.route.task, tab: "code", file, line });
  return <Show when={repo() && checkout()} fallback={<div class="note-panel task-panel"><span class="w500">No files for this task</span><span>{ws.t(props.m.view.project.hasGit === false ? "noCodeWithoutGit" : "noCode")}</span></div>}>
    <section class="task-panel" aria-label="Code"><CodeView repo={repo()!} checkout={checkout()!} file={props.route.file} line={props.route.line} at={at} /></section>
  </Show>;
}

const firstPick = (m: SessionModel) => (m.running[0] ?? m.active[0] ?? m.workers[0])?.id ?? CORTEX;

/** The approved task session. Everything on it comes from the port. */
export function TaskPage(props: { id: string }) {
  const ws = useWs();
  const model = createMemo<SessionModel>(prev => sessionModel(ws.store.index(), props.id) ?? prev!);
  const route = () => ws.router.route() as TaskRoute;
  const now = useTicker();
  const tab = () => route().tab;
  const live = () => tab() === "workers";
  const pick = () => route().worker ?? firstPick(model());
  const workerAt = (worker: string): Route => ({ name: "task", machine: route().machine, task: route().task, tab: "workers", worker });
  const onPick = (key: string) => ws.go(workerAt(key));
  const m = () => model();
  createBodyMark("tab", () => tab() ?? "conversation");
  createBodyMark("stop", () => ["paused", "stopping"].includes(m().view.task.status) ? "stopped" : undefined);
  // A narrow window holds the sidebar away on every page: the workspace shell does it (workspace.tsx).
  const shown = useContext(ScreenActive);
  createEffect(() => {
    if (!ws.demo?.autoplay() || !shown()) return;
    const timer = setInterval(() => ws.demo?.step(props.id), 3500);
    onCleanup(() => clearInterval(timer));
  });
  const stopping = () => m().workers.filter(w => w.status === "unknown").length;
  // Each strip is one item for the life of the page, so a change of words never redraws the rest of the lane.
  const stoppingItem: StripItem = { id: "stopping", tone: "warn", icon: <I n="loader" s={14} class="spin st-warn" />, domId: taskWhyId(props.id),
    get text() { return `Stopping. ${stopping() ? `${stopping()} worker${stopping() === 1 ? " is" : "s are"} not confirmed as stopped.` : "The stop is not confirmed yet."} Close and Archive stay off until it is.`; },
    action: <button class="btn btn-line btn-sm" {...ws.needs("confirm_stop")} onClick={() => ws.run({ type: "confirm_stop", taskId: props.id, now: ws.store.clock() })}>Retry</button> };
  const archived: StripItem = { id: "archived", tone: "info", icon: <I n="archive" s={14} class="faint" />, get text() { return `Archived, read-only. ${daysLeft(archiveRow(ws.store.index(), m().view).left)} before cleanup.`; },
    action: <button class="btn btn-line btn-sm" {...ws.needs("restore_task")} onClick={() => ws.run({ type: "restore_task", taskId: props.id, now: ws.store.clock() })}><I n="undo" s={12} />Restore</button> };
  const [checking, setChecking] = createSignal(false), [rechecked, setRechecked] = createSignal(false);
  const recheck = async () => {
    if (checking()) return;
    setChecking(true); setRechecked(false);
    try { await ws.store.reload(); setRechecked(true); }
    finally { setChecking(false); }
  };
  const offline: StripItem = { id: "offline", tone: "info", icon: <I n="alert" s={14} />,
    get text() { return <>
      <strong>{m().mode === "unconfirmed" ? "Task state not confirmed." : "Machine disconnected."}</strong>{" "}
      {checking() ? "Re-checking the latest state. Sending and task controls stay off." :
        rechecked() && ws.store.readIssue() ? "Re-check could not read the latest state. Try again, or open " :
        m().mode === "unconfirmed" ? `${rechecked() ? "Still not confirmed. " : ""}The machine is connected, but it cannot confirm whether this task is running or stopped. Re-check, or open ` :
        "This machine cannot be reached. Re-check the connection, or open "}
      <Show when={!checking()}>Machine settings → <A class="lnk" to={{ name: "settings", machine: m().view.machine.id, section: "diagnostics" }}>Diagnostics</A>.</Show>
    </>; },
    action: <button class="btn btn-line btn-sm" aria-busy={checking() ? "true" : undefined} onClick={() => void recheck()}>Re-check state</button> };
  const unverified: StripItem = { id: "unverified", tone: "warn", icon: <I n="alert" s={14} class="st-warn" />, text: "Not verified. This task was closed as is, and the warning stays with its result." };
  const titleWhy: StripItem = { id: "title-why", tone: "info", icon: <I n="alert" s={14} class="st-warn" />, domId: taskWhyId(props.id), get text() { return titleBlock(m()) ?? ""; } };
  const status = createMemo<StripItem[]>(() => [
    ...(m().mode === "stopping" ? [stoppingItem] : []),
    ...(m().mode === "archived" ? [archived] : []),
    ...(["offline", "unconfirmed"].includes(m().mode) ? [offline] : []),
    ...(m().view.task.notVerifiedByCortex && m().view.task.closedAs === "as_is" ? [unverified] : []),
    ...(m().mode !== "stopping" && titleBlock(m()) ? [titleWhy] : []),
  ]);
  return <Page class="l-work l-task" scroll="none" head={<TaskBar m={m()} tab={tab()} route={route()} />} status={<StatusLane items={status()} />}>
    <div class="main-body">
      <Show when={live()} fallback={
        <div class="task-body">
          <Switch>
            <Match when={tab() === undefined}><Conversation model={model} now={now()} pick={pick()} onPick={onPick} /></Match>
            <Match when={tab() === "changes"}><ChangesTab m={m()} route={route()} /></Match>
            <Match when={tab() === "code"}><CodeTab m={m()} route={route()} /></Match>
            <Match when={tab() === "terminal"}><div class="note-panel task-panel"><span class="w500">Terminal</span><span>{ws.t("terminalNote")}</span>
              <button class="btn btn-line btn-sm" onClick={() => { if (!ws.panels.terminalOpen()) ws.toggleTerminal(); }}>Open the bottom panel</button></div></Match>
          </Switch>
        </div>}>
        <div class="task-body live">
          <section class="task-panel" aria-label="Workers"><WorkersList m={m()} now={now()} pick={pick()} route={route()} /></section>
          <section class="livepane" aria-label={`${pick() === CORTEX ? "Coordinator" : pick() === VERIFICATION ? "Runtime verification" : m().workers.find(w => w.id === pick())?.role ?? "Worker"} activity`}><label class="ws-worker-picker">Worker <select class="inp" value={pick()} onChange={e => onPick(e.currentTarget.value)}><option value={CORTEX}>Coordinator</option><For each={m().workers}>{w => <option value={w.id}>{w.role}</option>}</For><option value={VERIFICATION}>Runtime verification</option></select></label><WorkerPane model={model} pick={pick()} now={now()} /></section>
        </div>
      </Show>
    </div>
  </Page>;
}
