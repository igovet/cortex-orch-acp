import { batch, createEffect, createMemo, createSignal, createUniqueId, For, Match, on, onCleanup, Show, Switch, useContext, type Accessor } from "solid-js";
import { automaticQueueOf, COMPLEXITY_LABELS, dependencyProblem, isOpen, openPlanningQuestion, queueDrivenMode, runBlocker, sprintCloseBlockers, SPRINT_RUN_MODES, type AttachmentInput, type PlanItem, type Sprint, type SprintGraphEdit, type WorkspaceState } from "@cortex/workspace-domain";
import { attachmentName } from "./attachments";
import { dependentWhy, editNotice, linkProblem, linkWords, liveModel, placeWords, watchLiveGraphs, type LiveModel } from "./live-graph";
import { AttachChips } from "./composer";
import { useWs } from "./ctx";
import { autoGrow } from "./autogrow";
import { REQUESTING, RequestLabel } from "./request-feedback";
import { FieldEdit } from "./field-edit";
import { createBodyMark, ScreenActive } from "./freshness";
import { A, I, Keyed, MenuItem, Modal, placeFocus, Pop } from "./kit";
import { ActionBar, Page, revealCutIn, revealWhenCut, SectionHead, StatusLane, Toolbar, type StripItem, type StripTone } from "./layout";
import { Hint } from "./settings-kit";
import { Answered, QuestionForm, type AskAnswer } from "./questions";
import type { Route } from "./router";
import { activePlanningWarning, clock, decisionRoute, humanStatus, classifiedPlanningFailure, planningChangeRequired, planningFailureDetails, planningRequestInvalid, savedPlanningText, stoppedPlanningText, planningTaskOf, sprintModeDescription, sprintModeLabel, sprintTitle, sprintView, stopReason, type ProjectSummary } from "./selectors";
import { Conversation as TaskConversation, firstWorker, TaskWorkers, useSessionClock } from "./session";
import { sessionModel, type SessionModel } from "./session-model";
import { PlanningPlace } from "./task-place";
import { Skeleton } from "./skeleton";
import { share } from "./stable";
import { ResultText } from "./result-text";
import { ResultScope } from "./result-links";
import { CopySource, MessageTools } from "./message-copy";
import { SentMessages } from "./sent";
import { createLabel, itemTitle, planCaption, planGraph, planningAsks, planningThread, planRows, queueStall, separateWorktrees, sprintCheckoutLabel, sprintGap, sprintSummary, SPRINT_CLOSE_NOTE, type PlanRow } from "./sprint-model";
import { graphCounts, planLayers } from "./plan-graph";
import { GraphView, type GraphApi, type GraphLinks, type GraphMemory } from "./sprint-graph";
import { arrange, INLINE_FLOOR, inlineScale, type Opened } from "./sprint-space";
import { changedBy, SprintRevisions } from "./sprint-revisions";

type SprintRoute = Extract<Route, { name: "sprint" }>;
const useSprint = (id: () => string) => { const ws = useWs(); return createMemo(() => ws.store.state().sprints.find(s => s.id === id()) ?? null); };
/** The task a Manual sprint's Start control offers: the first suggestion that can start now, never the queue's Next. */
const firstReady = (index: ReturnType<ReturnType<typeof useWs>["store"]["index"]>, sprint: Sprint) => sprintView(index, sprint).suggested;

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
  return <Show when={props.sprint.checkout}><span class="cap muted" aria-label="Sprint checkout">Tasks: {sprintCheckoutLabel(props.sprint.checkout!)}</span></Show>;
}

export function RunMode(props: { sprint: Sprint; describedBy?: string }) {
  const ws = useWs();
  const set = (runMode: Sprint["runMode"]) => { if (props.sprint.runMode !== runMode) ws.run({ type: "set_run_mode", sprintId: props.sprint.id, runMode }); };
  const described = () => ws.gate("set_run_mode") ? props.describedBy : undefined;
  const blocked = (mode: Sprint["runMode"]) => mode !== "manual" && separateWorktrees(props.sprint) ? ws.t("separateWorktreesManualOnly") : undefined;
  const id = createUniqueId();
  return <div class="seg" role="group" aria-label="Sprint run mode">
    <For each={SPRINT_RUN_MODES}>{mode => <>
      <button aria-current={props.sprint.runMode === mode ? "true" : "false"} aria-describedby={[`${id}-${mode}`, described()].filter(Boolean).join(" ")} {...ws.needs("set_run_mode")} aria-disabled={blocked(mode) ? "true" : undefined} data-why={blocked(mode)} onClick={() => { if (!blocked(mode)) set(mode); }}>{sprintModeLabel[mode]}</button>
      <span hidden id={`${id}-${mode}`}>{sprintModeDescription[mode]}</span>
    </>}</For>
  </div>;
}

// The plan can name tasks the project's runtime has not loaded yet; the queue waits for them instead of the screen failing.
const queueBlocker = (state: WorkspaceState, sprint: Sprint) => sprint.runMode === "automatic" && automaticQueueOf(sprint)
  || sprint.taskIds.every(id => state.tasks.some(t => t.id === id))
  ? runBlocker(state, sprint) : "This sprint's tasks are not loaded yet";

/**
 * The sprint's one run control: Start in Manual, Run, Pause or Continue in a queued mode. `steady` keeps one width as its label swaps.
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
  // A run that cannot go takes the shared disabled look of a main action (layout.css): an outlined secondary only greys its
  // label, and beside the other outlined controls of the board bar a disabled Continue queue read as usable.
  const runCls = () => blocker() || ws.gate("run_sprint") ? `btn btn-primary${size()}` : cls();
  return <Show when={!props.sprint.archivedAt && props.sprint.planning === "created"}>
    <Show when={queueDrivenMode(props.sprint.runMode)} fallback={<Show when={next()}>{v => <button class={cls()} aria-describedby={described("start_task")} {...ws.needs("start_task")} onClick={() => ws.run({ type: "start_task", taskId: v().id, now: ws.store.clock() })}><RequestLabel pending={ws.store.pendingTask("start_task", v().id)} text={REQUESTING.start_task}>Start task {number()}</RequestLabel></button>}</Show>}>
      <Show when={props.sprint.queueState === "running"} fallback={<Show when={!complete()}>
        <button class={runCls()} aria-disabled={blocker() ? "true" : undefined} data-why={blocker()} aria-describedby={described("run_sprint", blocker())} {...ws.needs("run_sprint")} onClick={run}>{props.sprint.queueState === "paused" ? "Continue queue" : "Run sprint"}</button>
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
  const index = ws.store.index();
  const automatic = automaticQueueOf(sprint);
  if (automatic) return { tone: "warn", icon: automatic.kind === "owner_integration" ? "merge" : automatic.needsOwner ? "alert" : "pause", iconClass: "st-warn",
    taskId: automatic.taskId, role: "status", text: automatic.kind === "continuation" ? stopReason(automatic.reason) : humanStatus(automatic.reason) };
  // Queue state belongs to the service, not completeness of a window read.
  if (sprint.taskIds.some(id => !index.byId.has(id))) return null;
  const stall = queueStall(index, sprint);
  const waiting = queueDrivenMode(sprint.runMode) && sprint.planning === "created" && sprint.queueState !== "running" && sprint.queueState !== "complete";
  const blocker = waiting ? queueBlocker(index.state, sprint) : null;
  if (sprint.queueState === "paused" && sprint.pauseReason) return { tone: "warn", icon: "pause", iconClass: "st-warn", taskId: stall?.taskId ?? null,
    text: `Queue paused: ${stop(stall?.reason ?? blocker ?? humanStatus(sprint.pauseReason))}.${stall || blocker ? "" : queueDrivenMode(sprint.runMode) ? " Continue queue when you are ready." : " Start the next task when you are ready."}` };
  if (stall || blocker) return { tone: "warn", icon: "alert", iconClass: "st-warn", taskId: stall?.taskId ?? null, role: "status", text: `${stop(stall?.reason ?? blocker!)}.` };
  // Once every task of the sprint is closed nothing waits for confirmation any more, so the queue has nothing to say.
  if (sprint.queueState === "complete") return sprint.runMode === "automatic" || sprint.taskIds.every(id => index.byId.get(id)!.task.status === "done") ? null
    : { tone: "ok", icon: "check-circle", iconClass: "st-ok", taskId: null, text: "All queued tasks have run. Finished results wait for your confirmation; nothing closes by itself." };
  // A control the machine does not offer is greyed out: its reason still has to be somewhere the owner can read.
  const refused = ws.gate("set_run_mode") ?? ws.gate(!queueDrivenMode(sprint.runMode) ? "start_task" : sprint.queueState === "running" ? "pause_sprint" : "run_sprint");
  return refused ? { tone: "info", icon: "help", iconClass: "faint", taskId: null, role: "status", text: refused } : null;
}
const sameWords = (a: QueueWords | null, b: QueueWords | null) => JSON.stringify(a) === JSON.stringify(b);

/** The queue's one strip, or nothing when the queue is simply moving. The item keeps its identity, so a change of words redraws only the words. */
export function createQueueStrip(sprint: Accessor<Sprint | null | undefined>, domId?: string): Accessor<StripItem | null> {
  const ws = useWs();
  const words = createMemo(() => { const s = sprint(); return s ? queueWords(ws, s) : null; }, null, { equals: sameWords });
  const task = () => { const id = words()?.taskId; return id ? ws.store.index().byId.get(id) : undefined; };
  const icon = <I n={words()?.icon ?? "help"} s={14} class={words()?.iconClass} />;
  const open = <Show when={task()}>{v => <A class="lnk" to={decisionRoute(v())}>Open task</A>}</Show>;
  const item: StripItem = { id: "queue", domId, icon, action: open, get tone() { return words()?.tone ?? "info"; }, get text() { return words()?.text ?? ""; }, get role() { return words()?.role; } };
  return () => words() ? item : null;
}

/** What automatic running means, kept out of the way of the work and one press from it. */
const queueHelp = (sprint: Sprint) => `${sprintModeDescription[sprint.runMode]}${sprint.runMode === "automatic" ? " Finishing on a different target branch still needs confirmation." : ""}${sprint.queueState === "running" ? " Pause queue stops new starts; the task already running keeps going." : ""}`;
export function QueueHelp(props: { sprint: Sprint }) {
  return <Show when={queueDrivenMode(props.sprint.runMode)}><Hint text={queueHelp(props.sprint)} label="the automatic queue" /></Show>;
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
  return <FieldEdit band label="Sprint title" value={sprintTitle(props.sprint)} off={ws.gate("rename_sprint")} inputClass="ws-title-in" save={title => ws.run({ type: "rename_sprint", sprintId: props.sprint.id, title })}
    closed={{ class: "cur trunc ws-title", text: saved => <>Sprint: {saved}</> }} />;
}

export function SprintActions(props: { sprint: Sprint; tab: SprintRoute["tab"] }) {
  const ws = useWs();
  const planning = () => props.sprint.planning !== "created";
  const closeWhy = createMemo(() => {
    const blocked = sprintCloseBlockers(ws.store.state(), props.sprint);
    return ws.gate("close_sprint") ?? (blocked.length ? `Close sprint is blocked by: ${blocked.map(b => `${b.title}: ${b.reason}`).join("; ")}.` : undefined);
  });
  // A shared-worktree sprint is merged into its target once, after every member finished onto its branch.
  const mergeable = () => props.sprint.checkout?.kind === "worktree" && props.sprint.checkout.shared === true && !props.sprint.merge && !props.sprint.archivedAt
    && props.sprint.planning === "created" && (() => { const members = ws.store.state().tasks.filter(t => t.sprintId === props.sprint.id && !t.archive); return members.length > 0 && members.every(t => t.status === "done"); })();
  const mergeSprint = () => {
    const target = props.sprint.targetBranch ?? "the target branch";
    ws.open({kind:"confirm",title:"Merge the sprint branch?",action:"Merge sprint",note:`All finished tasks are merged into ${target} as one commit, then the sprint worktree is removed. The branch stays in Git.`,
      go:async () => (await ws.run({type:"merge_sprint",sprintId:props.sprint.id,confirmed:true})).ok});
  };
  const close = () => {
    if (closeWhy()) return;
    ws.open({kind:"confirm",title:"Close this sprint?",action:"Close sprint",note:SPRINT_CLOSE_NOTE,
      go:async () => { const r = await ws.run({type:"close_sprint",sprintId:props.sprint.id,confirmed:true}); if (r.ok) ws.go({name:"archive",machine:ws.machine().id}); return r.ok; }});
  };
  const remove = () => {
    const id = props.sprint.id, projectId = props.sprint.projectId;
    const count = new Set([...props.sprint.taskIds,...ws.store.state().tasks.filter(t => t.sprintId === id).map(t => t.id)]).size;
    const note = `Permanently delete this sprint and all ${count} tasks? The queue and running work will be stopped. Only clean app-created worktrees are removed; other worktrees are kept and their paths shown. The project folder and Git branches are never deleted. This cannot be undone.`;
    // A refusal is shown once, on the run's own error line (with any kept worktree paths), not repeated in the body.
    // A deletion that started but still waits for a task's stop has happened in part: the dialog closes, the receipt
    // says what is left, and the sprint's own Deleting strip and Retry delete… carry it on.
    const command = {type:"delete_sprint",projectId,sprintId:id,confirmed:true} as const;
    ws.open({kind:"confirm",title:"Delete this sprint and its tasks?",action:"Delete sprint",danger:true,note,pending: () => ws.store.pendingRequest(command) ? REQUESTING.delete_sprint : undefined,
      go:async () => {
        const r = await ws.run(command);
        if (r.ok) ws.go({name:"home",machine:ws.machine().id});
        return r.ok || r.outcome === "partial_effect";
      }});
  };
  const deleteButton = () => <button class="btn btn-line" {...ws.needs("delete_sprint")} onClick={remove}>Delete sprint…</button>;
  return <Show when={!props.sprint.archivedAt} fallback={<Show when={props.sprint.deletion} fallback={<><span class="st st-idle">Closed sprint</span>{deleteButton()}</>}>
    <span class="st st-warn">Deleting</span><button class="btn btn-line" {...ws.needs("delete_sprint")} onClick={remove}>Retry delete…</button>
  </Show>}><>
    <Show when={props.tab === "summary"}><QueueButton sprint={props.sprint} line /></Show>
    <Show when={props.sprint.planning === "created"}><button class="btn btn-line tb-fold" onClick={() => ws.newItem("task", props.sprint.projectId, props.sprint.id)}><I n="plus" s={14} />New task</button></Show>
    <Show when={mergeable()}><button class="btn btn-primary tb-fold" {...ws.needs("merge_sprint")} onClick={mergeSprint}>Merge sprint branch…</button></Show>
    <Show when={props.sprint.merge}><span class="cap muted">Merged into {props.sprint.merge!.target}</span></Show>
    <button class="btn btn-line tb-fold" aria-disabled={closeWhy() ? "true" : undefined} data-why={closeWhy()} onClick={close}>Close sprint…</button>
    <button data-tooltip="" class="ib" popovertarget="m-sprint-menu" aria-label="Sprint actions"><I n="more" /></button>
    <Pop id="m-sprint-menu" class="pop-right" align="right" label="Sprint actions">
      <Show when={props.sprint.planning === "created"}><div class="tb-fold-only"><MenuItem icon="plus" label="New task" onSelect={() => ws.newItem("task", props.sprint.projectId, props.sprint.id)} /></div></Show>
      <Show when={planning() && props.sprint.planning !== "cancelled"}><MenuItem icon="x" label="Cancel planning" needs="cancel_planning" onSelect={() => ws.run({ type: "cancel_planning", sprintId: props.sprint.id })} /></Show>
      <Show when={props.sprint.planning === "cancelled" && !planningChangeRequired(props.sprint)}><MenuItem icon="play" label={props.sprint.planningFailure ? "Retry planning" : "Resume planning"} needs="resume_planning" onSelect={() => ws.run({ type: "resume_planning", sprintId: props.sprint.id })} /></Show>
      <MenuItem icon="archive" label="Close sprint…" disabled={closeWhy()} onSelect={close} />
      <MenuItem icon="trash" label="Delete sprint…" needs="delete_sprint" onSelect={remove} />
    </Pop>
  </></Show>;
}

/** What the owner asked of the sprint, as the head of its planning conversation. */
function SprintBrief(props: { sprint: Sprint }) {
  const ws = useWs();
  return <div class="brief"><ResultText breaks text={props.sprint.description} /><AttachChips files={props.sprint.attachments} open />
    <div class="meta"><Show when={ws.t("planningBadge")}><span>{ws.t("planningBadge")}</span></Show><Show when={props.sprint.description}><CopySource text={props.sprint.description} /></Show></div></div>;
}

/** The proposed plan is a line in the conversation, and the line opens the Plan tab. */
function PlanLine(props: { sprint: Sprint; onOpenPlan?: () => void }) {
  const s = () => props.sprint;
  return <Show when={s().plan}>{plan => <div class="answered sp-plan-line"><I n="layers" s={14} />
    <span class="grow">{s().planning === "created" && s().materializedVersion !== null ? `Tasks created from Plan v${s().materializedVersion}` : `Plan v${plan().version} proposed: ${plan().items.length} task${plan().items.length === 1 ? "" : "s"}`}</span>
    <Show when={props.onOpenPlan}>{open => <button class="btn btn-line" onClick={() => open()()}>Open plan</button>}</Show>
  </div>}</Show>;
}

const planningRoute = (machine: string, sprint: Sprint, tab: "planning" | "workers", worker?: string): Route =>
  ({ name: "sprint", machine, project: sprint.projectId, sprint: sprint.id, tab, ...(worker ? { worker } : {}) });

/** One conversation from the first frame. The planner supplies a runtime feed only once its linked row exists. */
function SprintConversation(props: {
  sprint: Sprint; taskId: string | null; hidden: boolean; changingRequest: Accessor<boolean>;
  onOpenPlan: () => void; onUnsent: (unsent: boolean) => void;
}) {
  const ws = useWs();
  const s = () => props.sprint;
  const model = createMemo(() => { const id = props.taskId; return id ? sessionModel(ws.store.index(), id) : null; });
  const now = useSessionClock();
  const onPick = (key: string) => ws.go(planningRoute(ws.machine().id, s(), "workers", key));
  const saved = (runtimeVisible: Accessor<boolean>) => {
    const session = () => ws.store.state().sessions.find(x => x.taskId === s().planningTaskId);
    const activeDecisions = () => model()?.session.decisions.filter(d => d.kind === "question" && (isOpen(d) || d.held)) ?? [];
    const asks = createMemo<ReturnType<typeof planningAsks>>(prev => {
      // Answer linked decisions in the runtime feed, never via a saved sprint question while that feed loads.
      if (activeDecisions().length) return [];
      return share(prev, planningAsks(s(), model() ? undefined : session()));
    }, []);
    const thread = createMemo(() => {
      const task = runtimeVisible() ? model() : null;
      const showingQuestion = asks().some(a => a.id === "planning")
        || !!task && activeDecisions().some(d => d.text === openPlanningQuestion(s())?.text);
      if (!task) return planningThread(s(), model() ? undefined : session(), showingQuestion);
      const rows = task.session.timeline;
      const notes = new Set(rows.flatMap(e => e.kind === "cortex" ? [e.text.trim()] : []));
      const fileNames = (files?: AttachmentInput[]) => JSON.stringify((files ?? []).map(attachmentName).sort());
      const messageKey = (text: string, files?: AttachmentInput[]) => JSON.stringify([text.trim(), fileNames(files)]);
      const messagesById = new Map(task.messages.map(m => [m.id, m]));
      const messages = rows.flatMap(e => {
        if (e.kind === "said") {
          // Match the message whose file chips the shared Conversation displays on this text row.
          const sent = task.messages.find(m => m.workerId === null && m.delivery !== "queued" && m.text.trim() === e.text.trim());
          return [messageKey(e.text, sent?.attachments)];
        }
        const sent = e.kind === "message" ? messagesById.get(e.messageId) : undefined;
        return sent ? [messageKey(sent.text, sent.attachments)] : [];
      });
      const decisionIds = new Set(rows.flatMap(e => e.kind === "decision" ? [e.decisionId] : []));
      const answered = task.session.decisions.flatMap(d => decisionIds.has(d.id) && d.kind === "question" && d.answer && !d.held && !d.withdrawn
        ? [{ question: d.text, answer: d.answer }] : []);
      return planningThread(s(), undefined, showingQuestion).filter(e => {
        if (e.kind === "note") return !!e.attachments?.length || !notes.has(e.text.trim());
        // Each visible runtime row replaces at most one saved entry. Older saved answers may not carry the question.
        if (e.kind === "message") {
          const at = messages.indexOf(messageKey(e.text, e.attachments));
          if (at < 0) return true;
          messages.splice(at, 1);
          return false;
        }
        if (e.kind === "qa") {
          const at = answered.findIndex(d => (!e.question || d.question === e.question) && d.answer.text === e.text
            && (!e.attachments?.length || fileNames(d.answer.attachments) === fileNames(e.attachments)));
          if (at < 0) return true;
          answered.splice(at, 1);
          return false;
        }
        return true;
      });
    });
    const answer = (answers: AskAnswer[]) => {
      const [first] = answers;
      if (!first) return undefined;
      const planned = first.ask.id !== "planning";
      return ws.run({ type: "answer_planning", sprintId: s().id, answer: first.text, attachments: first.attachments, now: ws.store.clock(),
        ...(planned ? { decisionId: first.ask.id, optionId: first.optionId } : {}),
        ...(planned && answers.length > 1 ? { answers: answers.map(a => ({ decisionId: a.ask.id, answer: a.text, optionId: a.optionId, attachments: a.attachments })) } : {}) });
    };
    const sentHere = () => ({ kind: "sprint" as const, sprintId: s().id });
    const pending = () => {
      const phase = s().planning, task = model();
      if (phase !== "planning" && phase !== "question") return null;
      if (task) return task.mode === "draft" ? "Planning is starting…" : null;
      if (ws.machine().connection !== "connected") return "Planning conversation is unavailable while the machine is offline.";
      if (s().planningTaskId) return "Loading the planning conversation…";
      return phase === "planning" ? "Planning is starting…" : null;
    };
    const tail = <>
        <Keyed each={thread()} by={e => e.key}>{e => <Switch>
          <Match when={e().kind === "note"}><div class="ai"><ResultText breaks text={e().text} /><AttachChips files={e().attachments} open /></div><MessageTools text={e().text} /></Match>
          <Match when={e().kind === "qa"}><Answered question={e().question ?? ""} answer={e().text} at={e().at} files={e().attachments} /></Match>
          <Match when={e().kind === "message"}><div class="mine"><ResultText breaks text={e().text} /><AttachChips files={e().attachments} open /></div><div class="mine-meta"><I n="check" s={12} />Sent to the planner<Show when={e().at !== null}><span>{clock(e().at)}</span></Show><CopySource text={e().text} /></div></Match>
          <Match when={e().kind === "event"}><div class="answered"><I n="check" s={14} /><span>{e().text}</span><Show when={e().at !== null}><span class="faint">{clock(e().at)}</span></Show></div></Match>
        </Switch>}</Keyed>
        <SentMessages place={sentHere()} />
        <PlanLine sprint={s()} onOpenPlan={props.onOpenPlan} />
        <Show when={asks().length > 0}><QuestionForm asks={asks()} needs="answer_planning" blocked={() => undefined} projectId={s().projectId} note={() => null} onSend={answer}
          waiting={ask => ask.at === null ? "" : clock(ask.at)} /></Show>
        <Show when={pending()}>{label => <div class="answered" role="status">{label()}</div>}</Show>
    </>;
    return { tail, count: () => thread().length + ws.store.sends.shown(sentHere()).length + asks().length + (s().plan ? 1 : 0) };
  };
  const sprintFeed = { brief: <SprintBrief sprint={s()} />, saved, onUnsent: props.onUnsent };
  return <PlanningPlace.Provider value={{ sprint: s, changingRequest: props.changingRequest }}>
    <TaskConversation model={model} now={now()} pick={model() ? firstWorker(model()!) : ""} onPick={onPick} hidden={props.hidden} sprintFeed={sprintFeed} />
  </PlanningPlace.Provider>;
}

/** The planning task's workers, read on its sprint as an ordinary task's Work details. */
function PlanningTaskWorkers(props: { sprint: Sprint; taskId: string; worker?: string }) {
  const ws = useWs();
  const model = createMemo<SessionModel | null>(prev => sessionModel(ws.store.index(), props.taskId) ?? prev ?? null);
  const now = useSessionClock();
  createBodyMark("tab", () => "workers");
  const onPick = (key: string) => ws.go(planningRoute(model()?.view.machine.id ?? ws.machine().id, props.sprint, "workers", key));
  return <Show when={model()}>{m => <PlanningPlace.Provider value={{ sprint: () => props.sprint }}>
    <TaskWorkers model={m} now={now()} pick={props.worker ?? firstWorker(m())} onPick={onPick} />
  </PlanningPlace.Provider>}</Show>;
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

const SAVING = "Saving the last change";
/** The owner's edits of a live graph: one at a time, each with the revision shown, and an honest word when it did not go through. */
type LinkEditor = GraphLinks & { move: (taskId: string, direction: "up" | "down") => Promise<boolean> };
function createLinkEditor(ws: ReturnType<typeof useWs>, sprint: Accessor<Sprint>, model: Accessor<LiveModel>): LinkEditor {
  const [busy, setBusy] = createSignal(false);
  const [notice, setNotice] = createSignal<string | null>(null);
  // A new read of the graph is a new picture: an old notice no longer describes it.
  createEffect(on(() => model().revision, () => setNotice(null), { defer: true }));
  type Edit = SprintGraphEdit extends infer E ? E extends unknown ? Omit<E, "expectedGraphRevision" | "sprintId"> : never : never;
  const send = async (edit: Edit) => {
    const m = model();
    if (busy() || !m.revision || m.why) return false;
    setBusy(true); setNotice(null);
    try {
      const result = await ws.store.editLiveGraph({ ...edit, sprintId: sprint().id, expectedGraphRevision: m.revision } as SprintGraphEdit);
      setNotice(editNotice(result));
      return result.outcome === "changed";
    } finally { setBusy(false); }
  };
  const linked = (p: string, d: string) => model().member(d)?.dependencies?.includes(p) ?? false;
  return {
    get why() { return model().why; },
    get busy() { return busy(); }, get notice() { return notice(); }, get queueNote() { return model().queueNote; },
    linked,
    problem: (p, d) => linked(p, d) ? dependentWhy(model(), d) : linkProblem(model(), d, p),
    words: (p, d) => linkWords(model(), p, d),
    save: (p, d, present) => send({ action: "set_dependency", dependentTaskId: d, predecessorTaskId: p, present }),
    move: (taskId, direction) => send({ action: "move_priority", taskId, direction }),
  };
}

/** A live task's links and priority in its details: each prerequisite with Remove, a picker to add one, and Move up or down. */
function LiveLinks(props: { row: PlanRow; model: LiveModel; editor: LinkEditor }) {
  const key = () => props.row.item.key;
  const no = (id: string) => props.model.items.findIndex(i => i.key === id) + 1;
  const name = (id: string) => { const n = no(id); return n ? `${n} ${itemTitle(props.model.items[n - 1])}` : "A deleted task"; };
  // Every saved prerequisite, also one no longer drawn (a deleted task), so a bad link can still be removed.
  const deps = () => props.model.member(key())?.dependencies ?? props.row.item.dependsOn;
  const why = () => props.editor.why ?? dependentWhy(props.model, key());
  const [adding, setAdding] = createSignal("");
  const options = () => props.model.items.filter(i => i.key !== key() && !deps().includes(i.key));
  const addWhy = () => adding() ? props.editor.problem(adding(), key()) : "Choose a task first.";
  const priority = () => props.model.priority.filter(id => props.model.member(id)?.editable);
  const place = () => placeWords(props.model, key());
  const add = async () => { if (!addWhy() && !props.editor.busy && await props.editor.save(adding(), key(), true)) setAdding(""); };
  return <div class="ws-live-links">
    <div class="ins-h ws-read-s"><span>Waits for</span></div>
    <Show when={props.model.member(key())?.dependencies !== null} fallback={<p class="cap muted">This task could not be read, so its links are unknown.</p>}>
      <Show when={deps().length} fallback={<p class="cap muted">Nothing. It can start first.</p>}>
        <ul class="ws-plain-list"><For each={deps()}>{d => <li class="row"><span class="grow trunc">{name(d)}</span>
          <button class="btn btn-line btn-sm" aria-disabled={why() || props.editor.busy ? "true" : undefined} data-why={why() ?? (props.editor.busy ? SAVING : undefined)}
            aria-label={`Remove the link: task ${props.row.no} will no longer wait for ${name(d)}`} onClick={() => { if (!why() && !props.editor.busy) void props.editor.save(d, key(), false); }}>Remove</button></li>}</For></ul>
      </Show>
    </Show>
    <Show when={!why()} fallback={<p class="cap muted">{why()}</p>}>
      <label class="fld"><span class="lab">Add a prerequisite</span>
        <select class="inp" value={adding()} onChange={e => setAdding(e.currentTarget.value)} aria-label={`Task ${props.row.no} waits for`}>
          <option value="">Choose a task</option>
          <For each={options()}>{i => <option value={i.key}>{name(i.key)}</option>}</For>
        </select>
      </label>
      <Show when={adding()}><p class="cap">{props.editor.words(adding(), key())}</p><Show when={addWhy()}>{w => <p class="cap st-warn" role="status">{w()}</p>}</Show>
        <Show when={props.editor.queueNote}>{q => <p class="cap muted">{q()}</p>}</Show></Show>
      <button class="btn btn-line btn-sm" aria-disabled={addWhy() || props.editor.busy ? "true" : undefined} data-why={addWhy() ?? (props.editor.busy ? SAVING : undefined)} onClick={() => void add()}>Add link</button>
    </Show>
    <div class="ins-h ws-read-s"><span>Priority among ready tasks</span></div>
    <Show when={place()}>{p => <p class="cap">{p()}</p>}</Show>
    <Show when={!why()} fallback={<p class="cap muted">Only a draft that has not started can move.</p>}>
      <div class="row">
        <button class="btn btn-line btn-sm" aria-disabled={priority()[0] === key() || props.editor.busy ? "true" : undefined} data-why={priority()[0] === key() ? "Already first among ready tasks" : props.editor.busy ? SAVING : undefined} onClick={() => { if (priority()[0] !== key()) void props.editor.move(key(), "up"); }}><I n="arrow-up" s={14} />Move up</button>
        <button class="btn btn-line btn-sm" aria-disabled={priority().at(-1) === key() || props.editor.busy ? "true" : undefined} data-why={priority().at(-1) === key() ? "Already last among ready tasks" : props.editor.busy ? SAVING : undefined} onClick={() => { if (priority().at(-1) !== key()) void props.editor.move(key(), "down"); }}><I n="arrow-down" s={14} />Move down</button>
      </div>
      <Show when={props.editor.queueNote}>{q => <p class="cap muted">{q()}</p>}</Show>
      <p class="cap faint">Priority orders tasks that are ready at the same time. Links still decide what waits.</p>
    </Show>
    <Show when={props.editor.notice}>{n => <p class="cap st-warn" role="status">{n()}</p>}</Show>
  </div>;
}

/**
 * A plan may hold very long tasks, so the list names them and this holds the full text. It scrolls by itself and is always
 * docked beside the plan; on a window with no room for both it takes the plan's place and `back` names the way back.
 */
function PlanItemPanel(props: { row: PlanRow; total: number; onClose: () => void; back?: string; live?: { model: LiveModel; editor: LinkEditor } }) {
  let close!: HTMLButtonElement;
  const back = document.activeElement as HTMLElement | null;
  const shut = () => { props.onClose(); if (back?.isConnected) back.focus({ preventScroll: true }); };
  const shown = useContext(ScreenActive);
  const onKey = (e: KeyboardEvent) => { if (e.key === "Escape" && shown() && !document.querySelector("dialog[open]")) { e.preventDefault(); shut(); } };
  document.addEventListener("keydown", onKey);
  onCleanup(() => document.removeEventListener("keydown", onKey));
  const ws = useWs();
  // A draft an applied sprint revision changed says which task changed it.
  const revisedBy = (id: string) => { const sp = ws.store.index().byId.get(id)?.task.sprintId; return sp ? changedBy(ws.store.liveGraph(sp)?.graph ?? null, id, t => ws.store.index().byId.get(t)?.task.title ?? "?") : null; };
  queueMicrotask(() => placeFocus(close, { preventScroll: true }));
  return <aside class="ws-read" aria-label="Task details">
    <SectionHead class="ws-read-h" lead={props.back ? <button class="btn btn-sm" onClick={shut}><I n="arrow-left" s={14} />{props.back}</button> : undefined} title={`Task ${props.row.no} of ${props.total}`}><button data-tooltip="Close task details · Esc" ref={close} class="ib ws-ib" aria-label="Close task details" onClick={shut}><I n="x" s={14} /></button></SectionHead>
    <div class="ws-read-b selectable">
      <h2 class="ws-read-t">{itemTitle(props.row.item)}</h2>
      <Show when={props.live && revisedBy(props.row.item.key)}>{words => <div class="cap muted">{words()}</div>}</Show>
      <dl class="kv">
        <dt>Depends on</dt><dd>{props.row.deps.length ? `Tasks ${props.row.deps.join(", ")}` : "Starts first"}</dd>
        <Show when={props.row.item.complexity}>{c => <><dt>Complexity</dt><dd>{COMPLEXITY_LABELS[c()] ?? c()}</dd></>}</Show>
        <Show when={planState(props.row)}>{w => <><dt>State</dt><dd class={w().tone}>{w().text}</dd></>}</Show>
      </dl>
      <div class="ws-read-d"><ResultText breaks text={props.row.item.description} /></div>
      <Show when={props.live}>{l => <LiveLinks row={props.row} model={l().model} editor={l().editor} />}</Show>
      <Show when={props.row.item.acceptance}><div class="ins-h ws-read-s"><span>Acceptance and checks</span></div><div class="ws-read-d"><ResultText breaks text={props.row.item.acceptance} /></div></Show>
    </div>
    <Show when={props.row.view}>{v => <ActionBar class="ws-read-f"><A class="btn btn-line" to={decisionRoute(v())}>Open task<I n="arrow-right" s={14} /></A></ActionBar>}</Show>
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
  /** A created sprint: its live graph, which of the two surfaces is shown, and the owner's link and priority edits. */
  live: Accessor<LiveModel>; surface: "live" | "plan"; setSurface: (surface: "live" | "plan") => void; editor: LinkEditor;
};
function Plan(props: PlanProps) {
  const ws = useWs();
  const s = () => props.sprint;
  const [picked, setPicked] = createSignal<string | null>(null);
  const [viewing, setViewing] = createSignal<number | null>(null);
  const [errors, setErrors] = createSignal<Record<string, string>>({});
  const past = () => viewing() === null ? null : s().history?.find(h => h.version === viewing()) ?? null;
  /** After creation the sprint opens on its tasks as they are now; the approved plan stays one press away, read only. */
  const liveOn = () => s().planning === "created" && props.surface === "live" && props.draft === null && viewing() === null;
  const items = () => liveOn() ? props.live().items : props.draft ?? past()?.items ?? s().plan?.items ?? [];
  const version = () => liveOn() ? undefined : past()?.version ?? s().plan?.version;
  // States belong only to the version the tasks were made from, so a past version or a draft shows none. Live rows are the
  // tasks themselves, keyed by task id; plan rows find their tasks by the ids the service proved.
  const rows = createMemo(() => liveOn() ? planRows(ws.store.index(), s(), items(), "tasks")
    : planRows(ws.store.index(), s(), items(), props.draft ? null : version() ?? null, props.live().created));
  /** The live list in the saved priority; numbers stay the roster's, so a move never renumbers a task. */
  const listRows = createMemo(() => {
    const r = rows();
    if (!liveOn()) return r;
    const at = new Map(props.live().priority.map((id, i) => [id, i]));
    return [...r].sort((a, b) => (at.get(a.item.key) ?? Infinity) - (at.get(b.item.key) ?? Infinity) || a.no - b.no);
  });
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
  const heading = () => liveOn() ? "Live sprint" : version() ? `${created() && !past() ? "Approved plan" : "Plan"} v${version()}` : "Plan";
  const title = () => `${heading()}${items().length ? ` · ${plural(items().length)}` : ""}`;
  const memoryKey = () => liveOn() ? "live" : `${version() ?? 0}${editing() ? ":draft" : ""}`;
  const approved = () => s().materializedVersion ?? s().plan?.version;
  const priority = createMemo(() => liveOn() ? props.live().priority.filter(id => props.live().member(id)?.editable) : []);
  const movable = (key: string) => liveOn() && !props.editor.why && !!props.live().member(key)?.editable;
  const move = (key: string, dir: "up" | "down") => { if (!props.editor.busy && priority()[dir === "up" ? 0 : priority().length - 1] !== key) void props.editor.move(key, dir); };
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
  return <section class="sp-plan" aria-label={liveOn() ? "Live sprint" : version() ? `Plan v${version()}` : "Plan"} ref={revealCutIn} hidden={props.hidden} data-view={graph() ? "graph" : "list"}>
    <Show when={!(props.full && graph())}><SectionHead class="sp-plan-h" lead={lead} title={heading()} count={items().length > 0 ? items().length : undefined}>
      <Show when={created() && !editing()}><div class="seg" role="group" aria-label="Sprint graph">
        <button aria-current={liveOn() ? "true" : "false"} onClick={() => { setViewing(null); props.setSurface("live"); }}>Live sprint</button>
        <button aria-current={liveOn() ? "false" : "true"} onClick={() => props.setSurface("plan")}>{approved() ? `Approved plan v${approved()}` : "Approved plan"}</button>
      </div></Show>
      <Show when={past() || editing() || (created() && !liveOn())}><span class="gv-tag">{past() ? "Past version · read only" : editing() ? "Draft · not saved" : "Read only"}</span></Show>
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
      <Show when={past()} fallback={<Show when={s().history?.length && !liveOn()}><button class="btn btn-sm" popovertarget="m-plan-history">Previous plan</button></Show>}>
        <button class="btn btn-sm" onClick={() => setViewing(null)}>Back to v{s().plan?.version}</button>
      </Show>
      <Pop id="m-plan-history" class="pop-right" align="right" label="Previous plans">
        <For each={[...(s().history ?? [])].reverse()}>{h => <MenuItem label={`Plan v${h.version}`} sub={`${h.items.length} tasks`} onSelect={() => setViewing(h.version)} />}</For>
      </Pop>
    </SectionHead></Show>
    <Show when={items().length > 0} fallback={<div class="note-panel"><span class="w500">No plan yet</span><span>Linked tasks appear here after the conversation.</span></div>}>
      <Show when={!graph()} fallback={<Show when={memoryKey()} keyed>{key => <GraphView items={items()} rows={rows()} title={title()} mode={editing() ? "draft" : past() ? "past" : liveOn() ? "live" : "plan"}
        links={key === "live" && props.live().source === "service" ? props.editor : undefined}
        created={created()} manual={s().runMode === "manual"} picked={pick()} onPick={setPicked} details={props.reading} onDetails={k => { setPicked(k); props.onRead(k); }}
        full={props.full} onFull={props.setFull} request={graphRequest} onOpenList={() => setView("list")}
        memory={props.memory} memoryKey={key} active={props.active} api={props.api} />}</Show>}>
      <div class="sp-scroll" ref={watchWidth}>
        <div class={`pt-row th${accepted() ? "" : " ws-plain"}`}><span>#</span><span>Task</span><Show when={accepted()}><span>Acceptance and checks</span></Show><span>{created() ? "State" : "Depends on"}</span></div>
        <Show when={editing()} fallback={<Keyed each={listRows()} by={r => r.item.key}>{r => <div class={`pt-row${accepted() ? "" : " ws-plain"}`} data-selected={selected() === r().item.key ? "true" : undefined} onClick={() => setPicked(r().item.key)}>
          <span class="no">{r().no}</span>
          <span class="ws-pt-t"><button class="tt trunc" aria-current={selected() === r().item.key ? "true" : undefined}>{itemTitle(r().item)}</button><button class="lnk ws-more-l" aria-label={liveOn() ? `Links and details of task ${r().no}` : `Read more about task ${r().no}`} aria-expanded={props.reading === r().item.key} onClick={e => { e.stopPropagation(); setPicked(r().item.key); props.onRead(r().item.key); }}>{liveOn() ? "Links" : "Read more"}</button>
            <Show when={movable(r().item.key)}><span class="ws-prio" role="group" aria-label={`Priority of task ${r().no}`}>
              <button data-tooltip="" class="ib ws-ib" aria-label={`Move task ${r().no} up`} aria-disabled={priority()[0] === r().item.key || props.editor.busy ? "true" : undefined} data-why={priority()[0] === r().item.key ? "Already first among ready tasks" : props.editor.busy ? SAVING : undefined} onClick={e => { e.stopPropagation(); move(r().item.key, "up"); }}><I n="arrow-up" s={14} /></button>
              <button data-tooltip="" class="ib ws-ib" aria-label={`Move task ${r().no} down`} aria-disabled={priority().at(-1) === r().item.key || props.editor.busy ? "true" : undefined} data-why={priority().at(-1) === r().item.key ? "Already last among ready tasks" : props.editor.busy ? SAVING : undefined} onClick={e => { e.stopPropagation(); move(r().item.key, "down"); }}><I n="arrow-down" s={14} /></button>
            </span></Show><Show when={r().item.complexity}>{c => <span class="cap faint">{COMPLEXITY_LABELS[c()] ?? c()} complexity</span>}</Show></span>
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
        <Show when={!liveOn()}><Graph items={items()} selected={pick()} onPick={setPicked} width={paneW()} onOpen={() => setView("graph")} /></Show>
      </div>
      <ActionBar class="sp-foot" hint="Every task waits for your final confirmation.">
        <Show when={!created() && !past()}><button class="btn btn-line btn-sm" aria-disabled={props.request.why ? "true" : undefined} data-why={props.request.why ?? undefined} onClick={() => { if (!props.request.why) props.request.run(); }}>Request changes</button></Show>
        <Show when={selectedRow()?.view}>{v => <>
          <Show when={startable()}><button class="btn btn-line btn-sm" {...ws.needs("start_task")} onClick={() => ws.run({ type: "start_task", taskId: v().id, now: ws.store.clock() })}><RequestLabel pending={ws.store.pendingTask("start_task", v().id)} text={REQUESTING.start_task}>Start</RequestLabel></button></Show>
          <A class="btn btn-line btn-sm" to={decisionRoute(v())}>Open task</A>
        </>}</Show>
      </ActionBar>
      </Show>
    </Show>
  </section>;
}

/** Conversation is always available; Workers appears when the linked planner row is loaded. */
function Tabs(props: { route: SprintRoute; sprint: Sprint }) {
  const ws = useWs();
  const at = (tab: SprintRoute["tab"]): Route => ({ ...props.route, tab, worker: undefined });
  const board = (): Route => ({ name: "project", machine: props.route.machine, project: props.route.project, view: "board", scope: { kind: "sprint", id: props.sprint.id } });
  const task = () => planningTaskOf(ws.store.state(), props.sprint);
  return <><Toolbar small class="tabs" mid={<div class="ws-tablist" role="tablist" aria-label="Sprint views">
    <A class="tab" role="tab" to={at("planning")} aria-selected={props.route.tab === "planning" || (props.route.tab === "workers" && !task()) ? "true" : "false"}>Conversation</A>
    <Show when={task()}><A class="tab" role="tab" to={at("workers")} aria-selected={props.route.tab === "workers" ? "true" : "false"}>Workers</A></Show>
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
    <span><Show when={view().next ?? view().suggested} fallback="Manual: start each ready task yourself.">{v => <>Manual: task {props.sprint.taskIds.indexOf(v().id) + 1} is {view().ordered ? "suggested next" : "ready"}. Start it when you want, starting one never starts its neighbours.</>}</Show></span>
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
  // ---------- The live graph: read while this page shows a created sprint, and again whenever what it shows changes ----------
  watchLiveGraphs(ws.store, () => created() ? [s()] : []);
  const live = createMemo(() => liveModel(ws.store.index(), s(), ws.store.liveGraph(s().id)));
  const editor = createLinkEditor(ws, s, live);
  const [surface, setSurface] = createSignal<"live" | "plan">("live");
  const liveShown = () => created() && surface() === "live" && props.route.tab === "plan";
  // A different surface is different rows: what Details read belongs to the one left.
  createEffect(on(surface, () => setReading(null), { defer: true }));
  const cancelled = () => s().planning === "cancelled";
  const failure = createMemo(() => planningFailureDetails(s()));
  const warning = () => activePlanningWarning(s());
  const [editingRequest, setEditingRequest] = createSignal(false);
  const changingRequest = () => editingRequest() && cancelled() && !!failure();
  createEffect(() => { if (!cancelled() || !failure()) setEditingRequest(false); });
  const createBlocked = () => !s().plan ? "No plan yet"
    : draft() ? "Finish editing the plan first"
    : s().planning === "question" ? "Answer the open question first"
    : s().planning === "planning" ? "Still working on the plan"
    : cancelled() ? (planningChangeRequired(s()) ? "Change the request first" : failure() ? "Retry planning first" : "Resume planning first") : null;
  const create = () => { if (!createBlocked()) ws.run({ type: "materialize_plan", sprintId: s().id, version: s().plan!.version, now: ws.store.clock() }); };
  const n = () => s().plan?.items.length ?? 0;
  // The toolbar has no room for words: why its controls are off is said by the strips under it, which they name.
  const why = createUniqueId(), queueWhy = createUniqueId();
  const queueStrip = createQueueStrip(() => created() ? s() : null, queueWhy);
  const gapStrip = createGapStrip(s);
  const info = () => planningTaskOf(ws.store.state(), s())
    ? "Sprint planning. The planner studies the project read-only, with read-only workers, and proposes a plan; nothing is changed and no task runs until you create the tasks and start them."
    : "Sprint planning. The planning conversation runs on the chosen model; no task runs until you create the tasks and start them.";
  const planning: StripItem = { id: "planning", tone: "info", domId: why, icon: <I n="layers" s={14} class="faint" />,
    get text() { const r = createBlocked(); return r ? `Create tasks: ${r}. ${info()}` : info(); },
    action: <button class="btn btn-sm" {...ws.needs("cancel_planning")} onClick={() => ws.run({ type: "cancel_planning", sprintId: s().id })}>Cancel planning</button> };
  const retry = () => ws.run({ type: "resume_planning", sprintId: s().id });
  const stopped: StripItem = { id: "cancelled", tone: "info", domId: why, readable: true, icon: <I n="x" s={14} class="faint" />,
    text: `${stoppedPlanningText} ${savedPlanningText}`,
    action: <button class="btn btn-sm" {...ws.needs("resume_planning")} onClick={retry}>Resume planning</button> };
  const ownerStopped = () => failure()?.kind === "owner_stopped";
  const changeRequired = () => planningChangeRequired(s());
  const replaceRequest = () => planningRequestInvalid(s());
  const changeCommand = () => replaceRequest() ? "replace_planning_request" : "plan_message";
  const [requestEdit, setRequestEdit] = createSignal<{ original: string; diagnostic: string } | null>(null);
  const [description, setDescription] = createSignal("");
  const [savingRequest, setSavingRequest] = createSignal(false);
  const requestWhy = () => ws.gate("replace_planning_request")
    ?? (!replaceRequest() || !cancelled() ? "The sprint is no longer waiting for a shorter request."
      : !description().trim() ? "Write a request first."
      : description().trim() === requestEdit()?.original.trim() ? "Shorten the original request first." : undefined);
  const closeRequest = () => { if (!savingRequest()) setRequestEdit(null); };
  const saveRequest = async () => {
    if (savingRequest() || requestWhy()) return;
    setSavingRequest(true);
    try {
      const receipt = await ws.run({ type: "replace_planning_request", sprintId: s().id, description: description().trim() });
      if (receipt.ok) setRequestEdit(null);
    } finally { setSavingRequest(false); }
  };
  const failed: StripItem = { id: "planning-failure", domId: why,
    get readable() { return replaceRequest() || !!failure()?.kind; },
    get tone() { return ownerStopped() ? "info" : "fail"; },
    get role() { return ownerStopped() ? "status" : "alert"; },
    get icon() { return ownerStopped() ? <I n="x" s={14} class="faint" /> : <I n="alert" s={14} />; },
    get text() {
      const f = failure();
      if (!f) return;
      const reason = f.kind ? f.text : f.step ? `${f.step}: ${f.text}` : f.text;
      const retryEffect = ownerStopped() ? " Retry starts a new planning task." : "";
      const changeEffect = replaceRequest() || !f.kind ? "" : changeRequired() ? " Send an amendment to start planning again."
        : " Change request lets you send an amendment before starting again.";
      return `${reason}${retryEffect}${changeEffect} ${savedPlanningText}`;
    },
    action: <>
      <Show when={!changeRequired()}><button class="btn btn-sm" {...ws.needs("resume_planning")} onClick={retry}>Retry planning</button></Show>
      <button class={changeRequired() ? "btn btn-primary btn-sm" : "btn btn-sm"} {...ws.needs(changeCommand())} onClick={() => {
        if (ws.gate(changeCommand())) return;
        if (replaceRequest()) {
          const original = s().description;
          setDescription(original);
          setRequestEdit({ original, diagnostic: failure()!.text });
          return;
        }
        setEditingRequest(true);
        requestChanges();
      }}>Change request</button>
    </> };
  const warned: StripItem = { id: "planning-warning", tone: "warn", role: "status", icon: <I n="alert" s={14} />,
    get text() { return warning()?.text; } };
  const line: StripItem = { id: "created", tone: "info", icon: <I n="play" s={14} class="faint" />, text: <CreatedLine sprint={s()} /> };
  // A live graph read only in part is said, and editing stays off until it reads completely; Retry only reads again.
  const unreadStrip: StripItem = { id: "live-unread", tone: "warn", role: "status", icon: <I n="alert" s={14} />,
    get text() { const m = live(); return m.stale ? `Live graph not current: ${m.stale} The links shown are from the last read; editing is off.` : `Live graph read incomplete: ${m.unread} task${m.unread === 1 ? "" : "s"} could not be read. They are shown as placeholders and links cannot be changed.`; },
    action: <button class="btn btn-sm" onClick={() => void ws.store.readLiveGraph(s().id, null, true)}>Retry</button> };
  const divergedStrip: StripItem = { id: "live-diverged", tone: "info", role: "status", icon: <I n="layers" s={14} class="faint" />,
    get text() { return `${live().divergence ?? ""} The approved plan stays as approved.`; },
    action: <button class="btn btn-sm" onClick={() => setSurface("plan")}>Show approved plan</button> };
  // Revisions a sprint task proposed that wait for the owner: one card each, with Apply and Reject.
  // Readable: the card (its changes and the Apply and Reject buttons) is shown whole, never clamped to the strip's two lines.
  const revisionStrip: StripItem = { id: "sprint-revisions", tone: "info", role: "status", readable: true, icon: <I n="layers" s={14} class="faint" />,
    get text() { return <SprintRevisions sprint={s()} graph={ws.store.liveGraph(s().id)?.graph ?? null} />; } };
  const revisionStrips = (): StripItem[] => (ws.store.liveGraph(s().id)?.graph?.revisions ?? []).some(r => r.status === "awaiting_owner") ? [revisionStrip] : [];
  const liveStrips = (): StripItem[] => !liveShown() ? [] : [
    ...(live().source === "service" && (live().unread > 0 || live().stale) ? [unreadStrip] : []),
    ...(live().divergence ? [divergedStrip] : []),
  ];
  const status = createMemo<StripItem[]>(() => created()
    ? [line, ...(gapStrip() ? [gapStrip()!] : []), ...revisionStrips(), ...liveStrips(), ...(queueStrip() ? [queueStrip()!] : [])]
    : cancelled() ? [failure() ? failed : stopped] : warning() ? [warned, planning] : [planning]);

  // ---------- Space: the chat and the plan are tabs; in the plan, one yield rule and Full view ----------
  const panels = ws.panels;
  const at = (tab: SprintRoute["tab"]): Route => ({ ...props.route, tab, worker: undefined });
  // Workers requires a real linked task. Conversation remains mounted independently of that link.
  const planningTask = createMemo(() => planningTaskOf(ws.store.state(), s()));
  const workersTab = () => props.route.tab === "workers" && !!planningTask();
  /** Conversation or Workers: everything but the plan, each with the whole width. */
  const chatTab = () => props.route.tab !== "plan";
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
    const input = () => body.querySelector<HTMLTextAreaElement>('section[aria-label="Conversation"] .dock textarea');
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
  // The Workers tab's activity and its worker composer need the height too.
  const lane = () => fullOn() || ((onGraph() || workersTab()) && winH() < 800) ? status().filter(blocks) : status();
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
            <span class="st st-ok l-compact-hide"><I n="check-circle" s={12} />{s().taskIds.length} task{s().taskIds.length === 1 ? "" : "s"} · Plan v{s().materializedVersion} approved</span>
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
    <Modal open={!!requestEdit()} onClose={closeRequest} label="Edit original request">
      <div class="dlg-h"><h2>Edit original request</h2></div>
      <div class="dlg-b">
        <p class="cap muted">{requestEdit()?.diagnostic}</p>
        <label class="fld"><span class="lab">Original request</span>
          <textarea class="inp" rows="8" data-autofocus value={description()} readOnly={savingRequest()}
            ref={el => { autoGrow(el, description); queueMicrotask(() => { el.setSelectionRange(0, 0); el.scrollTop = 0; }); }}
            onInput={e => setDescription(e.currentTarget.value)} />
        </label>
      </div>
      <ActionBar place="dialog" hint={savingRequest() ? "Saving the request…" : requestWhy()}>
        <button class="btn btn-line" disabled={savingRequest()} onClick={closeRequest}>Cancel</button>
        <button class="btn btn-primary" disabled={savingRequest() || !!requestWhy()} onClick={() => void saveRequest()}>Save and retry</button>
      </ActionBar>
    </Modal>
    <div ref={body} class="sp-body" data-reading={details() ? "true" : undefined} data-full={fullOn() ? "" : undefined} style={{ "grid-template-columns": columns() }}>
      <SprintConversation sprint={s()} taskId={planningTask()} hidden={!chatTab() || workersTab()} changingRequest={changingRequest}
        onUnsent={setUnsent} onOpenPlan={() => ws.go(at("plan"))} />
      <Show when={workersTab() && planningTask()} keyed>{task => <PlanningTaskWorkers sprint={s()} taskId={task} worker={props.route.worker} />}</Show>
      <Plan sprint={s()} draft={draft()} setDraft={setDraft} reading={reading()} onRead={key => { if (key) setOpened("details"); setReading(key); }} rows={setRows}
        hidden={chatTab() || (space().replaced && details())} full={fullOn()} setFull={setFull} conv={conv}
        request={request} memory={memory} active={shown} api={setApi} live={live} surface={surface()} setSurface={setSurface} editor={editor} />
      <Show when={details()}><PlanItemPanel row={read()!} total={rows().length} back={space().replaced ? (graphOn() ? "Back to graph" : "Back to list") : undefined} onClose={() => setReading(null)}
        live={liveShown() && !draft() ? { model: live(), editor } : undefined} /></Show></div>
  </Page>;
}

function SummaryTab(props: { route: SprintRoute; sprint: Sprint }) {
  const ws = useWs();
  // The revision history comes with the live graph.
  watchLiveGraphs(ws.store, () => [props.sprint]);
  const data = createMemo(() => sprintSummary(ws.store.index(), props.sprint));
  const gap = createMemo(() => sprintGap(ws.store.index(), props.sprint));
  const cols = "18px minmax(0,2fr) 150px minmax(0,1.4fr) minmax(0,1fr)";
  const planning = (): Route => ({ ...props.route, tab: "planning" });
  const gapStrip = createGapStrip(() => props.sprint);
  const queueStrip = createQueueStrip(() => props.sprint);
  // An unfinished deletion says what is left and what it waits for; its retry is the header's Retry delete.
  // One strip for the life of the page: a machine read changes its words, never the strip.
  const deleting: StripItem = { id: "deletion", tone: "warn", role: "status", icon: <I n="hourglass" s={14} class="st-warn" />, get text() { return sprintView(ws.store.index(), props.sprint).state; } };
  const deletion = (): StripItem[] => props.sprint.deletion ? [deleting] : [];
  const status = createMemo<StripItem[]>(() => [...deletion(), ...(data().rows.length > 0 && gapStrip() ? [gapStrip()!] : []), ...(queueStrip() && !props.sprint.deletion ? [queueStrip()!] : [])]);
  return <Page class="l-work sp-page" scroll="none" head={<Tabs route={props.route} sprint={props.sprint} />} status={<StatusLane items={status()} />}>
    <div class="page"><div class="page-in">
      <div class="sum-grid">
        <div><div class="v">{data().ready}</div><div class="l">Ready to finish</div></div>
        <div><div class="v">{data().working}</div><div class="l">Working</div></div>
        <div><div class="v">{data().needs}</div><div class="l">Needs you</div></div>
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
      <SprintRevisions sprint={props.sprint} graph={ws.store.liveGraph(props.sprint.id)?.graph ?? null} history />
      <div class="cap faint">This is an index of real results. A check that did not run is shown as not run, never as a pass.</div>
    </div></div>
  </Page>;
}

/** Chat, Plan and Summary. The Board tab is the project board scoped to this sprint. */
export function SprintPage(props: { route: SprintRoute; summary: ProjectSummary }) {
  const ws = useWs();
  const sprint = useSprint(() => props.route.sprint);
  // A sprint not yet in the model is loading until a confirmed read says otherwise: only then is it missing.
  const loading = () => ws.store.sprintPending(props.route.sprint, props.route.project);
  return <Show when={sprint()} fallback={<Show when={!loading()} fallback={<div class="ws-skel-pane" aria-busy="true"><Skeleton kind="conversation" label="Loading the sprint…" /></div>}><div class="note-panel"><span class="w500">Sprint not found</span></div></Show>}>{s =>
    <ResultScope.Provider value={() => ({ projectId: s().projectId, taskId: planningTaskOf(ws.store.state(), s()) ?? undefined, projectCode: true })}>
      <Show when={!s().archivedAt} fallback={<SummaryTab route={props.route} sprint={s()} />}><Show when={props.route.tab !== "summary"} fallback={<SummaryTab route={props.route} sprint={s()} />}><PlanningTab route={props.route} sprint={s()} summary={props.summary} /></Show></Show>
    </ResultScope.Provider>
  }</Show>;
}
