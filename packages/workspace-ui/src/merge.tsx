import { createMemo, createSignal, createUniqueId, For, Show } from "solid-js";
import type { BlockChoice, CodeSide, ConflictBlock, ConflictFile, MergeState } from "@cortex/workspace-domain";
import { useWs } from "./ctx";
import { Code, GitLetter } from "./diff";
import { createTicker } from "./freshness";
import { A, Count, I } from "./kit";
import { ActionBar, Row, Strip, Whole } from "./layout";
import { AcceptBlocker, AcceptButton, OpenChecks, Outcomes } from "./review";
import { ResultText } from "./result-text";
import { acceptBlocked, behindTarget, conflictFacts, repoOf, cxOf, blockWord, fileName, fileResolved, finishBlocked, RESOLVE_STEPS, taskCheckout, type Cx } from "./review-model";
import type { Route } from "./router";
import { clock, sameBranch } from "./selectors";
import { closeBlocked, duration, type SessionModel } from "./session-model";

type TaskRoute = Extract<Route, { name: "task" }>;
/** Branch to branch. A long name shortens, and a press shows the whole of it. */
const Flow = (props: { from: string; to: string; behind?: number | null }) => <span class="flow"><Whole class="mono" text={props.from} /><I n="arrow-right" s={14} class="faint" /><Whole class="mono" text={props.to} />
  <Show when={props.behind}><span class="st st-warn ws-flow-b"><I n="alert" s={12} />{props.behind} behind</span></Show></span>;

const useTimer = (since: () => number | null) => {
  const ws = useWs();
  const [now, setNow] = createSignal(ws.store.clock());
  createTicker(() => setNow(ws.store.clock()), 1000);
  return () => {
    const at = since();
    if (at === null) return "";
    return duration(now() - at);
  };
};
/** The steps of a Cortex resolution, in the order they run. Done ones are ticks, the current one spins, the rest wait. */
function Steps(props: { merge: MergeState; line?: boolean }) {
  const file = () => fileName(conflictFacts(props.merge).firstOpen ?? "the conflict");
  const step = (label: string, i: number) => {
    const at = props.merge.step;
    return i < at ? <span class="cx-step"><I n="check-circle" s={14} class="st-ok" />{label}</span>
      : i === at ? <span class="cx-step now"><I n="loader" s={14} class="spin st-run" />{label}</span>
      : <span class="cx-step todo"><I n="circle" s={14} />{label}</span>;
  };
  return <For each={RESOLVE_STEPS(file())}>{(label, i) => <><Show when={props.line && i() > 0}><I n="chevron-right" s={12} class="faint" /></Show>{step(label, i())}</>}</For>;
}

/** Ready to merge: nothing reaches a branch from here until the owner confirms it in the dialog. */
export function ReadyPanel(props: { m: SessionModel; route: TaskRoute }) {
  const ws = useWs();
  const t = () => props.m.view.task;
  const view = () => props.m.view;
  const merge = () => t().merge;
  const cx = (): Cx => cxOf(t());
  const checkout = createMemo(() => taskCheckout(ws.store.state(), t()));
  const behind = () => behindTarget(checkout(), view().target);
  const at = (sub?: "conflict" | "diff"): Route => ({ name: "task", machine: props.route.machine, task: props.route.task, tab: "changes", sub });
  const dependents = createMemo(() => ws.store.state().tasks.filter(x => x.dependencies.includes(t().id)));
  const abort = () => ws.run({ type: "abort_merge", taskId: t().id, now: ws.store.clock() });
  const elapsed = useTimer(() => merge()?.handedAt ?? null);
  // A folder without Git has nothing to merge: the way out of Ready is closing, with Git set up only on request.
  const noGit = () => view().project.hasGit === false;
  const remote = () => !!repoOf(ws.store.state(), t().projectId)?.remote.trim();
  const same = () => !noGit() && sameBranch(t(), view().target);
  const contained = () => t().integrationFacts?.targetBranch === view().target && t().integrationFacts?.alreadyInTarget === true;
  const finish = () => finishBlocked(t());
  const closeWhy = () => closeBlocked(props.m);
  const gitPage = (): Route => ({ name: "project", machine: view().machine.id, project: t().projectId, view: "git", scope: { kind: "all" } });
  const CloseAsIs = () => <button class="btn btn-primary btn-sm" aria-disabled={closeWhy() ? "true" : undefined} data-why={closeWhy()} {...ws.needs("close_task")} onClick={() => { if (!closeWhy()) ws.closeTask(t().id); }}>Close as is…</button>;
  const SetUpGit = () => <A class="btn btn-line btn-sm" to={gitPage()}>Set up Git…</A>;
  // Accept points at the strip that says why it is off, so its bar keeps its own words for the other controls.
  const why = createUniqueId();
  const acceptWhy = () => ["review", "ready_to_merge"].includes(t().status) && !t().acceptance && acceptBlocked(t()) ? why : undefined;
  const bodyConflict = () => { const f = merge() ? conflictFacts(merge()!) : null; return f?.firstOpen ? <>Updating <span class="mono">{t().checkout.branch}</span> from <span class="mono">{view().target}</span> stopped with {blockWord(f.left)} in <span class="mono">{fileName(f.firstOpen)}</span>. Nothing was stashed or reset, and this task cannot merge until the conflict is resolved.</> : <>Merging <span class="mono">{view().target}</span> stopped at a conflict. Nothing was stashed or reset.</>; };
  return <section class="task-panel" aria-label={same() ? "Finish task" : "Ready to merge"}><div class="ready"><div class="ready-in">
    <Show when={t().integration?.method === "squash"}>{(() => {
      const i = () => t().integration!;
      return <div class="rcard"><div class="rcard-h"><I n="merge" s={14} class="st-merge" />Merged into <span class="mono">{i().targetBranch}</span><span class="r">Squash merge</span></div>
        <div class="rcard-b"><ResultText text={t().result?.summary} /></div><ActionBar place="card" class="rv-card-f"><A class="btn btn-line btn-sm" to={at("diff")}>Review the diff</A></ActionBar></div>;
    })()}</Show>
    <Show when={!same()}>
    <Show when={cx() === "none" && t().conflict === "conflict" || cx() === "failed"}>
      <div class="rcard">
        <div class="rcard-h"><I n="alert" s={14} class="st-bad" />Conflict with {view().target}<span class="r">Merge blocked</span></div>
        <div class="rcard-b">{bodyConflict()}</div>
        <Show when={merge()?.failure}><Row class="rv-stale" lead={<I n="alert" s={14} class="st-warn" />} title={merge()!.failure} end={<span>Reported by the merge</span>} /></Show>
        <div class="ways2">
          <div><span class="t">Resolve it yourself</span><span class="d">Choose ours, theirs or both for each block, then finish the merge.</span><A class="btn btn-line btn-sm" to={at("conflict")}>Open the conflict</A></div>
          <div><span class="t">Resolve automatically</span><span class="d">It resolves the block and runs the checks. The result comes back here for your review.</span><button class="btn btn-primary btn-sm" {...ws.needs("resolve_with_cortex")} onClick={() => ws.open({ kind: "cortex", taskId: t().id })}>Resolve automatically…</button></div>
        </div>
        <ActionBar place="card" class="rv-card-f" start={<Flow from={t().checkout.branch} to={view().target} />}><button class="btn btn-line btn-sm" {...ws.needs("abort_merge")} onClick={abort}>Abort merge</button></ActionBar>
      </div>
    </Show>
    <Show when={cx() === "running" && merge()}>{m => <div class="rcard">
      <div class="rcard-h"><I n="loader" s={14} class="spin st-run" />Resolving the conflict<span class="r tnum">{elapsed()}</span></div>
      <div class="rcard-b">The result comes back here for your review. Nothing reaches <span class="mono">{view().target}</span> until you confirm it.</div>
      <div class="cx-steps"><Steps merge={m()} /></div>
      <ActionBar place="card" class="rv-card-f" start={<Flow from={t().checkout.branch} to={view().target} />}><A class="btn btn-sm" to={at("conflict")}>Open the conflict</A>
        <button class="btn btn-sm" {...ws.needs("stop_resolution")} onClick={() => ws.run({ type: "stop_resolution", taskId: t().id, now: ws.store.clock() })}>Stop and resolve myself</button><button class="btn btn-line btn-sm" {...ws.needs("abort_merge")} onClick={abort}>Abort merge</button></ActionBar>
    </div>}</Show>
    </Show>
    <Show when={t().status === "ready_to_merge" && same() && !contained()}>
      <div class="rcard"><div class="rcard-h"><I n="check" s={14} />{t().integrationFacts?.commitAvailable ? `Commit to ${view().target}` : "Review task"}</div>
        <div class="rcard-b">{t().integrationFacts?.reason ?? (t().integrationFacts?.commitAvailable ? "This task works directly on its target branch. Only its recorded files can be committed here." : "Task-owned file evidence is unavailable. Close as is, or use Done outside the app after finishing in Git.")}
          <Show when={t().integrationFacts?.otherUncommitted}> Other tasks have uncommitted files in this shared checkout. They will not be included.</Show></div>
        <Outcomes checks={t().result?.checks ?? []} />
        <Show when={t().integrationFacts?.commitAvailable}><OpenChecks checks={t().result?.checks ?? []} taskId={t().id} /><AcceptBlocker m={props.m} id={why} /></Show>
        <ActionBar place="card" class="rv-card-f" start={<A class="lnk" to={at("diff")}>Review the diff</A>}><CloseAsIs />
          <Show when={t().integrationFacts?.commitAvailable && !t().acceptance}><AcceptButton m={props.m} small line describedBy={acceptWhy()} /></Show>
          <Show when={t().integrationFacts?.commitAvailable}><button class="btn btn-primary btn-sm" aria-disabled={finish() ? "true" : undefined} data-why={finish()} {...ws.needs("integrate")} onClick={() => { if (!finish()) ws.open({ kind: "merge", taskId: t().id, how: "squash" }); }}>Commit to <span class="l-branch">{view().target}</span>…</button></Show>
        </ActionBar></div>
    </Show>
    <Show when={t().integration?.method === "commit"}><div class="rcard"><div class="rcard-h"><I n="check" s={14} />Committed to {t().integration!.targetBranch}</div><div class="rcard-b"><ResultText text={t().result?.summary} /></div></div></Show>
    <Show when={t().status === "ready_to_merge" && contained()}>
      <div class="rcard"><div class="rcard-h"><I n="check" s={14} />Already in {t().integrationFacts!.targetBranch}</div>
        <div class="rcard-b">The task's recorded content landed in the local target branch. No Git operation is needed. Close it when you are ready; this records your closing decision only.</div>
        <ActionBar place="card" class="rv-card-f"><button class="btn btn-primary btn-sm" {...ws.needs("close_task")} onClick={() => ws.closeTask(t().id, "already_in_target")}>Close task…</button></ActionBar>
      </div>
    </Show>
    <Show when={t().status === "ready_to_merge" && !contained() && !same() && !t().acceptance && t().conflict === "none"}>
      <div class="rcard">
        <div class="rcard-h"><I n="merge" s={14} class="st-merge" />Ready to merge<Show when={t().resolvedBy}>{t().resolvedBy === "cortex" ? ", resolved automatically" : ", resolved by you"}</Show><span class="r">{merge()?.finishedAt != null ? `Finished ${clock(merge()!.finishedAt)}` : ""}</span></div>
        <div class="rcard-b">This is result v{t().result?.version}. <Show when={t().resolvedBy === "cortex"}>{merge()?.files.flatMap(f => f.blocks).find(b => b.choice === "cortex")?.cortexNote.replace("Resolved automatically: ", "Kept ") ?? ""} </Show>{noGit()
          ? "This folder has no Git repository, so there is nothing to merge. Accept it to record your decision, and close the task when you are done with it."
          : "Review and merge records your acceptance of exactly this version, then merges it once you confirm the branch."}</div>
        <Outcomes checks={t().result?.checks ?? []} />
        <OpenChecks checks={t().result?.checks ?? []} taskId={t().id} />
        <AcceptBlocker m={props.m} id={why} />
        <ActionBar place="card" class="rv-card-f" start={<><Show when={!noGit()}><Flow from={t().checkout.branch} to={view().target} /></Show><A class="lnk" to={at("diff")}>{noGit() ? "Review the result" : "Review the diff"}</A></>}>
          <Show when={merge()}><button class="btn btn-line btn-sm" {...ws.needs("abort_merge")} onClick={abort}>Abort merge</button></Show>
          <Show when={noGit()} fallback={<>
            <AcceptButton m={props.m} small line describedBy={acceptWhy()} />
            <button class="btn btn-primary btn-sm" aria-disabled={finish() ? "true" : undefined} data-why={finish()} {...ws.needs("integrate")} onClick={() => { if (!finish()) ws.open({ kind: "merge", taskId: t().id, how: "squash" }); }}>Review and merge…</button>
          </>}>
            <SetUpGit /><AcceptButton m={props.m} small line describedBy={acceptWhy()} /><CloseAsIs />
          </Show></ActionBar>
      </div>
    </Show>
    <Show when={t().status === "ready_to_merge" && !contained() && !same() && t().acceptance}>{a => <div class="rcard">
      <div class="rcard-h"><I n="merge" s={14} class="st-merge" />Ready to merge<span class="r">Accepted v{a().version} by you{a().at != null ? `, ${clock(a().at!)}` : ""}</span></div>
      <div class="rcard-b"><ResultText text={t().result?.summary} /><Show when={view().sprint && dependents().length > 0}> This task is part of the {view().sprint!.description} sprint, and <A class="lnk" to={{ name: "task", machine: view().machine.id, task: dependents()[0].id }}>{dependents()[0].title}</A> depends on its checked result. The queue does not wait for a merge.</Show></div>
      <Outcomes checks={t().result?.checks ?? []} />
      <Show when={t().integration?.method === "pr"}><Row class="rv-stale" lead={<I n="merge" s={14} class="faint" />} title={<>Pull request created into <span class="mono">{t().integration!.targetBranch}</span></>} detail={<>{t().integration!.draft ? "Draft. " : ""}{ws.t("prNote")}</>} end={<span>{ws.store.mode === "demo" ? "Mock" : "GitHub CLI"}</span>} /></Show>
      <Show when={!noGit()} fallback={<ActionBar place="card" class="rv-card-f"><SetUpGit /><CloseAsIs /></ActionBar>}>
      <ActionBar place="card" class="rv-card-f" start={<Flow from={t().checkout.branch} to={view().target} behind={behind()} />}>
        <Show when={remote()}><button class="btn btn-sm" aria-disabled={view().machine.ghAvailable === false || t().integration?.method === "pr" ? "true" : undefined} data-why={view().machine.ghAvailable === false ? "Install and sign in to GitHub CLI (gh) to create a pull request" : t().integration?.method === "pr" ? "A pull request is already open for this version" : undefined} {...ws.needs("integrate")} onClick={() => { if (view().machine.ghAvailable !== false && t().integration?.method !== "pr") ws.open({ kind: "merge", taskId: t().id, how: "pr" }); }}>Create PR…</button></Show>
        <button class="btn btn-primary btn-sm" {...ws.needs("integrate")} onClick={() => ws.open({ kind: "merge", taskId: t().id, how: "squash" })}>Merge…</button></ActionBar>
      </Show>
    </div>}</Show>
    <Show when={remote() && t().integrationFacts?.pushStatus}>{status => <div class="cap faint">{status() === "pushed" ? "Target branch is pushed (last fetched state)." : status() === "not_pushed" ? "Target branch has unpushed commits (last fetched state)." : "Push status unknown; set a tracking upstream and fetch to check."} This never blocks closing.</div>}</Show>
    <div class="cap faint">{contained() ? "Close task records your confirmation; no Git operation is needed." : same() ? "Close as is keeps the current result. Done outside the app is also available from the task menu or by moving the card to Done." : noGit()
      ? "Every task stops here. Without Git there is nothing to merge: close the task once you are done with its result, or set up Git to merge it."
      : remote() ? "Every task stops here, including tasks in an automatic sprint. You choose the branch and confirm each merge or pull request." : "You choose the local target branch and confirm each merge. No remote is needed."}</div>
  </div></div></section>;
}

const Lines = (props: { path: string; side: Pick<CodeSide, "start" | "lines" | "hit"> }) => <div class="code ws-free"><For each={props.side.lines}>{(line, i) =>
  <div class={`cl${props.side.hit.includes(i()) ? " hit" : ""}`}><span class="n">{props.side.start + i()}</span><span class="m"></span><span class="c"><Code path={props.path} text={line} /></span></div>}</For></div>;
const Column = (props: { title: string; side: CodeSide; path: string }) => <div><div class="cf-col"><b>{props.title}</b><span class="mono">{props.side.ref}</span></div><Lines path={props.path} side={props.side} /></div>;

function Block(props: { taskId: string; file: ConflictFile; block: ConflictBlock; cx: Cx }) {
  const ws = useWs();
  const pick = (choice: Exclude<BlockChoice, "cortex">) => ws.run({ type: "resolve_block", taskId: props.taskId, path: props.file.path, blockId: props.block.id, choice });
  const choice = () => props.block.choice;
  const preview = () => props.block.results[choice() ?? "both"];
  const label = () => choice() === "cortex" ? "Resolved automatically. You review it in Ready to merge" : props.cx === "running" ? "Current preview. It will be replaced" : choice() ? `Your choice: Accept ${choice()}. This is what will be committed` : "Preview of Accept both. Nothing is chosen yet";
  // Real helpers use this note for conflicts that require Git/editor resolution.
  // Demo blocks retain a future Cortex resolution note before any choice is made.
  const manualNote = () => ws.store.mode === "real" ? props.block.cortexNote : "";
  const editable = () => props.cx === "none" || props.cx === "failed";
  const btn = (c: Exclude<BlockChoice, "cortex">, text: string) => <button class={`btn ${choice() === c ? "btn-primary" : "btn-line"} btn-sm`} aria-pressed={choice() === c} {...ws.needs("resolve_block")} onClick={() => pick(c)}>{text}</button>;
  return <>
    <div class="cf3"><Column title="Base" side={props.block.base} path={props.file.path} /><Column title="Ours" side={props.block.ours} path={props.file.path} /><Column title="Theirs" side={props.block.theirs} path={props.file.path} /></div>
    <Show when={manualNote() && choice() !== "cortex"}><div class="cf-block"><I n="alert" s={14} class="st-warn" /><span>{props.block.cortexNote}</span></div></Show>
    <Show when={editable() && choice() !== "cortex" && !manualNote()}><div class="cf-block"><I n="alert" s={14} class="st-bad" /><span>{props.block.title}</span><span class="r">{btn("ours", "Accept ours")}{btn("theirs", "Accept theirs")}{btn("both", "Accept both")}</span></div></Show>
    <Show when={props.cx === "running"}><div class="cf-block"><I n="loader" s={14} class="spin st-run" /><span>This block is being resolved. Your choices are paused until it finishes.</span></div></Show>
    <Show when={choice() === "cortex"}><div class="cf-block ok"><I n="check-circle" s={14} class="st-ok" /><span>{props.block.cortexNote}</span></div></Show>
    <div class="cf-col"><b>Result</b><span>{label()}</span></div>
    <div class="cf-result"><Lines path={props.file.path} side={preview()} /></div>
  </>;
}

/** The two ways out of a conflict and the three-way view. Abort merge stays in every state, and nothing is ever stashed or reset. */
export function ConflictPanel(props: { m: SessionModel; route: TaskRoute }) {
  const ws = useWs();
  const t = () => props.m.view.task;
  const view = () => props.m.view;
  const cx = (): Cx => cxOf(t());
  const merge = () => t().merge;
  const [picked, setPicked] = createSignal<string | null>(null);
  const [blockAt, setBlockAt] = createSignal(0);
  const facts = () => merge() ? conflictFacts(merge()!) : null;
  const current = createMemo(() => { const m = merge(); return m?.files.find(f => f.path === (picked() ?? facts()?.firstOpen)) ?? m?.files[0] ?? null; });
  const block = () => current()?.blocks[Math.min(blockAt(), (current()?.blocks.length ?? 1) - 1)] ?? null;
  const abort = () => ws.run({ type: "abort_merge", taskId: t().id, now: ws.store.clock() });
  const elapsed = useTimer(() => merge()?.handedAt ?? null);
  const ready: Route = { name: "task", machine: props.route.machine, task: props.route.task, tab: "changes" };
  const canFinish = () => ((facts()?.left ?? 1) === 0 || ws.store.mode === "real") && (cx() === "none" || cx() === "failed");
  const AbortBtn = () => <button class="btn btn-line btn-sm" {...ws.needs("abort_merge")} onClick={abort}>Abort merge</button>;
  return <Show when={merge()} fallback={<div class="note-panel task-panel"><span class="w500">No merge in progress</span><A class="lnk" to={ready}>Back to Changes</A></div>}>{m => <section class="task-panel" aria-label="Merge conflict">
    <Show when={cx() === "none" || cx() === "running"}><Strip tone="fail" icon={<I n="alert" s={14} class="st-bad" />} action={<AbortBtn />}>Merging <span class="mono">{m().source}</span> into <span class="mono">{m().into}</span> stopped at {m().files.length} conflict{m().files.length === 1 ? "" : "s"}. Nothing was stashed or reset.</Strip></Show>
    <Show when={cx() === "done"}><Strip tone="ok" icon={<I n="check-circle" s={14} class="st-ok" />} action={<Show when={!t().acceptance}><AbortBtn /></Show>}>{t().resolvedBy === "cortex" ? "The last block was resolved and the checks ran." : "You finished the merge."} Nothing is merged until you confirm it in Ready to merge.</Strip></Show>
    <Show when={cx() === "failed"}><Strip tone="warn" icon={<I n="alert" s={14} class="st-warn" />} action={<AbortBtn />}>{m().failure}</Strip></Show>
    <Show when={cx() === "none" || cx() === "failed"}><div class="cf-ways">
      <div class="cf-way"><I n="pencil" /><span class="grow"><span class="t">Resolve it yourself</span><span class="d">Choose ours, theirs or both for the block below, then finish the merge.</span></span><span class="cap muted ws-none">{blockWord(facts()!.left)}</span></div>
      <div class="cf-way"><I n="sparkles" /><span class="grow"><span class="t">Resolve automatically</span><span class="d">It resolves the block and runs the checks. The result comes back through Ready to merge for your review.</span></span><button class="btn btn-primary ws-none" {...ws.needs("resolve_with_cortex")} onClick={() => ws.open({ kind: "cortex", taskId: t().id })}>Resolve automatically…</button></div>
    </div></Show>
    <Show when={cx() === "running"}><div class="cx-run"><Steps merge={m()} line /><span class="r"><span class="tnum faint">{elapsed()}</span><A class="btn btn-line btn-sm" to={{ name: "task", machine: props.route.machine, task: props.route.task, tab: "workers", worker: "cortex" }}>Watch</A><button class="btn btn-sm" {...ws.needs("stop_resolution")} onClick={() => ws.run({ type: "stop_resolution", taskId: t().id, now: ws.store.clock() })}>Stop and resolve myself</button></span></div></Show>
    <Show when={cx() === "done"}><div class="cx-run">
      <span class="cx-step"><I n="check-circle" s={14} class="st-ok" />Read both sides</span><I n="chevron-right" s={12} class="faint" />
      <span class="cx-step"><I n="check-circle" s={14} class="st-ok" />Resolved {m().files.reduce((n, f) => n + f.blocks.length, 0)} block{m().files.reduce((n, f) => n + f.blocks.length, 0) === 1 ? "" : "s"}</span><I n="chevron-right" s={12} class="faint" />
      <span class="cx-step"><I n="check-circle" s={14} class={t().result?.checks.some(c => c.outcome === "passed") ? "st-ok" : "faint"} />Checks: {t().result ? (() => { const c = t().result!.checks; const p = c.filter(x => x.outcome === "passed").length, n = c.length - p; return `${p} passed${n ? `, ${n} not passed` : ""}`; })() : "none"}</span><I n="chevron-right" s={12} class="faint" />
      <span class="cx-step now"><I n="merge" s={14} class="st-merge" />{t().acceptance ? "Accepted" : "Waiting for your review"}</span>
      <span class="r"><Show when={m().finishedAt !== null}><span class="faint">Finished {clock(m().finishedAt)}</span></Show></span>
    </div></Show>
    <Show when={m().files.length > 0} fallback={<div class="note-panel"><span class="w500">{ws.t("noConflictDetails")}</span><span>Abort the merge, or resolve it automatically.</span></div>}>
      <div class="cf-body">
        <div class="flist">
          <div class="wg">Conflicts<Count n={m().files.length} /></div>
          <For each={m().files}>{f => <button class="fitem" aria-current={current()?.path === f.path ? "true" : undefined} onClick={() => { setPicked(f.path); setBlockAt(0); }}><GitLetter status="C" /><span class="trunc">{fileName(f.path)}</span>
            <Show when={!fileResolved(f)} fallback={<span class="r st-ok">Resolved</span>}><span class={`r ${cx() === "running" ? "st-run" : "st-bad"}`}>{cx() === "running" ? "Resolving" : `${f.blocks.filter(b => !b.choice).length} left`}</span></Show></button>}</For>
          <Show when={m().clean.length > 0}><div class="wg">Merged cleanly<Count n={m().clean.length} /></div>
            <For each={m().clean}>{f => <div class="fitem"><GitLetter status={f.status} /><span class="trunc">{f.path}</span></div>}</For></Show>
        </div>
        <div class="cf-main">
          <Show when={current()}>{f => <div class="fhead"><I n="file" s={14} class="faint" /><code>{f().path}</code><span>Block {Math.min(blockAt(), f().blocks.length - 1) + 1} of {f().blocks.length}</span>
            <span class="r"><button class="ib ws-ib" classList={{ "ws-hide": blockAt() === 0 }} aria-label="Previous conflict" onClick={() => setBlockAt(n => Math.max(0, n - 1))}><I n="arrow-up" s={14} /></button>
              <button class="ib ws-ib" classList={{ "ws-hide": blockAt() >= f().blocks.length - 1 }} aria-label="Next conflict" onClick={() => setBlockAt(n => Math.min(f().blocks.length - 1, n + 1))}><I n="arrow-down" s={14} /></button></span></div>}</Show>
          <div class="cf-scroll"><Show when={current() && block()}><Block taskId={t().id} file={current()!} block={block()!} cx={cx()} /></Show></div>
          <ActionBar class="rv-foot" start={<span class="cap muted tnum">{facts()!.resolved} of {facts()!.files} files resolved.</span>} hint={cx() === "running" ? "Abort merge stays available during this." : cx() === "done" ? "Nothing merges on its own." : "Finish merge verifies the Git index; stage all manual resolutions first."}>
            <Show when={cx() === "done"} fallback={<button class="btn btn-primary" aria-disabled={canFinish() ? undefined : "true"} data-why={canFinish() ? undefined : "Resolve every block first"} {...ws.needs("finish_merge")} onClick={() => { if (canFinish()) ws.run({ type: "finish_merge", taskId: t().id, now: ws.store.clock() }); }}>Finish merge</button>}>
              <A class="btn btn-primary" to={ready}>Review in Ready to merge</A></Show></ActionBar>
        </div>
      </div>
    </Show>
  </section>}</Show>;
}
