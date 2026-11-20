import { createEffect, createMemo, createSignal, For, Match, onCleanup, Show, Switch, useContext, type Accessor, type JSX } from "solid-js";
import { capabilityOf, FEATURES, isOpen, type Activity, type Decision, type OwnerHoldAction, type TimelineEntry, type Worker } from "@cortex/workspace-domain";
import { CodeView } from "./code";
import { archiveRow, daysLeft } from "./archive-model";
import { AttachChips, Composer, TaskAttachments } from "./composer";
import { useWs } from "./ctx";
import { Confirmed, createBodyMark, createTicker, ScreenActive } from "./freshness";
import { A, closePop, Count, createFollow, I, Keyed, Loading, MenuItem, Pop, useWantOverview } from "./kit";
import { ConflictPanel, ReadyPanel } from "./merge";
import { Answered, QuestionForm, type Ask, type AskAnswer } from "./questions";
import { AcceptBlocker, ChangesPanel, ResultCard } from "./review";
import { ActionBar, Page, StatusLane, Toolbar, Whole, type StripItem } from "./layout";
import { taskWhyId, titleBlock } from "./task-chrome";
import { inConflict, repoOf, sentence, taskCheckout } from "./review-model";
import type { Route, TaskTab } from "./router";
import { age, clock, humanStatus, ownerHold, sprintRoute, stopReason, type OwnerHold } from "./selectors";
import { closeBlocked, contextOf, coordinatorLine, duration, elapsedOf, isFinished, messageDelivery, reviewLine, workersOutcome, phases, recordedCoordinatorRoute, sessionModel, tokens, unconfirmedState, verificationFace, workerFace, workerState, workingNow, type SessionModel } from "./session-model";
import { CORTEX, Pill, StopCommand, VERIFICATION, WorkerPane, WaitingIndicator, WorkingIndicator } from "./worker";
import { createOpenState, Fold, TextSteps } from "./feed";

import { ResultText, resultPreview } from "./result-text";
import { CopySource, MessageTools } from "./message-copy";
import { SentMessages } from "./sent";
import { viewOfText } from "./tool-names";
import { usePlanningSprint, useTaskRoutes } from "./task-place";

type TaskRoute = Extract<Route, { name: "task" }>;
const useTicker = () => {
  const ws = useWs();
  const [now, setNow] = createSignal(ws.store.clock());
  createTicker(() => setNow(ws.store.clock()), 1000);
  return now;
};

const workersNote = (m: SessionModel) => {
  // A worker last seen running under a task that has ended is named, never counted as active.
  const stale = m.workers.filter(w => w.stale).length;
  if (stale) return `${m.finished.length} finished, ${stale} not confirmed (the latest worker facts could not be read)`;
  if (m.active.length || m.finished.length !== m.workers.length) return `${m.active.length} active of ${m.workers.length}`;
  return workersOutcome(m.workers, 0).replace(/^0 active, /, "");
};

/** What the task is doing, in words. No percentage is made up: a phase or the last recorded fact, and the plan's real count. */
function progressLine(m: SessionModel) {
  if (m.view.unknown) return "Last recorded activity";
  const now = phases(m).find(p => p.state === "now")?.label.replace(/\s+\d+\/\d+$/, "");
  const plan = m.session.plan && m.planTotal ? `${m.planDone} of ${m.planTotal} steps done` : "";
  return [now ?? m.view.fact.text, plan].filter(Boolean).join(", ");
}

const TASK_AREAS: readonly { id: TaskTab; label: string }[] = [{ id: "workers", label: "Work details" }, { id: "changes", label: "Changes" }, { id: "code", label: "Code" }, { id: "terminal", label: "Terminal" }];

/**
 * The conversation is the task's default view. Work details, Changes, Code and Terminal stay one activation away, each
 * with its own route, and a detail view has one obvious way back. On a narrow pane the entries fold into one Details menu.
 */
function TaskBar(props: { m: SessionModel; tab: TaskTab | undefined; route: TaskRoute }) {
  const ws = useWs();
  const at = (tab?: TaskTab): Route => ({ name: "task", machine: props.route.machine, task: props.route.task, tab });
  const changes = () => props.m.session.changes;
  // Each area needs its own capability, never Git fetch: Work details is the runtime's record, Changes the result review, Code the files.
  // The same entry objects on every read, so a capability reread keeps each link's element (`For` keys by reference).
  const entries = () => TASK_AREAS.filter(t => t.id === "workers" || ws.store.mode === "demo"
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

/** A factual one-line summary of a team update, opened in place for the full list. A settled team reads as the header's
 *  outcome line: a cancelled or failed worker is never counted as finished. */
function teamLine(m: SessionModel, title: string, ids: string[]) {
  const list = m.workers.filter(w => ids.includes(w.id));
  const working = list.filter(w => !isFinished(w)).length;
  return `${title}: ${working ? `${working} of ${list.length} working` : workersOutcome(list, 0).replace(/^0 active, /, "")}`;
}

/** Why a recorded result is not offered as the current one right now, or null when it is. */
function resultHeld(m: SessionModel, pending: boolean): string | null {
  if (m.view.task.closedAs === "unconfirmed") return "The task was closed without verification";
  if (m.mode === "offline" || m.mode === "unconfirmed") return "State needs confirmation";
  if (pending || m.open.length > 0) return "A decision is pending";
  if (m.mode === "stopping") return "The stop is not confirmed yet";
  // Only work that runs says so. A task held for the owner's message (an interrupted turn, a stop) runs nothing.
  if (workingNow(m) || m.active.some(w => w.status === "working")) return "Work is running";
  if (m.mode === "live") return "Waiting for your message";
  return null;
}

function Brief(props: { m: SessionModel }) {
  const ws = useWs();
  const t = () => props.m.view.task;
  const s = () => props.m.session;
  // Above a conversation read only in part, the description stays where it is while earlier rows are put below it: the
  // reading position holds on what the reader looks at, not on this.
  return <div class="brief" data-follow-skip={s().historyEarlier ? "" : undefined}>
    <ResultText breaks text={t().description} />
    <TaskAttachments taskId={t().id} />
    <div class="meta">
      <Show when={s().createdAt !== null}><span>Created {clock(s().createdAt)}</span></Show>
      <Show when={s().base}>{b => <span>Base <span class="mono">{b().branch}</span> at <span class="mono">{b().commit}</span></span>}</Show>
      <Show when={ws.t("sessionBadge")}><span>{ws.t("sessionBadge")}</span></Show>
      <Show when={t().titleState === "pending" && ws.store.mode === "demo"}><span>Naming…</span></Show>
      <Show when={t().titleState === "failed"}><span class="st-warn">Title not generated <button class="lnk" {...ws.needs("generate_title")} onClick={() => ws.run({ type: "generate_title", taskId: t().id })}>Retry</button></span></Show>
      <Show when={t().description}><CopySource text={t().description} /></Show>
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

function WorkerRow(props: { w: Worker; now: number; selected: boolean; onPick: (el: Element) => void }) {
  const ws = useWs();
  const f = () => workerState(props.w);
  const goal = () => props.w.waitsFor ? `Waits for ${props.w.waitsFor}` : props.w.now ?? props.w.goal;
  return <button class="wrow" role="tab" aria-selected={props.selected ? "true" : "false"} onClick={e => props.onPick(e.currentTarget)}>
    <I n={f().icon} class={`${f().spin ? "spin " : ""}${f().tone === "st-idle" ? "faint" : f().tone}`} />
    <span class="trunc"><span class="role">{props.w.role}</span><br /><span class="goal">{goal()}</span></span>
    <span class={`route${props.w.route ? "" : " faint"}`}>{ws.route(props.w.route)}</span><span class={`st ${f().tone}`}>{f().label}</span><span class="el">{duration(elapsedOf(props.w, props.now))}</span>
  </button>;
}

/** The worker the team bar names first: the newest one at work, else the newest still active, else the newest one. */
export const leadWorker = (m: SessionModel): Worker | undefined => m.running[0] ?? m.active[0] ?? m.workers.find(w => !isFinished(w)) ?? m.workers[0];

/**
 * The task's workers, pinned directly above the composer and outside the scrolling feed: who works now reads at a glance, and
 * the whole list opens as a menu in the top layer, so opening it moves neither the feed nor the composer. Rows are keyed by
 * worker id, so a background read updates them in place.
 */
function TeamBar(props: { m: SessionModel; now: number; pick: string; onPick: (key: string) => void }) {
  const ws = useWs();
  const menu = () => `m-team-${props.m.view.id}`;
  const ids = createMemo(() => props.m.workers.map(w => w.id), undefined, { equals: (a, b) => a.length === b.length && a.every((id, i) => id === b[i]) });
  const worker = (id: string) => props.m.workers.find(w => w.id === id);
  const leadId = createMemo(() => leadWorker(props.m)?.id);
  const lead = () => { const id = leadId(); return id === undefined ? undefined : worker(id); };
  const where = () => { const c = props.m.view.task.checkout; return c.kind === "worktree" ? "Same worktree" : "Project folder"; };
  const others = () => Math.max(0, props.m.running.length - 1);
  const [stopping, setStopping] = createSignal<{ worker: Worker; item: Activity } | null>(null);
  const runningCommand = (w: Worker) => w.status === "working" ? w.activity.find(a => a.command?.state === "running") : undefined;
  const pick = (id: string, el: Element) => { closePop(el); props.onPick(id); };
  const routes = useTaskRoutes(() => ({ machine: props.m.view.machine.id, id: props.m.view.id }));
  return <Show when={ids().length > 0}>
    <div class="ws-team" role="group" aria-label="Workers now">
      <Show when={lead()}>{w => {
        const f = () => workerState(w());
        const line = () => w().waitsFor ? `Waits for ${w().waitsFor}` : isFinished(w()) ? resultPreview(w().result?.summary ?? w().goal) : w().now ?? w().goal;
        return <button type="button" class="ws-team-lead" aria-label={`${w().role}, ${f().label}. Open its activity`} onClick={e => pick(w().id, e.currentTarget)}>
          <I n={f().icon} s={14} class={`${f().spin ? "spin " : ""}${f().tone === "st-idle" ? "faint" : f().tone}`} />
          <span class="ws-team-role">{w().role}</span><span class={`st ${f().tone}`}>{f().label}</span>
          <span class="ws-team-now trunc">{line()}</span>
        </button>;
      }}</Show>
      <Show when={others() > 0}><span class="ws-team-more tnum">+{others()} working</span></Show>
      <button type="button" class="btn btn-sm ws-team-all" popovertarget={menu()} aria-haspopup="dialog" aria-label="All workers">
        <I n="users" s={14} /><span class="tnum">{teamLine(props.m, "Workers", ids())}</span><I n="chevron-down" s={12} />
      </button>
    </div>
    <Pop id={menu()} class="pop-up ws-team-pop" align="right" label="All workers">
      <div class="wt-h"><I n="users" s={14} class="faint" />Workers<span class="r">{where()}<Show when={props.m.view.task.checkout.branch}>, <span class="mono">{props.m.view.task.checkout.branch}</span></Show></span></div>
      <div class="ws-team-list" role="tablist" aria-orientation="vertical" aria-label="Workers">
        <For each={ids()}>{id => <Show when={worker(id)}>{w => <>
          <WorkerRow w={w()} now={props.now} selected={props.pick === id} onPick={el => pick(id, el)} />
          <Show when={runningCommand(w())}>{c => <div class="wexp"><div class="cmd"><I n="terminal" s={12} class="faint" /><code>{c().text}</code><span class="st st-run"><I n="loader" s={12} class="spin" />Running</span>
            <button class="btn btn-sm" {...ws.needs("stop_command")} onClick={e => { if (!ws.gate("stop_command")) { closePop(e.currentTarget); setStopping({ worker: w(), item: c() }); } }}>Stop…</button></div></div>}</Show>
        </>}</Show>}</For>
      </div>
      <div class="ws-team-foot"><A class="lnk" to={routes.workers()} onClick={e => closePop(e.currentTarget)}>Work details</A></div>
    </Pop>
    {/* The stop dialog stands outside the menu: a dialog inside a closed popover would never show. */}
    <StopCommand target={stopping()} onClose={() => setStopping(null)} />
  </Show>;
}

const decisionBlocked = (m: SessionModel, d: Decision) => d.unavailable ?? (d.suspended ? "Resume the task first" : undefined) ?? (m.mode === "paused" ? "Resume the task first" : m.mode === "live" ? undefined : "This task cannot take an answer now");

/** A permission asks one thing at a time. A settled decision reads as what was asked and what was said. */
function DecisionCard(props: { m: SessionModel; d: Decision; now: number }) {
  const ws = useWs();
  const blocked = () => decisionBlocked(props.m, props.d);
  // A runtime permission decision answers with one of its own two options; the label is the answer text.
  const option = (label: string) => props.d.options.find(o => o.label === label)?.optionId ?? null;
  const answer = (text: string, optionId: string | null) => { if (!blocked()) ws.run({ type: "answer_decision", taskId: props.m.view.id, decisionId: props.d.id, answer: text, optionId, now: ws.store.clock() }); };
  const waited = () => props.d.at === null ? "" : `waiting ${age(props.d.at, props.now)}`;
  const p = () => props.d.permission;
  const closeWhy = () => closeBlocked(props.m);
  // The task planning a sprint closes with its sprint's planning, never on its own.
  const planning = usePlanningSprint();
  return <Switch>
    {/* A withdrawn question reads as its one-line reason; one the runtime replaced says so, so two stops never read as two different reasons. */}
    <Match when={props.d.withdrawn}><div class="answered"><I n="x" s={14} /><span class="grow">{withdrawnLine(props.d)}</span></div></Match>
    <Match when={props.d.answer}>{a => <Answered question={p() ? permissionLine(props.d) : props.d.text} answer={a().text} at={a().at} files={a().attachments} icon={props.d.kind === "permission" ? "shield" : "question"} />}</Match>
    <Match when={props.m.view.task.closedAs === "unconfirmed"}>
      <div class="answered"><I n="eye" s={14} /><span class="grow">{p() ? permissionLine(props.d) : props.d.text}</span><span class="faint">Kept on record; no answer needed</span></div>
    </Match>
    <Match when={p()}>{perm => <div class="dc" id={`decision-${props.d.id}`} role="group" aria-label={`Permission asked by ${props.d.from || "a worker"}`}>
      <div class="dc-h"><I n="shield" s={14} />Permission<Show when={props.d.from}><span class="muted">Asked by {props.d.from}</span></Show><span class="r">{waited()}</span></div>
      <div class="dc-q">{permissionLine(props.d)}</div>
      <Show when={blocked() && props.m.mode === "paused"}><div class="cap muted ws-dc-note">Preserved while paused. Resume the task to answer.</div></Show>
      <dl class="kv ws-dc-kv">
        <dt>Operation</dt><dd>{OPERATION[perm().operation] ?? perm().operation}</dd>
        <dt>Resource</dt><dd><PermissionPaths paths={perm().paths} /></dd>
        <dt>Why</dt><dd>{permissionWhy(perm())}</dd>
        <Show when={perm().access}><dt>Mode</dt><dd>{accessName(perm().access)}</dd></Show>
      </dl>
      <div class="cap muted ws-dc-note">Allow once lets this worker continue this request under the shown access mode. It does not grant a standing permission.</div>
      <div class="ws-dc-acts">
        <button type="button" class="btn btn-line btn-sm" aria-disabled={blocked() ? "true" : undefined} data-why={blocked()} {...ws.needs("answer_decision")} onClick={() => answer("Deny", option("Deny"))}>Deny</button>
        <button type="button" class="btn btn-primary btn-sm" aria-disabled={blocked() ? "true" : undefined} data-why={blocked()} {...ws.needs("answer_decision")} onClick={() => answer("Allow once", option("Allow once"))}>Allow once</button>
        <Show when={props.m.view.task.status !== "done" && !props.m.view.unknown && !planning()}>
          <button type="button" class="btn btn-line btn-sm" aria-disabled={closeWhy() ? "true" : undefined} data-why={closeWhy() ?? undefined} {...ws.needs("close_task")} onClick={() => { if (!closeWhy()) ws.closeTask(props.m.view.id); }}>Close task…</button>
        </Show>
      </div>
    </div>}</Match>
    <Match when={true}><div class="dc" id={`decision-${props.d.id}`}>
      <div class="dc-h"><I n="shield" s={14} />Permission<span class="r">{waited()}</span></div>
      <div class="dc-q">{props.d.text}</div>
      <Show when={props.d.unavailable}><div class="cap st-warn ws-dc-note">{props.d.unavailable}</div></Show>
      <Show when={blocked() && props.m.mode === "paused"}><div class="cap muted ws-dc-note">Preserved while paused. Resume the task to answer.</div></Show>
      <dl class="kv ws-dc-kv"><dt>Effect</dt><dd>{props.d.effect}</dd><dt>Scope</dt><dd>{props.d.scope}</dd><dt>If you deny</dt><dd>{props.d.ifDenied}</dd></dl>
      <div class="ws-dc-acts">
        <button class="btn btn-line btn-sm" aria-disabled={blocked() ? "true" : undefined} data-why={blocked()} {...ws.needs("answer_decision")} onClick={() => answer("You denied this", null)}>Deny</button>
        <button class="btn btn-primary btn-sm" aria-disabled={blocked() ? "true" : undefined} data-why={blocked()} {...ws.needs("answer_decision")} onClick={() => answer("You allowed once", null)}>Allow once</button>
      </div>
    </div></Match>
  </Switch>;
}

const OPERATION: Record<string, string> = { read: "read", write: "write", delete: "delete", run: "run", fetch: "fetch" };
const REASON: Record<string, string> = {
  outside_project: "It is outside the project.",
  credential: "It is credential or key material.",
  control: "It changes Git control files or engine settings.",
  deletion: "It deletes or moves files.",
  unresolvable_path: "Its path cannot be resolved.",
  network: "It fetches from the network.",
};
const accessName = (mode: string) => mode === "guarded" ? "Guarded" : mode === "development" ? "Development" : mode === "auto" ? "Auto" : mode;
/** One line: who wants to do what to how many paths. */
export const permissionLine = (d: Decision) => {
  const p = d.permission;
  if (!p) return d.text;
  const what = OPERATION[p.operation] ?? "use";
  const where = p.paths.length === 0 ? "" : p.paths.length === 1 ? ` ${p.paths[0]}` : ` ${p.paths.length} paths`;
  return `${d.from ? d.from.charAt(0).toUpperCase() + d.from.slice(1) : "A worker"} wants to ${what}${where}`;
};
export const permissionWhy = (p: NonNullable<Decision["permission"]>) => REASON[p.reason] ?? (p.reasonText ? `${p.reasonText.charAt(0).toUpperCase()}${p.reasonText.slice(1)}.` : "It is outside the usual bounds.");
/** The exact paths; past three, the rest fold behind a count. */
function PermissionPaths(props: { paths: string[] }) {
  const shown = () => props.paths.slice(0, 3);
  const rest = () => props.paths.slice(3);
  return <Show when={props.paths.length > 0} fallback={<span class="muted">No path named</span>}>
    <ul class="ws-perm-paths"><For each={shown()}>{p => <li class="mono">{p}</li>}</For></ul>
    <Show when={rest().length > 0}><details><summary>{rest().length} more</summary><ul class="ws-perm-paths"><For each={rest()}>{p => <li class="mono">{p}</li>}</For></ul></details></Show>
  </Show>;
}

/** Every open question of the task, as one form with one button that sends all of their answers. */
function Questions(props: { m: SessionModel; asks: Decision[]; now: number }) {
  const ws = useWs();
  const decision = (ask: Ask) => props.asks.find(d => d.id === ask.id)!;
  const blocked = (ask: Ask) => decisionBlocked(props.m, decision(ask));
  // The answer goes into the task's running session, whose model and effort are already chosen, so the form offers neither.
  // A sprint's planning task is answered through its sprint's planning, which keeps the sprint's state with the answer.
  const planning = usePlanningSprint();
  const send = (answers: AskAnswer[]) => {
    if (!answers.length || answers.some(a => blocked(a.ask))) return undefined;
    const [first] = answers;
    const batch = answers.length > 1 ? { answers: answers.map(a => ({ decisionId: a.ask.id, answer: a.text, optionId: a.optionId, attachments: a.attachments })) } : {};
    const sprint = planning();
    if (sprint) return ws.run({ type: "answer_planning", sprintId: sprint.id, decisionId: first.ask.id, answer: first.text, optionId: first.optionId, attachments: first.attachments, now: ws.store.clock(), ...batch });
    return ws.run({ type: "answer_decision", taskId: props.m.view.id, decisionId: first.ask.id, answer: first.text, optionId: first.optionId, attachments: first.attachments, now: ws.store.clock(), ...batch });
  };
  return <QuestionForm asks={props.asks} needs={planning() ? "answer_planning" : "answer_decision"} blocked={blocked} projectId={props.m.view.project.id} onSend={send} waiting={ask => ask.at === null ? "" : `waiting ${age(ask.at, props.now)}`}
    note={ask => <>
      <Show when={blocked(ask) && props.m.mode === "paused"}><div class="cap muted ws-dc-note">Preserved while paused. Resume the task to answer.</div></Show>
      <StopTargets m={props.m} d={decision(ask)} />
    </>} />;
}

/** The first line of a question: a runtime stop question's text starts with its reason. */
const firstLine = (text: string) => text.split("\n").map(l => l.trim()).find(Boolean) ?? text;
export const withdrawnLine = (d: Decision) => d.replacedBy ? `Replaced by the question below: ${d.headline ?? firstLine(d.text)}` : `Withdrawn: ${d.headline ?? firstLine(d.text)}`;

/** What the runtime's stop question names, as places to open: the task holding the files, the review report, the denied path. */
function StopTargets(props: { m: SessionModel; d: Decision }) {
  const machine = () => props.m.view.machine.id;
  const routes = useTaskRoutes(() => ({ machine: machine(), id: props.m.view.id }));
  const report = (id: string): Route => routes.workers(props.m.workers.find(w => w.result?.reportId === id)?.id);
  return <Show when={props.d.targets?.length}><div class="ws-dc-acts ws-dc-targets">
    <For each={props.d.targets}>{t => t.kind === "task"
      ? <A class="btn btn-line btn-sm" to={{ name: "task", machine: machine(), task: t.taskId }}><I n="external" s={12} />Open task{t.title ? ` “${t.title}”` : ""}</A>
      : t.kind === "report"
      ? <A class="btn btn-line btn-sm" to={report(t.reportId)}><I n="eye" s={12} />{t.role === "code-reviewer" ? "Open review findings" : "Open report"}</A>
      : <span class="cap muted"><I n="shield" s={12} />Writing denied by your rule: <code>{t.path}</code></span>}</For>
  </div></Show>;
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
    <Show when={t().conflict === "none"}><p class="dc-note">{ws.store.can(capabilityOf("clear_issue"))
      ? "Decision details are not available yet. Use Answer in the task header to open the decision; if details are still loading, try again after the conversation refreshes."
      : "The question for this step is not available. Continue lets the agent decide the next step from the recorded state; nothing is repeated. Or close the task as is."}</p></Show>
    <ActionBar place="card">
      <Show when={t().conflict === "none"} fallback={<><A class="btn btn-line btn-sm" to={conflict()}>Open the conflict</A><button class="btn btn-primary btn-sm" {...ws.needs("resolve_with_cortex")} onClick={() => ws.open({ kind: "cortex", taskId: t().id })}>Resolve automatically…</button></>}>
        {/* A way out this machine does not offer is not presented as one. */}
        <Show when={ws.store.can(capabilityOf("clear_issue"))} fallback={<>
          <button class="btn btn-primary btn-sm" {...ws.needs("retry_task")} onClick={e => void ws.busy(ws.run({ type: "retry_task", taskId: t().id, now: ws.store.clock() }), e.currentTarget)}>Continue</button>
          <button class="btn btn-line btn-sm" {...ws.needs("close_task")} aria-disabled={closeBlocked(props.m) ? "true" : undefined} data-why={closeBlocked(props.m) ?? undefined} onClick={() => { if (!closeBlocked(props.m)) ws.closeTask(t().id); }}>Close as is…</button>
        </>}>
          <button class="btn btn-line btn-sm" {...ws.needs("clear_issue")} onClick={() => ws.run({ type: "clear_issue", taskId: t().id })}>{ws.t("clearWait")}</button>
        </Show>
      </Show>
    </ActionBar>
  </div>;
}

/** What each owner action of a hold is called on its button. */
const HOLD_ACTION_LABEL: Record<OwnerHoldAction, string> = {
  allow_finish: "Allow Automatic to finish…", accept_result: "Accept with recorded checks…", allow_repair: "Let the agent fix it",
  allow_retries: "Allow more attempts", confirm_and_continue: "I checked it, continue", resume_task: "Resume", retry: "Try again",
  confirm_integration: "Review and confirm integration…", resolve_conflict: "Open the conflict", change_run_mode: "Open sprint settings",
  ask_agent: "Ask the agent to try again", close_as_is: "Close as is…",
};
const ASK_AGENT_TEXT = "Try again from the recorded state. Finish every unfinished or failed step of this task, obtain the required review and verification again, and publish a result.";

/**
 * The pinned decision of a held task, above the message box. It renders the service's decision alone (`ownerHold`): the
 * plain-language list of what is asked, the primary action that grants it so the queue finishes by itself, and the separate
 * Close as is… with its confirmation. It appears as soon as the hold arrives; an action that needs the result waits for it.
 * When the agent or the runtime acts next, it is one quiet line and asks nothing of the owner.
 */
export function OwnerHoldCard(props: { m: SessionModel; route: TaskRoute }) {
  const ws = useWs();
  const view = () => props.m.view;
  const hold = () => props.m.open.length ? null : ownerHold(view());
  // A sent action waits only for the hold it answered: a new hold (another code, reason or consent) offers its actions again.
  const holdKey = (h: OwnerHold | null) => h ? `${h.holdId ?? ""}|${h.code}|${h.reason}|${h.allowed}|${h.actions.join(",")}` : "";
  const [sentFor, setSentFor] = createSignal<{ action: string; key: string } | null>(null);
  const sent = () => { const s = sentFor(); return s && s.key === holdKey(hold()) ? s.action : null; };
  const setSent = (action: string) => setSentFor({ action, key: holdKey(hold()) });
  // A refusal is the answer to this hold: it stays in the card in plain words, and the refused action is not offered
  // again until the hold changes (a new hold offers its actions again).
  const [refusedFor, setRefusedFor] = createSignal<{ action: string; key: string; message: string } | null>(null);
  const refused = () => { const r = refusedFor(); return r && r.key === holdKey(hold()) ? r : null; };
  const refuse = (action: string, key: string, message: string) => setRefusedFor({ action, key, message });
  const error = () => refused()?.message ?? null;
  const title = (h: OwnerHold) => h.mode === "automatic" ? "Automatic needs your decision" : h.source === "queue" ? "The queue needs your decision" : "This task needs your decision";
  const offer = () => {
    const v = view(), r = v.task.result;
    return v.sprint && r?.sourceVersion ? { sprintId: v.sprint.id, targetBranch: v.target, sourceVersion: r.sourceVersion, digest: r.digest } : null;
  };
  const loading = (a: OwnerHoldAction) => (a === "allow_finish" || a === "accept_result") && !view().task.result ? "Loading the result…" : null;
  const why = (a: OwnerHoldAction) => (a === "close_as_is" ? closeBlocked(props.m) : loading(a) ?? (sent() === a ? "Sent. Waiting for the queue to act on it…" : null))
    ?? (refused()?.action === a ? refused()!.message : null);
  const run = (a: OwnerHoldAction, h: OwnerHold, control: HTMLElement) => {
    if (why(a)) return;
    const id = view().id, now = ws.store.clock();
    const key = holdKey(h);
    const send = (command: Parameters<typeof ws.run>[0]) => {
      void ws.busy(ws.run(command), control).then(receipt => receipt.ok ? setSent(a) : refuse(a, key, receipt.message))
        .catch(e => refuse(a, key, e instanceof Error ? e.message : "The request could not be sent."));
    };
    switch (a) {
      case "allow_finish": { const o = offer(); if (o) ws.open({ kind: "accept", taskId: id, automaticFinish: o }); return; }
      case "accept_result": ws.open({ kind: "accept", taskId: id }); return;
      case "allow_repair": if (h.sprintId) send({ type: "allow_queue_repair", sprintId: h.sprintId, taskId: id, code: h.code, ...(h.holdId ? { holdId: h.holdId } : {}), now }); return;
      case "allow_retries": case "resume_task": send({ type: "continue_task", taskId: id, now }); return;
      case "confirm_and_continue": send({ type: "retry_task", taskId: id, now }); return;
      case "retry": if (h.sprintId) send({ type: "run_sprint", sprintId: h.sprintId, now }); return;
      case "ask_agent": send({ type: "request_changes", taskId: id, text: ASK_AGENT_TEXT, now }); return;
      case "close_as_is": ws.closeTask(id, undefined, message => refuse(a, key, message)); return;
      default: return;
    }
  };
  const link = (a: OwnerHoldAction): Route | null => {
    const r = props.route;
    if (a === "confirm_integration") return { name: "task", machine: r.machine, task: r.task, tab: "changes" };
    if (a === "resolve_conflict") return { name: "task", machine: r.machine, task: r.task, tab: "changes", sub: "conflict" };
    if (a === "change_run_mode" && view().sprint) return sprintRoute(r.machine, view().sprint!);
    return null;
  };
  const NEEDS: Partial<Record<OwnerHoldAction, Parameters<typeof ws.needs>[0]>> = {
    allow_finish: "accept_result", accept_result: "accept_result", allow_repair: "allow_queue_repair", allow_retries: "continue_task", resume_task: "continue_task",
    confirm_and_continue: "retry_task", retry: "run_sprint", ask_agent: "request_changes", close_as_is: "close_task",
  };
  const Action = (p: { a: OwnerHoldAction; h: OwnerHold; primary: boolean }) => {
    const cls = `btn ${p.primary ? "btn-primary" : "btn-line"} btn-sm`;
    const to = link(p.a);
    const needs = NEEDS[p.a];
    return to ? <A class={cls} to={to}>{HOLD_ACTION_LABEL[p.a]}</A>
      : <button class={cls} {...(needs ? ws.needs(needs) : {})} aria-disabled={why(p.a) ? "true" : undefined} data-why={why(p.a) ?? undefined}
          onClick={e => run(p.a, p.h, e.currentTarget)}>{HOLD_ACTION_LABEL[p.a]}</button>;
  };
  return <Show when={hold()}>{h => <Show when={h().nextActor === "owner"} fallback={
    <div class="answered ws-hold-quiet" role="status"><I n="loader" s={14} /><span>{h().nextActor === "agent" ? "The agent is fixing this: " : "This continues by itself: "}{sentence(humanStatus(h().reason))}</span></div>}>
    <section class="chat-review-hold" aria-label="Your decision is needed"><div class="chat-review-box">
      <div class="chat-review-decision" role="status">
        <h3>{title(h())}</h3>
        <p class="chat-review-reason">{sentence(humanStatus(h().reason, view().fact.text))}</p>
        <Show when={h().asks.length}><p>What you are asked to allow:</p><ul class="chat-review-asks"><For each={h().asks}>{ask => <li>{ask}</li>}</For></ul></Show>
        <Show when={h().allowed}><p class="cap muted">You allowed this. The queue acts on it in its next step.</p></Show>
      </div>
      <Show when={error()}>{e => <div class="cap st-bad" role="alert">{e()}</div>}</Show>
      <ActionBar place="card">
        <For each={h().actions.filter(a => a !== "close_as_is")}>{(a, i) => <Action a={a} h={h()} primary={i() === 0} />}</For>
        <Show when={h().actions.includes("close_as_is")}><Action a="close_as_is" h={h()} primary={h().actions.length === 1} /></Show>
      </ActionBar>
    </div></section>
  </Show>}</Show>;
}

/**
 * The pinned decision of a task whose state Cortex cannot confirm (`unconfirmedState`, from the runtime's own warning). While a
 * session is still being stopped the runtime decides by itself: one quiet line. Otherwise it is the owner's: what happened in plain
 * words, where to inspect it, and Close as is… with its confirmation, which replays nothing.
 */
export function UnconfirmedCard(props: { m: SessionModel; route: TaskRoute }) {
  const ws = useWs();
  const state = () => unconfirmedState(props.m);
  const view = () => props.m.view;
  const [checking, setChecking] = createSignal(false);
  const recheck = () => {
    if (checking()) return;
    setChecking(true);
    void ws.store.reload().finally(() => setChecking(false));
  };
  const title = () => view().sprint?.runMode === "automatic" ? "Automatic needs your decision" : "This task needs your decision";
  const reason = () => state()?.kind === "exit_unconfirmed"
    ? "A process of this task did not confirm that it stopped, so it is not known whether it still runs. Nothing it did is repeated, and the task cannot continue while this is unknown. The service checks it again by itself the next time it starts."
    : state()?.closable
      ? "The saved records of this task disagree, so the service cannot confirm whether it is running or stopped."
      : "The service could not confirm this task’s state. Nothing is repeated.";
  const asks = () => state()?.kind === "exit_unconfirmed"
    ? ["Inspect what the task did: its workers and their activity are under Workers, and machine checks under Diagnostics.",
       "Or close the task as is: what it did stays as it is, the result is marked Not verified, and nothing is repeated."]
    : ["Re-check the state or open Diagnostics to inspect the records.",
       ...(state()?.closable ? ["Or close the task as is. This keeps the records, verifies nothing and does not stop or resume the run."] : [])];
  const workers = (): Route => ({ name: "task", machine: props.route.machine, task: props.route.task, tab: "workers" });
  const blocked = () => closeBlocked(props.m);
  return <Show when={state()}>{s => <Show when={s().kind !== "stopping"} fallback={
    <div class="answered ws-hold-quiet" role="status"><I n="loader" s={14} /><span>This continues by itself: the service is confirming that this task's processes stopped.</span></div>}>
    <section class="chat-review-hold" aria-label="Your decision is needed">
      <div class="chat-review-decision" role="status">
        <h3>{title()}</h3>
        <p class="chat-review-reason">{reason()}</p>
        <p>What you can do:</p><ul class="chat-review-asks"><For each={asks()}>{ask => <li>{ask}</li>}</For></ul>
      </div>
      <ActionBar place="card">
        <button class="btn btn-line btn-sm" aria-busy={checking() ? "true" : undefined} onClick={recheck}>Re-check state</button>
        <Show when={s().kind === "exit_unconfirmed"}><A class="btn btn-line btn-sm" to={workers()}>Open workers</A></Show>
        <A class="btn btn-line btn-sm" to={{ name: "settings", machine: view().machine.id, section: "diagnostics" }}>Open Diagnostics</A>
        <button class={`btn ${s().closable ? "btn-primary" : "btn-line"} btn-sm`} {...ws.needs("close_task")} aria-disabled={blocked() ? "true" : undefined}
          data-why={blocked() ?? undefined}
          onClick={() => { if (!blocked()) ws.closeTask(view().id); }}>Close as is…</button>
      </ActionBar>
    </section>
  </Show>}</Show>;
}

/** Whether the retained conversation below is all of it: still being read, or a read that did not complete, with its reason
 *  and a way to read it again. An empty conversation is never shown as if nothing had been said. */
function HistoryState(props: { m: SessionModel }) {
  const ws = useWs();
  const read = () => props.m.session.historyRead;
  return <Switch>
    <Match when={read()?.state === "loading"}><Loading class="cap faint" text="Loading the conversation…" /></Match>
    <Match when={read()?.state === "failed"}>
        <div class="answered" role="alert"><I n="alert" s={14} /><span>{props.m.session.timeline.length ? "The latest conversation could not be read" : "The conversation could not be read"}: {humanStatus((read() as { reason: string }).reason)}. Retrying automatically.</span>
        <button class="lnk" onClick={() => ws.store.retryHistory(props.m.view.id)}>Retry now</button></div>
    </Match>
  </Switch>;
}

/** A conversation read only in part says so at its top: earlier conversation exists and is not shown. It is read on
 *  request here, or as the reader scrolls near the top; while it is read, and when the read failed, the line says so. */
function Earlier(props: { m: SessionModel }) {
  const ws = useWs();
  const e = () => props.m.session.historyEarlier;
  const load = () => ws.store.loadEarlierHistory(props.m.view.id);
  return <Show when={e()}>{earlier => <div class="ws-earlier" data-follow-skip="">
    <Switch>
      <Match when={earlier().state === "loading"}><Loading class="cap faint" text="Loading earlier…" /></Match>
      <Match when={earlier().state === "failed"}>
        <div class="answered" role="alert"><I n="alert" s={14} /><span>Earlier conversation could not be read: {humanStatus((earlier() as { reason: string }).reason)}.</span>
          <button class="lnk" onClick={load}>Retry</button></div>
      </Match>
      <Match when={earlier().state === "available"}>
        <div class="answered" role="note"><I n="clock" s={14} /><span>Earlier conversation is not shown.</span>
          <button class="lnk" onClick={load}>Show earlier</button></div>
      </Match>
    </Switch>
  </div>}</Show>;
}

/** How many entries of `ids` were put before the ones held earlier (negative when entries at the front were dropped): each
 *  change is measured from the first earlier entry that is still there. New entries are those after the front. */
export function createPrepended(ids: Accessor<string[]>) {
  let seen: string[] = [];
  return createMemo<number>(total => {
    const now = ids(), at = new Map(now.map((id, i) => [id, i]));
    let moved = 0;
    for (let k = 0; k < seen.length; k++) { const i = at.get(seen[k]); if (i !== undefined) { moved = i - k; break; } }
    seen = now;
    return total + moved;
  }, 0);
}

/**
 * A task's conversation: its brief, timeline, questions, result and composer. The task that plans a sprint reads it on its
 * sprint's page (PlanningPlace): `brief` is then the sprint's own description, `tail` the proposed plan's line, and its
 * plan stands for approval on the sprint, so neither a result card nor a task decision is offered here.
 */
export function Conversation(props: { model: Accessor<SessionModel>; now: number; pick: string; onPick: (key: string) => void; route?: TaskRoute; hidden?: boolean; brief?: JSX.Element; tail?: JSX.Element }) {
  const ws = useWs();
  const m = () => props.model();
  const planning = usePlanningSprint();
  const opened = createOpenState();
  let scroller: HTMLDivElement | undefined;
  // The team is not a feed entry: it stands pinned above the composer (TeamBar), so the feed never ends in a workers block.
  const ids = createMemo(() => m().session.timeline.filter(e => e.kind !== "workers").map(e => e.id));
  const entries = createMemo(() => new Map(m().session.timeline.map(e => [e.id, e])));
  const entry = (id: string) => entries().get(id)!;
  // Earlier rows read above the ones on screen are neither new nor the owner's latest: only entries after them count.
  const older = createPrepended(ids);
  const follow = createFollow(() => scroller, () => ids().length - older());
  // What the owner just sent, and the work it starts, come into view above the composer even from a reader who had scrolled
  // up: left where it was, the reply's Working line stayed out of view below.
  // A message counts from the press, while it is still on its way, and once its record takes its place it is not counted twice.
  const sentHere = () => { const s = planning(); return s ? { kind: "sprint" as const, sprintId: s.id } : { kind: "task" as const, taskId: m().view.id }; };
  const mine = createMemo(() => ids().slice(Math.max(0, older())).filter(id => { const k = entry(id).kind; return k === "said" || k === "message"; }).length + ws.store.sends.shown(sentHere()).length);
  let mineSeen = mine();
  createEffect(() => { const n = mine(); if (n > mineSeen) follow.jump(); mineSeen = n; });
  const decision = (id: string) => m().session.decisions.find(d => d.id === id)!;
  const message = (id: string) => m().messages.find(x => x.id === id)!;
  // The reader scrolled up to within a screen of the top of a conversation read only in part: its earlier part is read. A
  // failed read waits for Retry; one under way is not asked for again.
  const nearTop = () => {
    if (m().session.historyEarlier?.state !== "available" || !scroller || follow.following() || scroller.scrollTop >= scroller.clientHeight) return;
    ws.store.loadEarlierHistory(m().view.id);
  };
  const stranded = () => m().view.column === "needs" && !m().view.unknown && m().open.length === 0;
  // Questions asked together are one form until their answers are recorded.
  const asks = createMemo(() => m().view.task.closedAs === "unconfirmed" ? [] : m().session.decisions.filter(d => d.kind === "question" && (isOpen(d) || d.held)));
  const inAsks = (id: string) => asks().some(d => d.id === id);
  // While a decision waits or work runs, no completed result and no Accept stand beside it: the last one stays as history.
  const held = () => m().view.task.result ? resultHeld(m(), asks().length > 0) : null;
  const changesAt = (): Route => ({ name: "task", machine: m().view.machine.id, task: m().view.id, tab: "changes" });
  const team = () => <TeamBar m={m()} now={props.now} pick={props.pick} onPick={props.onPick} />;
  return <section class="task-panel" aria-label="Conversation" hidden={props.hidden}>
    <div class="timeline" ref={scroller} onScroll={() => { follow.onScroll(); nearTop(); }}>
      <div class="tl-in">
        <Show when={props.brief} fallback={<Brief m={m()} />}>{props.brief}</Show>
        <HistoryState m={m()} />
        <Earlier m={m()} />
        <For each={ids()}>{id => <Switch>
          <Match when={entry(id).kind === "cortex"}>{(() => {
            const text = () => (entry(id) as Extract<TimelineEntry, { kind: "cortex" }>).text;
            return <><div class="ai"><ResultText breaks text={text()} /></div><MessageTools text={text()} /></>;
          })()}</Match>
          <Match when={entry(id).kind === "said"}>{(() => {
            const text = () => (entry(id) as Extract<TimelineEntry, { kind: "said" }>).text;
            // The runtime's record of this message (same text), when the facts carry it: its files and delivery state.
            const sent = () => m().messages.find(x => x.workerId === null && x.delivery !== "queued" && x.text.trim() === text().trim());
            return <><div class="mine"><ResultText breaks text={text()} /><AttachChips files={sent()?.attachments} open /></div>
              <Show when={sent()}>{s => <div class="mine-meta"><I n="check" s={12} />{messageDelivery(s(), m().mode === "paused")}</div>}</Show><MessageTools text={text()} end /></>;
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
          <Match when={entry(id).kind === "message"}>{(() => {
            const x = () => message((entry(id) as Extract<TimelineEntry, { kind: "message" }>).messageId);
            return <>
              <div class="mine"><ResultText breaks text={x().text} /><AttachChips files={x().attachments} open /></div>
              <div class="mine-meta"><I n="check" s={12} />{messageDelivery(x(), m().mode === "paused")}<Show when={x().at !== null}><span>{clock(x().at)}</span></Show><CopySource text={x().text} /></div>
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
        <SentMessages place={sentHere()} />
        {/* Keyed by decision id: a refreshed record of an open decision keeps its card, and the answer being written in it. */}
        <Keyed each={m().open.filter(d => !m().session.timeline.some(e => e.kind === "decision" && e.decisionId === d.id))} by={d => d.id}>{d => <Show when={d().kind === "question"} fallback={<DecisionCard m={m()} d={d()} now={props.now} />}><Questions m={m()} asks={[d()]} now={props.now} /></Show>}</Keyed>
        {/* Only a task that waits on the owner reads as waiting for a decision: a cancelled or failed run has none. */}
        <Show when={stranded() && m().view.task.observedPhase !== "recovery" && (m().view.task.status === "needs_you" || m().view.task.conflict !== "none")}><Waiting m={m()} /></Show>
        {props.tail}
        <Show when={m().view.task.result && !held() && !planning()}><ResultCard m={m()} /></Show>
        <Show when={m().view.task.result && held() && !planning()}>
          <div class="answered ws-prev-result" role="note"><I n="eye" s={14} /><span>Previous result v{m().view.task.result!.version}, read-only. {held()}.</span> <A class="lnk ws-ml4" to={changesAt()}>Open</A></div>
        </Show>
        {/* The last line says what the task is doing now, as its header does: waiting for an answer is not working. */}
        <Show when={m().mode === "live" && m().view.task.observedPhase !== "recovery"}>
          <Show when={workingNow(m())} fallback={<Show when={!stranded()}><WaitingIndicator /></Show>}><WorkingIndicator text={coordinatorLine(m())} /></Show>
        </Show>
      </div>
    </div>
    {/* Everything pinned below the feed is in the dock. Latest hangs over the feed, not over a decision card. */}
    <div class="dock">
      <Pill follow={follow} />
      <Show when={props.route && !planning()}><OwnerHoldCard m={m()} route={props.route!} /><UnconfirmedCard m={m()} route={props.route!} /></Show>
      <Show when={ws.can(capabilityOf("send_message"))} fallback={team()}><Composer model={props.model} team={team()} folded={m().open.some(d => d.kind === "question")} /></Show>
    </div>
  </section>;
}

function WorkerCard(props: { w: Worker; now: number; to: Route; selected: boolean }) {
  const ws = useWs();
  const f = () => workerState(props.w);
  const line = () => props.w.waitsFor ? `Waits for ${props.w.waitsFor}` : reviewLine(props.w) ?? (isFinished(props.w) ? resultPreview(props.w.result?.summary ?? props.w.goal) : props.w.now ?? props.w.goal);
  const elapsed = () => duration(elapsedOf(props.w, props.now));
  return <A class="wcard" role="tab" to={props.to} aria-selected={props.selected ? "true" : "false"}>
    <span class="l1"><span class="ic"><I n={f().icon} s={14} class={`${f().spin ? "spin " : ""}${f().tone === "st-idle" ? "faint" : f().tone}`} /></span>{props.w.role}<span class={`st ${f().tone}`}>{f().label}</span></span>
    <span class="l2 trunc" style={{ display: "block" }}>{line()}</span>
    <span class="l3"><Show when={ws.route(props.w.route)}>{route => <span>{route()}</span>}</Show><Show when={elapsed()}><span>{elapsed()}</span></Show>
      <Show when={contextOf(props.w.context)}>{c => <span>Context {c().percent}%</span>}</Show><Show when={props.w.usage !== null}><span>Usage {tokens(props.w.usage!)}</span></Show></span>
  </A>;
}

/** The coordinator, the workers by lane, runtime verification and the finished workers, each opening its activity beside it. */
export function WorkersList(props: { m: SessionModel; now: number; pick: string }) {
  const routes = useTaskRoutes(() => ({ machine: props.m.view.machine.id, id: props.m.view.id }));
  const at = (worker: string): Route => routes.workers(worker);
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
    <WriteDenials m={props.m} />
  </div>;
}

/** The owner's `deny-write` paths in force for this task (runtime facts): path, when set, and Remove, which lifts that one denial only. */
function WriteDenials(props: { m: SessionModel }) {
  const ws = useWs();
  const denials = () => props.m.view.task.writeDenials ?? [];
  return <Show when={denials().length > 0}>
    <div class="wg">Write denials<Count n={denials().length} /></div>
    <For each={denials()}>{d => <div class="wcard lane" aria-label={`Write denied: ${d.path}`}>
      <span class="l1"><span class="ic"><I n="lock" s={14} class="faint" /></span><span class="mono trunc">{d.path}</span>
        <button class="btn btn-line btn-sm" {...ws.needs("allow_write")} aria-label={`Remove the write denial for ${d.path}`}
          onClick={() => ws.run({ type: "allow_write", taskId: props.m.view.id, path: d.path })}>Remove</button></span>
      <span class="l2 trunc" style={{ display: "block" }}>Set by your message{d.setAt !== null ? ` ${clock(d.setAt)}` : ""}. Writes there are refused.</span>
    </div>}</For>
  </Show>;
}

/** The same Changes tab carries the diff review, Ready to merge and the conflict, so a task's state decides what it opens on. */
function ChangesTab(props: { m: SessionModel; route: TaskRoute }) {
  const t = () => props.m.view.task;
  const merging = () => !props.m.view.automaticFact && (inConflict(t()) || t().status === "ready_to_merge" || t().integration?.method === "squash" || t().integration?.method === "commit");
  return <Switch fallback={<ChangesPanel m={props.m} route={props.route} />}>
    <Match when={!props.m.view.automaticFact && props.route.sub === "conflict"}><ConflictPanel m={props.m} route={props.route} /></Match>
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
  // Days before cleanup come from the machine's retention, read only for an archived task's page.
  useWantOverview(() => m().mode === "archived");
  const archived: StripItem = { id: "archived", tone: "info", icon: <I n="archive" s={14} class="faint" />, get text() { return ws.store.overviewLoaded() ? `Archived, read-only. ${daysLeft(archiveRow(ws.store.index(), m().view).left)} before cleanup.` : "Archived, read-only."; },
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
  // A recorded recovery reason is shown in words; Stop and Close task stay available.
  const interrupted: StripItem = { id: "interrupted", tone: "info", icon: <I n="pause" s={14} />, get text() {
    const reason = m().view.task.issue;
    return !reason || reason === "turn_interrupted" ? "This turn was interrupted. Send a message to continue from the recorded state. Nothing has been restarted."
      : `${sentence(stopReason(reason))} Send a message to continue from the recorded state, or use Stop or Close task. Nothing has been restarted.`;
  } };
  const unverified: StripItem = { id: "unverified", tone: "warn", icon: <I n="alert" s={14} class="st-warn" />, text: "Not verified. This task was closed as is, and the warning stays with its result." };
  const titleWhy: StripItem = { id: "title-why", tone: "info", icon: <I n="alert" s={14} class="st-warn" />, domId: taskWhyId(props.id), get text() { return titleBlock(m()) ?? ""; } };
  const status = createMemo<StripItem[]>(() => [
    ...(m().mode === "stopping" ? [stoppingItem] : []),
    ...(m().mode === "archived" ? [archived] : []),
    ...(["offline", "unconfirmed"].includes(m().mode) ? [offline] : []),
    ...(m().mode === "live" && !m().view.ownerDecision && m().view.task.observedPhase === "recovery" ? [interrupted] : []),
    ...(m().view.task.notVerifiedByCortex && m().view.task.closedAs === "as_is" ? [unverified] : []),
    ...(m().view.task.closedAs === "unconfirmed" ? [{
      id: "closed-unconfirmed", tone: "warn" as const, icon: <I n="alert" s={14} />,
      text: <>You closed this task without verification. Its records still disagree. <A class="lnk" to={{ name: "settings", machine: m().view.machine.id, section: "diagnostics" }}>Open Diagnostics</A>.</>,
    }] : []),
    ...(m().mode !== "stopping" && titleBlock(m()) ? [titleWhy] : []),
  ]);
  return <Page class="l-work l-task" scroll="none" head={<TaskBar m={m()} tab={tab()} route={route()} />} status={<StatusLane items={status()} />}>
    <div class="main-body">
      <Show when={live()} fallback={
        <div class="task-body">
          <Switch>
            <Match when={tab() === undefined}><Conversation model={model} now={now()} pick={pick()} onPick={onPick} route={route()} /></Match>
            <Match when={tab() === "changes"}><ChangesTab m={m()} route={route()} /></Match>
            <Match when={tab() === "code"}><CodeTab m={m()} route={route()} /></Match>
            <Match when={tab() === "terminal"}><div class="note-panel task-panel"><span class="w500">Terminal</span><span>{ws.t("terminalNote")}</span>
              <button class="btn btn-line btn-sm" onClick={() => { if (!ws.panels.terminalOpen()) ws.toggleTerminal(); }}>Open the bottom panel</button></div></Match>
          </Switch>
        </div>}>
        <TaskWorkers model={model} now={now()} pick={pick()} onPick={onPick} />
      </Show>
    </div>
  </Page>;
}

/** A task's Work details: the list of who works on it, and the picked one's activity beside it. */
export function TaskWorkers(props: { model: Accessor<SessionModel>; now: number; pick: string; onPick: (key: string) => void }) {
  const m = () => props.model();
  return <div class="task-body live">
    <section class="task-panel" aria-label="Workers"><WorkersList m={m()} now={props.now} pick={props.pick} /></section>
    <section class="livepane" aria-label={`${props.pick === CORTEX ? "Coordinator" : props.pick === VERIFICATION ? "Runtime verification" : m().workers.find(w => w.id === props.pick)?.role ?? "Worker"} activity`}><label class="ws-worker-picker">Worker <select class="inp" value={props.pick} onChange={e => props.onPick(e.currentTarget.value)}><option value={CORTEX}>Coordinator</option><Keyed each={m().workers} by={w => w.id}>{w => <option value={w().id}>{w().role}</option>}</Keyed><option value={VERIFICATION}>Runtime verification</option></select></label><WorkerPane model={props.model} pick={props.pick} now={props.now} /></section>
  </div>;
}
/** The worker a Work details view opens on when none is named. */
export const firstWorker = (m: SessionModel) => firstPick(m);
/** A task session's one-second clock, for elapsed times. */
export const useSessionClock = useTicker;
