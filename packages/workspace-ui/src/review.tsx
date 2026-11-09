import { createEffect, createMemo, createSignal, For, Match, on, Show, Switch } from "solid-js";
import type { Check, FileDiff, GitFile } from "@cortex/workspace-domain";
import { useWs } from "./ctx";
import { Counts, Diff, GitLetter, type HunkActions } from "./diff";
import { A, I, MenuItem, Pop } from "./kit";
import type { Route } from "./router";
import { checkCounts, clock, sameBranch } from "./selectors";
import { acceptBlocked, checksLine, firstChanged, mergeBlocked, openChecks, reviewFor, stale, taskCheckout } from "./review-model";
import type { SessionModel } from "./session-model";
import { CHECK_ICON } from "./worker";
import { ResultText } from "./result-text";

type TaskRoute = Extract<Route, { name: "task" }>;
const sourceLabel = (c: Check, none: string) => c.source === "runtime" ? "Recorded by runtime" : c.source === "worker" ? "Reported by worker" : none;

/** Only reported outcomes have a counter; absent outcomes do not fill the card with zeros. */
export function Outcomes(props: { checks: Check[] }) {
  const c = () => checkCounts(props.checks);
  const cell = (n: number, on: string, icon: string, label: string) => <Show when={n > 0}><div><span class={`v ${on}`}><I n={icon} />{n}</span><span class="l">{label}</span></div></Show>;
  const columns = () => Object.values(c()).filter(n => n > 0).length;
  return <div class="outc" style={{ "grid-template-columns": `repeat(${Math.max(1, columns())}, minmax(0, 1fr))` }}>
    {cell(c().passed, "st-ok", "check-circle", "Passed")}
    {cell(c().failed, "st-bad", "x-circle", "Failed")}
    {cell(c().not_run, "st-idle", "circle", "Not run")}
    {cell(c().unverified, "st-idle", "help", "Unverified")}
  </div>;
}
function RerunCheck(props: { taskId: string; check: Check }) {
  const ws = useWs();
  const task = () => ws.store.index().byId.get(props.taskId)!.task;
  const available = () => !stale(task()) && !task().integration && task().conflict === "none" && ["review", "ready_to_merge"].includes(task().status);
  return <Show when={props.check.running} fallback={<Show when={props.check.outcome !== "passed" && props.check.kind !== "manual" && props.check.kind !== "policy" && props.check.rerunnable === true && !!props.check.command?.length && available()}>
    <button class="btn btn-line btn-sm" aria-label={`Rerun ${props.check.name}`} {...ws.needs("rerun_check")} onClick={() => ws.run({ type: "rerun_check", taskId: props.taskId, digest: task().result!.digest, checkIndex: task().result!.checks.indexOf(props.check) })}>Rerun check</button>
  </Show>}><span role="status">{ws.t("rerunning")}</span></Show>;
}
export function OpenChecks(props: { checks: Check[]; taskId?: string }) {
  const ws = useWs();
  return <><Show when={props.checks.some(c => c.kind === "manual")}><section aria-label="Check manually"><h3>Check manually</h3><For each={props.checks.filter(c => c.kind === "manual")}>{c => <div class="nv"><I n="eye" s={14} /><span>{c.name}<span class="why">{c.detail || "Needs a person’s judgement. Not counted as passed or failed; does not block Accept."}</span></span></div>}</For></section></Show><Show when={openChecks(props.checks).length > 0}><div>
  <For each={[...openChecks(props.checks)].sort((a, b) => (a.outcome === "failed" ? 0 : 1) - (b.outcome === "failed" ? 0 : 1))}>{c =>
    <div class="nv"><I n={CHECK_ICON[c.outcome].icon} s={14} class={CHECK_ICON[c.outcome].tone} /><span>{c.name}<span class="why">{c.detail}</span><Show when={c.technicalReason}><span class="why">{c.kind === "policy" ? "Not run: " : "Could not check: "}{c.technicalReason}</span></Show><Show when={c.command?.length}><details><summary>Command and output</summary><pre>{c.command!.join(" ")}</pre><Show when={c.cwd}><div>Working folder: {c.cwd}</div></Show><Show when={c.exitCode !== undefined}><div>Exit code: {c.exitCode}</div></Show><pre>{c.output || "No command output recorded."}</pre></details></Show></span><span class="src">{sourceLabel(c, ws.t("unsourcedCheck"))}<Show when={props.taskId}>{id => <RerunCheck taskId={id()} check={c} />}</Show></span></div>}</For>
</div></Show><For each={props.checks.filter(c => c.kind !== "manual" && c.outcome === "passed" && c.output)}>{c => <details class="nv"><summary>{c.name} — Passed · Command and output</summary><pre>{c.command?.join(" ")}</pre><div>Working folder: {c.cwd}</div><pre>{c.output}</pre></details>}</For></>;
}

/**
 * When Accept is off, the reason is written out beside it with the one action that clears it, so a result never sits in
 * Review with no way forward. Nothing here accepts for the owner: each action only asks for what the reason names.
 */
export function AcceptBlocker(props: { m: SessionModel }) {
  const ws = useWs();
  const t = () => props.m.view.task;
  const reason = () => ["review", "ready_to_merge"].includes(t().status) && !t().acceptance ? acceptBlocked(t()) : null;
  const checks = () => t().result?.checks ?? [];
  const rerunnable = () => checks().filter(c => c.kind !== "manual" && c.kind !== "policy" && c.rerunnable === true && !!c.command?.length && c.outcome !== "passed" && !c.running);
  const canRerun = () => !stale(t()) && !t().integration && t().conflict === "none" && rerunnable().length > 0;
  const rerun = async () => {
    const r = t().result!;
    for (const c of rerunnable()) { const sent = await ws.run({ type: "rerun_check", taskId: t().id, digest: r.digest, checkIndex: r.checks.indexOf(c) }); if (!sent.ok) return; }
  };
  const ask = (text: string) => ws.run({ type: "request_changes", taskId: t().id, text, now: ws.store.clock() });
  const conflict = (): Route => ({ name: "task", machine: props.m.view.machine.id, task: t().id, tab: "changes", sub: "conflict" });
  return <>
    <Show when={ws.store.acceptanceIssue()?.taskId === t().id && !t().acceptance && ws.store.acceptanceIssue()}>{issue => <div class="rv-why rv-accept-error" role="alert"><span class="grow">{issue().message} Acceptance was not confirmed. Refresh the result before trying again.</span><button class="btn btn-line btn-sm" onClick={() => void ws.store.reload()}>Refresh result</button></div>}</Show>
    <Show when={reason()}>{why => <div class="rv-why" role="status"><I n="alert" s={14} class="st-warn" /><span class="grow">{why()}.</span>
    <Switch>
      <Match when={stale(t())}><button class="btn btn-line btn-sm" {...ws.needs("request_changes")} onClick={() => ws.open({ kind: "changes", taskId: t().id, text: "The files changed after this version. Please check the work again and publish a fresh result." })}>Request a new result…</button></Match>
      <Match when={t().conflict !== "none"}><A class="btn btn-line btn-sm" to={conflict()}>Open the conflict</A></Match>
      <Match when={checks().length === 0}><button class="btn btn-line btn-sm" {...ws.needs("request_changes")} onClick={() => void ask("No checks were reported for this result. Run the checks this work needs and publish the result again.")}>Ask to run checks</button></Match>
      <Match when={canRerun()}>
        <button class="btn btn-line btn-sm" {...ws.needs("rerun_check")} onClick={e => void ws.busy(rerun(), e.currentTarget)}>{rerunnable().some(c => c.outcome === "failed") ? (rerunnable().length === 1 ? "Rerun the failed check" : "Rerun failed checks") : (rerunnable().length === 1 ? "Run the missing check" : "Run the missing checks")}</button>
        <Show when={rerunnable().some(c => c.outcome === "failed")}><button class="btn btn-line btn-sm" {...ws.needs("request_changes")} onClick={() => void ask(`Fix the failing checks: ${rerunnable().filter(c => c.outcome === "failed").map(c => c.name).join(", ")}.`)}>Ask to fix</button></Show>
      </Match>
      <Match when={t().reviewActions?.unblockAction === "request_changes" || t().result?.acceptanceAvailable === false}><button class="btn btn-line btn-sm" {...ws.needs("request_changes")} onClick={() => ws.open({ kind: "changes", taskId: t().id, text: "Please complete the missing review and checks, then publish a fresh result." })}>Request a new result…</button></Match>
      <Match when={t().reviewActions?.unblockAction === "refresh"}><button class="btn btn-line btn-sm" onClick={() => void ws.store.reload()}>Refresh result</button></Match>
    </Switch>
  </div>}</Show></>;
}

/** The decision on a result, at the top of Changes: what it says, what was checked, and Accept or Request changes. */
export function ReviewBar(props: { m: SessionModel }) {
  const ws = useWs();
  const t = () => props.m.view.task;
  const r = () => t().result!;
  const [more, setMore] = createSignal(false);
  const failed = () => r().checks.some(c => c.outcome === "failed");
  const allPassed = () => r().checks.length > 0 && r().checks.every(c => c.outcome === "passed");
  const ready = (): Route => ({ name: "task", machine: props.m.view.machine.id, task: t().id, tab: "changes" });
  const reviewing = () => ["review", "ready_to_merge", "done"].includes(t().status) && !t().integration;
  return <section class="rv-bar" aria-label="Review this result">
    <div class="rv-h"><I n={t().status === "done" ? "check-circle" : "eye"} s={14} class="faint" /><span class="w500">Result v{r().version}</span>
      <span class={`st ${failed() ? "st-bad" : allPassed() ? "st-ok" : "st-idle"}`}><I n={failed() ? "x-circle" : allPassed() ? "check-circle" : "circle"} s={12} />{checksLine(r().checks)}</span></div>
    <div class={`rv-sum${more() ? " open" : ""}`}><ResultText text={r().summary} /></div>
    <Show when={r().summary.length > 280}><button class="lnk rv-more" onClick={() => setMore(!more())}>{more() ? "Show less" : "Show the whole result"}</button></Show>
    <Show when={props.m.view.project.hasGit === false}><div class="rv-note"><I n="folder" s={14} class="faint" /><span>{ws.t("changesWithoutGit")}</span></div></Show>
    <AcceptBlocker m={props.m} />
    <div class="rv-acts">
      <Switch fallback={<span class="cap muted grow">Accept moves the task to Ready to merge. Request changes sends a message and reopens it.</span>}>
        <Match when={t().status === "ready_to_merge"}><span class="cap muted grow">{t().acceptance ? `You accepted v${t().acceptance!.version}.` : "Checks passed."} {sameBranch(t(), props.m.view.target) ? "The task waits for your confirmation." : "The task waits in Ready to merge."}</span></Match>
        <Match when={t().status === "done"}><span class="cap muted grow">This task is closed.</span></Match>
      </Switch>
      <Show when={reviewing()}><button class="btn btn-line" {...ws.needs("request_changes")} onClick={() => ws.open({ kind: "changes", taskId: t().id })}>Request changes…</button></Show>
      <Show when={t().status === "ready_to_merge"} fallback={<Show when={t().status !== "done"}><span class="rv-accept"><AcceptButton m={props.m} /><span class="cap faint">Moves the task to Ready to merge</span></span></Show>}><Show when={!t().acceptance}><AcceptButton m={props.m} line /></Show><A class="btn btn-primary" to={ready()}>{sameBranch(t(), props.m.view.target) ? "Finish task" : "Open Ready to merge"}</A></Show>
    </div>
  </section>;
}

export function AcceptButton(props: { m: SessionModel; small?: boolean; line?: boolean }) {
  const ws = useWs();
  const t = () => props.m.view.task;
  const blocked = () => acceptBlocked(t());
  return <button class={`btn ${props.line ? "btn-line" : "btn-primary"}${props.small ? " btn-sm" : ""}`} aria-disabled={blocked() ? "true" : undefined} data-why={blocked()} {...ws.needs("accept_result")} onClick={() => { if (!blocked()) ws.accept(t().id); }}>Accept v{t().result?.version ?? ""}</button>;
}

/** The result in the conversation. It says what is not proven before it offers Accept. */
export function ResultCard(props: { m: SessionModel }) {
  const ws = useWs();
  const t = () => props.m.view.task;
  const r = () => t().result!;
  const c = () => props.m.session.changes;
  const changes = (): Route => ({ name: "task", machine: props.m.view.machine.id, task: t().id, tab: "changes", sub: "diff" });
  const ready = (): Route => ({ name: "task", machine: props.m.view.machine.id, task: t().id, tab: "changes" });
  return <div class="rcard">
    <div class="rcard-h"><I n={t().status === "done" ? "check-circle" : "eye"} s={14} class="faint" />Result v{r().version}<span class="r">{r().at != null ? `${clock(r().at!)}, ` : ""}{t().acceptance ? `accepted by you${t().acceptance!.at != null ? ` ${clock(t().acceptance!.at!)}` : ""}` : ws.store.mode === "real" ? "Runtime result" : "reviewed and checked"}</span></div>
    <div class="rcard-b"><ResultText text={r().summary} /></div>
    <Show when={r().checks.length} fallback={<div class="rcard-note cap muted">No checks reported</div>}><Outcomes checks={r().checks} /></Show>
    <For each={r().warnings}>{warning => <div class="rcard-note cap st-warn">{warning}</div>}</For>
    <OpenChecks checks={r().checks} taskId={t().id} />
    <Show when={stale(t())}><div class="nv"><I n="alert" s={14} class="st-warn" /><span>Stale result<span class="why">The worktree changed after this version, so it cannot be accepted. Request a new result.</span></span><span class="src">Recorded by runtime</span></div></Show>
    <Show when={props.m.view.project.hasGit === false}><div class="rcard-note cap muted">{ws.t("changesWithoutGit")}</div></Show>
    <AcceptBlocker m={props.m} />
    <div class="rcard-f"><Show when={c()}>{x => <span class="cap muted tnum">{x().files} files, <Counts add={x().add} del={x().del} /></span>}</Show><span class="grow"></span>
      <Show when={["review", "ready_to_merge", "done"].includes(t().status) && !t().integration}><button class="btn btn-sm" {...ws.needs("request_changes")} onClick={() => ws.open({ kind: "changes", taskId: t().id })}>Request changes…</button></Show>
      <A class="btn btn-line btn-sm" to={changes()}>Review changes</A>
      <Show when={t().status === "ready_to_merge"} fallback={<Show when={t().status !== "done"}><AcceptButton m={props.m} small /></Show>}><Show when={!t().acceptance}><AcceptButton m={props.m} small line /></Show><A class="btn btn-primary btn-sm" to={ready()}>{sameBranch(t(), props.m.view.target) ? "Finish task" : "Open Ready to merge"}</A></Show>
    </div>
  </div>;
}

function FileHead(props: { file: FileDiff; working: GitFile | null; onOpen: () => void; onStage: () => void; onRevert: () => void }) {
  const reason = () => props.working ? undefined : "Committed in this result. Stage and Revert act on uncommitted changes.";
  return <div class="fhead"><I n="file" s={14} class="faint" /><code>{props.file.path}</code><Counts add={props.file.add} del={props.file.del} />
    <span class="r">
      <button class="btn btn-sm" aria-disabled={reason() ? "true" : undefined} data-why={reason()} onClick={() => { if (!reason()) props.onStage(); }}>{props.working?.stage === "staged" ? "Unstage" : "Stage"}</button>
      <button class="btn btn-sm" aria-disabled={reason() ? "true" : undefined} data-why={reason()} onClick={() => { if (!reason()) props.onRevert(); }}>Revert</button>
      <button class="btn btn-sm" onClick={() => props.onOpen()}>Open in editor</button>
    </span></div>;
}

/** The diff review. Accept, Commit and Merge stay three separate groups, because they are three different decisions. */
export function ChangesPanel(props: { m: SessionModel; route: TaskRoute }) {
  const ws = useWs();
  const t = () => props.m.view.task;
  const state = () => ws.store.state();
  const review = createMemo(() => reviewFor(state(), t().id));
  const checkout = createMemo(() => taskCheckout(state(), t()));
  const [scope, setScope] = createSignal("result");
  const [layout, setLayout] = createSignal<"unified" | "split">("unified");
  const [hideSpace, setHideSpace] = createSignal(false);
  const [current, setCurrent] = createSignal<string | null>(null);
  const resultLabel = () => t().result ? `Result v${t().result!.version}` : "All changes";
  const scopes = createMemo(() => [{ id: "result", label: resultLabel(), sub: "The exact version you accept", paths: review()?.files.map(f => f.path) ?? [] }, ...(review()?.scopes ?? [])]);
  const active = () => scopes().find(s => s.id === scope()) ?? scopes()[0];
  const files = createMemo(() => { const shown = new Set(active().paths); return (review()?.files ?? []).filter(f => shown.has(f.path)); });
  const totals = () => files().reduce((n, f) => ({ add: n.add + f.add, del: n.del + f.del }), { add: 0, del: 0 });
  createEffect(on(files, list => { if (!list.some(f => f.path === current())) setCurrent(list[0]?.path ?? null); }));
  const drafts = () => review()?.comments.filter(c => !c.sent).length ?? 0;
  const working = (path: string) => checkout()?.files.find(f => f.path === path) ?? null;
  const go = (file: FileDiff) => ws.go({ name: "task", machine: props.route.machine, task: props.route.task, tab: "code", file: file.path, line: firstChanged(file) });
  const send = () => { if (drafts()) ws.run({ type: "send_comments", taskId: t().id, now: ws.store.clock() }); };
  const checks = () => t().result?.checks ?? [];
  const counts = () => checkCounts(checks());
  const askFix = (c: Check) => ws.run({ type: "request_changes", taskId: t().id, text: `Fix the failing check: ${c.name}. ${c.detail}`, now: ws.store.clock() });
  const canAsk = () => t().status === "review" || (t().status === "ready_to_merge" && !t().integration);
  const hunkActions = (path: string): HunkActions | undefined => {
    const c = checkout(), file = working(path);
    if (!c || !file) return { reason: "Committed in this result. Stage and Revert act on uncommitted changes.", stage: () => undefined, revert: () => undefined, staged: () => false };
    return { stage: i => ws.run({ type: "git_stage", checkoutId: c.id, path, staged: !file.hunks[i]?.staged, hunk: i }), revert: i => ws.open({ kind: "revert", checkoutId: c.id, path, hunk: i }), staged: i => !!file.hunks[i]?.staged };
  };
  const pick = (path: string) => { setCurrent(path); document.getElementById(`diff-${path}`)?.scrollIntoView?.({ block: "start" }); };
  const blockedMerge = () => mergeBlocked(t());
  const noGit = () => props.m.view.project.hasGit === false;
  const commitTo = (): Route => ({ name: "project", machine: props.route.machine, project: t().projectId, view: "git", scope: { kind: "all" }, checkout: checkout()?.id });
  return <section class="task-panel" aria-label="Changes" onKeyDown={e => { if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) { e.preventDefault(); send(); } }}>
    <Show when={t().result}><ReviewBar m={props.m} /></Show>
    <Show when={!noGit()}><div class="chg-tools">
      <button class="pick ws-pick" popovertarget="m-diff-scope"><span class="faint">Show</span>{active().label}<I n="chevron-down" s={12} /></button>
      <Pop id="m-diff-scope" class="pop-anchor" align="left" label="Which changes to show" style={{ width: "240px" }}>
        <For each={scopes()}>{s => <MenuItem label={s.label} sub={s.sub} check={s.id === active().id} onSelect={() => setScope(s.id)} />}</For>
      </Pop>
      <div class="seg" role="group" aria-label="Diff layout">
        <button aria-current={layout() === "unified" ? "true" : "false"} onClick={() => setLayout("unified")}>Unified</button>
        <button aria-current={layout() === "split" ? "true" : "false"} onClick={() => setLayout("split")}>Split</button>
      </div>
      <label class="sw-l"><span class="sw"><input type="checkbox" aria-label="Hide whitespace changes" checked={hideSpace()} onChange={e => setHideSpace(e.currentTarget.checked)} /><i></i></span>Hide whitespace</label>
      <span class="grow"></span>
      <span class="tnum">{files().length} file{files().length === 1 ? "" : "s"}, <Counts add={totals().add} del={totals().del} /></span>
      <Show when={review()?.sha}><span class="faint">Pinned result version</span></Show>
    </div></Show>
    <Show when={t().result && checks().length > 0}>
      <details class="evid" open>
        <summary><I n="chevron-right" s={12} class="faint" /><span class="w500">Evidence</span>
          <Show when={counts().passed}><span class="st st-ok"><I n="check-circle" s={12} />{counts().passed} passed</span></Show>
          <Show when={counts().failed}><span class="st st-bad"><I n="x-circle" s={12} />{counts().failed} failed</span></Show>
          <Show when={counts().not_run}><span class="st st-idle"><I n="circle" s={12} />{counts().not_run} not run</span></Show>
          <Show when={counts().unverified}><span class="st st-idle"><I n="help" s={12} />{counts().unverified} unverified</span></Show></summary>
        <div class="evid-b"><OpenChecks checks={checks().filter(c => c.kind === "manual")} /><For each={[...openChecks(checks()), ...checks().filter(c => c.outcome === "passed" && c.kind !== "manual")]}>{c => <>
          <div class="chk"><I n={CHECK_ICON[c.outcome].icon} s={14} class={CHECK_ICON[c.outcome].tone} /><span>{c.name}<Show when={c.detail || c.technicalReason}><span class="why">{c.technicalReason || c.detail}</span></Show></span>
            <span class="src"><RerunCheck taskId={t().id} check={c} /><Show when={c.outcome === "failed" && canAsk()} fallback={`${{ passed: "Passed", failed: "Failed", not_run: "Not run", unverified: "Unverified" }[c.outcome]}, ${sourceLabel(c, ws.t("unsourcedCheck")).toLowerCase()}`}>
              <button class="btn btn-line btn-sm ws-fix" {...ws.needs("request_changes")} onClick={() => askFix(c)}>Ask to fix</button></Show></span></div><Show when={c.command?.length}><details><summary>Command and output</summary><pre>{c.command!.join(" ")}</pre><div>Working folder: {c.cwd}</div><Show when={c.exitCode !== undefined}><div>Exit code: {c.exitCode}</div></Show><pre>{c.output || "No command output recorded."}</pre></details></Show></>}</For></div>
      </details>
    </Show>
    <Show when={files().length > 0} fallback={<Show when={!noGit()} fallback={<div class="note-panel"><I n="folder" class="faint" /><span class="w500">No diff without Git</span><span>{ws.t("changesWithoutGit")}</span></div>}><div class="note-panel"><span class="w500">{review()?.unavailableReason ? "Diff unavailable" : review() ? "No changes in this view" : "No diff recorded"}</span><span>{review()?.unavailableReason ?? (review() ? (scopes().length > 1 ? "Pick another view in Show." : "This result did not change any files.") : ws.t("noTaskDiff"))}</span></div></Show>}>
      <div class="chg-body">
        <div class="flist"><For each={files()}>{f => <button class="fitem" aria-current={current() === f.path ? "true" : undefined} onClick={() => pick(f.path)}><GitLetter status={f.status} /><span class="trunc">{f.path}</span><span class="r"><Counts add={f.add} del={f.del} /></span></button>}</For></div>
        <div class="dview"><For each={files()}>{f => <div id={`diff-${f.path}`}>
          <FileHead file={f} working={working(f.path)} onOpen={() => go(f)} onStage={() => { const c = checkout(), w = working(f.path); if (c && w) ws.run({ type: "git_stage", checkoutId: c.id, path: f.path, staged: w.stage !== "staged" }); }} onRevert={() => { const c = checkout(); if (c) ws.open({ kind: "revert", checkoutId: c.id, path: f.path }); }} />
          <Diff file={f} layout={layout()} hideSpace={hideSpace()} actions={hunkActions(f.path)} taskId={t().id} comments={review()?.comments.filter(c => c.path === f.path)} />
        </div>}</For></div>
      </div>
    </Show>
    <div class="chg-foot">
      <span class="cap muted">{drafts() ? `${drafts()} comment${drafts() === 1 ? "" : "s"}` : "No comments"}</span>
      <button class="btn btn-line btn-sm" aria-disabled={drafts() ? undefined : "true"} data-why={drafts() ? undefined : "Add a comment on a line first"} {...ws.needs("send_comments")} onClick={send}>Send<span class="key">Ctrl+Enter</span></button>
      <span class="grow"></span>
      <Show when={!noGit()}>
      <div class="grp"><Show when={checkout()} fallback={<button class="btn btn-line" aria-disabled="true" data-why={ws.t("noCheckoutRecord")}>Commit…</button>}><A class="btn btn-line" to={commitTo()}>Commit…</A></Show></div>
      <span class="vr"></span>
      <Show when={!sameBranch(t(), props.m.view.target)}><div class="grp"><button class="btn btn-line" aria-disabled={blockedMerge() ? "true" : undefined} data-why={blockedMerge()} {...ws.needs("integrate")} onClick={() => { if (!blockedMerge()) ws.open({ kind: "merge", taskId: t().id, how: "squash" }); }}>Merge…</button></div></Show>
      </Show>
    </div>
  </section>;
}
