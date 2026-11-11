import { batch, createEffect, createMemo, createSignal, createUniqueId, For, Match, on, onCleanup, Show, Switch, useContext, type Accessor } from "solid-js";
import { dependencyProblem, runBlocker, type AttachmentInput, type PlanItem, type Sprint, type TaskSettings, type WorkspaceState } from "@cortex/workspace-domain";
import { reportHeight } from "./autogrow";
import { AttachChips, createAttachments, createDictation, MessageComposer, RouteBar } from "./composer";
import { useWs } from "./ctx";
import { createFollow } from "./follow";
import { createUnsent, ScreenActive } from "./freshness";
import { A, I, Keyed, MenuItem, placeFocus, Pop } from "./kit";
import { ActionBar, Page, revealCutIn, revealWhenCut, SectionHead, StatusLane, Toolbar, type StripItem, type StripTone } from "./layout";
import { Hint } from "./settings-kit";
import { Answered, QuestionTabs, type Ask } from "./questions";
import type { Route } from "./router";
import { clock, sprintTitle, sprintView, type ProjectSummary } from "./selectors";
import { ResultText } from "./result-text";
import { createLabel, itemTitle, planCaption, planGraph, planningAsks, planningThread, planRows, queueStall, sprintGap, sprintSummary, type PlanRow } from "./sprint-model";
import { graphCounts, planLayers } from "./plan-graph";
import { GraphView, type GraphApi, type GraphMemory } from "./sprint-graph";
import { arrange, INLINE_FLOOR, inlineScale, type Opened } from "./sprint-space";

type SprintRoute = Extract<Route, { name: "sprint" }>;
const useSprint = (id: () => string) => { const ws = useWs(); return createMemo(() => ws.store.state().sprints.find(s => s.id === id()) ?? null); };
const firstReady = (index: ReturnType<ReturnType<typeof useWs>["store"]["index"]>, sprint: Sprint) => sprintView(index, sprint).next;

const targetOf = (sprint: Sprint, summary: ProjectSummary) => sprint.targetBranch ?? summary.project.defaultBranch;
/** The branches a sprint can merge to, as menu rows. */
export function TargetChoices(props: { sprint: Sprint; summary: ProjectSummary }) {
  const ws = useWs();
  const branches = () => [...new Set([props.summary.project.defaultBranch, props.summary.project.openBranch])];
  return <>
    <div class="mi-h">Target branch</div>
    <For each={branches()}>{b => <MenuItem label={<span class="mono">{b}</span>} sub={b === props.summary.project.defaultBranch ? "Git default branch" : undefined} check={b === targetOf(props.sprint, props.summary)}
      needs="set_target" onSelect={() => ws.run({ type: "set_target", sprintId: props.sprint.id, branch: b === props.summary.project.defaultBranch ? null : b })} />}</For>
  </>;
}

/** Where the sprint's tasks merge. Each task can still pick its own target in Ready to merge. */
export function SprintTarget(props: { sprint: Sprint; summary: ProjectSummary; class?: string }) {
  return <>
    <button class={`chip${props.class ? ` ${props.class}` : ""}`} popovertarget="m-target" aria-label={`Target branch ${targetOf(props.sprint, props.summary)}`}><span class="faint l-compact-hide">Target branch</span><span class="mono l-branch">{targetOf(props.sprint, props.summary)}</span><I n="chevron-down" s={12} /></button>
    <Pop id="m-target" class="pop-right" align="right" label="Target branch" style={{ width: "280px" }}>
      <TargetChoices sprint={props.sprint} summary={props.summary} />
    </Pop>
  </>;
}

function SprintCheckout(props: { sprint: Sprint }) {
  return <Show when={props.sprint.checkout}><span class="cap muted" aria-label="Sprint checkout">Tasks: {props.sprint.checkout!.kind === "worktree" ? "Separate worktree per task" : "Project folder"}</span></Show>;
}

export function RunMode(props: { sprint: Sprint; describedBy?: string }) {
  const ws = useWs();
  const set = (runMode: "manual" | "automatic") => { if (props.sprint.runMode !== runMode) ws.run({ type: "set_run_mode", sprintId: props.sprint.id, runMode }); };
  const described = () => ws.gate("set_run_mode") ? props.describedBy : undefined;
  return <div class="seg" role="group" aria-label="Sprint run mode">
    <button aria-current={props.sprint.runMode === "manual" ? "true" : "false"} aria-describedby={described()} {...ws.needs("set_run_mode")} onClick={() => set("manual")}>Manual</button>
    <button aria-current={props.sprint.runMode === "automatic" ? "true" : "false"} aria-describedby={described()} {...ws.needs("set_run_mode")} onClick={() => set("automatic")}>Automatic</button>
  </div>;
}

// The plan can name tasks the project's runtime has not loaded yet; the queue waits for them instead of the screen failing.
const queueBlocker = (state: WorkspaceState, sprint: Sprint) => sprint.taskIds.every(id => state.tasks.some(t => t.id === id)) ? runBlocker(state, sprint) : "This sprint's tasks are not loaded yet";

/**
 * The sprint's one run control: Start in Manual, Run, Pause or Continue in Automatic. `steady` keeps one width as its label swaps.
 * A refusal is said in the strip `describedBy` names, or by a press; the control itself never holds the words.
 */
export function QueueButton(props: { sprint: Sprint; line?: boolean; small?: boolean; steady?: boolean; describedBy?: string }) {
  const ws = useWs();
  const size = () => `${props.small ? " btn-sm" : ""}${props.steady ? " l-act-slot" : ""}`;
  const blocker = () => queueBlocker(ws.store.state(), props.sprint);
  const described = (type: "run_sprint" | "pause_sprint" | "start_task", reason?: string | null) => reason || ws.gate(type) ? props.describedBy : undefined;
  const next = () => firstReady(ws.store.index(), props.sprint);
  const number = () => { const v = next(); return v ? props.sprint.taskIds.indexOf(v.id) + 1 : 0; };
  const run = () => { if (!blocker()) ws.run({ type: "run_sprint", sprintId: props.sprint.id, now: ws.store.clock() }); };
  const complete = () => props.sprint.queueState === "complete";
  const cls = () => `btn ${props.line ? "btn-line" : "btn-primary"}${size()}`;
  return <Show when={!props.sprint.archivedAt && props.sprint.planning === "created"}>
    <Show when={props.sprint.runMode === "automatic"} fallback={<Show when={next()}>{v => <button class={cls()} aria-describedby={described("start_task")} {...ws.needs("start_task")} onClick={() => ws.run({ type: "start_task", taskId: v().id, now: ws.store.clock() })}>Start task {number()}</button>}</Show>}>
      <Show when={props.sprint.queueState === "running"} fallback={<Show when={!complete()}>
        <button class={cls()} aria-disabled={blocker() ? "true" : undefined} data-why={blocker()} aria-describedby={described("run_sprint", blocker())} {...ws.needs("run_sprint")} onClick={run}>{props.sprint.queueState === "paused" ? "Continue queue" : "Run sprint"}</button>
      </Show>}>
        <button class={`btn btn-line${size()}`} aria-describedby={described("pause_sprint")} {...ws.needs("pause_sprint")} onClick={() => ws.run({ type: "pause_sprint", sprintId: props.sprint.id })}>Pause queue</button>
      </Show>
    </Show>
  </Show>;
}

type QueueWords = { tone: StripTone; icon: string; iconClass: string; text: string; taskId: string | null; role?: "status" };
const stop = (text: string) => text.replace(/\.+$/, "");
/** What the queue is doing or waiting for, when that needs saying. One answer, so the same words appear on every screen and only once on each. */
function queueWords(ws: ReturnType<typeof useWs>, sprint: Sprint): QueueWords | null {
  const index = ws.store.index(), stall = queueStall(index, sprint);
  const waiting = sprint.runMode === "automatic" && sprint.planning === "created" && sprint.queueState !== "running" && sprint.queueState !== "complete";
  const blocker = waiting ? queueBlocker(index.state, sprint) : null;
  if (sprint.queueState === "paused" && sprint.pauseReason) return { tone: "warn", icon: "pause", iconClass: "st-warn", taskId: stall?.taskId ?? null,
    text: `Queue paused: ${stop(stall?.reason ?? blocker ?? sprint.pauseReason)}.${stall || blocker ? "" : " Continue queue when you are ready."}` };
  if (stall || blocker) return { tone: "warn", icon: "alert", iconClass: "st-warn", taskId: stall?.taskId ?? null, role: "status", text: `${stop(stall?.reason ?? blocker!)}.` };
  if (sprint.queueState === "complete") return { tone: "ok", icon: "check-circle", iconClass: "st-ok", taskId: null, text: "All queued tasks have run. Finished results wait for your confirmation; nothing closes by itself." };
  // A control the machine does not offer is greyed out: its reason still has to be somewhere the owner can read.
  const refused = ws.gate("set_run_mode") ?? ws.gate(sprint.runMode !== "automatic" ? "start_task" : sprint.queueState === "running" ? "pause_sprint" : "run_sprint");
  return refused ? { tone: "info", icon: "help", iconClass: "faint", taskId: null, role: "status", text: refused } : null;
}
const sameWords = (a: QueueWords | null, b: QueueWords | null) => JSON.stringify(a) === JSON.stringify(b);

/** The queue's one strip, or nothing when the queue is simply moving. The item keeps its identity, so a change of words redraws only the words. */
export function createQueueStrip(sprint: Accessor<Sprint | null | undefined>, domId?: string): Accessor<StripItem | null> {
  const ws = useWs();
  const words = createMemo(() => { const s = sprint(); return s ? queueWords(ws, s) : null; }, null, { equals: sameWords });
  const task = () => { const id = words()?.taskId; return id ? ws.store.index().byId.get(id) : undefined; };
  const icon = <I n={words()?.icon ?? "help"} s={14} class={words()?.iconClass} />;
  const open = <Show when={task()}>{v => <A class="lnk" to={{ name: "task", machine: v().machine.id, task: v().id }}>Open task</A>}</Show>;
  const item: StripItem = { id: "queue", domId, icon, action: open, get tone() { return words()?.tone ?? "info"; }, get text() { return words()?.text ?? ""; }, get role() { return words()?.role; } };
  return () => words() ? item : null;
}

/** What automatic running means, kept out of the way of the work and one press from it. */
const queueHelp = (sprint: Sprint) => `Passed checks leave each task ready for your final confirmation and start the next ready task. Nothing finishes without your confirmation.${sprint.queueState === "running" ? " Pause queue stops new starts; the task already running keeps going." : ""}`;
export function QueueHelp(props: { sprint: Sprint }) {
  return <Show when={props.sprint.runMode === "automatic"}><Hint text={queueHelp(props.sprint)} label="the automatic queue" /></Show>;
}

/** Some of the sprint's tasks could not be read: say how many, keep the membership, and read again on request. One strip for the life of the page. */
function createGapStrip(sprint: Accessor<Sprint>): Accessor<StripItem | null> {
  const ws = useWs();
  const gap = createMemo(() => sprintGap(ws.store.index(), sprint()));
  const item: StripItem = { id: "gap", tone: "warn", role: "status", icon: <I n="alert" s={14} class="st-warn" />, get text() { const g = gap(); return g ? `${g.title}. ${g.text}` : ""; },
    action: <button class="btn btn-sm" onClick={() => void ws.store.reload()}>Retry</button> };
  return () => gap()?.retry ? item : null;
}

export function SprintCrumb(props: { sprint: Sprint }) {
  const ws = useWs();
  const [text, setText] = createSignal<string | null>(null);
  const save = () => { const t = text(); if (t?.trim()) ws.ok({ type: "rename_sprint", sprintId: props.sprint.id, title: t }, () => { setText(null); }); };
  return <Show when={text() !== null} fallback={<button class="cur trunc ws-title" onClick={() => setText(sprintTitle(props.sprint))}>Sprint: {sprintTitle(props.sprint)}</button>}>
    <input class="inp ws-title-in" aria-label="Sprint title" value={text()!} onInput={e => setText(e.currentTarget.value)} ref={el => queueMicrotask(() => { el.focus(); el.select(); })} onBlur={() => setText(null)}
      onKeyDown={e => { if (e.key === "Enter") { e.preventDefault(); save(); } else if (e.key === "Escape") { e.stopPropagation(); setText(null); } }} />
  </Show>;
}

export function SprintActions(props: { sprint: Sprint; tab: SprintRoute["tab"] }) {
  const ws = useWs();
  const planning = () => props.sprint.planning !== "created";
  const close = () => ws.open({kind:"confirm",title:"Close this sprint?",action:"Close sprint",
    note:"The sprint moves to Archive and its automatic queue stops. Unfinished tasks stay in the project, outside the active sprint lists; tasks already running continue. No task is cancelled, accepted, merged, archived or deleted. Close or archive individual tasks separately.",
    go:async () => { const r = await ws.run({type:"close_sprint",sprintId:props.sprint.id,confirmed:true}); if (r.ok) ws.go({name:"archive",machine:ws.machine().id}); return r.ok; }});
  return <Show when={!props.sprint.archivedAt} fallback={<span class="st st-idle">Closed sprint</span>}><>
    <Show when={props.tab === "summary"}><QueueButton sprint={props.sprint} line /></Show>
    <Show when={props.sprint.planning === "created"}><button class="btn btn-line tb-fold" onClick={() => ws.newItem("task", props.sprint.projectId, props.sprint.id)}><I n="plus" s={14} />New task</button></Show>
    <button class="btn btn-line tb-fold" {...ws.needs("close_sprint")} onClick={close}>Close sprint…</button>
    <button class="ib" popovertarget="m-sprint-menu" aria-label="Sprint actions"><I n="more" /></button>
    <Pop id="m-sprint-menu" class="pop-right" align="right" label="Sprint actions">
      <Show when={props.sprint.planning === "created"}><div class="tb-fold-only"><MenuItem icon="plus" label="New task" onSelect={() => ws.newItem("task", props.sprint.projectId, props.sprint.id)} /></div></Show>
      <Show when={planning() && props.sprint.planning !== "cancelled"}><MenuItem icon="x" label="Cancel planning" needs="cancel_planning" onSelect={() => ws.run({ type: "cancel_planning", sprintId: props.sprint.id })} /></Show>
      <Show when={props.sprint.planning === "cancelled"}><MenuItem icon="play" label="Resume planning" needs="resume_planning" onSelect={() => ws.run({ type: "resume_planning", sprintId: props.sprint.id })} /></Show>
      <MenuItem icon="archive" label="Close sprint…" needs="close_sprint" onSelect={close} />
    </Pop>
  </></Show>;
}

/** Stays mounted while hidden, so a half-written message and its attachments survive. `onUnsent` says whether any is waiting. */
function Conversation(props: { sprint: Sprint; hidden?: boolean; onUnsent?: (unsent: boolean) => void; onOpenPlan?: () => void }) {
  const ws = useWs();
  const s = () => props.sprint;
  const [text, setText] = createSignal("");
  const mic = createDictation();
  const files = createAttachments();
  const session = () => ws.store.state().sessions.find(x => x.taskId === s().planningTaskId);
  const asks = createMemo(() => planningAsks(s(), session()));
  const thread = createMemo(() => planningThread(s(), session()));
  const answer = (ask: Ask, text: string, optionId: string | null, attachments?: AttachmentInput[]) => ws.run({ type: "answer_planning", sprintId: s().id, answer: text, attachments, now: ws.store.clock(), ...(ask.id === "planning" ? {} : { decisionId: ask.id, optionId }) });
  const closed = () => s().planning === "created" ? "Tasks are created. Edit them on the Board." : s().planning === "cancelled" ? "Planning is cancelled. Resume it to continue." : null;
  const send = (value: string) => { if (value.trim() && !files.busy()) ws.ok({ type: "plan_message", sprintId: s().id, text: value, attachments: files.files(), now: ws.store.clock() }, () => { setText(""); files.clear(); }); };
  const settings = (): TaskSettings => ({ model: "opus[1m]", effort: "high", mode: "development", language: "auto", ...s().settings });
  createUnsent(() => !!text().trim() || files.files().length > 0);
  createEffect(() => props.onUnsent?.(!!text().trim() || files.files().length > 0));
  const [height, setHeight] = createSignal(0);
  let dock!: HTMLDivElement, timeline: HTMLDivElement | undefined;
  reportHeight(() => dock, setHeight);
  // A conversation reads from its latest line, as the task's does: it opens at its end and stays there until the owner scrolls up.
  const follow = createFollow(() => timeline, () => thread().length + asks().length + (s().plan?.version ?? 0));
  return <section class="sp-conv" aria-label="Planning conversation" hidden={props.hidden}>
    <div class="timeline" ref={timeline} onScroll={follow.onScroll} style={{ "--dock-clear": `${height() + 16}px` }}><div class="tl-in">
      <div class="brief"><ResultText breaks text={s().description} /><AttachChips files={s().attachments} open /><div class="meta"><Show when={ws.t("planningBadge")}><span>{ws.t("planningBadge")}</span></Show></div></div>
      <Keyed each={thread()} by={e => e.key}>{e => <Switch>
        <Match when={e().kind === "note"}><div class="ai"><ResultText breaks text={e().text} /><AttachChips files={e().attachments} open /></div></Match>
        <Match when={e().kind === "qa"}><Answered question={e().question ?? ""} answer={e().text} at={e().at} files={e().attachments} /></Match>
        <Match when={e().kind === "message"}><div class="mine"><ResultText breaks text={e().text} /><AttachChips files={e().attachments} open /></div><div class="mine-meta"><I n="check" s={12} />Sent to the planner<Show when={e().at !== null}><span>{clock(e().at)}</span></Show></div></Match>
        <Match when={e().kind === "event"}><div class="answered"><I n="check" s={14} /><span>{e().text}</span><Show when={e().at !== null}><span class="faint">{clock(e().at)}</span></Show></div></Match>
      </Switch>}</Keyed>
      {/* The proposed plan is a line in the conversation, and the line opens the Plan tab. */}
      <Show when={s().plan}>{plan => <div class="answered sp-plan-line"><I n="layers" s={14} />
        <span class="grow">{s().planning === "created" && s().materializedVersion !== null ? `Tasks created from Plan v${s().materializedVersion}` : `Plan v${plan().version} proposed: ${plan().items.length} task${plan().items.length === 1 ? "" : "s"}`}</span>
        <Show when={props.onOpenPlan}>{open => <button class="btn btn-line" onClick={() => open()()}>Open plan</button>}</Show>
      </div>}</Show>
      <Show when={asks().length > 0}><QuestionTabs asks={asks()} needs="answer_planning" blocked={() => undefined} note={() => null} onAnswer={answer}
        route={ask => <RouteBar id={`plan-answer-${ask.id}`} up compact needs="set_sprint_settings" settings={settings()} onChange={patch => ws.run({ type: "set_sprint_settings", sprintId: s().id, settings: patch })} />} waiting={ask => ask.at === null ? "" : clock(ask.at)} /></Show>
      <Show when={s().planning === "planning"}><div class="answered"><I n="loader" s={14} class="spin" />Working on the plan</div></Show>
    </div></div>
    <div class="dock" ref={dock}>
      <MessageComposer label="Message the planner" inputId="ws-plan-input" placeholder="Ask for changes to the plan…" text={text()} onText={setText} files={files} mic={mic} off={closed() ?? undefined}
        route={<RouteBar id="plan" up compact needs="set_sprint_settings" settings={settings()} onChange={patch => ws.run({ type: "set_sprint_settings", sprintId: s().id, settings: patch })} />}
        send={{ label: "Send to the planner", needs: "plan_message", why: files.busy() ? "Attachments are still loading" : text().trim() ? undefined : "Write a message first", go: () => send(text()), insertAndSend: send }} />
    </div>
  </section>;
}

function DependencyPicker(props: { item: PlanItem; items: PlanItem[]; n: number; onToggle: (dep: string) => void }) {
  const id = () => `m-dep-${props.n}`;
  const no = (key: string) => props.items.findIndex(i => i.key === key) + 1;
  const label = () => props.item.dependsOn.map(no).join(", ") || "None";
  return <>
    <button class="pick ws-dep" popovertarget={id()} aria-label={`Task ${props.n} depends on ${label()}`}><span class="trunc">{label()}</span><I n="chevron-down" s={12} /></button>
    <Pop id={id()} class="pop-right" align="right" label={`Task ${props.n} depends on`} style={{ width: "240px" }}>
      <div class="mi-h">Task {props.n} waits for</div>
      <For each={props.items.filter(i => i.key !== props.item.key)}>{i => <MenuItem label={`${no(i.key)} ${itemTitle(i)}`} check={props.item.dependsOn.includes(i.key)} onSelect={() => props.onToggle(i.key)} />}</For>
    </Pop>
  </>;
}

/**
 * The small graph under the list, at its own size and shrunk only when the pane is narrower. Once its labels would fall
 * under 11 px it gives its place to one row that opens the Graph view. `selected` is an explicit pick, never a default.
 */
function Graph(props: { items: PlanItem[]; selected: string | null; onPick: (key: string) => void; width: number; onOpen: () => void }) {
  const g = createMemo(() => planGraph(props.items, props.selected));
  const fits = () => inlineScale(g(), props.width - 24) >= INLINE_FLOOR;
  const layers = () => { let n = 0; for (const d of planLayers(props.items).values()) n = Math.max(n, d + 1); return n; };
  return <Show when={fits()} fallback={<div class="gv-open-row"><span class="trunc">Dependency graph · {graphCounts(props.items.length, layers())}</span><button class="btn btn-sm btn-line" onClick={() => props.onOpen()}>Open graph</button></div>}><div class="depg" aria-label="Dependency graph">
    <svg viewBox={`0 0 ${g().width} ${g().height}`} style={{ width: `${g().width}px`, "max-width": "100%", height: "auto" }} role="img" aria-label={g().label}>
      <Keyed each={g().edges} by={e => e.key}>{e => <path class={`e${e().on ? " on" : ""}`} d={e().d} />}</Keyed>
      <Keyed each={g().nodes} by={n => n.key}>{n => <g class="ws-node" onClick={() => props.onPick(n().key)}><rect class={`nd${n().on ? " on" : ""}`} x={n().x} y={n().y} width={n().w} height={n().h} rx="7" /><text x={n().x + 12} y={n().y + 21}>{n().label}</text></g>}</Keyed>
    </svg>
    <div class="cap muted ws-mt4">{planCaption(props.items, props.selected)}</div>
  </div></Show>;
}

const planState = (r: PlanRow) => {
  if (!r.word) return null;
  if ((r.word.text === "Blocked" || r.word.text === "Queued") && r.waits.length) return { tone: "st-idle", icon: "hourglass", text: `Waits for ${r.waits.join(", ")}` };
  const ready = r.word.text === "Next" || r.word.text === "Queued";
  return { tone: r.word.tone, icon: r.word.text === "Next" ? "play" : r.word.icon, spin: r.word.spin, text: ready ? "Ready" : r.word.text };
};

/**
 * A plan may hold very long tasks, so the list names them and this holds the full text. It scrolls by itself and is always
 * docked beside the plan; on a window with no room for both it takes the plan's place and `back` names the way back.
 */
function PlanItemPanel(props: { row: PlanRow; total: number; onClose: () => void; back?: string }) {
  let close!: HTMLButtonElement;
  const back = document.activeElement as HTMLElement | null;
  const shut = () => { props.onClose(); if (back?.isConnected) back.focus({ preventScroll: true }); };
  const shown = useContext(ScreenActive);
  const onKey = (e: KeyboardEvent) => { if (e.key === "Escape" && shown() && !document.querySelector("dialog[open]")) { e.preventDefault(); shut(); } };
  document.addEventListener("keydown", onKey);
  onCleanup(() => document.removeEventListener("keydown", onKey));
  queueMicrotask(() => placeFocus(close, { preventScroll: true }));
  return <aside class="ws-read" aria-label="Task details">
    <SectionHead class="ws-read-h" lead={props.back ? <button class="btn btn-sm" onClick={shut}><I n="arrow-left" s={14} />{props.back}</button> : undefined} title={`Task ${props.row.no} of ${props.total}`}><button ref={close} class="ib ws-ib" aria-label="Close task details" onClick={shut}><I n="x" s={14} /></button></SectionHead>
    <div class="ws-read-b selectable">
      <h2 class="ws-read-t">{itemTitle(props.row.item)}</h2>
      <dl class="kv">
        <dt>Depends on</dt><dd>{props.row.deps.length ? `Tasks ${props.row.deps.join(", ")}` : "Starts first"}</dd>
        <Show when={planState(props.row)}>{w => <><dt>State</dt><dd class={w().tone}>{w().text}</dd></>}</Show>
      </dl>
      <div class="ws-read-d"><ResultText breaks text={props.row.item.description} /></div>
      <Show when={props.row.item.acceptance}><div class="ins-h ws-read-s"><span>Acceptance and checks</span></div><div class="ws-read-d"><ResultText breaks text={props.row.item.acceptance} /></div></Show>
    </div>
    <Show when={props.row.view}>{v => <ActionBar class="ws-read-f"><A class="btn btn-line" to={{ name: "task", machine: v().machine.id, task: v().id }}>Open task<I n="arrow-right" s={14} /></A></ActionBar>}</Show>
  </aside>;
}

/** The chat, seen from the Plan tab: what waits there, and the way to it. */
type ConvControl = { chip: { text: string; warn: boolean } | null; show: () => void };
type PlanProps = {
  sprint: Sprint; draft: PlanItem[] | null; setDraft: (items: PlanItem[] | null) => void; reading: string | null; onRead: (key: string | null) => void; rows?: (rows: PlanRow[]) => void;
  /** Details has taken the plan's place. */
  hidden?: boolean; full: boolean; setFull: (on: boolean) => void; conv: ConvControl;
  request: { why: string | null; run: () => void };
  memory: Map<string, GraphMemory>; active: Accessor<boolean>; api?: (api: GraphApi | null) => void;
};
function Plan(props: PlanProps) {
  const ws = useWs();
  const s = () => props.sprint;
  const [picked, setPicked] = createSignal<string | null>(null);
  const [viewing, setViewing] = createSignal<number | null>(null);
  const [errors, setErrors] = createSignal<Record<string, string>>({});
  const past = () => viewing() === null ? null : s().history?.find(h => h.version === viewing()) ?? null;
  const items = () => props.draft ?? past()?.items ?? s().plan?.items ?? [];
  const version = () => past()?.version ?? s().plan?.version;
  // States belong only to the version the tasks were made from, so a past version or a draft shows none.
  const rows = createMemo(() => planRows(ws.store.index(), s(), items(), props.draft ? null : version() ?? null));
  createEffect(() => props.rows?.(rows()));
  const selected = () => picked() ?? items()[0]?.key ?? null;
  const keys = createMemo(() => new Set(items().map(i => i.key)));
  /** The graphs light and dim only for a task the owner picked in the version shown, never the list's default first row. */
  const pick = () => { const p = picked(); return p !== null && keys().has(p) ? p : null; };
  const accepted = () => items().some(i => i.acceptance);
  const created = () => s().planning === "created";
  const editing = () => props.draft !== null;
  const view = () => ws.panels.planView();
  const graph = () => view() === "graph" && items().length > 0;
  const setView = (v: "list" | "graph") => { if (v !== "graph") props.setFull(false); ws.panels.setPlanView(v); };
  const plural = (n: number) => `${n} task${n === 1 ? "" : "s"}`;
  const title = () => `${version() ? `Plan v${version()}` : "Plan"}${items().length ? ` · ${plural(items().length)}` : ""}`;
  const memoryKey = () => `${version() ?? 0}${editing() ? ":draft" : ""}`;
  const graphRequest = { get shown() { return !created() && !past(); }, get why() { return props.request.why; }, run: () => props.request.run() };
  const [paneW, setPaneW] = createSignal(0);
  const watchWidth = (el: HTMLElement) => {
    if (typeof ResizeObserver === "undefined") return;
    const seen = new ResizeObserver(([entry]) => setPaneW(entry.contentRect.width));
    seen.observe(el);
    onCleanup(() => seen.disconnect());
  };
  const selectedRow = () => rows().find(r => r.item.key === selected());
  const startable = () => { const r = selectedRow(); return s().runMode === "manual" && r?.view?.task.status === "draft" && r.waits.length === 0; };
  const toggle = (key: string, dep: string) => {
    const list = props.draft!, item = list.find(i => i.key === key)!;
    const adding = !item.dependsOn.includes(dep);
    const problem = adding ? dependencyProblem(list, key, dep) : null;
    setErrors({ ...errors(), [key]: problem ?? "" });
    if (problem) return;
    props.setDraft(list.map(i => i.key === key ? { ...i, dependsOn: adding ? [...i.dependsOn, dep] : i.dependsOn.filter(d => d !== dep) } : i));
  };
  const done = () => {
    const draft = props.draft!;
    ws.ok({ type: "edit_plan", sprintId: s().id, items: draft, now: ws.store.clock() }, () => { props.setDraft(null); setErrors({}); });
  };
  const lead = <Show when={props.conv.chip}>{c => <button class={`chip${c().warn ? " st-warn" : ""}`} onClick={() => props.conv.show()}><Show when={c().warn}><I n="question" s={12} /></Show>{c().text}</button>}</Show>;
  return <section class="sp-plan" aria-label={version() ? `Plan v${version()}` : "Plan"} ref={revealCutIn} hidden={props.hidden} data-view={graph() ? "graph" : "list"}>
    <Show when={!(props.full && graph())}><SectionHead class="sp-plan-h" lead={lead} title={version() ? `Plan v${version()}` : "Plan"} count={items().length > 0 ? items().length : undefined}>
      <Show when={past() || editing()}><span class="gv-tag">{past() ? "Past version · read only" : "Draft · not saved"}</span></Show>
      <Show when={items().length > 0}><div class="seg" role="group" aria-label="Plan view">
        <button aria-current={graph() ? "false" : "true"} onClick={() => setView("list")}>List</button>
        <button aria-current={graph() ? "true" : "false"} onClick={() => setView("graph")}>Graph</button>
      </div></Show>
      <Show when={!created() && s().plan && !past()}>
        <Show when={editing()} fallback={<button class="btn btn-sm" onClick={() => { setView("list"); props.setDraft(structuredClone(s().plan!.items)); }}>Edit</button>}>
          <button class="btn btn-sm" onClick={() => { props.setDraft(null); setErrors({}); }}>Cancel</button><button class="btn btn-sm" onClick={done}>Done editing</button>
        </Show>
      </Show>
      {/* A first plan has nothing before it, so there is nothing to offer. */}
      <Show when={past()} fallback={<Show when={s().history?.length}><button class="btn btn-sm" popovertarget="m-plan-history">Previous plan</button></Show>}>
        <button class="btn btn-sm" onClick={() => setViewing(null)}>Back to v{s().plan?.version}</button>
      </Show>
      <Pop id="m-plan-history" class="pop-right" align="right" label="Previous plans">
        <For each={[...(s().history ?? [])].reverse()}>{h => <MenuItem label={`Plan v${h.version}`} sub={`${h.items.length} tasks`} onSelect={() => setViewing(h.version)} />}</For>
      </Pop>
    </SectionHead></Show>
    <Show when={items().length > 0} fallback={<div class="note-panel"><span class="w500">No plan yet</span><span>Linked tasks appear here after the conversation.</span></div>}>
      <Show when={!graph()} fallback={<Show when={memoryKey()} keyed>{key => <GraphView items={items()} rows={rows()} title={title()} mode={editing() ? "draft" : past() ? "past" : "plan"}
        created={created()} manual={s().runMode === "manual"} picked={pick()} onPick={setPicked} details={props.reading} onDetails={k => { setPicked(k); props.onRead(k); }}
        full={props.full} onFull={props.setFull} request={graphRequest} onOpenList={() => setView("list")}
        memory={props.memory} memoryKey={key} active={props.active} api={props.api} />}</Show>}>
      <div class="sp-scroll" ref={watchWidth}>
        <div class={`pt-row th${accepted() ? "" : " ws-plain"}`}><span>#</span><span>Task</span><Show when={accepted()}><span>Acceptance and checks</span></Show><span>{created() ? "State" : "Depends on"}</span></div>
        <Show when={editing()} fallback={<Keyed each={rows()} by={r => r.item.key}>{r => <div class={`pt-row${accepted() ? "" : " ws-plain"}`} data-selected={selected() === r().item.key ? "true" : undefined} onClick={() => setPicked(r().item.key)}>
          <span class="no">{r().no}</span>
          <span class="ws-pt-t"><button class="tt trunc" aria-current={selected() === r().item.key ? "true" : undefined}>{itemTitle(r().item)}</button><button class="lnk ws-more-l" aria-label={`Read more about task ${r().no}`} aria-expanded={props.reading === r().item.key} onClick={e => { e.stopPropagation(); setPicked(r().item.key); props.onRead(r().item.key); }}>Read more</button></span>
          <Show when={accepted()}><span class="ac trunc">{r().item.acceptance}</span></Show>
          <span class="dep"><Show when={planState(r())} fallback={<Show when={r().deps.length} fallback={<span class="faint">Starts first</span>}><span class="muted">After {r().deps.join(", ")}</span></Show>}>{w => <span class={`st ${w().tone}`}><I n={w().icon} s={12} class={w().spin ? "spin" : undefined} />{w().text}</span>}</Show></span>
        </div>}</Keyed>}>
          <Keyed each={rows()} by={r => r.item.key}>{r => <div class={`pt-row ws-static${accepted() ? "" : " ws-plain"}`}>
            <span class="no">{r().no}</span>
            <span><input class="inp ws-pick" value={itemTitle(r().item)} aria-label={`Task ${r().no} title`} onInput={e => props.setDraft(props.draft!.map(i => i.key === r().item.key ? { ...i, title: e.currentTarget.value } : i))} /></span>
            <Show when={accepted()}><span class="ac"><ResultText breaks text={r().item.acceptance} /></span></Show>
            <span class="dep"><DependencyPicker item={r().item} items={props.draft!} n={r().no} onToggle={dep => toggle(r().item.key, dep)} /></span>
            <Show when={errors()[r().item.key]}><span class="err"><I n="alert" s={12} /> {errors()[r().item.key]}</span></Show>
          </div>}</Keyed>
        </Show>
        <Graph items={items()} selected={pick()} onPick={setPicked} width={paneW()} onOpen={() => setView("graph")} />
      </div>
      <ActionBar class="sp-foot" hint="Every task waits for your final confirmation.">
        <Show when={!created() && !past()}><button class="btn btn-line btn-sm" aria-disabled={props.request.why ? "true" : undefined} data-why={props.request.why ?? undefined} onClick={() => { if (!props.request.why) props.request.run(); }}>Request changes</button></Show>
        <Show when={selectedRow()?.view}>{v => <>
          <Show when={startable()}><button class="btn btn-line btn-sm" {...ws.needs("start_task")} onClick={() => ws.run({ type: "start_task", taskId: v().id, now: ws.store.clock() })}>Start</button></Show>
          <A class="btn btn-line btn-sm" to={{ name: "task", machine: v().machine.id, task: v().id }}>Open task</A>
        </>}</Show>
      </ActionBar>
      </Show>
    </Show>
  </section>;
}

/** Chat, Plan, Board and Summary: one tab each, so the chat and the plan each have the whole width. */
function Tabs(props: { route: SprintRoute; sprint: Sprint }) {
  const at = (tab: SprintRoute["tab"]): Route => ({ ...props.route, tab });
  const board = (): Route => ({ name: "project", machine: props.route.machine, project: props.route.project, view: "board", scope: { kind: "sprint", id: props.sprint.id } });
  return <><Toolbar small class="tabs" mid={<div class="ws-tablist" role="tablist" aria-label="Sprint views">
    <A class="tab" role="tab" to={at("planning")} aria-selected={props.route.tab === "planning" ? "true" : "false"}>Chat</A>
    <A class="tab" role="tab" to={at("plan")} aria-selected={props.route.tab === "plan" ? "true" : "false"}>Plan</A>
    <A class="tab" role="tab" to={board()} aria-selected="false">Board</A>
    <A class="tab" role="tab" to={at("summary")} aria-selected={props.route.tab === "summary" ? "true" : "false"}>Summary</A>
  </div>} /><Show when={props.sprint.checkout}><Toolbar small mid={<SprintCheckout sprint={props.sprint} />} /></Show></>;
}

function CreatedLine(props: { sprint: Sprint }) {
  const ws = useWs();
  const view = createMemo(() => sprintView(ws.store.index(), props.sprint));
  const then = () => view().views.filter(v => v.task.status === "draft" && v.id !== view().next?.id && view().next && v.task.dependencies.includes(view().next!.id)).map(v => v.task.title);
  const blocked = () => view().blocked - then().length;
  return <Show when={props.sprint.runMode === "manual"} fallback={<span>
    <Show when={view().now}>{v => <><span class="faint">Now running</span> {v().task.title}</>}</Show>
    <Show when={view().next}>{v => <><span class="faint ws-gap">Next</span> {v().task.title}</>}</Show>
    <Show when={then().length > 0}><span class="faint ws-gap">Then</span> {then().join(", ")}</Show>
    <Show when={blocked() > 0}><span class="faint ws-gap">Blocked</span> {blocked()} task{blocked() === 1 ? "" : "s"}</Show>
  </span>}>
    <span><Show when={view().next} fallback="Manual: start each ready task yourself.">{v => <>Manual: task {props.sprint.taskIds.indexOf(v().id) + 1} is ready. Start it when you want, starting one never starts its neighbours.</>}</Show></span>
  </Show>;
}

function PlanningTab(props: { route: SprintRoute; sprint: Sprint; summary: ProjectSummary }) {
  const ws = useWs();
  const s = () => props.sprint;
  const [draft, setDraft] = createSignal<PlanItem[] | null>(null);
  const [reading, setReading] = createSignal<string | null>(null);
  const [rows, setRows] = createSignal<PlanRow[]>([]);
  const read = () => rows().find(r => r.item.key === reading());
  const created = () => s().planning === "created";
  const cancelled = () => s().planning === "cancelled";
  const createBlocked = () => !s().plan ? "No plan yet"
    : draft() ? "Finish editing the plan first"
    : s().planning === "question" ? "Answer the open question first"
    : s().planning === "planning" ? "Still working on the plan"
    : cancelled() ? "Resume planning first" : null;
  const create = () => { if (!createBlocked()) ws.run({ type: "materialize_plan", sprintId: s().id, version: s().plan!.version, now: ws.store.clock() }); };
  const n = () => s().plan?.items.length ?? 0;
  // The toolbar has no room for words: why its controls are off is said by the strips under it, which they name.
  const why = createUniqueId(), queueWhy = createUniqueId();
  const queueStrip = createQueueStrip(() => created() ? s() : null, queueWhy);
  const gapStrip = createGapStrip(s);
  const info = "Sprint planning. The planning conversation runs on the chosen model; no task runs until you create the tasks and start them.";
  const planning: StripItem = { id: "planning", tone: "info", domId: why, icon: <I n="layers" s={14} class="faint" />,
    get text() { const r = createBlocked(); return r ? `Create tasks: ${r}. ${info}` : info; },
    action: <button class="btn btn-sm" {...ws.needs("cancel_planning")} onClick={() => ws.run({ type: "cancel_planning", sprintId: s().id })}>Cancel planning</button> };
  const stopped: StripItem = { id: "cancelled", tone: "info", domId: why, icon: <I n="x" s={14} class="faint" />,
    get role() { return s().planningFailure ? "alert" as const : undefined; }, get text() { return s().planningFailure ?? "Planning is cancelled. Your draft, answers and plan are kept."; },
    action: <button class="btn btn-sm" {...ws.needs("resume_planning")} onClick={() => ws.run({ type: "resume_planning", sprintId: s().id })}>{s().planningFailure ? "Retry planning" : "Resume planning"}</button> };
  const line: StripItem = { id: "created", tone: "info", icon: <I n="play" s={14} class="faint" />, text: <CreatedLine sprint={s()} /> };
  const status = createMemo<StripItem[]>(() => created()
    ? [line, ...(gapStrip() ? [gapStrip()!] : []), ...(queueStrip() ? [queueStrip()!] : [])]
    : [cancelled() ? stopped : planning]);

  // ---------- Space: the chat and the plan are tabs; in the plan, one yield rule and Full view ----------
  const panels = ws.panels;
  const at = (tab: SprintRoute["tab"]): Route => ({ ...props.route, tab });
  const chatTab = () => props.route.tab === "planning";
  const shown = useContext(ScreenActive);
  const memory = new Map<string, GraphMemory>();
  const [api, setApi] = createSignal<GraphApi | null>(null);
  const [full, setFull] = createSignal(false);
  const [opened, setOpened] = createSignal<Opened>("none");
  const [winW, setWinW] = createSignal(window.innerWidth), [winH, setWinH] = createSignal(window.innerHeight);
  const onResize = () => { setWinW(window.innerWidth); setWinH(window.innerHeight); };
  window.addEventListener("resize", onResize);
  onCleanup(() => window.removeEventListener("resize", onResize));
  const graphOn = createMemo(() => panels.planView() === "graph" && (draft() ?? s().plan?.items ?? []).length > 0);
  const view = () => graphOn() ? "graph" as const : "list" as const;
  const fullOn = () => full() && graphOn() && !chatTab();
  const isReading = createMemo(() => !!read());
  // Held still while nothing it decides changes, so a machine read redraws none of the panels.
  const space = createMemo(() => arrange({ width: winW(), view: view(), full: fullOn(), sidebar: !panels.sidebarHidden(), sidebarW: panels.sidebarWidth(), conv: false, details: isReading(), opened: opened() }),
    undefined, { equals: (a, b) => !!a && !!b && a.sidebar === b.sidebar && a.conv === b.conv && a.details === b.details && a.replaced === b.replaced && a.convW === b.convW && a.main === b.main && a.swap === b.swap });
  const details = () => !!read() && space().details && !chatTab();
  // The panel opened last wins: when it leaves no room, Details closes and the selection stays.
  createEffect(() => { if (read() && !space().details) setReading(null); });
  createEffect(on(panels.sidebarHidden, (hidden, was) => { if (was && !hidden) setOpened("sb"); }, { defer: true }));
  const owner = {};
  createEffect(() => panels.yieldSidebar(owner, !shown() || chatTab() ? null : fullOn() ? "full" : !panels.sidebarHidden() && !space().sidebar ? "room" : null, () => setOpened("sb")));
  onCleanup(() => panels.yieldSidebar(owner, null));
  const onGraph = () => graphOn() && !chatTab();
  createEffect(() => panels.offerGraph(owner, shown() && n() > 0 ? {
    show: onGraph() ? null : () => { panels.setPlanView("graph"); ws.go(at("plan")); },
    fit: onGraph() ? () => api()?.fit() : null, full: onGraph() ? () => setFull(!full()) : null, fullOn: fullOn(),
  } : null));
  onCleanup(() => panels.offerGraph(owner, null));
  const session = () => ws.store.state().sessions.find(x => x.taskId === s().planningTaskId);
  const asks = createMemo(() => planningAsks(s(), session()).length);
  const [unsent, setUnsent] = createSignal(false);
  // What waits in the chat, said on the Plan tab's head; the chip leads to the Chat tab.
  const chip = createMemo(() => fullOn() ? null
    : asks() ? { warn: true, text: asks() === 1 ? "Question waiting" : `${asks()} questions waiting` }
    : s().planning === "planning" ? { warn: false, text: "Working on the plan" }
    : unsent() ? { warn: false, text: "Unsent message" } : null, null, { equals: (a, b) => a?.text === b?.text });
  const showConv = () => ws.go(at("planning"));
  // Ctrl+\ moves between the chat and the plan; Full view stays until the owner leaves it.
  const toggleConv = () => { if (fullOn()) return; ws.go(at(chatTab() ? "plan" : "planning")); };
  let body!: HTMLDivElement;
  /** Request changes opens the chat, leaving Full view if need be, and puts the cursor in its message box. Selection and view stay. */
  const requestChanges = () => {
    batch(() => { setFull(false); showConv(); });
    const input = () => body.querySelector<HTMLTextAreaElement>("#ws-plan-input");
    input()?.focus();
    queueMicrotask(() => input()?.focus());
  };
  // Held as one object, so a machine read that changes nothing redraws nothing in the plan's head.
  const conv: ConvControl = { get chip() { return chip(); }, show: showConv };
  const request = { why: null, run: requestChanges };
  const onKey = (e: KeyboardEvent) => {
    if (!shown() || document.querySelector("dialog[open]") || !(e.ctrlKey || e.metaKey) || e.altKey || e.shiftKey) return;
    if (e.key === "\\" || e.code === "Backslash") { e.preventDefault(); toggleConv(); }
  };
  document.addEventListener("keydown", onKey);
  onCleanup(() => document.removeEventListener("keydown", onKey));
  // Full view and short windows keep only the strips that block work. Stopped planning blocks it whatever its tone: its
  // explanation and its Retry or Resume stay.
  const blocks = (i: StripItem) => i.tone === "warn" || i.tone === "fail" || i === stopped;
  const lane = () => fullOn() || (onGraph() && winH() < 800) ? status().filter(blocks) : status();
  const columns = () => {
    const sp = space();
    if (chatTab() || sp.swap) return "minmax(0, 1fr)";
    return [sp.replaced && details() ? "" : "minmax(0, 1fr)", details() ? (sp.replaced ? "minmax(0, 1fr)" : "var(--inspector-w)") : ""].filter(Boolean).join(" ");
  };
  return <Page class="l-work sp-page" scroll="none"
    head={<Show when={!fullOn()}>
      <Toolbar class="sp-h" label="Sprint"
        mid={<>
          {/* A long title shortens here; a press opens the whole of it. */}
          <span class="tt trunc" ref={revealWhenCut} data-whole="">{sprintTitle(s())}</span>
          <Show when={created()} fallback={<span class="st st-idle l-tight-hide"><I n={cancelled() ? "x" : "pencil"} s={12} />{cancelled() ? "Planning cancelled" : "Planning"}</span>}>
            <span class="st st-ok l-compact-hide"><I n="check-circle" s={12} />{s().taskIds.length} task{s().taskIds.length === 1 ? "" : "s"} created from Plan v{s().materializedVersion}</span>
          </Show>
        </>}
        end={<>
          <RunMode sprint={s()} describedBy={created() ? queueWhy : why} /><QueueHelp sprint={s()} />
          <SprintTarget sprint={s()} summary={props.summary} class="l-tight-hide" />
          <Show when={created()} fallback={<button class="btn btn-primary" aria-disabled={createBlocked() ? "true" : undefined} data-why={createBlocked()} aria-describedby={createBlocked() ? why : undefined} {...ws.needs("materialize_plan")} onClick={create} aria-label={s().plan ? createLabel(n(), s().plan!.version) : undefined}>{s().plan ? <>{createLabel(n(), s().plan!.version).replace(/ from Plan v\d+$/, "")}<span class="l-tight-hide">from Plan v{s().plan!.version}</span></> : "Create tasks"}</button>}><QueueButton sprint={s()} steady describedBy={queueWhy} /></Show>
        </>} />
      <Tabs route={props.route} sprint={s()} />
    </Show>}
    status={<StatusLane items={lane()} />}>
    <div ref={body} class="sp-body" data-reading={details() ? "true" : undefined} data-full={fullOn() ? "" : undefined} style={{ "grid-template-columns": columns() }}>
      <Conversation sprint={s()} hidden={!chatTab()} onUnsent={setUnsent} onOpenPlan={() => ws.go(at("plan"))} />
      <Plan sprint={s()} draft={draft()} setDraft={setDraft} reading={reading()} onRead={key => { if (key) setOpened("details"); setReading(key); }} rows={setRows}
        hidden={chatTab() || (space().replaced && details())} full={fullOn()} setFull={setFull} conv={conv}
        request={request} memory={memory} active={shown} api={setApi} />
      <Show when={details()}><PlanItemPanel row={read()!} total={rows().length} back={space().replaced ? (graphOn() ? "Back to graph" : "Back to list") : undefined} onClose={() => setReading(null)} /></Show></div>
  </Page>;
}

function SummaryTab(props: { route: SprintRoute; sprint: Sprint }) {
  const ws = useWs();
  const data = createMemo(() => sprintSummary(ws.store.index(), props.sprint));
  const gap = createMemo(() => sprintGap(ws.store.index(), props.sprint));
  const cols = "18px minmax(0,2fr) 150px minmax(0,1.4fr) minmax(0,1fr)";
  const planning = (): Route => ({ ...props.route, tab: "planning" });
  const gapStrip = createGapStrip(() => props.sprint);
  const queueStrip = createQueueStrip(() => props.sprint);
  const status = createMemo<StripItem[]>(() => [...(data().rows.length > 0 && gapStrip() ? [gapStrip()!] : []), ...(queueStrip() ? [queueStrip()!] : [])]);
  return <Page class="l-work sp-page" scroll="none" head={<Tabs route={props.route} sprint={props.sprint} />} status={<StatusLane items={status()} />}>
    <div class="page"><div class="page-in">
      <div class="sum-grid">
        <div><div class="v">{data().ready}</div><div class="l">Ready to finish</div></div>
        <div><div class="v">{data().working}</div><div class="l">Working</div></div>
        <div><div class="v">{data().next}</div><div class="l">Next</div></div>
        <div><div class="v">{data().blocked}</div><div class="l">Blocked or queued</div></div>
        <div><div class="v">{data().merged}</div><div class="l">Merged</div></div>
      </div>
      <Show when={data().rows.length > 0} fallback={<div class="note-panel" role="status"><span class="w500">{gap()?.title ?? "No tasks to show"}</span><span>{gap()?.text ?? ""}</span>
        <Show when={gap()?.retry}><button class="btn btn-line btn-sm" onClick={() => void ws.store.reload()}>Retry</button></Show>
        <Show when={props.sprint.planning !== "created"}><A class="btn btn-line btn-sm" to={planning()}>Open planning</A></Show>
        <Show when={!props.sprint.archivedAt && props.sprint.planning === "created" && !props.sprint.taskIds.length}><button class="btn btn-line btn-sm" onClick={() => ws.newItem("task", props.sprint.projectId, props.sprint.id)}><I n="plus" s={14} />New task</button></Show>
      </div>}>
        <div class="surface"><div class="gt">
          <div class="gr th" style={{ "--cols": cols }}><span></span><span>Task</span><span>State</span><span>Checks</span><span>Branch</span></div>
          <Keyed each={data().rows} by={r => r.view.id}>{r => <A class="gr" style={{ "--cols": cols }} to={{ name: "task", machine: r().view.machine.id, task: r().view.id }}>
            <I n={r().word.icon} s={14} class={`${r().word.spin ? "spin " : ""}${r().word.tone}`} /><span class="w500 trunc">{r().view.task.title}</span>
            <span class={`st ${r().word.tone}`}>{r().word.text}</span><span class="cap muted trunc">{r().checks}</span><span class="cap faint mono trunc">{r().branch}</span>
          </A>}</Keyed>
        </div></div>
      </Show>
      <Show when={data().closed > 0}><div class="cap muted">{data().closed} closed without a merge.</div></Show>
      <div class="cap faint">This is an index of real results. A check that did not run is shown as not run, never as a pass.</div>
    </div></div>
  </Page>;
}

/** Chat, Plan and Summary. The Board tab is the project board scoped to this sprint. */
export function SprintPage(props: { route: SprintRoute; summary: ProjectSummary }) {
  const sprint = useSprint(() => props.route.sprint);
  return <Show when={sprint()} fallback={<div class="note-panel"><span class="w500">Sprint not found</span></div>}>{s =>
    <Show when={!s().archivedAt} fallback={<SummaryTab route={props.route} sprint={s()} />}><Show when={props.route.tab !== "summary"} fallback={<SummaryTab route={props.route} sprint={s()} />}><PlanningTab route={props.route} sprint={s()} summary={props.summary} /></Show></Show>
  }</Show>;
}
