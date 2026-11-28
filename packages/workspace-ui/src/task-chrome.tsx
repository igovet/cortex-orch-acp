import { createEffect, createMemo, createSignal, For, on, Show, type JSX } from "solid-js";
import { capabilityOf, taskRenamable, type Task } from "@cortex/workspace-domain";
import { asOf, automaticAcceptanceHold, checkCounts, clock, dayClock, engineLimits, humanStatus, nothingToCommit, sameBranch, windowTitle, sprintModeLabel, sprintTitle, type TaskView } from "./selectors";
import { useWs } from "./ctx";
import { taskActionsFor } from "./task-actions-model";
import { TaskMenuButton } from "./task-menu-trigger";
import { REQUESTING, RequestLabel, RequestStatus } from "./request-feedback";
import { FieldEdit } from "./field-edit";


import { A, FactLine, I, Keyed, Modal, Pop } from "./kit";
import { ActionBar, revealWhenCut } from "./layout";
import { behindTarget, conflictLine, finishBlocked, mergeControls, taskCheckout } from "./review-model";
import { pauseSettingsPage, checkedResult, checksOf, checkWhy, closeBlocked, closeConsequences, contextLine, editedFiles, externalDoneBlocked, externalExitPending, languageLabel, modeLabel, pausedExternalClose, pausedExternalCloseBlocked, resumeState, sessionModel, stateChip, stoppedAt, stopLabel, taskSettings, tokens, unconfirmedState, usesMachineRoute, workerModel, workerState, workersOutcome, type SessionModel } from "./session-model";
import { CHECK_ICON, VERIFICATION } from "./worker";
import { archiveBlocked, ARCHIVE_STOP_UNCONFIRMED } from "./git-actions-model";
import { spendRows, StallBadge, useLiveSpend } from "./task-spend";
import { LoadingStrip } from "./skeleton";

const useModel = (id: () => string) => {
  const ws = useWs();
  return createMemo(() => sessionModel(ws.store.index(), id()));
};

/** The crumb is the title: a click renames it in place, started or not, and a manual title always wins over a late generated one. */
export function taskTitle(task: Pick<Task, "title" | "titleState" | "description">): string {
  return task.titleState === "failed" ? task.description.split("\n")[0].slice(0, 60) : task.title;
}

export function TaskTitle(props: { view: TaskView }) {
  const ws = useWs();
  const title = () => props.view.task.title;
  const shown = () => taskTitle(props.view.task);
  return <Show when={taskRenamable(props.view.task)} fallback={<span ref={revealWhenCut} class="cur trunc ws-title" data-whole="">{shown()}</span>}>
    <FieldEdit band label="Task title" value={title()} off={ws.gate("rename_task")} inputClass="ws-title-in" save={text => ws.run({ type: "rename_task", taskId: props.view.id, title: text })}
      closed={{ class: "cur trunc ws-title", text: saved => saved === title() ? shown() : saved, start: () => title().startsWith("Title unavailable") || props.view.task.titleState === "pending" ? "" : title() }} />
  </Show>;
}

/** Where the task works and where it merges. The target stays changeable until merge. */
export function BranchChip(props: { view: TaskView }) {
  const ws = useWs();
  const m = useModel(() => props.view.id);
  const t = () => props.view.task;
  const worktree = () => t().checkout.kind === "worktree";
  const terminalReason = () => worktree() ? "The terminal opens in the project folder only. Open it from the project board." : undefined;
  const uncommitted = () => taskCheckout(ws.store.state(), t())?.files.length ?? m()?.session.changes?.files;
  return <>
    <button class="chip branch-chip" popovertarget="m-wt" aria-label={`${worktree() ? "Worktree" : "Project folder"}${t().checkout.branch ? ` ${t().checkout.branch}` : ""}`}><I n="branch" s={12} /><span class="w-wide">{worktree() ? "Worktree" : "Project folder"}</span><Show when={t().checkout.branch}><span data-tooltip-cut="" class="mono trunc" style={{ "max-width": "170px" }}>{t().checkout.branch}</span></Show></button>
    <Pop id="m-wt" class="pop-anchor" align="left" label="Checkout" style={{ width: "300px", padding: "10px 12px" }}>
      <div class="st st-idle" style={{ "margin-bottom": "8px" }}><I n="branch" s={12} />{worktree() ? "Separate worktree" : "Project folder"}</div>
      <dl class="kv">
        <Show when={t().checkout.branch}><dt>Branch</dt><dd class="mono">{t().checkout.branch}</dd></Show>
        <Show when={m()?.session.base}>{b => <><dt>Base</dt><dd class="mono">{b().branch}</dd></>}</Show>
        <Show when={props.view.target}><dt>{sameBranch(t(), props.view.target) ? "Target branch" : "Merge into"}</dt><dd class="mono">{props.view.target}</dd></Show>
        <Show when={uncommitted() !== undefined}><dt>Uncommitted</dt><dd>{uncommitted() ? `${uncommitted()} files` : "None"}</dd></Show>
      </dl>
      <Show when={ws.terminalAvailable()}>
        <div class="row" style={{ "margin-top": "10px" }}>
          <button class="btn btn-line btn-sm" aria-disabled={worktree() ? "true" : undefined} aria-describedby={worktree() ? "worktree-terminal-reason" : undefined} data-why={terminalReason()} onClick={e => {
            if (worktree()) return;
            (e.currentTarget.closest("[popover]") as HTMLElement).hidePopover?.();
            if (!ws.panels.terminalOpen()) ws.toggleTerminal();
          }}>{worktree() ? "Open worktree terminal" : "Open terminal here"}</button>
        </div>
        <Show when={terminalReason()}>{reason => <p id="worktree-terminal-reason" class="cap muted">{reason()}</p>}</Show>
      </Show>
    </Pop>
  </>;
}

/** The strip on the task page that says why the main action in the title band is off. */
export const taskWhyId = (id: string) => `task-why-${id}`;
/** Why the main action in the title band is off, or null. The title band has no room for it, so the task page says it once. */
export function titleBlock(m: SessionModel): string | null {
  const v = m.view, t = v.task;
  if (t.status === "ready_to_merge" && m.mode === "review" && !t.integrationFacts?.alreadyInTarget) {
    if (sameBranch(t, v.target)) return t.integrationFacts?.commitAvailable ? finishBlocked(t) : null;
    return v.project.hasGit !== false ? finishBlocked(t) : closeBlocked(m);
  }
  if (m.mode === "ended") return closeBlocked(m);
  if (m.mode === "closed") return archiveBlocked(t, v.machine) ?? null;
  return m.mode === "paused" ? closeBlocked(m) : null;
}

/**
 * The task's one next step that is not on the screen already (Answer, Review changes, a closing in Ready to merge, Continue),
 * the Git controls and the task menu. Its state reads in the strip under the tabs, the sidebar and the composer; Start, Stop
 * and Resume are the composer's action button and stay in the menu. Offline and unconfirmed tasks offer nothing that could mislead.
 */
export function TaskActions(props: { view: TaskView; git?: JSX.Element; place?: "conversation" }) {
  const ws = useWs();
  const model = useModel(() => props.view.id);
  const actions = createMemo(() => taskActionsFor(props.view, ws));
  const action = (id: string) => actions().find(a => a.id === id);
  const task = () => props.view.task;
  const canMessage = () => ws.can(capabilityOf("send_message"));
  const closePrimary = () => model()?.mode === "ended" || (model()?.mode === "review" && task().status === "ready_to_merge"
    && (task().integrationFacts?.alreadyInTarget || (sameBranch(task(), props.view.target) ? !task().integrationFacts?.commitAvailable : props.view.project.hasGit === false)));
  const closeLabel = () => model()?.mode === "ended" || task().integrationFacts?.alreadyInTarget
    || (sameBranch(task(), props.view.target) && nothingToCommit(task())) ? "Close task…" : "Close as is…";
  const Button = (p: { id: string; line?: boolean; css?: string; label?: string; icon?: boolean }) =>
    <Show when={action(p.id)}>{a => <button class={`btn ${p.line ? "btn-line" : "btn-primary"}${p.css ? ` ${p.css}` : ""}`}
      aria-label={p.label ?? a().label} aria-disabled={a().why ? "true" : undefined}
      aria-describedby={a().why ? taskWhyId(props.view.id) : undefined} data-why={a().why} data-task-action={a().id}
      onClick={() => void a().run()}>
      <Show when={p.icon}><I n={a().icon} s={12} /></Show>
      <Show when={p.id === "start"} fallback={<span>{p.label ?? a().label}</span>}>
        <RequestLabel pending={ws.store.pendingTask("start_task", props.view.id)} text={REQUESTING.start_task}>Start</RequestLabel>
      </Show>
    </button>}</Show>;
  const conversation = () => {
    const route = ws.router.route();
    return route.name === "task" && route.task === props.view.id && !route.tab && !route.live;
  };
  const next = () => <>
    <Show when={closePrimary()}><Button id="close" label={closeLabel()} /></Show>
    <Button id="rework" line css="tb-rework" icon />
    <Button id="continue" icon />
    <Show when={!canMessage()}>
      <Show when={model()?.mode === "live" && task().status === "working"}><Button id="stop" line label={stopLabel(ws.can(capabilityOf("continue_task")))} icon /></Show>
      <Show when={action("resume")?.enabled}><Button id="resume" icon /></Show>
      <Show when={model()?.mode === "draft"}><Button id="start" icon /></Show>
    </Show>
  </>;
  if (props.place === "conversation") {
    return <Show when={closePrimary() || action("rework") || action("continue") || !canMessage()}>
      <ActionBar place="card" label="Task actions">{next()}</ActionBar>
    </Show>;
  }
  return <>
    <StallBadge task={task()} />
    <Button id="answer" />
    <Button id="needed" />
    <Show when={!conversation()}>
      <Show when={action("review")}>{a => <A class="btn btn-primary" data-task-action={a().id}
        to={{ name: "task", machine: props.view.machine.id, task: props.view.id, tab: "changes", sub: "diff" }}>{a().label}</A>}</Show>
      {next()}
    </Show>
    <Show when={!task().archive}><Button id="archive" line css="tb-archive" icon /></Show>
    {props.git}
    <TaskMenuButton view={props.view} label="Task actions" />
  </>;
}

/** A row with nothing to say is left out, never shown as an empty line. */
// Keyed by each row's label: the rows are built afresh on every read, and a row rebuilt for nothing drops the text the owner selected in it.
const Kv = (props: { rows: [string, string, boolean?][] }) => {
  // A label shown twice (two engines' windows of one name) keeps one row each.
  const rows = () => { const seen = new Map<string, number>(); return props.rows.filter(r => r[1]).map(r => { const n = seen.get(r[0]) ?? 0; seen.set(r[0], n + 1); return { key: `${r[0]}#${n}`, r }; }); };
  return <dl class="kv"><Keyed each={rows()} by={x => x.key}>{x => <><dt>{x().r[0]}</dt><dd class={x().r[2] ? "mono" : undefined}>{x().r[1]}</dd></>}</Keyed></dl>;
};
function SessionPanel(props: { m: SessionModel; pick: string | null }) {
  const ws = useWs();
  const m = () => props.m;
  const t = () => m().view.task;
  const spend = useLiveSpend(t);
  const taskSpendRows = createMemo(() => spendRows(spend()));
  // Rows carry only what is known: a figure nobody reported is left out, and a section with no rows goes with it.
  const usage = (): [string, string][] => {
    const s = m().session, context = contextLine(s.context), rows: [string, string][] = [];
    if (context) rows.push(["Context", context]);
    if (s.tokens !== null) rows.push(["Task tokens", tokens(s.tokens)]);
    if (s.planUsage) rows.push(["Plan", `${s.planUsage.percent}%, snapshot ${clock(s.planUsage.observedAt)}`]);
    return rows;
  };
  // Only reported windows, each by itself; a stale snapshot says when it was taken.
  const limitRows = () => limits().rows.flatMap(r => r.windows.flatMap((w): [string, string][] => {
    if (w.usedPercent === null) return [];
    const seen = asOf(w.observedAt, ws.store.clock());
    return [[windowTitle(r, w), `${w.usedPercent}% of this week${w.resetAt ? `, resets ${dayClock(w.resetAt)}` : ""}${seen?.stale ? `, ${seen.text.toLowerCase()}` : ""}`]];
  }));
  const chip = () => stateChip(m());
  const settings = () => taskSettings(m());
  const checks = createMemo(() => checksOf(m()));
  const counts = () => checkCounts(checks());
  const limits = createMemo(() => engineLimits(ws.store.state(), m().view.machine));
  const open = () => m().open.length;
  const paused = () => m().mode === "paused" || m().mode === "stopping";
  const selected = () => m().workers.find(w => w.id === props.pick) ?? null;
  const shownChecks = () => checks().filter(c => c.outcome !== "passed").slice(0, 5);
  const hiddenChecks = () => checks().filter(c => c.outcome !== "passed").length - shownChecks().length;
  const checkout = createMemo(() => taskCheckout(ws.store.state(), t()));
  const blocks = () => ws.store.state().tasks.filter(x => x.dependencies.includes(t().id)).length;
  const note = () => {
    if (ws.store.mode === "real" && t().conflict !== "none") {
      const a = mergeControls(t());
      return a.uncertain || (a.inspect && !a.finish) ? "Inspect the merge before making changes."
        : a.active ? "Handoff recorded. Stop requests cancellation; manual actions wait for confirmation."
        : "Open Changes for the available merge actions.";
    }
    if (m().view.automaticFact) return "";
    if (m().view.ownerIntegration) return humanStatus(m().view.ownerIntegration!.reason);
    if (t().conflict === "resolving") return `Handed over by you${t().merge?.handedAt != null ? ` at ${clock(t().merge!.handedAt)}` : ""}. You can stop it and take over at any time.`;
    if (t().conflict !== "none") return "Resolve it yourself, resolve it automatically or abort the merge.";
    // A recovery says what the shared rule says: Continue only where it is offered, the reason where a stop or a process exit is
    // not confirmed, and that it goes on by itself where the runtime continues it. Anything else shows the recorded fact.
    if (t().observedPhase === "recovery" && !m().view.unknown) {
      const run = resumeState(m().view);
      return run?.action === "retry_task" ? "Continue lets the coordinator decide the next step from the recorded state; nothing is replayed. Or send a message."
        : run?.kind === "evidence" || run?.kind === "automatic" ? run.why : "";
    }
    if (t().status === "review") return `Finished. Accepting v${t().result?.version ?? ""} is your decision.`;
    if (t().status === "ready_to_merge") return t().acceptance ? "Waiting for you to confirm." : m().view.project.hasGit === false ? "Review the result before closing the task." : sameBranch(t(), m().view.target) ? "Nothing is committed or closed without your confirmation." : "Nothing merges without your confirmation.";
    if (m().mode === "ended") return resumeState(m().view)?.why || "The run ended. Close task moves it to Done and keeps what was done.";
    if (m().mode === "closed" && t().status === "done") return "Archive keeps the task and its work without another confirmation.";
    // A pause whose stop or process exit is not confirmed cannot be resumed yet; it says so instead of offering Resume.
    if (m().mode === "paused" && resumeState(m().view)?.kind === "evidence") return resumeState(m().view)!.why;
    return m().mode === "live" && m().view.sprint?.runMode !== "automatic" ? `${open() ? `${open()} question${open() === 1 ? "" : "s"} for you. ` : ""}${chip().next ? `Next step is ${chip().next}.` : ""}`.trim() : paused() ? "Resume, or send any message. Nothing was deleted." : "";
  };
  const base = () => { const b = m().session.base; if (!b) return []; const behind = behindTarget(checkout(), b.branch); return [["Base", behind ? `${b.branch}, ${behind} behind` : b.branch, true] as [string, string, boolean]]; };
  const uncommitted = (): [string, string][] => { const c = checkout(); return c ? [["Uncommitted", c.files.length ? `${c.files.length} files` : "None"]] : m().session.changes ? [["Uncommitted", `${m().session.changes!.files} files`]] : []; };
  return <>
    <div class="ins-sec">
      <div class="ins-h"><span>State</span></div>
      <div class={`st ${chip().tone}`} style={{ "margin-bottom": "6px" }}><I n={chip().icon} s={14} class={chip().spin ? "spin" : undefined} />{chip().text}{chip().wide ?? ""}
        {/* The next step, which the title band no longer shows, unless the note below already names it. */}
        <Show when={chip().next && !String(note() ?? "").includes(chip().next!)}><I n="chevron-right" s={12} class="faint" /><span class="muted">{chip().next}</span></Show></div>
      <Show when={note()}><div class="cap muted">{note()}</div></Show>
      <Show when={pauseSettingsPage(m().view)}>{page => <div class="cap"><A class="lnk" to={{ name: "settings", section: "orchestration", machine: m().view.machine.id, page: page() }}>Open Engines and models</A></div>}</Show>
      <Show when={!note() && !m().view.unknown && m().view.fact.text !== chip().text + (chip().wide ?? "")}><div class="cap muted"><FactLine fact={m().view.fact} /></div></Show>
    </div>
    <Show when={t().archive?.stopUnconfirmed || t().archiveHold === "unconfirmed" && t().process === "unknown"}>
      <div class="cap st-warn">{ARCHIVE_STOP_UNCONFIRMED}</div>
    </Show>
    <Show when={m().view.task.status === "paused" && t().process === "stopped"}>
      <div class="ins-sec"><div class="ins-h"><span>Stop effect</span></div>
        <Kv rows={[["Workers", `Stopped${stoppedAt(m()) === null ? "" : ` at ${clock(stoppedAt(m()))}`}`], ["Checkout", "Untouched"], ["Decisions", open() ? `${open()} still open` : "None open"], ["Queue", `${m().queue.length} kept`]]} /></div>
    </Show>
    <Show when={selected()}>{w => <div class="ins-sec"><div class="ins-h"><span>Selected worker</span></div>
      <Kv rows={[["Worker", w().role], ["State", workerState(w()).label], ["Model", workerModel(w(), ws.route)], ...(w().scope.length ? [["Scope", w().scope.join(", "), true] as [string, string, boolean]] : []), ["Edited", `${editedFiles(w()).files} files, +${editedFiles(w()).add} −${editedFiles(w()).del}`], ["Commands", `${w().activity.filter(a => a.command).length} run`]]} /></div>}</Show>
    <div class="ins-sec">
      <div class="ins-h"><span>{t().checkout.kind === "worktree" ? "Worktree" : "Checkout"}</span></div>
      <Kv rows={[...(t().checkout.kind === "worktree" ? [["Path", (t().checkout as { path: string }).path, true] as [string, string, boolean]] : []), ["Branch", t().checkout.branch, true], ...base(), [sameBranch(t(), m().view.target) ? "Target branch" : "Merge into", m().view.target, true], ...uncommitted(), ["Conflicts", t().merge && t().conflict !== "none" ? (ws.store.mode === "real" && !t().merge!.files.length ? "No paths displayed" : conflictLine(t().merge!)) : "None"]]} />
    </div>
    <Show when={m().view.sprint}>{s => <div class="ins-sec"><div class="ins-h"><span>Sprint</span></div>
      <Kv rows={[["Sprint", sprintTitle(s())], ["Run mode", sprintModeLabel[s().runMode]], ["Blocks", `${blocks()} task${blocks() === 1 ? "" : "s"}`]]} /></div>}</Show>
    <Show when={checks().length > 0}>
      <div class="ins-sec"><div class="ins-h"><span>Checks</span></div>
        <div class="chk-sum">
          <Show when={counts().passed}><span class="st st-ok"><I n="check-circle" s={12} />{counts().passed} passed</span></Show>
          <Show when={counts().failed}><span class="st st-bad"><I n="x-circle" s={12} />{counts().failed} failed</span></Show>
          <Show when={counts().not_run}><span class="st st-idle"><I n="circle" s={12} />{counts().not_run} not run</span></Show>
          <Show when={counts().unverified}><span class="st st-idle"><I n="help" s={12} />{counts().unverified} unverified</span></Show>
        </div>
        <For each={shownChecks()}>{c => <><div class="chk"><I n={CHECK_ICON[c.outcome].icon} s={14} class={CHECK_ICON[c.outcome].tone} /><span class="trunc">{c.name}</span><span class="src">{c.outcome === "not_run" ? "Not run" : c.outcome === "unverified" ? "Unverified" : "Failed"}</span></div>
          <div class="ws-chk-why">{checkWhy(c)}</div></>}</For>
        <Show when={hiddenChecks() > 0}><div class="cap faint">{hiddenChecks()} more in the result</div></Show>
      </div>
    </Show>
    <Show when={m().workers.length > 0}>
      <div class="ins-sec"><div class="ins-h"><span>Workers</span><span>{workersOutcome(m().workers, m().active.length)}</span></div>
        <Keyed each={m().workers} by={w => w.id}>{w => { const f = () => workerState(w()); return <A class="wins ws-wins" to={{ name: "task", machine: m().view.machine.id, task: m().view.id, tab: "workers", worker: w().id }}><I n={f().icon} s={14} class={`${f().spin ? "spin " : ""}${f().tone === "st-idle" ? "faint" : f().tone}`} /><span class="trunc">{w().role}</span><span class={`st ${f().tone}`}>{f().label}</span></A>; }}</Keyed>
      </div>
    </Show>
    <div class="ins-sec"><div class="ins-h"><span>Route and access</span></div>
      {/* Until the task's saved route is read, a placeholder: never the machine's defaults standing in for the task's own. */}
      <Show when={ws.store.taskRouteKnown?.(m().view.id) ?? true} fallback={<LoadingStrip label="Reading this task's route" class="ws-route-pending" />}>
        <Kv rows={[["Coordinator model", ws.route(settings()) ?? ""], ...(ws.store.state().options.modes.length ? [["Access", modeLabel(settings().mode)] as [string, string]] : []), ...(ws.store.state().options.languages.length ? [["Language", languageLabel(settings().language)] as [string, string]] : [])]} />
        <Show when={usesMachineRoute(m()) && ws.route(settings())}><div class="cap faint">What this machine sets for every task.</div></Show>
      </Show></div>
    {/* What the task spent, as its engines reported it (`native_progress`). */}
    <Show when={taskSpendRows().length > 0}>
      <div class="ins-sec"><div class="ins-h"><span>Spend</span></div><Kv rows={taskSpendRows()} /></div>
    </Show>
    <Show when={usage().length > 0}>
      <div class="ins-sec"><div class="ins-h"><span>Usage</span></div><Kv rows={usage()} /></div>
    </Show>
    <Show when={limitRows().length > 0}>
      <div class="ins-sec"><div class="ins-h"><span>Engine limits</span></div><Kv rows={limitRows()} /></div>
    </Show>
  </>;
}
export function TaskInspector(props: { id: string; pick: string | null }) {
  const m = useModel(() => props.id);
  return <Show when={m()}>{model => <SessionPanel m={model()} pick={props.pick === VERIFICATION ? null : props.pick} />}</Show>;
}

/** External completion is an owner record, never a check, acceptance or Git operation. */
export function ExternalDoneDialog(props: { taskId: string | null; how: "external" | "already_in_target"; onClose: () => void }) {
  const ws = useWs();
  const m = createMemo(() => props.taskId ? sessionModel(ws.store.index(), props.taskId) : null);
  const bound = () => props.how === "external" && !!m() && pausedExternalClose(m()!.view);
  const [refused, setRefused] = createSignal<string | null>(null);
  const [reviewedExecution, setReviewedExecution] = createSignal<{ executionId: string | null; bound: boolean } | null>(null);
  const [closing, setClosing] = createSignal(false);
  createEffect(on(() => props.taskId, () => {
    setRefused(null);
    setReviewedExecution(m() ? { executionId: m()!.view.task.executionId ?? null, bound: bound() } : null);
  }));
  const waiting = () => !!m() && externalExitPending(m()!.view);
  const reason = () => {
    const model = m();
    if (!model) return "Refresh the task first.";
    const reviewed = reviewedExecution();
    if (!reviewed || reviewed.executionId !== (model.view.task.executionId ?? null) || reviewed.bound !== bound())
      return "The execution changed. Reopen this dialog to review the current task.";
    return bound() ? pausedExternalCloseBlocked(model) : externalDoneBlocked(model);
  };
  const confirm = async () => {
    const id = props.taskId;
    if (reason() || closing() || !id) return;
    const task = m()!.view.task, choice = props.how;
    setClosing(true);
    setRefused(null);
    try {
      const receipt = await ws.store.dispatch({
        type: "close_task", taskId: id, choice, confirmed: true, now: ws.store.clock(),
        ...(bound() ? { executionId: task.executionId!, expectedVersion: task.sourceVersion! } : {}),
      }, { quiet: true });
      if (props.taskId !== id) return;
      if (receipt.ok) props.onClose();
      else setRefused(receipt.message);
    } finally { setClosing(false); }
  };
  return <Modal open={!!props.taskId} onClose={props.onClose} label="Done outside the app">
    <div class="dlg-h"><h2>Done outside the app</h2><button data-tooltip="" class="ib" aria-label="Keep working" onClick={props.onClose}><I n="x" /></button></div>
    <div class="dlg-b"><p>{props.how === "already_in_target" ? `Already in ${m()?.view.task.integrationFacts?.targetBranch ?? "the target branch"}. ` : ""}Close this task as completed outside the app?</p>
      <p>{bound() ? "Your files, Git history and existing check evidence stay unchanged. The task is marked as done outside the app, and the sprint queue stays paused." : "No acceptance, merge or passing checks will be recorded. Your files and existing check evidence stay unchanged. Finished dependencies can continue in the sprint queue."}</p>
      <Show when={bound() && waiting() && !refused()}>
        <p role="status">{`Waiting for confirmed exit of ${m()!.view.task.issue}. Check again after it stops.`}</p>
      </Show>
      <Show when={refused()}><p role="status">{refused()}</p></Show>
    </div>
    <RequestStatus text={props.taskId && ws.store.pendingTask("close_task", props.taskId) ? REQUESTING.close_task : undefined} />
    <ActionBar place="dialog"><button class="btn btn-line btn-lg" onClick={props.onClose}>Keep working</button>
      <button class="btn btn-primary btn-lg" aria-disabled={reason() || closing() ? "true" : undefined} data-why={reason() ?? undefined} {...ws.needs("close_task")} onClick={confirm}>{bound() && (waiting() || refused()) ? "Check stop and close" : "Close task"}</button>
    </ActionBar>
  </Modal>;
}

/** Closing cannot be undone, so it says what will happen, in numbers. Cancel is the only red button, and only once chosen. */
export function CloseDialog(props: { taskId: string | null; onClose: () => void; onRefused?: (message: string) => void }) {
  const ws = useWs();
  const [choice, setChoice] = createSignal<"as_is" | "cancel">("as_is");
  const m = createMemo(() => props.taskId ? sessionModel(ws.store.index(), props.taskId) : null);
  // A memo of the id: a refresh that replaces the task record must not reset the owner's choice.
  const openedTask = createMemo(() => props.taskId);
  const signature = (model: SessionModel) => JSON.stringify([
    model.view.id, model.view.task.sourceVersion, model.view.task.executionId,
    model.view.task.result?.sourceVersion, model.view.task.result?.digest,
    model.view.sprint?.automaticQueue, model.view.task.status, unconfirmedState(model),
  ]);
  // A refusal stays in the dialog (a passing notice is dismissed by the next click) until the task changes or the
  // owner tries again.
  const [refused, setRefused] = createSignal<{ taskId: string; choice: string; signature: string; message: string } | null>(null);
  const refusal = () => { const r = refused(), model = m(); return r && model && r.taskId === props.taskId && r.signature === signature(model) ? r : null; };
  const [reviewedHold, setReviewedHold] = createSignal<string | null>(null);
  createEffect(on(openedTask, id => {
    setChoice("as_is");
    setRefused(null);
    const model = m();
    setReviewedHold(id && model && (automaticAcceptanceHold(model.view) || unconfirmedState(model)?.kind === "records") ? signature(model) : null);
  }));
  const paused = () => m()?.mode === "paused";
  // A failed or cancelled run has nothing left to stop or keep working on.
  const ended = () => m()?.mode === "ended";
  const records = () => !!m() && unconfirmedState(m()!)?.kind === "records";
  const automaticHold = () => !!m() && automaticAcceptanceHold(m()!.view);
  const [closing, setClosing] = createSignal(false);
  const blocked = () => {
    const model = m();
    if (!model) return "Refresh the task first.";
    if ((reviewedHold() || records()) && signature(model) !== reviewedHold()) return "The task changed. Reopen this dialog to review its current result.";
    return closeBlocked(model);
  };
  const close = () => {
    const id = props.taskId!;
    if (closing() || blocked()) return;
    setClosing(true);
    setRefused(null);
    const chosen = choice(), model = m()!;
    // Quiet: the refusal is shown here and by whoever opened the dialog, not as a passing notice.
    const base = { type: "close_task" as const, taskId: id, confirmed: true, now: ws.store.clock() };
    const task = model.view.task;
    void ws.store.dispatch(records()
      ? { ...base, choice: "unconfirmed", executionId: task.executionId!, expectedVersion: task.sourceVersion! }
      : { ...base, choice: chosen }, { quiet: true })
      .then(receipt => {
        if (receipt.ok) { if (props.taskId === id) props.onClose(); return; }
        setRefused({ taskId: id, choice: chosen, signature: signature(model), message: receipt.message });
        props.onRefused?.(receipt.message);
      })
      .finally(() => setClosing(false));
  };
  return <Modal open={!!props.taskId && !!m()} onClose={props.onClose} label="Close task">
    <Show when={m()}>{model => <>
      <div class="dlg-h"><h2>Close “{model().view.task.title}”?</h2><button data-tooltip="" class="ib" aria-label="Keep working" onClick={() => props.onClose()}><I n="x" /></button></div>
      <div class="dlg-b">
        <ul class="conseq"><Keyed each={closeConsequences(model())} by={l => `${l.icon}:${l.text}`}>{l => <li><I n={l().icon} s={14} /><span>{l().text}</span></li>}</Keyed></ul>
        <Show when={!records()}><div style={{ display: "grid", gap: "6px" }}>
          <label class="rc"><input type="radio" name="close-choice" checked={choice() === "as_is"} onChange={() => setChoice("as_is")} /><span><span class="t">Close as is</span><span class="d">{automaticHold() ? "This finishes the dependency and lets Automatic start the next ready task." : checkedResult(model()) ? "Keep what was done. The checked result and its evidence stay on record, and the task moves to Done." : "Keep what was done and decide about merging later. The result carries a lasting Not verified warning."}</span></span></label>
          <label class="rc danger"><input type="radio" name="close-choice" checked={choice() === "cancel"} onChange={() => setChoice("cancel")} /><span><span class="t">Cancel task</span><span class="d">Nothing from this task is needed. The task moves to Done as Cancelled. The checkout stays until you delete it.{automaticHold() ? " Cancelled dependencies can still keep later queued tasks blocked." : ""}</span></span></label>
        </div></Show>
        <Show when={!ended() && !automaticHold() && !records()}><Show when={paused()} fallback={<Show when={ws.can(capabilityOf("continue_task"))}><div class="note-box plain"><I n="pause" s={14} /><span>Only want to pause? <button class="lnk" {...ws.needs("stop_task")} onClick={() => { const id = props.taskId!; props.onClose(); ws.stopTask(id); }}>Stop</button> keeps everything and can be resumed. It needs no confirmation.</span></div></Show>}>
          <div class="note-box plain"><I n="pause" s={14} /><span>Only want to pause? This task is already stopped, and Resume picks it up again.</span></div>
        </Show></Show>
        <Show when={refusal()}>{r => <div class="note-box st-bad" role="alert"><I n="alert" s={14} /><span>{r().message}</span></div>}</Show>
      </div>
      <RequestStatus text={props.taskId && ws.store.pendingTask("close_task", props.taskId) ? REQUESTING.close_task : undefined} />
      <ActionBar place="dialog" hint={blocked() ?? (automaticHold() || records() ? undefined : "Closing keeps the work. Deleting it is a separate, explicit step in the Archive.")}>
        <button class="btn btn-line btn-lg" data-autofocus onClick={() => props.onClose()}>{paused() ? "Keep it paused" : ended() || records() ? "Keep it open" : "Keep working"}</button>
        <Show when={records() || choice() === "as_is"} fallback={<button class="btn btn-danger btn-lg" disabled={closing()} aria-disabled={blocked() ? "true" : undefined} data-why={blocked() ?? undefined} onClick={close}>Cancel task</button>}><button class="btn btn-primary btn-lg" disabled={closing()} aria-disabled={blocked() ? "true" : undefined} data-why={blocked() ?? undefined} onClick={close}>{records() ? "Close as is" : automaticHold() ? "Close as is and continue" : "Close task"}</button></Show>
      </ActionBar>
    </>}</Show>
  </Modal>;
}
