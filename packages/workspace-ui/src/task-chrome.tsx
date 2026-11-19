import { createEffect, createMemo, createSignal, For, on, Show, type JSX } from "solid-js";
import { capabilityOf, taskRenamable, type Task } from "@cortex/workspace-domain";
import { asOf, automaticAcceptanceHold, checkCounts, clock, dayClock, engineLimits, humanStatus, nothingToCommit, ownerHold, sameBranch, windowTitle, sprintModeLabel, sprintTitle, type TaskView } from "./selectors";
import { useWs } from "./ctx";
import { FieldEdit } from "./field-edit";
import { onScreen } from "./freshness";
import { A, FactLine, I, Keyed, Modal, Pop } from "./kit";
import { ActionBar, revealWhenCut } from "./layout";
import { behindTarget, conflictLine, finishBlocked, taskCheckout } from "./review-model";
import { checkedResult, checksOf, checkWhy, closeBlocked, closeConsequences, contextLine, editedFiles, languageLabel, modeLabel, sessionModel, stateChip, stoppedAt, stopLabel, taskSettings, tokens, unconfirmedState, usesMachineRoute, workerFace, workerModel, workersOutcome, type SessionModel } from "./session-model";
import { CHECK_ICON, VERIFICATION } from "./worker";
import { archiveBlocked } from "./git-actions-model";

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
  const uncommitted = () => taskCheckout(ws.store.state(), t())?.files.length ?? m()?.session.changes?.files;
  return <>
    <button class="chip branch-chip" popovertarget="m-wt" aria-label={`${worktree() ? "Worktree" : "Project folder"}${t().checkout.branch ? ` ${t().checkout.branch}` : ""}`}><I n="branch" s={12} /><span class="w-wide">{worktree() ? "Worktree" : "Project folder"}</span><Show when={t().checkout.branch}><span class="mono trunc" style={{ "max-width": "170px" }}>{t().checkout.branch}</span></Show></button>
    <Pop id="m-wt" class="pop-anchor" align="left" label="Checkout" style={{ width: "300px", padding: "10px 12px" }}>
      <div class="st st-idle" style={{ "margin-bottom": "8px" }}><I n="branch" s={12} />{worktree() ? "Separate worktree" : "Project folder"}</div>
      <dl class="kv">
        <Show when={t().checkout.branch}><dt>Branch</dt><dd class="mono">{t().checkout.branch}</dd></Show>
        <Show when={m()?.session.base}>{b => <><dt>Base</dt><dd class="mono">{b().branch}</dd></>}</Show>
        <Show when={props.view.target}><dt>{sameBranch(t(), props.view.target) ? "Target branch" : "Merge into"}</dt><dd class="mono">{props.view.target}</dd></Show>
        <Show when={uncommitted() !== undefined}><dt>Uncommitted</dt><dd>{uncommitted() ? `${uncommitted()} files` : "None"}</dd></Show>
      </dl>
      <Show when={ws.terminalAvailable()}><div class="row" style={{ "margin-top": "10px" }}><button class="btn btn-line btn-sm" aria-disabled={worktree() ? "true" : undefined} data-why={worktree() ? "This service offers project shells only. Open the project terminal from its board." : undefined} onClick={e => { if (worktree()) return; (e.currentTarget.closest("[popover]") as HTMLElement).hidePopover?.(); if (!ws.panels.terminalOpen()) ws.toggleTerminal(); }}>{worktree() ? "Open worktree terminal" : "Open terminal here"}</button></div></Show>
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
  if (m.mode === "closed") return archiveBlocked(t) ?? null;
  return m.mode === "paused" ? closeBlocked(m) : null;
}

/**
 * The task's one next step that is not on the screen already (Answer, Review changes, a closing in Ready to merge, Continue),
 * the Git controls and the task menu. Its state reads in the strip under the tabs, the sidebar and the composer; Start, Stop
 * and Resume are the composer's action button and stay in the menu. Offline and unconfirmed tasks offer nothing that could mislead.
 */
export function TaskActions(props: { view: TaskView; onMenu: (el: HTMLElement) => void; git?: JSX.Element }) {
  const ws = useWs();
  const m = useModel(() => props.view.id);
  const why = () => taskWhyId(props.view.id);
  const go = () => {
    const task = props.view.id, tab = ws.router.route();
    if (tab.name === "task" && (tab.tab || tab.live)) ws.go({ name: "task", machine: tab.machine, task });
    queueMicrotask(() => onScreen().querySelector(".dc")?.scrollIntoView({ block: "center" }));
  };
  const mode = () => m()?.mode;
  const blocked = () => m() ? closeBlocked(m()!) : null;
  const archiveWhy = () => archiveBlocked(props.view.task);
  return <>
    <Show when={mode() === "live" && props.view.task.conflict === "none" && ((m()?.open.length ?? 0) > 0 || (props.view.column === "needs" && props.view.task.observedPhase !== "recovery"))}><button class="btn btn-primary" onClick={go}>Answer</button></Show>
    <Show when={props.view.task.status === "review" && mode() === "review"}><A class="btn btn-primary" to={{ name: "task", machine: props.view.machine.id, task: props.view.id, tab: "changes", sub: "diff" }}>Review changes</A></Show>
    <Show when={props.view.task.status === "ready_to_merge" && mode() === "review"}>
      {/* Merge… and Commit to the target… are the Git button's primary action in this state; only closing stays here. */}
      <Show when={!props.view.task.integrationFacts?.alreadyInTarget} fallback={<button class="btn btn-primary" {...ws.needs("close_task")} onClick={() => ws.closeTask(props.view.id, "already_in_target")}>Close task…</button>}>
      <Show when={sameBranch(props.view.task, props.view.target) && !props.view.task.integrationFacts?.commitAvailable}>
        <button class="btn btn-primary" {...ws.needs("close_task")} onClick={() => ws.closeTask(props.view.id)}>{nothingToCommit(props.view.task) ? "Close task…" : "Close as is…"}</button>
      </Show>
      <Show when={!sameBranch(props.view.task, props.view.target) && props.view.project.hasGit === false}>
        <button class="btn btn-primary" aria-describedby={blocked() ? why() : undefined} aria-disabled={blocked() ? "true" : undefined} data-why={blocked()} {...ws.needs("close_task")} onClick={() => { if (!blocked()) ws.closeTask(props.view.id); }}>Close as is…</button>
      </Show>
      </Show>
    </Show>
    {/* A run that ended by itself (failed or cancelled) only waits for the owner to close it. */}
    <Show when={mode() === "ended"}>
      <button class="btn btn-primary" aria-describedby={blocked() ? why() : undefined} aria-disabled={blocked() ? "true" : undefined} data-why={blocked()} {...ws.needs("close_task")} onClick={() => { if (!blocked()) ws.closeTask(props.view.id); }}>Close task…</button>
    </Show>
    {/* A closed task's next step is the Archive: one command, with its work, and nothing asks. */}
    <Show when={mode() === "closed"}>
      <button class="btn btn-line" aria-describedby={archiveWhy() ? why() : undefined} aria-disabled={archiveWhy() ? "true" : undefined} data-why={archiveWhy()} {...ws.needs("archive_task")} onClick={() => { if (!archiveWhy()) ws.archiveTask(props.view.id); }}><I n="archive" s={12} />Archive</button>
    </Show>
    {/* A queue hold decided for the owner: its actions are on the pinned card above the message box; this brings it into view. */}
    <Show when={ownerHold(props.view)?.nextActor === "owner" || ["exit_unconfirmed", "records"].includes(m() ? unconfirmedState(m()!)?.kind ?? "" : "")}><button class="btn btn-primary" onClick={() => {
      const tab = ws.router.route();
      if (tab.name === "task" && (tab.tab || tab.live)) ws.go({ name: "task", machine: tab.machine, task: props.view.id });
      queueMicrotask(() => onScreen().querySelector(".chat-review-hold")?.scrollIntoView({ block: "center" }));
    }}>See what is needed</button></Show>
    {/* A held task waits for the owner: say what is expected with one concrete action, not only a message box. */}
    <Show when={mode() === "live" && props.view.task.observedPhase === "recovery" && props.view.task.conflict === "none"}><button class="btn btn-primary" {...ws.needs("retry_task")} onClick={() => ws.run({ type: "retry_task", taskId: props.view.id, now: ws.store.clock() })}><I n="play" s={12} />Continue</button></Show>
    {/* Start, Stop and Resume are the composer's action button. Where this machine offers no message composer, they stay here. */}
    <Show when={!ws.can(capabilityOf("send_message"))}>
      <Show when={mode() === "live" && props.view.task.status === "working"}><button class="btn btn-line" {...ws.needs("stop_task")} onClick={() => ws.stopTask(props.view.id)}><I n="stop" s={12} class="st-bad" />{stopLabel(ws.can(capabilityOf("continue_task")))}</button></Show>
      <Show when={mode() === "paused"}><button class="btn btn-primary" {...ws.needs("continue_task")} onClick={() => ws.run({ type: "continue_task", taskId: props.view.id, now: ws.store.clock() })}><I n="play" s={12} />Resume</button></Show>
      <Show when={mode() === "draft"}><button class="btn btn-primary" {...ws.needs("start_task")} onClick={() => ws.run({ type: "start_task", taskId: props.view.id, now: ws.store.clock() })}><I n="play" s={12} />Start</button></Show>
    </Show>
    {props.git}
    <button class="ib" aria-label="Task actions" onClick={e => props.onMenu(e.currentTarget)}><I n="more" /></button>
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
    if (m().view.automaticFact) return "";
    if (m().view.ownerIntegration) return humanStatus(m().view.ownerIntegration!.reason);
    if (t().conflict === "resolving") return `Handed over by you${t().merge?.handedAt != null ? ` at ${clock(t().merge!.handedAt)}` : ""}. You can stop it and take over at any time.`;
    if (t().conflict !== "none") return "Resolve it yourself, resolve it automatically or abort the merge.";
    if (t().observedPhase === "recovery" && !m().view.unknown) return "Continue lets the coordinator decide the next step from the recorded state; nothing is replayed. Or send a message.";
    if (t().status === "review") return `Finished. Accepting v${t().result?.version ?? ""} is your decision.`;
    if (t().status === "ready_to_merge") return t().acceptance ? "Waiting for you to confirm." : m().view.project.hasGit === false ? "Review the result before closing the task." : sameBranch(t(), m().view.target) ? "Nothing is committed or closed without your confirmation." : "Nothing merges without your confirmation.";
    if (m().mode === "ended") return "The run ended by itself. Close task moves it to Done and keeps what was done.";
    if (m().mode === "closed" && !archiveBlocked(t())) return "Archive moves it to the Archive with its work, without asking anything.";
    return m().mode === "live" && m().view.sprint?.runMode !== "automatic" ? `${open() ? `${open()} question${open() === 1 ? "" : "s"} for you. ` : ""}Next step is Review.` : paused() ? "Resume, or send any message. Nothing was deleted." : "";
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
      <Show when={!note() && !m().view.unknown && m().view.fact.text !== chip().text + (chip().wide ?? "")}><div class="cap muted"><FactLine fact={m().view.fact} /></div></Show>
    </div>
    <Show when={m().view.task.status === "paused"}>
      <div class="ins-sec"><div class="ins-h"><span>Stop effect</span></div>
        <Kv rows={[["Workers", `Stopped${stoppedAt(m()) === null ? "" : ` at ${clock(stoppedAt(m()))}`}`], ["Checkout", "Untouched"], ["Decisions", open() ? `${open()} still open` : "None open"], ["Queue", `${m().queue.length} kept`]]} /></div>
    </Show>
    <Show when={selected()}>{w => <div class="ins-sec"><div class="ins-h"><span>Selected worker</span></div>
      <Kv rows={[["Worker", w().role], ["State", workerFace(w().status).label], ["Model", workerModel(w(), ws.route)], ...(w().scope.length ? [["Scope", w().scope.join(", "), true] as [string, string, boolean]] : []), ["Edited", `${editedFiles(w()).files} files, +${editedFiles(w()).add} −${editedFiles(w()).del}`], ["Commands", `${w().activity.filter(a => a.command).length} run`]]} /></div>}</Show>
    <div class="ins-sec">
      <div class="ins-h"><span>{t().checkout.kind === "worktree" ? "Worktree" : "Checkout"}</span></div>
      <Kv rows={[...(t().checkout.kind === "worktree" ? [["Path", (t().checkout as { path: string }).path, true] as [string, string, boolean]] : []), ["Branch", t().checkout.branch, true], ...base(), [sameBranch(t(), m().view.target) ? "Target branch" : "Merge into", m().view.target, true], ...uncommitted(), ["Conflicts", t().merge && t().conflict !== "none" ? conflictLine(t().merge!) : "None"]]} />
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
        <Keyed each={m().workers} by={w => w.id}>{w => { const f = () => workerFace(w().status); return <A class="wins ws-wins" to={{ name: "task", machine: m().view.machine.id, task: m().view.id, tab: "workers", worker: w().id }}><I n={f().icon} s={14} class={`${f().spin ? "spin " : ""}${f().tone === "st-idle" ? "faint" : f().tone}`} /><span class="trunc">{w().role}</span><span class={`st ${f().tone}`}>{f().label}</span></A>; }}</Keyed>
      </div>
    </Show>
    <div class="ins-sec"><div class="ins-h"><span>Route and access</span></div>
      <Kv rows={[["Coordinator model", ws.route(settings()) ?? ""], ...(ws.store.state().options.modes.length ? [["Access", modeLabel(settings().mode)] as [string, string]] : []), ...(ws.store.state().options.languages.length ? [["Language", languageLabel(settings().language)] as [string, string]] : [])]} />
      <Show when={usesMachineRoute(m()) && ws.route(settings())}><div class="cap faint">What this machine sets for every task.</div></Show></div>
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
  const reason = () => {
    const model = m();
    if (!model) return "Refresh the task first.";
    if (model.view.task.process !== "stopped" || ["working", "needs_you", "stopping"].includes(model.view.task.status))
      return "Stop the task and wait for every worker to stop first.";
    return closeBlocked(model);
  };
  const confirm = () => { if (!reason() && props.taskId) ws.ok({ type: "close_task", taskId: props.taskId, choice: props.how, confirmed: true, now: ws.store.clock() }, props.onClose); };
  return <Modal open={!!props.taskId} onClose={props.onClose} label="Done outside the app">
    <div class="dlg-h"><h2>Done outside the app</h2><button class="ib" aria-label="Keep working" onClick={props.onClose}><I n="x" /></button></div>
    <div class="dlg-b"><p>{props.how === "already_in_target" ? `Already in ${m()?.view.task.integrationFacts?.targetBranch ?? "the target branch"}. ` : ""}Close this task as completed outside the app?</p>
      <p>No acceptance, merge or passing checks will be recorded. Your files and existing check evidence stay unchanged. Finished dependencies can continue in the sprint queue.</p>
    </div>
    <ActionBar place="dialog"><button class="btn btn-line btn-lg" onClick={props.onClose}>Keep working</button>
      <button class="btn btn-primary btn-lg" aria-disabled={reason() ? "true" : undefined} data-why={reason() ?? undefined} {...ws.needs("close_task")} onClick={confirm}>Close task</button>
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
      <div class="dlg-h"><h2>Close “{model().view.task.title}”?</h2><button class="ib" aria-label="Keep working" onClick={() => props.onClose()}><I n="x" /></button></div>
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
      <ActionBar place="dialog" hint={blocked() ?? (automaticHold() || records() ? undefined : "Closing keeps the work. Deleting it is a separate, explicit step in the Archive.")}>
        <button class="btn btn-line btn-lg" data-autofocus onClick={() => props.onClose()}>{paused() ? "Keep it paused" : ended() || records() ? "Keep it open" : "Keep working"}</button>
        <Show when={records() || choice() === "as_is"} fallback={<button class="btn btn-danger btn-lg" disabled={closing()} aria-disabled={blocked() ? "true" : undefined} data-why={blocked() ?? undefined} onClick={close}>Cancel task</button>}><button class="btn btn-primary btn-lg" disabled={closing()} aria-disabled={blocked() ? "true" : undefined} data-why={blocked() ?? undefined} onClick={close}>{records() ? "Close as is" : automaticHold() ? "Close as is and continue" : "Close task"}</button></Show>
      </ActionBar>
    </>}</Show>
  </Modal>;
}
