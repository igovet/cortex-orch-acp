import { createMemo, createSignal, For, Show, type JSX } from "solid-js";
import type { Attachment } from "@cortex/workspace-domain";
import { attachmentPreview } from "./attachments";
import { autoGrow } from "./autogrow";
import { EditTask } from "./task-edit";
import { createAttachments, createDictation, MessageComposer } from "./composer";
import { useWs } from "./ctx";
import { I, MenuItem, Modal, Pop } from "./kit";
import { ActionBar, Row, Whole } from "./layout";
import { behindTarget, checksLine, conflictFacts, fileName, needsOverride, openChecks, repoOf, targetOptions, taskCheckout, worktreeWork } from "./review-model";
import { clock, sameBranch } from "./selectors";
import { acceptBlocked } from "./review-model";
import { CHECK_ICON } from "./worker";

export type WsDialog =
  | { kind: "edit-task"; taskId: string }
  | { kind: "accept"; taskId: string }
  | { kind: "changes"; taskId: string; text?: string }
  | { kind: "cortex"; taskId: string }
  | { kind: "merge"; taskId: string; how: "squash" | "pr" }
  | { kind: "delete"; checkoutId: string }
  | { kind: "push"; checkoutId: string }
  | { kind: "revert"; checkoutId: string; path: string; hunk?: number }
  | { kind: "revert-commit"; checkoutId: string; sha: string; message: string }
  | { kind: "amend"; checkoutId: string; message: string; pushed: boolean }
  | { kind: "branch"; projectId: string; at?: string }
  | { kind: "switch-profile"; profileId: string; name: string; changes: string[]; drafts: number }
  | { kind: "confirm"; title: string; note: string; action: string; danger?: boolean; go: () => boolean | Promise<boolean> }
  | { kind: "prompt"; title: string; label: string; value?: string; hint?: string; action: string; optional?: boolean; go: (text: string) => boolean | Promise<boolean> }
  /** A description of work, written in the shared composer: the plane creates the task, nothing starts it. */
  | { kind: "describe"; title: string; label: string; placeholder: string; effect: string; send: string; optional?: boolean; settings: Parameters<ReturnType<typeof useWs>["route"]>[0]; go: (text: string) => boolean | Promise<boolean> }
  | { kind: "add-machine" } | { kind: "add-project" } | { kind: "add-connection" } | { kind: "clear-old" }
  | { kind: "unmerged"; taskId: string }
  | { kind: "attachment"; file: Attachment };
type Of<K extends WsDialog["kind"]> = Extract<WsDialog, { kind: K }>;

const Head = (props: { title: JSX.Element; close: string; onClose: () => void }) => <div class="dlg-h"><h2>{props.title}</h2><button class="ib" aria-label={props.close} onClick={() => props.onClose()}><I n="x" /></button></div>;
const useTask = (id: string) => { const ws = useWs(); return createMemo(() => ws.store.index().byId.get(id)!); };

/** Accepting with open checks is the owner's call, but never an accident: the checks are listed and it takes a tick. */
function Accept(props: { d: Of<"accept">; onClose: () => void }) {
  const ws = useWs();
  const view = useTask(props.d.taskId);
  const [sure, setSure] = createSignal(false);
  const r = () => view().task.result!;
  const accept = () => { if (sure() && !acceptBlocked(view().task)) ws.ok({ type: "accept_result", taskId: props.d.taskId, digest: r().digest, openChecks: true, now: ws.store.clock() }, () => { props.onClose(); }); };
  return <>
    <Head title={`Accept v${r().version} with open checks?`} close="Keep reviewing" onClose={props.onClose} />
    <div class="dlg-b">
      <div class="lab">{checksLine(r().checks)}</div>
      <Show when={r().checks.length > 0} fallback={<div class="cap muted">No checks were reported for this version.</div>}>
        <div class="pre ws-rows"><For each={openChecks(r().checks)}>{c => <Row lead={<I n={CHECK_ICON[c.outcome].icon} s={14} class={CHECK_ICON[c.outcome].tone} />} title={c.name} detail={<>{c.detail ? `${c.detail} ` : ""}Technical blocker: {c.technicalReason}</>} />}</For></div>
      </Show>
      <label class="ck"><input type="checkbox" checked={sure()} onChange={e => setSure(e.currentTarget.checked)} /><span>I accept v{r().version} knowing these checks did not pass<span class="hint ws-block">They stay on the record, and acceptance applies to exactly this version.</span></span></label>
    </div>
    <ActionBar place="dialog" hint="Accepting does not commit, push or merge.">
      <button class="btn btn-line btn-lg" data-autofocus onClick={() => props.onClose()}>Keep reviewing</button>
      <button class="btn btn-primary btn-lg" aria-disabled={sure() && !acceptBlocked(view().task) ? undefined : "true"} data-why={acceptBlocked(view().task) ?? (sure() ? undefined : "Tick the box first")} {...ws.needs("accept_result")} onClick={accept}>Accept v{r().version}</button></ActionBar>
  </>;
}

/** The task's recorded route, read-only: these drafts run on the task's own model and effort. */
function TaskRoute(props: { settings: Parameters<ReturnType<typeof useWs>["route"]>[0] }) {
  const ws = useWs();
  return <div class="c-route"><Show when={ws.route(props.settings)}>{r => <span class="sel mc-fixed" role="note" aria-label={`Task model: ${r()}`}><I n="cpu" s={14} /><span class="sel-t">{r()}</span></span>}</Show></div>;
}

function Changes(props: { d: Of<"changes">; onClose: () => void }) {
  const ws = useWs();
  const view = useTask(props.d.taskId);
  const [text, setText] = createSignal(props.d.text ?? "");
  const files = createAttachments(), mic = createDictation();
  const send = () => { if (text().trim() && !files.busy()) ws.ok({ type: "request_changes", taskId: props.d.taskId, text: text(), attachments: files.files(), now: ws.store.clock() }, () => { props.onClose(); }); };
  return <>
    <Head title={`Request changes to v${view().task.result?.version ?? ""}`} close="Cancel" onClose={props.onClose} />
    <div class="dlg-b">
      <MessageComposer class="mc-inline" label="What should change" placeholder="Say what should change…" text={text()} onText={setText} files={files} mic={mic} autofocus
        effect="Sending reopens this task. Any acceptance of this version goes away." route={<TaskRoute settings={view().task.settings} />}
        send={{ label: "Send and reopen", needs: "request_changes", why: text().trim() ? files.busy() ? "Wait for the files to finish attaching" : undefined : "Say what should change", go: send }} />
    </div>
    <ActionBar place="dialog"><button class="btn btn-line btn-lg" onClick={() => props.onClose()}>Cancel</button></ActionBar>
  </>;
}

/** The only place a merge or a pull request happens. The target and every precondition are shown before the owner confirms. */
function Merge(props: { d: Of<"merge">; onClose: () => void }) {
  const ws = useWs();
  const view = useTask(props.d.taskId);
  const t = () => view().task;
  const state = () => ws.store.state();
  const checkout = createMemo(() => taskCheckout(state(), t()));
  const isWorktree = () => t().checkout.kind === "worktree";
  const remote = () => !!repoOf(state(), t().projectId)?.remote.trim();
  const [choice, setHow] = createSignal(props.d.how);
  const how = () => remote() ? choice() : "squash";
  const [other, setOther] = createSignal<string | null>(null);
  // Generated commit text is English regardless of the task or conversation language.
  const [message, setMessage] = createSignal(`Complete task changes\n\nRecord reviewed result v${t().result?.version ?? 1}.`);
  const [drop, setDrop] = createSignal(true);
  const [title, setTitle] = createSignal("Complete task changes");
  const [body, setBody] = createSignal(`${t().result?.summary ?? ""}\n\nChecks: ${checksLine(t().result?.checks ?? [])}.`.trim());
  const [draft, setDraft] = createSignal(false);
  const target = () => view().target;
  const same = () => sameBranch(t(), target());
  const def = () => view().project.defaultBranch;
  const setTarget = (branch: string) => ws.run({ type: "set_target", taskId: t().id, branch: branch === def() ? null : branch });
  const behind = () => behindTarget(checkout(), target());
  const update = () => {
    ws.ok({ type: "update_from_target", taskId: t().id, now: ws.store.clock() }, () => { props.onClose(); ws.go({ name: "task", machine: view().machine.id, task: t().id, tab: "changes" }); });
  };
  // An automatic task arrives here without an acceptance. Confirming records it for exactly this version first, then merges;
  // a refused merge leaves the acceptance on record and the dialog open, and nothing is resent by itself.
  const pending = () => !t().acceptance;
  const [sure, setSure] = createSignal(false);
  const overrides = () => pending() && needsOverride(t().result?.checks ?? []);
  const why = () => same() && !t().integrationFacts?.commitAvailable ? t().integrationFacts?.reason ?? "Refresh the task before committing." : pending() ? acceptBlocked(t()) ?? (overrides() && !sure() ? "Tick the box about the open checks first" : null) : null;
  // The target is the one part of the label that can be long, so only it shortens; the whole label stays the button's name.
  const label = () => same() ? `Confirm commit to ${target()}` : pending() ? `Accept v${t().result?.version} and ${how() === "squash" ? `merge into ${target()}` : "create PR"}` : how() === "squash" ? `Confirm merge into ${target()}` : ws.store.mode === "demo" ? "Push and create PR" : "Create PR (no push)";
  const noGh = () => !same() && how() === "pr" && ws.store.mode === "real" && !view().machine.ghAvailable;
  const blocked = () => noGh() ? "GitHub CLI (gh) is unavailable." : why();
  let working = false;
  const confirm = (e: MouseEvent) => {
    if (working || why()) return;
    const control = e.currentTarget as HTMLElement;
    working = true;
    void ws.busy((async () => {
      let digest = t().acceptance?.digest;
      if (!digest) {
        const r = t().result!;
        const accepted = await ws.run({ type: "accept_result", taskId: t().id, digest: r.digest, ...(overrides() ? { openChecks: true } : {}), now: ws.store.clock() });
        if (!accepted.ok) return;
        digest = r.digest;
      }
      const common = { type: "integrate" as const, taskId: t().id, confirmed: true, digest, targetBranch: target(), now: ws.store.clock() };
      const done = same() || how() === "squash" ? await ws.run({ ...common, method: same() ? "commit" : "squash", message: message(), deleteWorktree: isWorktree() && drop() })
        : await ws.run({ ...common, method: "pr", title: title(), message: body(), draft: draft() });
      if (done.ok) props.onClose();
    })().finally(() => { working = false; }), control);
  };
  const seeChecks = () => { props.onClose(); ws.go({ name: "task", machine: view().machine.id, task: t().id, tab: "changes", sub: "diff" }); };
  const openCheck = () => { const c = t().result?.checks ?? []; return c.some(x => x.outcome === "failed") ? { icon: "x-circle", tone: "st-bad" } : c.length && c.every(x => x.outcome === "passed") ? { icon: "check-circle", tone: "st-ok" } : { icon: "circle", tone: "faint" }; };
  const a = () => t().acceptance;
  const hasOpen = () => Object.values(a()?.open ?? {}).some(n => n > 0);
  const files = () => checkout()?.files.length ?? null;
  return <>
    <Head title={<>Finish “{t().title}”</>} close="Cancel" onClose={props.onClose} />
    <div class="dlg-b">
      <Show when={same()}><div class="w500">Commit to {target()}</div><p>Only this task’s recorded pending files will be committed locally.</p><div class="pre"><For each={t().integrationFacts?.pendingPaths}>{path => <div class="mono">{path}</div>}</For></div><Show when={t().integrationFacts?.otherUncommitted}><p>Other tasks have uncommitted files in this shared checkout. They will not be included.</p></Show></Show>
      <Show when={!same()}>
      <div class="two">
        <div class="fld"><span class="lab">From</span><span class="pick mono ws-static"><I n="branch" s={14} class="faint" /><Whole text={t().checkout.branch} /></span></div>
        <div class="fld"><span class="lab">Merge into <Show when={target() === def()}><span class="faint">Git default branch</span></Show></span>
          <button class="pick wide mono" popovertarget="m-merge-target" aria-label={`Merge into ${target()}`}><I n="branch" s={14} class="faint" /><span class="l-whole">{target()}</span><I n="chevron-down" s={12} /></button>
          <Pop id="m-merge-target" class="pop-anchor" align="left" label="Merge into" style={{ width: "300px" }}>
            <div class="mi-h">Merge into</div>
            <For each={targetOptions(state(), view())}>{o => <MenuItem label={<span class="mono">{o.name}</span>} sub={o.sub} check={o.name === target()} needs="set_target" onSelect={() => setTarget(o.name)} />}</For>
            <div class="mi-sep"></div>
            <MenuItem label="Other branch…" onSelect={() => setOther("")} />
          </Pop>
          <Show when={other() !== null}><div class="ws-other-b"><input class="inp" aria-label="Other target branch" placeholder="Branch name" value={other()!} onInput={e => setOther(e.currentTarget.value)} ref={el => queueMicrotask(() => el.focus())}
            onKeyDown={e => { if (e.key === "Enter" && other()!.trim()) { e.preventDefault(); void setTarget(other()!).then(r => { if (r.ok) setOther(null); }); } else if (e.key === "Escape") { e.stopPropagation(); setOther(null); } }} /></div></Show>
        </div>
      </div>
      <div class="two">
        <label class="rc"><input type="radio" name="merge-how" checked={how() === "squash"} onChange={() => setHow("squash")} /><span><span class="t">Squash merge locally</span><span class="d">One commit on <span class="mono">{target()}</span>, in this repository.</span></span></label>
        <Show when={remote()}><label class="rc"><input type="radio" name="merge-how" disabled={ws.store.mode === "real" && !view().machine.ghAvailable} checked={how() === "pr"} onChange={() => setHow("pr")} /><span><span class="t">Create a pull request</span><span class="d">{ws.store.mode === "demo" ? <>Pushes <span class="mono">{t().checkout.branch}</span> to <span class="mono">{repoOf(state(), t().projectId)?.remote ?? "origin"}</span> and opens a PR.</> : <>Uses GitHub CLI. Push the branch yourself first in <button class="lnk" onClick={() => { props.onClose(); ws.go({ name: "project", machine: view().machine.id, project: t().projectId, view: "git", scope: { kind: "all" } }); }}>Git</button>; this action never pushes.</>}<Show when={ws.store.mode === "real" && !view().machine.ghAvailable}> <span class="st-warn">Install and sign in to GitHub CLI (gh) first.</span></Show></span></span></label></Show>
      </div>
      </Show>
      <div class="pre">
        <Show when={!pending()} fallback={<div class="chk"><I n="eye" s={14} class="faint" /><span>Result v{t().result?.version} is accepted as your decision when you confirm</span><span class="src">Not accepted yet</span></div>}>
          <div class="chk"><I n="check-circle" s={14} class={hasOpen() ? "st-warn" : "st-ok"} /><span>Result v{a()?.version} accepted by you{hasOpen() ? ", with open checks" : ""}</span><span class="src">{a()?.at != null ? clock(a()!.at!) : ""}</span></div>
        </Show>
        <Show when={!same()}>
        <Show when={behind() !== null} fallback={<div class="chk"><I n="help" s={14} class="faint" /><span>Not compared with <span class="mono">{target()}</span> yet</span></div>}>
          <div class="chk"><I n={behind() ? "alert" : "check-circle"} s={14} class={behind() ? "st-warn" : "st-ok"} /><span>{behind() ? <>Behind <span class="mono">{target()}</span> by {behind()} commit{behind() === 1 ? "" : "s"}</> : <>Up to date with <span class="mono">{target()}</span></>}</span>
            <Show when={behind()}><span class="src"><button class="btn btn-line btn-sm ws-pre-btn" onClick={update}>Update from {target()}</button></span></Show></div>
        </Show>
        <Show when={files() !== null} fallback={<div class="chk"><I n="help" s={14} class="faint" /><span>{ws.t("noCheckoutState")}</span></div>}>
          <div class="chk"><I n={files() ? "alert" : "check-circle"} s={14} class={files() ? "st-warn" : "st-ok"} /><span>{isWorktree() ? "Worktree" : "Project folder"} {files() ? `has ${files()} uncommitted file${files() === 1 ? "" : "s"}` : "is clean"}</span><span class="src">{files() ? (ws.store.mode === "demo" ? "Not part of this merge" : "Will be committed after confirmation") : "No uncommitted files"}</span></div>
        </Show>
        </Show>
        <div class="chk"><I n={openCheck().icon} s={14} class={openCheck().tone} /><span>Checks: {checksLine(t().result?.checks ?? [])}</span><span class="src"><button class="lnk" onClick={seeChecks}>See checks</button></span></div>
      </div>
      <Show when={overrides()}><label class="ck"><input type="checkbox" checked={sure()} onChange={e => setSure(e.currentTarget.checked)} /><span>I accept v{t().result?.version} knowing these checks did not pass<span class="hint ws-block">{checksLine(t().result?.checks ?? [])}. They stay on the record.</span></span></label></Show>
      <Show when={same() || how() === "squash"} fallback={<div class="ws-fields">
        <label class="fld"><span class="lab">Title</span><input class="inp" value={title()} onInput={e => setTitle(e.currentTarget.value)} /></label>
        <label class="fld"><span class="lab">Description <span class="faint">Generated from Result v{a()?.version}, with its checks</span></span><textarea class="inp" rows="3" ref={el => autoGrow(el, body)} value={body()} onInput={e => setBody(e.currentTarget.value)} /></label>
        <label class="ck"><input type="checkbox" checked={draft()} onChange={e => setDraft(e.currentTarget.checked)} /><span>Open as a draft pull request</span></label>
      </div>}>
        <div class="ws-fields">
          <label class="fld"><span class="lab">Commit message <span class="faint">Generated, you can edit it</span></span><textarea class="inp" rows="3" ref={el => autoGrow(el, message)} value={message()} onInput={e => setMessage(e.currentTarget.value)} /></label>
          <Show when={isWorktree() && !same()}><label class="ck"><input type="checkbox" checked={drop()} onChange={e => setDrop(e.currentTarget.checked)} /><span>Delete the worktree after merging<span class="hint ws-block">Never removes unmerged or uncommitted work.</span></span></label></Show>
        </div>
      </Show>
    </div>
    <ActionBar place="dialog" hint={same() ? "Nothing is committed until you confirm." : pending() ? "Nothing is accepted or merged until you confirm." : "Nothing happens until you confirm."}>
      <button class="btn btn-line btn-lg" data-autofocus onClick={() => props.onClose()}>Cancel</button>
      <button class="btn btn-primary btn-lg" aria-label={label()} aria-disabled={blocked() ? "true" : undefined} data-why={blocked() ?? undefined} onClick={e => { if (!blocked()) confirm(e); }}>
        <Show when={same() || how() === "squash"} fallback={label()}>{same() ? "Confirm commit to " : pending() ? `Accept v${t().result?.version} and merge into ` : "Confirm merge into "}<span class="l-branch">{target()}</span></Show></button></ActionBar>
  </>;
}

/** A saved attachment: an image shows here, and anything can be saved as a copy. Nothing is executed or handed to another program. */
function AttachmentView(props: { d: Of<"attachment">; onClose: () => void }) {
  const ws = useWs();
  const [saved, setSaved] = createSignal(""), [failure, setFailure] = createSignal(""), [saving, setSaving] = createSignal(false);
  const preview = () => attachmentPreview(props.d.file);
  const save = async () => {
    if (saving()) return;
    setSaving(true); setSaved(""); setFailure("");
    try { const where = await ws.store.saveAttachment(props.d.file); if (where.path) setSaved(`Saved to ${where.path}.`); }
    catch (e) { setFailure(e instanceof Error ? e.message : "The file could not be saved."); }
    finally { setSaving(false); }
  };
  return <>
    <Head title={props.d.file.name} close="Close" onClose={props.onClose} />
    <div class="dlg-b">
      <Show when={preview()} fallback={<div class="cap muted">There is no preview for this kind of file. Save a copy to open it.</div>}>{src => <img class="ws-att-big" src={src()} alt={props.d.file.name} />}</Show>
      <Show when={saved()}><div class="cap muted" role="status">{saved()}</div></Show>
      <Show when={failure()}><div class="cap st-warn" role="alert">{failure()}</div></Show>
    </div>
    <ActionBar place="dialog"><button class="btn btn-line btn-lg" data-autofocus onClick={() => props.onClose()}>Close</button><button class="btn btn-primary btn-lg" aria-disabled={saving() ? "true" : undefined} data-why={saving() ? "Saving this file" : undefined} onClick={() => void save()}>Save a copy</button></ActionBar>
  </>;
}

function AutoResolve(props: { d: Of<"cortex">; onClose: () => void }) {
  const ws = useWs();
  const view = useTask(props.d.taskId);
  const [guidance, setGuidance] = createSignal("");
  const files = createAttachments(), mic = createDictation();
  const t = () => view().task;
  const f = () => t().merge ? conflictFacts(t().merge!) : null;
  const kept = () => t().merge?.files.find(x => x.blocks.every(b => b.choice));
  const go = () => { if (!files.busy()) ws.ok({ type: "resolve_with_cortex", taskId: t().id, guidance: guidance(), attachments: files.files(), now: ws.store.clock() }, () => { props.onClose(); }); };
  return <>
    <Head title="Resolve this conflict automatically?" close="Keep resolving myself" onClose={props.onClose} />
    <div class="dlg-b">
      <div class="lab">What will happen</div>
      <ul class="conseq">
        <li><I n="file" s={14} /><span>Read both sides, the base and the commits behind them.</span></li>
        <li><I n="pencil" s={14} /><span>Resolve the {f()?.left ?? 0} block{f()?.left === 1 ? "" : "s"} left<Show when={f()?.firstOpen}> in <span class="mono">{fileName(f()!.firstOpen!)}</span></Show>.<Show when={kept()}> Your choice in <span class="mono">{fileName(kept()!.path)}</span> stays as you made it.</Show></span></li>
        <li><I n="check-circle" s={14} /><span>Run the required checks in the worktree <span class="mono">{t().checkout.branch}</span>.</span></li>
        <li><I n="merge" s={14} /><span>Send the result to Ready to merge as a normal diff, for your review.</span></li>
      </ul>
      <div class="note-box plain"><I n="shield" s={14} /><span>Nothing is merged or pushed on its own. Nothing reaches <span class="mono">{view().target}</span> until you confirm it, and Abort merge stays available the whole time.</span></div>
      <div class="fld"><span class="lab">Guidance <span class="faint">Optional</span></span>
        <MessageComposer class="mc-inline" label="Guidance" placeholder="For example: keep the tz argument from main" text={guidance()} onText={setGuidance} files={files} mic={mic} route={<TaskRoute settings={t().settings} />} />
      </div>
    </div>
    <ActionBar place="dialog">
      <button class="btn btn-line btn-lg" data-autofocus onClick={() => props.onClose()}>Cancel</button>
      <button class="btn btn-primary btn-lg" aria-disabled={files.busy() ? "true" : undefined} data-why={files.busy() ? "Wait for the files to finish attaching" : undefined} {...ws.needs("resolve_with_cortex")} onClick={go}>Resolve automatically</button></ActionBar>
  </>;
}

/** Deleting says what goes, in numbers. With unmerged work, keeping it is the default and discarding has to be named. */
function DeleteWorktree(props: { d: Of<"delete">; onClose: () => void }) {
  const ws = useWs();
  const found = () => { for (const r of ws.store.state().repos) { const c = r.checkouts.find(x => x.id === props.d.checkoutId); if (c) return c; } return null; };
  const c = () => found()!;
  const work = () => worktreeWork(ws.store.state(), c());
  const [discard, setDiscard] = createSignal(false);
  const unmerged = () => work().files + work().commits > 0;
  const blocked = () => { const t = work().task; return t && !(t.status === "done" && t.process === "stopped") ? `Close “${t.title}” before deleting its worktree.` : null; };
  const counts = () => `${work().files} file${work().files === 1 ? "" : "s"}, ${work().commits} commit${work().commits === 1 ? "" : "s"}`;
  const go = () => { ws.ok({ type: "delete_worktree", checkoutId: c().id, confirmed: true, discard: discard(), now: ws.store.clock() }, () => { props.onClose(); }); };
  return <Show when={found()}>
    <Head title="Delete this worktree?" close="Keep it" onClose={props.onClose} />
    <div class="dlg-b">
      <ul class="conseq">
        <li><I n="folder" s={14} /><span>Removes the folder <span class="mono">{c().path}</span>.</span></li>
        <li><I n="branch" s={14} /><span>Branch <span class="mono">{c().branch}</span>: {work().commits} unmerged commit{work().commits === 1 ? "" : "s"}.</span></li>
        <li><I n="file" s={14} /><span>{work().files} uncommitted file{work().files === 1 ? "" : "s"}.</span></li>
      </ul>
      <Show when={blocked()}>{why => <div class="note-box"><I n="alert" s={14} /><span>{why()}</span></div>}</Show>
      <Show when={unmerged() && !blocked()}><div class="ws-fields">
        <label class="rc"><input type="radio" name="wt-keep" checked={!discard()} onChange={() => setDiscard(false)} /><span><span class="t">Keep the work</span><span class="d">Nothing is deleted, and its task is not archived.</span></span></label>
        <label class="rc danger"><input type="radio" name="wt-keep" checked={discard()} onChange={() => setDiscard(true)} /><span><span class="t">Delete and discard {counts()}</span><span class="d">This cannot be undone.</span></span></label>
      </div></Show>
    </div>
    <ActionBar place="dialog">
      <button class="btn btn-line btn-lg" data-autofocus onClick={() => props.onClose()}>Keep it</button>
      <Show when={!blocked() && (!unmerged() || discard())}><button class={`btn ${unmerged() ? "btn-danger" : "btn-primary"} btn-lg`} {...ws.needs("delete_worktree")} onClick={go}>{unmerged() ? `Delete and discard ${counts()}` : "Delete worktree"}</button></Show>
    </ActionBar>
  </Show>;
}

function NewBranch(props: { d: Of<"branch">; onClose: () => void }) {
  const ws = useWs();
  const [name, setName] = createSignal("");
  const go = () => { if (name().trim()) ws.ok({ type: "create_branch", projectId: props.d.projectId, name: name(), at: props.d.at }, () => { props.onClose(); }); };
  return <>
    <Head title="Create a branch" close="Cancel" onClose={props.onClose} />
    <div class="dlg-b"><Show when={props.d.at}><div class="cap muted">From commit <span class="mono">{props.d.at}</span>.</div></Show>
      <label class="fld"><span class="lab">Branch name</span><input class="inp" data-autofocus value={name()} onInput={e => setName(e.currentTarget.value)} onKeyDown={e => { if (e.key === "Enter") { e.preventDefault(); go(); } }} /></label></div>
    <ActionBar place="dialog"><button class="btn btn-line btn-lg" onClick={() => props.onClose()}>Cancel</button><button class="btn btn-primary btn-lg" aria-disabled={name().trim() ? undefined : "true"} data-why={name().trim() ? undefined : "Name the branch first"} {...ws.needs("create_branch")} onClick={go}>Create branch</button></ActionBar>
  </>;
}

const Confirm = (props: { title: string; note: JSX.Element; action: string; danger?: boolean; onClose: () => void; onGo: () => void }) => <>
  <Head title={props.title} close="Cancel" onClose={props.onClose} />
  <div class="dlg-b"><div class="cap muted">{props.note}</div></div>
  <ActionBar place="dialog"><button class="btn btn-line btn-lg" data-autofocus onClick={() => props.onClose()}>Cancel</button><button class={`btn ${props.danger ? "btn-danger" : "btn-primary"} btn-lg`} onClick={() => props.onGo()}>{props.action}</button></ActionBar>
</>;

function Prompt(props: { d: Of<"prompt">; onClose: () => void }) {
  const [text, setText] = createSignal(props.d.value ?? "");
  const ready = () => props.d.optional || !!text().trim();
  const go = () => { if (ready()) void Promise.resolve(props.d.go(text())).then(done => { if (done) props.onClose(); }); };
  return <>
    <Head title={props.d.title} close="Cancel" onClose={props.onClose} />
    <div class="dlg-b">
      <label class="fld"><span class="lab">{props.d.label}</span><input class="inp" data-autofocus aria-label={props.d.label} value={text()} onInput={e => setText(e.currentTarget.value)} onKeyDown={e => { if (e.key === "Enter") { e.preventDefault(); go(); } }} /></label>
      <Show when={props.d.hint}><div class="cap muted">{props.d.hint}</div></Show>
    </div>
    <ActionBar place="dialog"><button class="btn btn-line btn-lg" onClick={() => props.onClose()}>Cancel</button><button class="btn btn-primary btn-lg" aria-disabled={ready() ? undefined : "true"} data-why={ready() ? undefined : "Fill in the field first"} onClick={go}>{props.d.action}</button></ActionBar>
  </>;
}

/** A request that becomes a task, such as collecting project knowledge: the shared composer, text only, on the machine's default route. */
function Describe(props: { d: Of<"describe">; onClose: () => void }) {
  const [text, setText] = createSignal(""), [busy, setBusy] = createSignal(false);
  const files = createAttachments(), mic = createDictation();
  const why = () => busy() ? "Creating the task" : props.d.optional || text().trim() ? undefined : "Describe it first";
  const go = () => { if (why()) return; setBusy(true); void Promise.resolve(props.d.go(text())).then(done => { setBusy(false); if (done) props.onClose(); }); };
  return <>
    <Head title={props.d.title} close="Cancel" onClose={props.onClose} />
    <div class="dlg-b">
      <MessageComposer class="mc-inline" label={props.d.label} placeholder={props.d.placeholder} text={text()} onText={setText} files={files} mic={mic} autofocus
        effect={props.d.effect} attachOff="This request is text only. Add files to the task once it is created." route={<TaskRoute settings={props.d.settings} />}
        send={{ label: props.d.send, needs: "create_task", why: why(), go }} />
    </div>
    <ActionBar place="dialog"><button class="btn btn-line btn-lg" onClick={() => props.onClose()}>Cancel</button></ActionBar>
  </>;
}

/** Every dialog the review and Git screens open. One lives at a time, and each confirms before anything changes. */
export function ReviewDialogs() {
  const ws = useWs();
  const d = () => ws.dialog();
  const close = () => ws.closeDialog();
  const of = <K extends WsDialog["kind"]>(kind: K) => { const x = d(); return x?.kind === kind ? x as Of<K> : null; };
  const open = (name: string) => d()?.kind === name;
  const run = (command: Parameters<typeof ws.run>[0]) => { ws.ok(command, () => { close(); }); };
  const running = (id: string) => { for (const r of ws.store.state().repos) { const c = r.checkouts.find(x => x.id === id); if (c?.taskId) return ws.store.state().tasks.find(t => t.id === c.taskId)?.process === "live"; } return false; };
  return <>
    <Modal open={open("confirm")} onClose={close} label="Confirm"><Show when={of("confirm")} keyed>{x => <Confirm title={x.title} note={x.note} action={x.action} danger={x.danger} onClose={close} onGo={() => { void Promise.resolve(x.go()).then(done => { if (done) close(); }); }} />}</Show></Modal>
    <Modal open={open("prompt")} onClose={close} label="Name"><Show when={of("prompt")} keyed>{x => <Prompt d={x} onClose={close} />}</Show></Modal>
    <Modal open={open("describe")} onClose={close} label={of("describe")?.title ?? "Describe"}><Show when={of("describe")} keyed>{x => <Describe d={x} onClose={close} />}</Show></Modal>
    <Modal open={open("edit-task")} onClose={close} label="Edit task"><Show when={of("edit-task")} keyed>{x => <EditTask taskId={x.taskId} onClose={close} />}</Show></Modal>
    <Modal open={open("accept")} onClose={close} label="Accept with open checks"><Show when={of("accept")} keyed>{x => <Accept d={x} onClose={close} />}</Show></Modal>
    <Modal open={open("changes")} onClose={close} label="Request changes"><Show when={of("changes")} keyed>{x => <Changes d={x} onClose={close} />}</Show></Modal>
    <Modal open={open("attachment")} onClose={close} label="Attachment"><Show when={of("attachment")} keyed>{x => <AttachmentView d={x} onClose={close} />}</Show></Modal>
    <Modal open={open("merge")} onClose={close} label="Finish task" class="wide"><Show when={of("merge")} keyed>{x => <Merge d={x} onClose={close} />}</Show></Modal>
    <Modal open={open("cortex")} onClose={close} label="Resolve automatically"><Show when={of("cortex")} keyed>{x => <AutoResolve d={x} onClose={close} />}</Show></Modal>
    <Modal open={open("delete")} onClose={close} label="Delete worktree"><Show when={of("delete")} keyed>{x => <DeleteWorktree d={x} onClose={close} />}</Show></Modal>
    <Modal open={open("branch")} onClose={close} label="Create a branch"><Show when={of("branch")} keyed>{x => <NewBranch d={x} onClose={close} />}</Show></Modal>
    <Modal open={open("push")} onClose={close} label="Push"><Show when={of("push")} keyed>{x => {
      const found = () => { for (const r of ws.store.state().repos) { const c = r.checkouts.find(k => k.id === x.checkoutId); if (c) return { remote: r.remote, branch: c.branch }; } return null; };
      return <Confirm title={`Push ${found()?.branch} to ${found()?.remote}?`} note={ws.t("pushNote")} action="Push" onClose={close} onGo={() => run({ type: "git_push", checkoutId: x.checkoutId, remote: found()!.remote, branch: found()!.branch, confirmed: true })} />;
    }}</Show></Modal>
    <Modal open={open("revert")} onClose={close} label="Revert"><Show when={of("revert")} keyed>{x => <Confirm title={x.hunk === undefined ? `Revert ${fileName(x.path)}?` : "Revert this hunk?"} danger action="Revert"
      note={<>This discards your uncommitted change to <span class="mono">{x.path}</span>.{running(x.checkoutId) ? " A task is working in this checkout, so this can block verification." : ""}</>}
      onClose={close} onGo={() => run({ type: "git_revert", checkoutId: x.checkoutId, path: x.path, hunk: x.hunk, confirmed: true, now: ws.store.clock() })} />}</Show></Modal>
    <Modal open={open("revert-commit")} onClose={close} label="Revert commit"><Show when={of("revert-commit")} keyed>{x => <Confirm title="Revert this commit?" action="Revert commit" note={<>Adds a new commit that undoes <span class="mono">{x.sha}</span>, “{x.message}”. History stays as it is.</>}
      onClose={close} onGo={() => run({ type: "revert_commit", checkoutId: x.checkoutId, sha: x.sha, confirmed: true, now: ws.store.clock() })} />}</Show></Modal>
    <Modal open={open("amend")} onClose={close} label="Amend last commit"><Show when={of("amend")} keyed>{x => <Confirm title="Amend the last commit?" danger={x.pushed} action="Amend" note={x.pushed ? "That commit is already pushed. Amending rewrites it, and pushing it again would need a force push, which never happens automatically." : "Replaces the last commit's message and adds the staged files to it."}
      onClose={close} onGo={() => run({ type: "git_commit", checkoutId: x.checkoutId, message: x.message, amend: true, confirmed: true, now: ws.store.clock() })} />}</Show></Modal>
  </>;
}
