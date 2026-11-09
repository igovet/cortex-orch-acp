import { createEffect, createMemo, createSignal, For, Match, onCleanup, Show, Switch } from "solid-js";
import { dependencyProblem, runBlocker, type AttachmentInput, type PlanItem, type Sprint, type TaskSettings } from "@cortex/workspace-domain";
import { createDictation, MicBar, MicButton, RouteBar } from "./composer";
import { useWs } from "./ctx";
import { A, I, Keyed, MenuItem, Pop } from "./kit";
import { Answered, QuestionTabs, type Ask } from "./questions";
import type { Route } from "./router";
import { clock, sprintTitle, sprintView, type ProjectSummary } from "./selectors";
import { ResultText } from "./result-text";
import { createLabel, itemTitle, planCaption, planGraph, planningAsks, planningThread, planRows, queueStall, sprintArchiveBlocked, sprintGap, sprintSummary, type PlanRow } from "./sprint-model";

type SprintRoute = Extract<Route, { name: "sprint" }>;
const useSprint = (id: () => string) => { const ws = useWs(); return createMemo(() => ws.store.state().sprints.find(s => s.id === id()) ?? null); };
const firstReady = (index: ReturnType<ReturnType<typeof useWs>["store"]["index"]>, sprint: Sprint) => sprintView(index, sprint).next;

/** Where the sprint's tasks merge. Each task can still pick its own target in Ready to merge. */
export function SprintTarget(props: { sprint: Sprint; summary: ProjectSummary }) {
  const ws = useWs();
  const target = () => props.sprint.targetBranch ?? props.summary.project.defaultBranch;
  const branches = () => [...new Set([props.summary.project.defaultBranch, props.summary.project.openBranch])];
  return <>
    <button class="chip" popovertarget="m-target"><span class="faint">Target branch</span><span class="mono">{target()}</span><I n="chevron-down" s={12} /></button>
    <Pop id="m-target" class="pop-right" align="right" label="Target branch" style={{ width: "280px" }}>
      <div class="mi-h">Target branch</div>
      <For each={branches()}>{b => <MenuItem label={<span class="mono">{b}</span>} sub={b === props.summary.project.defaultBranch ? "Git default branch" : undefined} check={b === target()}
        needs="set_target" onSelect={() => ws.run({ type: "set_target", sprintId: props.sprint.id, branch: b === props.summary.project.defaultBranch ? null : b })} />}</For>
    </Pop>
  </>;
}

export function RunMode(props: { sprint: Sprint }) {
  const ws = useWs();
  const set = (runMode: "manual" | "automatic") => { if (props.sprint.runMode !== runMode) ws.run({ type: "set_run_mode", sprintId: props.sprint.id, runMode }); };
  return <div class="seg" role="group" aria-label="Sprint run mode">
    <button aria-current={props.sprint.runMode === "manual" ? "true" : "false"} {...ws.needs("set_run_mode")} onClick={() => set("manual")}>Manual</button>
    <button aria-current={props.sprint.runMode === "automatic" ? "true" : "false"} {...ws.needs("set_run_mode")} onClick={() => set("automatic")}>Automatic</button>
  </div>;
}

/** The sprint's one run control: Start in Manual, Run, Pause or Continue in Automatic. A refusal shows on the button, before the click. */
export function QueueButton(props: { sprint: Sprint; line?: boolean; small?: boolean }) {
  const ws = useWs();
  const size = () => props.small ? " btn-sm" : "";
  // The plan can name tasks the project's runtime has not loaded yet; the queue waits for them instead of the screen failing.
  const blocker = () => { const state = ws.store.state();
    return props.sprint.taskIds.every(id => state.tasks.some(t => t.id === id)) ? runBlocker(state, props.sprint) : "This sprint's tasks are not loaded yet"; };
  const next = () => firstReady(ws.store.index(), props.sprint);
  const number = () => { const v = next(); return v ? props.sprint.taskIds.indexOf(v.id) + 1 : 0; };
  const run = () => { if (!blocker()) ws.run({ type: "run_sprint", sprintId: props.sprint.id, now: ws.store.clock() }); };
  const complete = () => props.sprint.queueState === "complete";
  const cls = () => `btn ${props.line ? "btn-line" : "btn-primary"}${size()}`;
  return <Show when={props.sprint.planning === "created"}>
    <Show when={props.sprint.runMode === "automatic"} fallback={<Show when={next()}>{v => <button class={cls()} {...ws.needs("start_task")} onClick={() => ws.run({ type: "start_task", taskId: v().id, now: ws.store.clock() })}>Start task {number()}</button>}</Show>}>
      <Show when={props.sprint.queueState === "running"} fallback={<Show when={!complete()}>
        <button class={cls()} aria-disabled={blocker() ? "true" : undefined} data-why={blocker()} {...ws.needs("run_sprint")} onClick={run}>{props.sprint.queueState === "paused" ? "Continue queue" : "Run sprint"}</button>
      </Show>}>
        <button class={`btn btn-line${size()}`} {...ws.needs("pause_sprint")} onClick={() => ws.run({ type: "pause_sprint", sprintId: props.sprint.id })}>Pause queue</button>
      </Show>
    </Show>
  </Show>;
}

/** Why a queue is not moving, in one line, with the task to open. Always the same words on the plan, the board and the summary. */
export function QueueNote(props: { sprint: Sprint }) {
  const ws = useWs();
  const stall = createMemo(() => queueStall(ws.store.index(), props.sprint));
  const paused = () => props.sprint.queueState === "paused" && props.sprint.pauseReason;
  const open = (id: string | null | undefined) => { const v = id ? ws.store.index().byId.get(id) : undefined; return v ? <span class="acts"><A class="lnk" to={{ name: "task", machine: v.machine.id, task: v.id }}>Open task</A></span> : null; };
  return <>
    <Show when={paused()}>{r => <div class="strip warn"><I n="pause" s={14} class="st-warn" /><span>Queue paused: {stall()?.reason ?? r()}. {stall()?.taskId ? "Review this task, then Continue queue." : "Continue queue when you are ready."}</span>{open(stall()?.taskId)}</div>}</Show>
    <Show when={!paused() && stall()}>{b => <div class="strip warn" role="status"><I n="alert" s={14} class="st-warn" /><span>{b().reason}.</span>{open(b().taskId)}</div>}</Show>
    <Show when={props.sprint.runMode === "automatic"}><div class="strip info"><I n="eye" s={14} class="faint" /><span>Passed checks leave each task ready for your final confirmation and start the next ready task. Nothing finishes without your confirmation.{props.sprint.queueState === "running" ? " Pause queue stops new starts; the task already running keeps going." : ""}</span></div></Show>
    <Show when={props.sprint.queueState === "complete"}><div class="strip ok"><I n="check-circle" s={14} class="st-ok" /><span>All queued tasks have run. Finished results wait for your confirmation; nothing closes by itself.</span></div></Show>
  </>;
}

/** Some of the sprint's tasks could not be read: say how many, keep the membership, and read again on request. */
function GapNote(props: { sprint: Sprint }) {
  const ws = useWs();
  const gap = createMemo(() => sprintGap(ws.store.index(), props.sprint));
  return <Show when={gap()?.retry && gap()}>{g => <div class="strip warn" role="status"><I n="alert" s={14} class="st-warn" /><span>{g().title}. {g().text}</span><span class="acts"><button class="btn btn-sm" onClick={() => void ws.store.reload()}>Retry</button></span></div>}</Show>;
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
  const archiveWhy = () => sprintArchiveBlocked(ws.store.index(), props.sprint);
  const archive = () => { if (archiveWhy()) return; for (const id of props.sprint.taskIds) ws.archiveTask(id); };
  return <>
    <Show when={props.tab === "summary"}><QueueButton sprint={props.sprint} line /></Show>
    <button class="ib" popovertarget="m-sprint-menu" aria-label="Sprint actions"><I n="more" /></button>
    <Pop id="m-sprint-menu" class="pop-right" align="right" label="Sprint actions">
      <Show when={planning() && props.sprint.planning !== "cancelled"}><MenuItem icon="x" label="Cancel planning" needs="cancel_planning" onSelect={() => ws.run({ type: "cancel_planning", sprintId: props.sprint.id })} /></Show>
      <Show when={props.sprint.planning === "cancelled"}><MenuItem icon="play" label="Resume planning" needs="resume_planning" onSelect={() => ws.run({ type: "resume_planning", sprintId: props.sprint.id })} /></Show>
      <MenuItem icon="archive" label="Archive" sub={archiveWhy() ?? undefined} disabled={archiveWhy() ?? undefined} onSelect={archive} />
    </Pop>
  </>;
}

function Conversation(props: { sprint: Sprint }) {
  const ws = useWs();
  const s = () => props.sprint;
  const [text, setText] = createSignal("");
  const mic = createDictation();
  const session = () => ws.store.state().sessions.find(x => x.taskId === s().planningTaskId);
  const asks = createMemo(() => planningAsks(s(), session()));
  const thread = createMemo(() => planningThread(s(), session()));
  const answer = (ask: Ask, text: string, optionId: string | null, attachments?: AttachmentInput[]) => ws.run({ type: "answer_planning", sprintId: s().id, answer: text, attachments, now: ws.store.clock(), ...(ask.id === "planning" ? {} : { decisionId: ask.id, optionId }) });
  const closed = () => s().planning === "created" ? "The tasks exist now. Change them on the board." : s().planning === "cancelled" ? "Planning is cancelled. Resume it to continue." : null;
  const send = (value: string) => { if (value.trim()) ws.ok({ type: "plan_message", sprintId: s().id, text: value, now: ws.store.clock() }, () => { setText(""); }); };
  const settings = (): TaskSettings => ({ model: "opus[1m]", effort: "high", mode: "development", language: "auto", ...s().settings });
  const append = (spoken: string) => setText(text().trim() ? `${text().trimEnd()} ${spoken}` : spoken);
  return <section class="sp-conv" aria-label="Planning conversation">
    <div class="timeline"><div class="tl-in">
      <div class="brief"><ResultText breaks text={s().description} /><div class="meta"><Show when={ws.t("planningBadge")}><span>{ws.t("planningBadge")}</span></Show></div></div>
      <Keyed each={thread()} by={e => e.key}>{e => <Switch>
        <Match when={e().kind === "note"}><div class="ai"><ResultText breaks text={e().text} /></div></Match>
        <Match when={e().kind === "qa"}><Answered question={e().question ?? ""} answer={e().text} at={e().at} /></Match>
        <Match when={e().kind === "message"}><div class="mine"><ResultText breaks text={e().text} /></div><div class="mine-meta"><I n="check" s={12} />Sent to the planner<Show when={e().at !== null}><span>{clock(e().at)}</span></Show></div></Match>
        <Match when={e().kind === "event"}><div class="answered"><I n="check" s={14} /><span>{e().text}</span><Show when={e().at !== null}><span class="faint">{clock(e().at)}</span></Show></div></Match>
      </Switch>}</Keyed>
      <Show when={asks().length > 0}><QuestionTabs asks={asks()} needs="answer_planning" blocked={() => undefined} note={() => null} onAnswer={answer} waiting={ask => ask.at === null ? "" : clock(ask.at)} /></Show>
      <Show when={s().planning === "planning"}><div class="answered"><I n="loader" s={14} class="spin" />Working on the plan</div></Show>
    </div></div>
    <div class="dock"><div class="composer">
      <MicBar mic={mic} onInsert={append} />
      <textarea id="ws-plan-input" aria-label="Message the planner" placeholder={closed() ?? "Ask for changes to the plan…"} disabled={!!closed()} value={text()} onInput={e => setText(e.currentTarget.value)}
        onKeyDown={e => { if (e.key === "Enter" && !e.shiftKey && !e.isComposing) { e.preventDefault(); send(text()); } }} />
      <div class="c-bar">
        <RouteBar id="plan" up compact needs="set_sprint_settings" settings={settings()} onChange={patch => ws.run({ type: "set_sprint_settings", sprintId: s().id, settings: patch })} />
        <div class="c-end">
          <MicButton mic={mic} disabled={closed() ? "Voice input is off here now" : undefined} />
          <button class="btn btn-primary btn-sm c-send ws-round" aria-disabled={text().trim() && !closed() ? undefined : "true"} data-why={closed() ?? (text().trim() ? undefined : "Type a message first")} aria-label="Send to the planner" {...ws.needs("plan_message")} onClick={() => { if (!closed()) send(text()); }}><I n="arrow-up" s={14} /></button>
        </div>
      </div>
    </div></div>
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

function Graph(props: { items: PlanItem[]; selected: string | null; onPick: (key: string) => void }) {
  const g = createMemo(() => planGraph(props.items, props.selected));
  return <div class="depg" aria-label="Dependency graph">
    <svg viewBox={`0 0 ${g().width} ${g().height}`} style={{ width: `${g().width}px`, "max-width": "100%", height: "auto" }} role="img" aria-label={g().label}>
      <Keyed each={g().edges} by={e => e.key}>{e => <path class={`e${e().on ? " on" : ""}`} d={e().d} />}</Keyed>
      <Keyed each={g().nodes} by={n => n.key}>{n => <g class="ws-node" onClick={() => props.onPick(n().key)}><rect class={`nd${n().on ? " on" : ""}`} x={n().x} y={n().y} width={n().w} height={n().h} rx="7" /><text x={n().x + 12} y={n().y + 21}>{n().label}</text></g>}</Keyed>
    </svg>
    <div class="cap muted ws-mt4">{planCaption(props.items, props.selected)}</div>
  </div>;
}

const planState = (r: PlanRow) => {
  if (!r.word) return null;
  if ((r.word.text === "Blocked" || r.word.text === "Queued") && r.waits.length) return { tone: "st-idle", icon: "hourglass", text: `Waits for ${r.waits.join(", ")}` };
  const ready = r.word.text === "Next" || r.word.text === "Queued";
  return { tone: r.word.tone, icon: r.word.text === "Next" ? "play" : r.word.icon, spin: r.word.spin, text: ready ? "Ready" : r.word.text };
};

/** A plan may hold very long tasks, so the list names them and this holds the full text. It scrolls by itself and floats over the plan on a narrow window. */
function PlanItemPanel(props: { row: PlanRow; total: number; onClose: () => void }) {
  let close!: HTMLButtonElement;
  const back = document.activeElement as HTMLElement | null;
  const shut = () => { props.onClose(); if (back?.isConnected) back.focus({ preventScroll: true }); };
  const onKey = (e: KeyboardEvent) => { if (e.key === "Escape" && !document.querySelector("dialog[open]")) { e.preventDefault(); shut(); } };
  document.addEventListener("keydown", onKey);
  onCleanup(() => document.removeEventListener("keydown", onKey));
  queueMicrotask(() => close?.focus({ preventScroll: true }));
  return <aside class="ws-read" aria-label="Task details">
    <div class="ws-read-h"><span class="cap faint">Task {props.row.no} of {props.total}</span><button ref={close} class="ib ws-ib" aria-label="Close task details" onClick={shut}><I n="x" s={14} /></button></div>
    <div class="ws-read-b selectable">
      <h2 class="ws-read-t">{itemTitle(props.row.item)}</h2>
      <dl class="kv">
        <dt>Depends on</dt><dd>{props.row.deps.length ? `Tasks ${props.row.deps.join(", ")}` : "Starts first"}</dd>
        <Show when={planState(props.row)}>{w => <><dt>State</dt><dd class={w().tone}>{w().text}</dd></>}</Show>
      </dl>
      <div class="ws-read-d"><ResultText breaks text={props.row.item.description} /></div>
      <Show when={props.row.item.acceptance}><div class="ins-h ws-read-s"><span>Acceptance and checks</span></div><div class="ws-read-d"><ResultText breaks text={props.row.item.acceptance} /></div></Show>
    </div>
    <Show when={props.row.view}>{v => <div class="ws-read-f"><A class="btn btn-line ws-wide" to={{ name: "task", machine: v().machine.id, task: v().id }}>Open task<I n="arrow-right" s={14} /></A></div>}</Show>
  </aside>;
}

function Plan(props: { sprint: Sprint; draft: PlanItem[] | null; setDraft: (items: PlanItem[] | null) => void; reading: string | null; onRead: (key: string | null) => void; rows?: (rows: PlanRow[]) => void }) {
  const ws = useWs();
  const s = () => props.sprint;
  const [picked, setPicked] = createSignal<string | null>(null);
  const [viewing, setViewing] = createSignal<number | null>(null);
  const [errors, setErrors] = createSignal<Record<string, string>>({});
  const past = () => viewing() === null ? null : s().history?.find(h => h.version === viewing()) ?? null;
  const items = () => props.draft ?? past()?.items ?? s().plan?.items ?? [];
  const rows = createMemo(() => planRows(ws.store.index(), s(), items()));
  createEffect(() => props.rows?.(rows()));
  const selected = () => picked() ?? items()[0]?.key ?? null;
  const accepted = () => items().some(i => i.acceptance);
  const created = () => s().planning === "created";
  const editing = () => props.draft !== null;
  const version = () => past()?.version ?? s().plan?.version;
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
  return <section class="sp-plan" aria-label={version() ? `Plan v${version()}` : "Plan"}>
    <div class="sp-plan-h"><I n="list" s={14} class="faint" /><Show when={version()} fallback="Plan">Plan v{version()}</Show><Show when={items().length > 0}><span class="cap faint ws-w400">{items().length} task{items().length === 1 ? "" : "s"}</span></Show>
      <span class="r">
        <Show when={!created() && s().plan && !past()}>
          <Show when={editing()} fallback={<button class="btn btn-sm" onClick={() => props.setDraft(structuredClone(s().plan!.items))}>Edit</button>}>
            <button class="btn btn-sm" onClick={() => { props.setDraft(null); setErrors({}); }}>Cancel</button><button class="btn btn-sm" onClick={done}>Done editing</button>
          </Show>
        </Show>
        <Show when={past()} fallback={<button class="btn btn-sm" popovertarget="m-plan-history" aria-disabled={s().history?.length ? undefined : "true"} data-why={s().history?.length ? undefined : "This is the first plan"}>Previous plan</button>}>
          <button class="btn btn-sm" onClick={() => setViewing(null)}>Back to v{s().plan?.version}</button>
        </Show>
        <Pop id="m-plan-history" class="pop-right" align="right" label="Previous plans">
          <For each={[...(s().history ?? [])].reverse()}>{h => <MenuItem label={`Plan v${h.version}`} sub={`${h.items.length} tasks`} onSelect={() => setViewing(h.version)} />}</For>
        </Pop>
      </span></div>
    <Show when={items().length > 0} fallback={<div class="note-panel"><span class="w500">No plan yet</span><span>Linked tasks appear here after the conversation.</span></div>}>
      <div class="sp-scroll">
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
        <Graph items={items()} selected={selected()} onPick={setPicked} />
      </div>
      <div class="sp-foot"><span class="cap muted">Every task waits for your final confirmation.</span>
        <span class="r">
          <Show when={!created()}><button class="btn btn-line btn-sm" onClick={() => document.getElementById("ws-plan-input")?.focus()}>Request changes</button></Show>
          <Show when={selectedRow()?.view}>{v => <>
            <Show when={startable()}><button class="btn btn-line btn-sm" {...ws.needs("start_task")} onClick={() => ws.run({ type: "start_task", taskId: v().id, now: ws.store.clock() })}>Start</button></Show>
            <A class="btn btn-line btn-sm" to={{ name: "task", machine: v().machine.id, task: v().id }}>Open task</A>
          </>}</Show>
        </span></div>
    </Show>
  </section>;
}

function Tabs(props: { route: SprintRoute; sprint: Sprint }) {
  const at = (tab: SprintRoute["tab"]): Route => ({ ...props.route, tab });
  const board = (): Route => ({ name: "project", machine: props.route.machine, project: props.route.project, view: "board", scope: { kind: "sprint", id: props.sprint.id } });
  return <div class="tabs" role="tablist" aria-label="Sprint views">
    <A class="tab" role="tab" to={at("planning")} aria-selected={props.route.tab === "planning" ? "true" : "false"}>Planning</A>
    <A class="tab" role="tab" to={board()} aria-selected="false">Board</A>
    <A class="tab" role="tab" to={at("summary")} aria-selected={props.route.tab === "summary" ? "true" : "false"}>Summary</A>
    <span class="grow"></span>
  </div>;
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
  return <>
    <div class="sp-h">
      <span class="tt">{sprintTitle(s())}</span>
      <Show when={created()} fallback={<span class="st st-idle"><I n={cancelled() ? "x" : "pencil"} s={12} />{cancelled() ? "Planning cancelled" : "Planning"}</span>}>
        <span class="st st-ok"><I n="check-circle" s={12} />{s().taskIds.length} task{s().taskIds.length === 1 ? "" : "s"} created from Plan v{s().materializedVersion}</span>
      </Show>
      <span class="grow"></span>
      <RunMode sprint={s()} />
      <SprintTarget sprint={s()} summary={props.summary} />
      <Show when={created()} fallback={<button class="btn btn-primary" aria-disabled={createBlocked() ? "true" : undefined} data-why={createBlocked()} {...ws.needs("materialize_plan")} onClick={create}>{s().plan ? createLabel(n(), s().plan!.version) : "Create tasks"}</button>}><QueueButton sprint={s()} /></Show>
    </div>
    <Tabs route={props.route} sprint={s()} />
    <Show when={created()} fallback={<Show when={cancelled()} fallback={<div class="strip info"><I n="layers" s={14} class="faint" /><span>Sprint planning. The planning conversation runs on the chosen model; no task runs until you create the tasks and start them.</span><span class="acts"><button class="btn btn-sm" {...ws.needs("cancel_planning")} onClick={() => ws.run({ type: "cancel_planning", sprintId: s().id })}>Cancel planning</button></span></div>}>
      <div class="strip info" role={s().planningFailure ? "alert" : undefined}><I n="x" s={14} class="faint" /><span>{s().planningFailure ?? "Planning is cancelled. Your draft, answers and plan are kept."}</span><span class="acts"><button class="btn btn-sm" {...ws.needs("resume_planning")} onClick={() => ws.run({ type: "resume_planning", sprintId: s().id })}>{s().planningFailure ? "Retry planning" : "Resume planning"}</button></span></div>
    </Show>}>
      <div class="strip info"><I n="play" s={14} class="faint" /><CreatedLine sprint={s()} /></div>
      <GapNote sprint={s()} />
      <QueueNote sprint={s()} />
    </Show>
    <div class="sp-body" data-reading={read() ? "true" : undefined}><Conversation sprint={s()} /><Plan sprint={s()} draft={draft()} setDraft={setDraft} reading={reading()} onRead={setReading} rows={setRows} />
      <Show when={!!read()}><PlanItemPanel row={read()!} total={rows().length} onClose={() => setReading(null)} /></Show></div>
  </>;
}

function SummaryTab(props: { route: SprintRoute; sprint: Sprint }) {
  const ws = useWs();
  const data = createMemo(() => sprintSummary(ws.store.index(), props.sprint));
  const gap = createMemo(() => sprintGap(ws.store.index(), props.sprint));
  const cols = "18px minmax(0,2fr) 150px minmax(0,1.4fr) minmax(0,1fr)";
  const planning = (): Route => ({ ...props.route, tab: "planning" });
  return <>
    <Tabs route={props.route} sprint={props.sprint} />
    <Show when={data().rows.length > 0}><GapNote sprint={props.sprint} /></Show>
    <QueueNote sprint={props.sprint} />
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
        <Show when={props.sprint.planning === "created" && !props.sprint.taskIds.length}><button class="btn btn-line btn-sm" onClick={() => ws.newItem("task", props.sprint.projectId, props.sprint.id)}><I n="plus" s={14} />New task</button></Show>
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
  </>;
}

/** Plan and Summary. The Board tab is the project board scoped to this sprint. */
export function SprintPage(props: { route: SprintRoute; summary: ProjectSummary }) {
  const sprint = useSprint(() => props.route.sprint);
  return <Show when={sprint()} fallback={<div class="note-panel"><span class="w500">Sprint not found</span></div>}>{s =>
    <Show when={props.route.tab === "planning"} fallback={<SummaryTab route={props.route} sprint={s()} />}><PlanningTab route={props.route} sprint={s()} summary={props.summary} /></Show>
  }</Show>;
}
