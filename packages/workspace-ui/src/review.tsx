import { createEffect, createMemo, createSignal, createUniqueId, For, Match, on, Show, Switch } from "solid-js";
import type { Check, FileDiff, GitFile } from "@cortex/workspace-domain";
import { useWs } from "./ctx";
import { Counts, Diff, GitLetter, type HunkActions } from "./diff";
import { A, I, MenuItem, Pop } from "./kit";
import { ActionBar, Page, Region, Row, SectionHead, Strip, Toolbar, Whole } from "./layout";
import type { Route } from "./router";
import { checkCounts, clock, sameBranch } from "./selectors";
import { acceptBlocked, checksLine, firstChanged, mergeBlocked, openChecks, reviewFor, stale, taskCheckout } from "./review-model";
import type { SessionModel } from "./session-model";
import { CHECK_ICON } from "./worker";
import { ResultText } from "./result-text";

type TaskRoute = Extract<Route, { name: "task" }>;
const sourceLabel = (c: Check, none: string) => c.source === "runtime" ? "Recorded by runtime" : c.source === "worker" ? "Reported by worker" : none;
const OUTCOME_WORD = { passed: "Passed", failed: "Failed", not_run: "Not run", unverified: "Unverified" } as const;

/** One line of counts. Only reported outcomes are named, so absent ones never read as zeros. */
export function Outcomes(props: { checks: Check[] }) {
  const c = () => checkCounts(props.checks);
  const cell = (n: number, tone: string, icon: string, label: string) => <Show when={n > 0}><span class={`st ${tone}`}><I n={icon} s={12} />{n} {label}</span></Show>;
  return <div class="rv-outc">
    {cell(c().passed, "st-ok", "check-circle", "passed")}
    {cell(c().failed, "st-bad", "x-circle", "failed")}
    {cell(c().not_run, "st-idle", "circle", "not run")}
    {cell(c().unverified, "st-idle", "help", "unverified")}
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
const CommandOutput = (props: { c: Check }) => <Show when={props.c.command?.length}><details class="rv-cmd"><summary>Command and output</summary><pre>{props.c.command!.join(" ")}</pre>
  <Show when={props.c.cwd}><div>Working folder: {props.c.cwd}</div></Show><Show when={props.c.exitCode !== undefined}><div>Exit code: {props.c.exitCode}</div></Show><pre>{props.c.output || "No command output recorded."}</pre></details></Show>;

/**
 * The checks of a result by kind, each under its own heading on the rows' edge: what a person has to judge, what did not pass,
 * and what passed (every one with `all`, else only those with recorded output). A cut name or detail opens with a click.
 */
export function CheckRows(props: { checks: Check[]; taskId?: string; all?: boolean; onAsk?: (c: Check) => void }) {
  const ws = useWs();
  const manual = () => props.checks.filter(c => c.kind === "manual");
  const open = () => [...openChecks(props.checks)].sort((a, b) => (a.outcome === "failed" ? 0 : 1) - (b.outcome === "failed" ? 0 : 1));
  const passed = () => props.checks.filter(c => c.kind !== "manual" && c.outcome === "passed" && (props.all || !!c.output));
  const icon = (c: Check) => <I n={CHECK_ICON[c.outcome].icon} s={14} class={CHECK_ICON[c.outcome].tone} />;
  const detail = (c: Check) => c.detail || c.technicalReason ? <>{c.detail}<Show when={c.technicalReason}>{c.detail ? " " : ""}{c.kind === "policy" ? "Not run: " : "Could not check: "}{c.technicalReason}</Show></> : undefined;
  const source = (c: Check) => `${OUTCOME_WORD[c.outcome]}, ${sourceLabel(c, ws.t("unsourcedCheck")).toLowerCase()}`;
  return <>
    <Show when={manual().length > 0}><section class="rv-checks" aria-label="Check manually"><SectionHead title="Check manually" count={manual().length} />
      <For each={manual()}>{c => <Row lead={<I n="eye" s={14} class="faint" />} title={c.name} detail={c.detail || "Needs a person’s judgement. Not counted as passed or failed; does not block Accept."} />}</For>
    </section></Show>
    <Show when={open().length > 0}><section class="rv-checks" aria-label="Not passed"><SectionHead title="Not passed" count={open().length} />
      <For each={open()}>{c => <>
        <Row lead={icon(c)} title={c.name} detail={detail(c)} end={<>
          <Show when={props.taskId}>{id => <RerunCheck taskId={id()} check={c} />}</Show>
          <Show when={c.outcome === "failed" && props.onAsk} fallback={<span>{source(c)}</span>}><button class="btn btn-line btn-sm" {...ws.needs("request_changes")} onClick={() => props.onAsk!(c)}>Ask to fix</button></Show>
        </>} />
        <CommandOutput c={c} />
      </>}</For>
    </section></Show>
    <Show when={passed().length > 0}><section class="rv-checks" aria-label="Passed"><SectionHead title="Passed" count={passed().length} />
      <For each={passed()}>{c => <><Row lead={icon(c)} title={c.name} detail={c.detail || undefined} end={<span>{sourceLabel(c, ws.t("unsourcedCheck"))}</span>} /><CommandOutput c={c} /></>}</For>
    </section></Show>
  </>;
}
/** The checks a result card lists: what is not proven, before it offers Accept. */
export function OpenChecks(props: { checks: Check[]; taskId?: string }) {
  return <CheckRows checks={props.checks} taskId={props.taskId} />;
}

/** Whether the strip that says why Accept is off is on screen, so Accept can point at it. */
const acceptStripShown = (m: SessionModel) => { const t = m.view.task; return ["review", "ready_to_merge"].includes(t.status) && !t.acceptance && !!acceptBlocked(t); };

/**
 * When Accept is off, the reason is written out beside it with the one action that clears it, so a result never sits in
 * Review with no way forward. Nothing here accepts for the owner: each action only asks for what the reason names.
 */
export function AcceptBlocker(props: { m: SessionModel; id?: string }) {
  const ws = useWs();
  const t = () => props.m.view.task;
  const reason = () => acceptStripShown(props.m) ? acceptBlocked(t()) : null;
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
    <Show when={ws.store.acceptanceIssue()?.taskId === t().id && !t().acceptance && ws.store.acceptanceIssue()}>{issue => <Strip class="rv-accept-error" tone="fail" role="alert" icon={<I n="alert" s={14} class="st-bad" />}
      action={<button class="btn btn-line btn-sm" onClick={() => void ws.store.reload()}>Refresh result</button>}>{issue().message} Acceptance was not confirmed. Refresh the result before trying again.</Strip>}</Show>
    <Show when={reason()}>{why => <Strip id={props.id} class="rv-why" tone="warn" role="status" icon={<I n="alert" s={14} class="st-warn" />} action={<Switch>
      <Match when={stale(t())}><button class="btn btn-line btn-sm" {...ws.needs("request_changes")} onClick={() => ws.open({ kind: "changes", taskId: t().id, text: "The files changed after this version. Please check the work again and publish a fresh result." })}>Request a new result…</button></Match>
      <Match when={t().conflict !== "none"}><A class="btn btn-line btn-sm" to={conflict()}>Open the conflict</A></Match>
      <Match when={checks().length === 0}><button class="btn btn-line btn-sm" {...ws.needs("request_changes")} onClick={() => void ask("No checks were reported for this result. Run the checks this work needs and publish the result again.")}>Ask to run checks</button></Match>
      <Match when={canRerun()}>
        <button class="btn btn-line btn-sm" {...ws.needs("rerun_check")} onClick={e => void ws.busy(rerun(), e.currentTarget)}>{rerunnable().some(c => c.outcome === "failed") ? (rerunnable().length === 1 ? "Rerun the failed check" : "Rerun failed checks") : (rerunnable().length === 1 ? "Run the missing check" : "Run the missing checks")}</button>
        <Show when={rerunnable().some(c => c.outcome === "failed")}><button class="btn btn-line btn-sm" {...ws.needs("request_changes")} onClick={() => void ask(`Fix the failing checks: ${rerunnable().filter(c => c.outcome === "failed").map(c => c.name).join(", ")}.`)}>Ask to fix</button></Show>
      </Match>
      <Match when={t().reviewActions?.unblockAction === "request_changes" || t().result?.acceptanceAvailable === false}><button class="btn btn-line btn-sm" {...ws.needs("request_changes")} onClick={() => ws.open({ kind: "changes", taskId: t().id, text: "Please complete the missing review and checks, then publish a fresh result." })}>Request a new result…</button></Match>
      <Match when={t().reviewActions?.unblockAction === "refresh"}><button class="btn btn-line btn-sm" onClick={() => void ws.store.reload()}>Refresh result</button></Match>
    </Switch>}>{why()}.</Strip>}</Show></>;
}

/** The decision on a result, at the top of Changes: the version, its checks, Request changes and Accept, in one row. */
export function ReviewBar(props: { m: SessionModel }) {
  const ws = useWs();
  const why = createUniqueId();
  const t = () => props.m.view.task;
  const r = () => t().result!;
  const failed = () => r().checks.some(c => c.outcome === "failed");
  const allPassed = () => r().checks.length > 0 && r().checks.every(c => c.outcome === "passed");
  const ready = (): Route => ({ name: "task", machine: props.m.view.machine.id, task: t().id, tab: "changes" });
  const reviewing = () => ["review", "ready_to_merge", "done"].includes(t().status) && !t().integration;
  const hint = () => t().status === "ready_to_merge" ? `${t().acceptance ? `You accepted v${t().acceptance!.version}.` : "Checks passed."} ${sameBranch(t(), props.m.view.target) ? "The task waits for your confirmation." : "The task waits in Ready to merge."}`
    : t().status === "done" ? "This task is closed." : "Accept moves the task to Ready to merge. Request changes reopens it.";
  const describedBy = () => acceptStripShown(props.m) ? why : undefined;
  return <section class="rv-bar" aria-label="Review this result">
    <ActionBar start={<><I n={t().status === "done" ? "check-circle" : "eye"} s={14} class="faint" /><span class="w500">Result v{r().version}</span>
      <span class={`st ${failed() ? "st-bad" : allPassed() ? "st-ok" : "st-idle"}`}><I n={failed() ? "x-circle" : allPassed() ? "check-circle" : "circle"} s={12} />{checksLine(r().checks)}</span></>} hint={hint()}>
      <Show when={reviewing()}><button class="btn btn-line" {...ws.needs("request_changes")} onClick={() => ws.open({ kind: "changes", taskId: t().id })}>Request changes…</button></Show>
      <Show when={t().status === "ready_to_merge"} fallback={<Show when={t().status !== "done"}><AcceptButton m={props.m} describedBy={describedBy()} /></Show>}><Show when={!t().acceptance}><AcceptButton m={props.m} line describedBy={describedBy()} /></Show><A class="btn btn-primary" to={ready()}>{sameBranch(t(), props.m.view.target) ? "Finish task" : "Open Ready to merge"}</A></Show>
    </ActionBar>
    <AcceptBlocker m={props.m} id={why} />
  </section>;
}

export function AcceptButton(props: { m: SessionModel; small?: boolean; line?: boolean; describedBy?: string }) {
  const ws = useWs();
  const t = () => props.m.view.task;
  const blocked = () => acceptBlocked(t());
  return <button class={`btn ${props.line ? "btn-line" : "btn-primary"}${props.small ? " btn-sm" : ""}`} aria-describedby={blocked() ? props.describedBy : undefined} aria-disabled={blocked() ? "true" : undefined} data-why={blocked()} {...ws.needs("accept_result")} onClick={() => { if (!blocked()) ws.accept(t().id); }}>Accept v{t().result?.version ?? ""}</button>;
}

/** What the result says: the summary, cut to a few lines until asked for the whole, and where its changes are without Git. */
function ResultSummary(props: { m: SessionModel }) {
  const ws = useWs();
  const r = () => props.m.view.task.result!;
  const [more, setMore] = createSignal(false);
  const id = createUniqueId();
  return <div class="rv-top-in">
    <div id={id} class={`rv-sum${more() ? " open" : ""}`}><ResultText text={r().summary} /></div>
    <Show when={r().summary.length > 280}><button class="lnk rv-more" aria-controls={id} aria-expanded={more()} onClick={() => setMore(!more())}>{more() ? "Show less" : "Show the whole result"}</button></Show>
    <Show when={props.m.view.project.hasGit === false}><div class="rv-note"><I n="folder" s={14} class="faint" /><span>{ws.t("changesWithoutGit")}</span></div></Show>
  </div>;
}

/** The result in the conversation. It says what is not proven before it offers Accept. */
export function ResultCard(props: { m: SessionModel }) {
  const ws = useWs();
  const why = createUniqueId();
  const t = () => props.m.view.task;
  const r = () => t().result!;
  const c = () => props.m.session.changes;
  const changes = (): Route => ({ name: "task", machine: props.m.view.machine.id, task: t().id, tab: "changes", sub: "diff" });
  const ready = (): Route => ({ name: "task", machine: props.m.view.machine.id, task: t().id, tab: "changes" });
  const describedBy = () => acceptStripShown(props.m) ? why : undefined;
  return <div class="rcard">
    <div class="rcard-h"><I n={t().status === "done" ? "check-circle" : "eye"} s={14} class="faint" />Result v{r().version}<span class="r">{r().at != null ? `${clock(r().at!)}, ` : ""}{t().acceptance ? `accepted by you${t().acceptance!.at != null ? ` ${clock(t().acceptance!.at!)}` : ""}` : ws.store.mode === "real" ? "Runtime result" : "reviewed and checked"}</span></div>
    <div class="rcard-b"><ResultText text={r().summary} /></div>
    <Show when={r().checks.length} fallback={<div class="rcard-note cap muted">No checks reported</div>}><Outcomes checks={r().checks} /></Show>
    <For each={r().warnings}>{warning => <div class="rcard-note cap st-warn">{warning}</div>}</For>
    <OpenChecks checks={r().checks} taskId={t().id} />
    <Show when={stale(t())}><Row class="rv-stale" lead={<I n="alert" s={14} class="st-warn" />} title="Stale result" detail="The worktree changed after this version, so it cannot be accepted. Request a new result." end={<span>Recorded by runtime</span>} /></Show>
    <Show when={props.m.view.project.hasGit === false}><div class="rcard-note cap muted">{ws.t("changesWithoutGit")}</div></Show>
    <AcceptBlocker m={props.m} id={why} />
    <ActionBar place="card" class="rv-card-f" label="Result actions" start={<><A class="lnk" to={changes()}>Review changes</A><Show when={c()}>{x => <span class="cap muted tnum">{x().files} files, <Counts add={x().add} del={x().del} /></span>}</Show></>}>
      <Show when={["review", "ready_to_merge", "done"].includes(t().status) && !t().integration}><button class="btn btn-sm" {...ws.needs("request_changes")} onClick={() => ws.open({ kind: "changes", taskId: t().id })}>Request changes…</button></Show>
      <Show when={t().status === "ready_to_merge"} fallback={<Show when={t().status !== "done"}><AcceptButton m={props.m} small describedBy={describedBy()} /></Show>}><Show when={!t().acceptance}><AcceptButton m={props.m} small line describedBy={describedBy()} /></Show><A class="btn btn-primary btn-sm" to={ready()}>{sameBranch(t(), props.m.view.target) ? "Finish task" : "Open Ready to merge"}</A></Show>
    </ActionBar>
  </div>;
}

/** A file's head in the diff. Stage and Revert act on uncommitted changes, so a file committed in the result offers neither. */
function FileHead(props: { file: FileDiff; working: GitFile | null; onOpen: () => void; onStage: () => void; onRevert: () => void }) {
  return <div class="fhead"><I n="file" s={14} class="faint" /><Whole class="mono fhead-path" text={props.file.path} /><Counts add={props.file.add} del={props.file.del} />
    <span class="r">
      <Show when={props.working} fallback={<span class="cap faint">Committed in this result</span>}>{w => <>
        <button class="btn btn-sm" onClick={() => props.onStage()}>{w().stage === "staged" ? "Unstage" : "Stage"}</button>
        <button class="btn btn-sm" onClick={() => props.onRevert()}>Revert</button>
      </>}</Show>
      <button class="btn btn-sm" onClick={() => props.onOpen()}>Open in editor</button>
    </span></div>;
}

/** Evidence under the summary: counts in its head, then every check. It starts open unless every check passed. */
function Evidence(props: { m: SessionModel; onAsk: (c: Check) => void }) {
  const t = () => props.m.view.task;
  const checks = () => t().result?.checks ?? [];
  const counts = () => checkCounts(checks());
  const canAsk = () => t().status === "review" || (t().status === "ready_to_merge" && !t().integration);
  const settled = () => checks().every(c => c.kind !== "manual" && c.outcome === "passed");
  return <details class="evid" open={!settled()}>
    <summary class="evid-h"><I n="chevron-right" s={12} class="faint" /><span class="w500">Evidence</span><span class="grow"></span>
      <Show when={counts().passed}><span class="st st-ok"><I n="check-circle" s={12} />{counts().passed} passed</span></Show>
      <Show when={counts().failed}><span class="st st-bad"><I n="x-circle" s={12} />{counts().failed} failed</span></Show>
      <Show when={counts().not_run}><span class="st st-idle"><I n="circle" s={12} />{counts().not_run} not run</span></Show>
      <Show when={counts().unverified}><span class="st st-idle"><I n="help" s={12} />{counts().unverified} unverified</span></Show></summary>
    <div class="evid-b"><CheckRows checks={checks()} taskId={t().id} all onAsk={canAsk() ? props.onAsk : undefined} /></div>
  </details>;
}

/**
 * The diff review. Accept, Commit and Merge stay three separate groups, because they are three different decisions.
 * With a diff, the result and its evidence share one bounded region and the diff keeps the rest; without one, the page scrolls
 * as a whole. Comments need a line of a diff, so a project without Git has no comment bar.
 */
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
  const askFix = (c: Check) => ws.run({ type: "request_changes", taskId: t().id, text: `Fix the failing check: ${c.name}. ${c.detail}`, now: ws.store.clock() });
  const hunkActions = (path: string): HunkActions | undefined => {
    const c = checkout(), file = working(path);
    if (!c || !file) return undefined;
    return { stage: i => ws.run({ type: "git_stage", checkoutId: c.id, path, staged: !file.hunks[i]?.staged, hunk: i }), revert: i => ws.open({ kind: "revert", checkoutId: c.id, path, hunk: i }), staged: i => !!file.hunks[i]?.staged };
  };
  const pick = (path: string) => { setCurrent(path); document.getElementById(`diff-${path}`)?.scrollIntoView?.({ block: "start" }); };
  const blockedMerge = () => mergeBlocked(t());
  const noGit = () => props.m.view.project.hasGit === false;
  const bounded = () => !noGit() && files().length > 0;
  const commitTo = (): Route => ({ name: "project", machine: props.route.machine, project: t().projectId, view: "git", scope: { kind: "all" }, checkout: checkout()?.id });
  const top = () => <>
    <Show when={t().result}><ResultSummary m={props.m} /></Show>
    <Show when={t().result && checks().length > 0}><Evidence m={props.m} onAsk={askFix} /></Show>
  </>;
  const tools = () => <Toolbar small class="chg-tools" label="Diff view" start={<>
    <button class="pick ws-pick" popovertarget="m-diff-scope"><span class="faint">Show</span>{active().label}<I n="chevron-down" s={12} /></button>
    <Pop id="m-diff-scope" class="pop-anchor" align="left" label="Which changes to show" style={{ width: "240px" }}>
      <For each={scopes()}>{s => <MenuItem label={s.label} sub={s.sub} check={s.id === active().id} onSelect={() => setScope(s.id)} />}</For>
    </Pop>
    <div class="seg" role="group" aria-label="Diff layout">
      <button aria-current={layout() === "unified" ? "true" : "false"} onClick={() => setLayout("unified")}>Unified</button>
      <button aria-current={layout() === "split" ? "true" : "false"} onClick={() => setLayout("split")}>Split</button>
    </div>
    <label class="sw-l"><span class="sw"><input type="checkbox" aria-label="Hide whitespace changes" checked={hideSpace()} onChange={e => setHideSpace(e.currentTarget.checked)} /><i></i></span><span class="l-tight-hide">Hide whitespace</span></label>
  </>} end={<>
    <span class="tnum">{files().length} file{files().length === 1 ? "" : "s"}, <Counts add={totals().add} del={totals().del} /></span>
    <Show when={review()?.sha}><span class="faint l-compact-hide">Pinned result version</span></Show>
  </>} />;
  return <section class="task-panel rv-panel" aria-label="Changes" onKeyDown={e => { if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) { e.preventDefault(); send(); } }}>
    <Page class={bounded() ? "l-work" : undefined} scroll={bounded() ? "none" : "vertical"}
      head={<Show when={t().result}><ReviewBar m={props.m} /></Show>}
      foot={<Show when={!noGit()}><ActionBar class="rv-foot" label="Review comments" start={<>
        <span class="cap muted">{drafts() ? `${drafts()} comment${drafts() === 1 ? "" : "s"}` : "No comments"}</span>
        <button class="btn btn-line" aria-disabled={drafts() ? undefined : "true"} data-why={drafts() ? undefined : "Add a comment on a line first"} {...ws.needs("send_comments")} onClick={send}>Send<span class="key">Ctrl+Enter</span></button>
      </>}>
        <Show when={checkout()} fallback={<button class="btn btn-line" aria-disabled="true" data-why={ws.t("noCheckoutRecord")}>Commit…</button>}><A class="btn btn-line" to={commitTo()}>Commit…</A></Show>
        <Show when={!sameBranch(t(), props.m.view.target)}><span class="vr"></span><button class="btn btn-line" aria-disabled={blockedMerge() ? "true" : undefined} data-why={blockedMerge()} {...ws.needs("integrate")} onClick={() => { if (!blockedMerge()) ws.open({ kind: "merge", taskId: t().id, how: "squash" }); }}>Merge…</button></Show>
      </ActionBar></Show>}>
      <Show when={bounded()} fallback={<>
        {top()}
        <Show when={!noGit()}>{tools()}<div class="rv-note rv-diff-note"><span class="w500">{review()?.unavailableReason ? "Diff unavailable." : review() ? "No changes in this view." : "No diff recorded."}</span>
          <span>{review()?.unavailableReason ?? (review() ? (scopes().length > 1 ? "Pick another view in Show." : "This result did not change any files.") : ws.t("noTaskDiff"))}</span></div></Show>
      </>}>
        <Region class="rv-top">{top()}</Region>
        {tools()}
        <div class="chg-body">
          <div class="flist"><For each={files()}>{f => <button class="fitem" aria-current={current() === f.path ? "true" : undefined} onClick={() => pick(f.path)}><GitLetter status={f.status} /><span class="trunc">{f.path}</span><span class="r"><Counts add={f.add} del={f.del} /></span></button>}</For></div>
          <div class="dview"><For each={files()}>{f => <div id={`diff-${f.path}`}>
            <FileHead file={f} working={working(f.path)} onOpen={() => go(f)} onStage={() => { const c = checkout(), w = working(f.path); if (c && w) ws.run({ type: "git_stage", checkoutId: c.id, path: f.path, staged: w.stage !== "staged" }); }} onRevert={() => { const c = checkout(); if (c) ws.open({ kind: "revert", checkoutId: c.id, path: f.path }); }} />
            <Diff file={f} layout={layout()} hideSpace={hideSpace()} actions={hunkActions(f.path)} taskId={t().id} comments={review()?.comments.filter(c => c.path === f.path)} />
          </div>}</For></div>
        </div>
      </Show>
    </Page>
  </section>;
}
