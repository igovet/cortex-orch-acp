import { createEffect, createMemo, createSignal, For, Show } from "solid-js";
import { capabilityOf } from "@cortex/workspace-domain";
import { checkCounts, clock, dayClock, engineLimits, sameBranch, sprintTitle, type TaskView } from "./selectors";
import { useWs } from "./ctx";
import { A, FactLine, I, Modal, Pop } from "./kit";
import { behindTarget, conflictLine, finishBlocked, taskCheckout } from "./review-model";
import { checkedResult, checksOf, checkWhy, closeBlocked, closeConsequences, contextLine, editedFiles, languageLabel, modeLabel, sessionModel, stateChip, stoppedAt, stopLabel, taskSettings, tokens, usesMachineRoute, workerFace, type SessionModel } from "./session-model";
import { CHECK_ICON, VERIFICATION } from "./worker";

const useModel = (id: () => string) => {
  const ws = useWs();
  return createMemo(() => sessionModel(ws.store.index(), id()));
};

/** The crumb is the title: a click renames it in place, and a manual title always wins over a late generated one. */
export function TaskTitle(props: { view: TaskView }) {
  const ws = useWs();
  const [text, setText] = createSignal<string | null>(null);
  const shown = () => props.view.task.titleState === "failed" ? props.view.task.description.split("\n")[0].slice(0, 60) : props.view.task.title;
  const save = () => { const t = text(); if (t?.trim()) ws.ok({ type: "rename_task", taskId: props.view.id, title: t }, () => { setText(null); }); };
  return <Show when={text() !== null} fallback={<button class="cur trunc ws-title" {...ws.needs("rename_task")} onClick={() => ws.gate("rename_task") || setText(props.view.task.title.startsWith("Title unavailable") || props.view.task.titleState === "pending" ? "" : props.view.task.title)}>{shown()}</button>}>
    <input class="inp ws-title-in" aria-label="Task title" value={text()!} onInput={e => setText(e.currentTarget.value)} ref={el => queueMicrotask(() => { el.focus(); el.select(); })} onBlur={() => setText(null)}
      onKeyDown={e => { if (e.key === "Enter") { e.preventDefault(); save(); } else if (e.key === "Escape") { e.stopPropagation(); setText(null); } }} />
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
    <button class="chip branch-chip" popovertarget="m-wt"><I n="branch" s={12} /><span class="w-wide">{worktree() ? "Worktree" : "Main checkout"}</span><Show when={t().checkout.branch}><span class="mono trunc" style={{ "max-width": "170px" }}>{t().checkout.branch}</span></Show></button>
    <Pop id="m-wt" class="pop-anchor" align="left" label="Checkout" style={{ width: "300px", padding: "10px 12px" }}>
      <div class="st st-idle" style={{ "margin-bottom": "8px" }}><I n="branch" s={12} />{worktree() ? "Separate worktree" : "Main checkout"}</div>
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

/** State, the one primary action, Stop and the task menu. Offline and unconfirmed tasks offer nothing that could mislead. */
export function TaskActions(props: { view: TaskView; onMenu: (el: HTMLElement) => void }) {
  const ws = useWs();
  const m = useModel(() => props.view.id);
  const chip = () => m() ? stateChip(m()!) : null;
  const go = () => {
    const task = props.view.id, tab = ws.router.route();
    if (tab.name === "task" && (tab.tab || tab.live)) ws.go({ name: "task", machine: tab.machine, task });
    queueMicrotask(() => document.querySelector(".dc")?.scrollIntoView({ block: "center" }));
  };
  const mode = () => m()?.mode;
  const blocked = () => m() ? closeBlocked(m()!) : null;
  return <>
    <Show when={chip()}>{c => <span class="chip"><I n={c().icon} s={12} class={`${c().spin ? "spin " : ""}${c().tone}`} /><span style={{ color: "var(--fg)" }}>{c().text}<Show when={c().wide}><span class="w-wide">{c().wide}</span></Show></span>
      <Show when={c().next}><I n="chevron-right" s={12} class="faint w-wide" /><span class="w-wide">{c().next}</span></Show></span>}</Show>
    <Show when={mode() === "live" && props.view.task.conflict === "none" && ((m()?.open.length ?? 0) > 0 || props.view.column === "needs")}><button class="btn btn-primary" onClick={go}>Answer</button></Show>
    <Show when={props.view.task.status === "review" && mode() === "review"}><A class="btn btn-primary" to={{ name: "task", machine: props.view.machine.id, task: props.view.id, tab: "changes", sub: "diff" }}>Review changes</A></Show>
    <Show when={props.view.task.status === "ready_to_merge" && mode() === "review"}>
      <Show when={!props.view.task.integrationFacts?.alreadyInTarget} fallback={<button class="btn btn-primary" {...ws.needs("close_task")} onClick={() => ws.closeTask(props.view.id, "already_in_target")}>Close task…</button>}>
      <Show when={sameBranch(props.view.task, props.view.target)}>
        <Show when={props.view.task.integrationFacts?.commitAvailable} fallback={<button class="btn btn-primary" {...ws.needs("close_task")} onClick={() => ws.closeTask(props.view.id)}>Close as is…</button>}>
          <button class="btn btn-primary" aria-disabled={finishBlocked(props.view.task) ? "true" : undefined} {...ws.needs("integrate")} onClick={() => { if (!finishBlocked(props.view.task)) ws.open({ kind: "merge", taskId: props.view.id, how: "squash" }); }}>Commit to {props.view.target}…</button>
        </Show>
      </Show>
      <Show when={!sameBranch(props.view.task, props.view.target)}>
      <Show when={props.view.project.hasGit !== false} fallback={<button class="btn btn-primary" aria-disabled={blocked() ? "true" : undefined} data-why={blocked()} {...ws.needs("close_task")} onClick={() => { if (!blocked()) ws.closeTask(props.view.id); }}>Close as is…</button>}>
        <button class="btn btn-primary" aria-disabled={finishBlocked(props.view.task) ? "true" : undefined} data-why={finishBlocked(props.view.task)} {...ws.needs("integrate")} onClick={() => { if (!finishBlocked(props.view.task)) ws.open({ kind: "merge", taskId: props.view.id, how: "squash" }); }}>{props.view.task.acceptance ? "Merge…" : "Review and merge…"}</button>
      </Show>
      </Show>
      </Show>
    </Show>
    <Show when={mode() === "live"}><button class="btn btn-line" {...ws.needs("stop_task")} onClick={() => ws.stopTask(props.view.id)}><I n="stop" s={12} class="st-bad" />{stopLabel(ws.can(capabilityOf("continue_task")))}</button></Show>
    <Show when={mode() === "paused"}><button class="btn btn-primary" {...ws.needs("continue_task")} onClick={() => ws.run({ type: "continue_task", taskId: props.view.id, now: ws.store.clock() })}><I n="play" s={12} />Resume</button></Show>
    <Show when={mode() === "paused" || mode() === "stopping"}><button class="btn btn-line" aria-disabled={blocked() ? "true" : undefined} data-why={blocked()} {...ws.needs("close_task")} onClick={() => { if (!blocked()) ws.closeTask(props.view.id); }}>Close task…</button></Show>
    <Show when={mode() === "draft"}><button class="btn btn-primary" {...ws.needs("start_task")} onClick={() => ws.run({ type: "start_task", taskId: props.view.id, now: ws.store.clock() })}><I n="play" s={12} />Start</button></Show>
    <button class="ib" aria-label="Task actions" onClick={e => props.onMenu(e.currentTarget)}><I n="more" /></button>
  </>;
}

/** A row with nothing to say is left out, never shown as an empty line. */
const Kv = (props: { rows: [string, string, boolean?][] }) => <dl class="kv"><For each={props.rows.filter(r => r[1])}>{r => <><dt>{r[0]}</dt><dd class={r[2] ? "mono" : undefined}>{r[1]}</dd></>}</For></dl>;
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
  const limitRows = () => limits().rows.flatMap((r): [string, string][] => r.usedPercent === null ? [] : [[r.label, `${r.usedPercent}% of this week${r.resetAt ? `, resets ${dayClock(r.resetAt)}` : ""}`]]);
  const chip = () => stateChip(m());
  const settings = () => taskSettings(m());
  const checks = createMemo(() => checksOf(m()));
  const counts = () => checkCounts(checks());
  const limits = createMemo(() => engineLimits(ws.store.state(), m().view.machine));
  const open = () => m().open.length;
  const paused = () => m().mode === "paused" || m().mode === "stopping";
  const finishedCount = () => m().finished.length;
  const selected = () => m().workers.find(w => w.id === props.pick) ?? null;
  const shownChecks = () => checks().filter(c => c.outcome !== "passed").slice(0, 5);
  const hiddenChecks = () => checks().filter(c => c.outcome !== "passed").length - shownChecks().length;
  const checkout = createMemo(() => taskCheckout(ws.store.state(), t()));
  const blocks = () => ws.store.state().tasks.filter(x => x.dependencies.includes(t().id)).length;
  const note = () => {
    if (t().conflict === "resolving") return `Handed over by you${t().merge?.handedAt != null ? ` at ${clock(t().merge!.handedAt)}` : ""}. You can stop it and take over at any time.`;
    if (t().conflict !== "none") return "Resolve it yourself, resolve it automatically or abort the merge.";
    if (t().status === "review") return `Finished. Accepting v${t().result?.version ?? ""} is your decision.`;
    if (t().status === "ready_to_merge") return t().acceptance ? "Waiting for you to confirm." : m().view.project.hasGit === false ? "Review the result before closing the task." : sameBranch(t(), m().view.target) ? "Nothing is committed or closed without your confirmation." : "Nothing merges without your confirmation.";
    return m().mode === "live" ? `${open() ? `${open()} question${open() === 1 ? "" : "s"} for you. ` : ""}Next step is Review.` : paused() ? "Resume, or send any message. Nothing was deleted." : "";
  };
  const base = () => { const b = m().session.base; if (!b) return []; const behind = behindTarget(checkout(), b.branch); return [["Base", behind ? `${b.branch}, ${behind} behind` : b.branch, true] as [string, string, boolean]]; };
  const uncommitted = (): [string, string][] => { const c = checkout(); return c ? [["Uncommitted", c.files.length ? `${c.files.length} files` : "None"]] : m().session.changes ? [["Uncommitted", `${m().session.changes!.files} files`]] : []; };
  return <>
    <div class="ins-sec">
      <div class="ins-h"><span>State</span></div>
      <div class={`st ${chip().tone}`} style={{ "margin-bottom": "6px" }}><I n={chip().icon} s={14} class={chip().spin ? "spin" : undefined} />{chip().text}{chip().wide ?? ""}</div>
      <Show when={note()}><div class="cap muted">{note()}</div></Show>
      <Show when={!note() && m().view.fact.text !== chip().text + (chip().wide ?? "")}><div class="cap muted"><FactLine fact={m().view.fact} /></div></Show>
    </div>
    <Show when={m().view.task.status === "paused"}>
      <div class="ins-sec"><div class="ins-h"><span>Stop effect</span></div>
        <Kv rows={[["Workers", `Stopped${stoppedAt(m()) === null ? "" : ` at ${clock(stoppedAt(m()))}`}`], ["Checkout", "Untouched"], ["Decisions", open() ? `${open()} still open` : "None open"], ["Queue", `${m().queue.length} kept`]]} /></div>
    </Show>
    <Show when={selected()}>{w => <div class="ins-sec"><div class="ins-h"><span>Selected worker</span></div>
      <Kv rows={[["Worker", w().role], ["State", workerFace(w().status).label], ...(w().scope.length ? [["Scope", w().scope.join(", "), true] as [string, string, boolean]] : []), ["Edited", `${editedFiles(w()).files} files, +${editedFiles(w()).add} −${editedFiles(w()).del}`], ["Commands", `${w().activity.filter(a => a.command).length} run`]]} /></div>}</Show>
    <div class="ins-sec">
      <div class="ins-h"><span>{t().checkout.kind === "worktree" ? "Worktree" : "Checkout"}</span></div>
      <Kv rows={[...(t().checkout.kind === "worktree" ? [["Path", (t().checkout as { path: string }).path, true] as [string, string, boolean]] : []), ["Branch", t().checkout.branch, true], ...base(), [sameBranch(t(), m().view.target) ? "Target branch" : "Merge into", m().view.target, true], ...uncommitted(), ["Conflicts", t().merge && t().conflict !== "none" ? conflictLine(t().merge!) : "None"]]} />
    </div>
    <Show when={m().view.sprint}>{s => <div class="ins-sec"><div class="ins-h"><span>Sprint</span></div>
      <Kv rows={[["Sprint", sprintTitle(s())], ["Run mode", s().runMode === "automatic" ? "Automatic" : "Manual"], ["Blocks", `${blocks()} task${blocks() === 1 ? "" : "s"}`]]} /></div>}</Show>
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
      <div class="ins-sec"><div class="ins-h"><span>Workers</span><span>{m().active.length} active, {finishedCount()} finished</span></div>
        <For each={m().workers}>{w => { const f = () => workerFace(w.status); return <A class="wins ws-wins" to={{ name: "task", machine: m().view.machine.id, task: m().view.id, tab: "workers", worker: w.id }}><I n={f().icon} s={14} class={`${f().spin ? "spin " : ""}${f().tone === "st-idle" ? "faint" : f().tone}`} /><span class="trunc">{w.role}</span><span class={`st ${f().tone}`}>{f().label}</span></A>; }}</For>
      </div>
    </Show>
    <div class="ins-sec"><div class="ins-h"><span>Route and access</span></div>
      <Kv rows={[["Model", ws.route(settings()) ?? ""], ...(ws.store.state().options.modes.length ? [["Access", modeLabel(settings().mode)] as [string, string]] : []), ...(ws.store.state().options.languages.length ? [["Language", languageLabel(settings().language)] as [string, string]] : [])]} />
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
      <Show when={reason()}><div class="note-box plain" role="status">{reason()}</div></Show>
    </div>
    <div class="dlg-f"><button class="btn btn-line btn-lg" onClick={props.onClose}>Keep working</button>
      <Show when={!reason()}><button class="btn btn-primary btn-lg" {...ws.needs("close_task")} onClick={confirm}>Close task</button></Show>
    </div>
  </Modal>;
}

/** Closing cannot be undone, so it says what will happen, in numbers. Cancel is the only red button, and only once chosen. */
export function CloseDialog(props: { taskId: string | null; onClose: () => void }) {
  const ws = useWs();
  const [choice, setChoice] = createSignal<"as_is" | "cancel">("as_is");
  const m = createMemo(() => props.taskId ? sessionModel(ws.store.index(), props.taskId) : null);
  createEffect(() => { if (props.taskId) setChoice("as_is"); });
  const paused = () => m()?.mode === "paused";
  const close = () => {
    const id = props.taskId!;
    ws.ok({ type: "close_task", taskId: id, choice: choice(), confirmed: true, now: ws.store.clock() }, () => { props.onClose(); });
  };
  return <Modal open={!!props.taskId && !!m()} onClose={props.onClose} label="Close task">
    <Show when={m()}>{model => <>
      <div class="dlg-h"><h2>Close “{model().view.task.title}”?</h2><button class="ib" aria-label="Keep working" onClick={() => props.onClose()}><I n="x" /></button></div>
      <div class="dlg-b">
        <ul class="conseq"><For each={closeConsequences(model())}>{l => <li><I n={l.icon} s={14} /><span>{l.text}</span></li>}</For></ul>
        <div style={{ display: "grid", gap: "6px" }}>
          <label class="rc"><input type="radio" name="close-choice" checked={choice() === "as_is"} onChange={() => setChoice("as_is")} /><span><span class="t">Close as is</span><span class="d">{checkedResult(model()) ? "Keep what was done. The checked result and its evidence stay on record, and the task moves to Done." : "Keep what was done and decide about merging later. The result carries a lasting Not verified warning."}</span></span></label>
          <label class="rc danger"><input type="radio" name="close-choice" checked={choice() === "cancel"} onChange={() => setChoice("cancel")} /><span><span class="t">Cancel task</span><span class="d">Nothing from this task is needed. The task moves to Done as Cancelled. The checkout stays until you delete it.</span></span></label>
        </div>
        <Show when={paused()} fallback={<Show when={ws.can(capabilityOf("continue_task"))}><div class="note-box plain"><I n="pause" s={14} /><span>Only want to pause? <button class="lnk" {...ws.needs("stop_task")} onClick={() => { const id = props.taskId!; props.onClose(); ws.stopTask(id); }}>Stop</button> keeps everything and can be resumed. It needs no confirmation.</span></div></Show>}>
          <div class="note-box plain"><I n="pause" s={14} /><span>Only want to pause? This task is already stopped, and Resume picks it up again.</span></div>
        </Show>
      </div>
      <div class="dlg-f">
        <span class="note">Deleting unmerged work is a separate question at archive time.</span>
        <button class="btn btn-line btn-lg" data-autofocus onClick={() => props.onClose()}>{paused() ? "Keep it paused" : "Keep working"}</button>
        <Show when={choice() === "as_is"} fallback={<button class="btn btn-danger btn-lg" onClick={close}>Cancel task</button>}><button class="btn btn-primary btn-lg" onClick={close}>Close task</button></Show>
      </div>
    </>}</Show>
  </Modal>;
}
