import { createEffect, createMemo, createSignal, createUniqueId, For, on, onCleanup, Show, type JSX } from "solid-js";
import { capabilityOf, type Attachment, type Check, type WorkspaceCommand } from "@cortex/workspace-domain";
import { attachmentPreview } from "./attachments";
import { draftNotice, WRITING_CHANGES } from "./copy";
import { autoGrow } from "./autogrow";
import { EditTask } from "./task-edit";
import { CommitDialog, PullDialog, UpdateDialog } from "./git-actions";
import { createAttachments, createDictation, MessageComposer } from "./composer";
import { useWs } from "./ctx";
import { createRequestFeedback, REQUESTING, RequestStatus } from "./request-feedback";
import { I, MenuItem, Modal, Pop, useWantOverview } from "./kit";
import { ActionBar, Row, Whole } from "./layout";
import { behindTarget, checksLine, conflictFacts, fileName, mergeControls, finishBlocked, needsOverride, repoOf, targetOptions, taskCheckout, worktreeWork } from "./review-model";
import { automaticFinishOffer, clock, ownerHold, sameBranch } from "./selectors";
import { same as sameData } from "./stable";
import { acceptBlocked } from "./review-model";
import { CHECK_ICON } from "./worker";

export type WsDialog =
  | { kind: "edit-task"; taskId: string }
  | { kind: "accept"; taskId: string; automaticFinish?: NonNullable<ReturnType<typeof automaticFinishOffer>> }
  | { kind: "changes"; taskId: string; text?: string }
  | { kind: "cortex"; taskId: string }
  | { kind: "clear-legacy-merge"; taskId: string }
  | { kind: "merge"; taskId: string; how: "squash" | "pr" }
  | { kind: "delete"; checkoutId: string }
  | { kind: "push"; checkoutId: string; pullFirst?: (push: Extract<WorkspaceCommand, { type: "git_push" }>) => Promise<boolean> }
  | { kind: "commit"; checkoutId: string; taskId?: string }
  | { kind: "pull"; checkoutId: string; taskId?: string }
  | { kind: "update"; taskId: string }
  | { kind: "revert"; checkoutId: string; path: string; hunk?: number }
  | { kind: "revert-commit"; checkoutId: string; sha: string; message: string }
  | { kind: "amend"; checkoutId: string; message: string; pushed: boolean }
  | { kind: "branch"; projectId: string; at?: string }
  | { kind: "switch-profile"; profileId: string; name: string; changes: string[]; drafts: number }
  | { kind: "confirm"; title: string; note: string; action: string; danger?: boolean; pending?: () => string | undefined; go: () => boolean | Promise<boolean> }
  | { kind: "prompt"; title: string; label: string; value?: string; hint?: string; action: string; optional?: boolean; go: (text: string) => boolean | Promise<boolean> }
  /** A description of work, written in the shared composer: the plane creates the task, nothing starts it. */
  | { kind: "describe"; title: string; label: string; placeholder: string; effect: string; send: string; optional?: boolean; settings: Parameters<ReturnType<typeof useWs>["route"]>[0]; go: (text: string, run: ReturnType<typeof useWs>["run"]) => boolean | Promise<boolean> }
  | { kind: "add-machine" } | { kind: "add-project" } | { kind: "add-connection" } | { kind: "clear-old" }
  | { kind: "attachment"; file: Attachment };
type Of<K extends WsDialog["kind"]> = Extract<WsDialog, { kind: K }>;

const Head = (props: { title: JSX.Element; close: string; onClose: () => void }) => <div class="dlg-h"><h2>{props.title}</h2><button data-tooltip={`${props.close} · Esc`} class="ib" aria-label={props.close} onClick={() => props.onClose()}><I n="x" /></button></div>;
const useTask = (id: string) => { const ws = useWs(); return createMemo(() => ws.store.index().byId.get(id)!); };

function ReviewCheckRow(props: { check: Check }) {
  const c = () => props.check;
  const reason = () => {
    const detail = c().detail.trim(), technical = c().technicalReason?.trim();
    return technical && detail !== technical ? [detail, technical].filter(Boolean).join(" ") : detail;
  };
  return <Row lead={<I n={CHECK_ICON[c().outcome].icon} s={14} class={CHECK_ICON[c().outcome].tone} />}
    title={c().name} detail={c().outcome === "not_run" ? <><strong>Not run</strong>{reason() ? `: ${reason()}` : ""}</> : reason()} />;
}

/** Accepting with open checks is the owner's call, but never an accident: the checks are listed and it takes a tick. */
function Accept(props: { d: Of<"accept">; onClose: () => void }) {
  const ws = useWs();
  const view = useTask(props.d.taskId);
  const [sure, setSure] = createSignal(false);
  const initial = view().task.result!;
  const r = () => view()?.task.result ?? initial;
  const shownChecks = createMemo(() => r().checks.filter(c => c.outcome === "not_run" || (c.kind !== "manual" && c.outcome !== "passed")));
  const automaticFinish = () => props.d.automaticFinish;
  const [sending, setSending] = createSignal(false);
  const blocked = () => {
    if (sending()) return "Saving your decision…";
    if (!view()?.task.result) return "This result changed. Reopen the decision.";
    const consent = automaticFinish();
    // The service decided that this hold asks for the finish; the approval stays bound to the exact result shown.
    if (consent) {
      const r = view().task.result, hold = ownerHold(view());
      return hold?.actions.includes("allow_finish") && r?.digest === consent.digest && r.sourceVersion === consent.sourceVersion && view().target === consent.targetBranch
        ? null : "This result or target changed. Close and reopen the approval.";
    }
    // Hybrid: the queue asks for the owner's acceptance of this result with its recorded checks.
    if (ownerHold(view())?.actions.includes("accept_result")) return null;
    return acceptBlocked(view().task);
  };
  const accept = async () => {
    if (!sure() || blocked()) return;
    const consent = automaticFinish();
    setSending(true);
    try {
      const result = await ws.run({ type: "accept_result", taskId: props.d.taskId, digest: r().digest, openChecks: true,
        ...(consent ? { sourceVersion: consent.sourceVersion, automaticFinish: { sprintId: consent.sprintId, targetBranch: consent.targetBranch } } : {}), now: ws.store.clock() });
      if (result.ok && ws.dialog() === props.d) props.onClose();
    } finally { setSending(false); }
  };
  return <>
    <Head title={automaticFinish() ? `Allow Automatic to finish v${initial.version}?` : `Accept v${r().version} with open checks?`} close="Keep reviewing" onClose={props.onClose} />
    <div class="dlg-b">
      <div class="lab">{checksLine(r().checks, { includeManualSkips: true })}</div>
      <Show when={shownChecks().length > 0}>
        <div class="pre ws-rows"><For each={shownChecks()}>{c => <ReviewCheckRow check={c} />}</For></div>
      </Show>
      <Show when={automaticFinish()} fallback={<label class="ck"><input type="checkbox" checked={sure()} onChange={e => setSure(e.currentTarget.checked)} /><span>I accept v{r().version} knowing these checks did not pass<span class="hint ws-block">They stay on the record, and acceptance applies to exactly this version.</span></span></label>}>
        <div class="note-box plain"><I n="check-circle" s={14} /><span>Allow Automatic to verify this exact result, commit the task's local changes on <span class="mono">{automaticFinish()!.targetBranch ?? "the current branch"}</span>, and close the task after verification succeeds. No push, merge, or other result is included.</span></div>
        <label class="ck"><input type="checkbox" checked={sure()} onChange={e => setSure(e.currentTarget.checked)} /><span>I approve this Automatic finish for result v{initial.version}<span class="hint ws-block">Open or skipped checks remain recorded and are not silently marked passed.</span></span></label>
      </Show>
    </div>
    <RequestStatus text={ws.store.pendingTask("accept_result", props.d.taskId) ? REQUESTING.accept_result : undefined} />
    <ActionBar place="dialog" hint={automaticFinish() ? "Automatic acts only after the runtime rechecks this exact result." : "Accepting does not commit, push or merge."}>
      <button class="btn btn-line btn-lg" data-autofocus onClick={() => props.onClose()}>Keep reviewing</button>
      <button class="btn btn-primary btn-lg" aria-disabled={sure() && !blocked() ? undefined : "true"} data-why={blocked() ?? (sure() ? undefined : "Tick the box first")} {...ws.needs("accept_result")} onClick={accept}>{automaticFinish() ? "Allow Automatic to finish" : `Accept v${r().version}`}</button></ActionBar>
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
      <MessageComposer class="mc-inline" label="What should change" placeholder="Say what should change. Tag @files or use /skills." text={text()} onText={setText} files={files} mic={mic} autofocus
        suggest={{ projectId: view().task.projectId, taskId: props.d.taskId, skills: true }}
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
  // Equal choices stay the same objects, so an open target menu keeps its rows on a background read.
  const targets = createMemo(() => targetOptions(state(), view()), undefined, { equals: sameData });
  const isWorktree = () => t().checkout.kind === "worktree";
  const isBound = () => { const binding = t().checkout; return binding.kind === "worktree" && !!binding.id; };
  // The target branches and the remote shown here are Git data: read once as the dialog opens, and kept current while it is open.
  onCleanup(ws.store.interest(t().projectId, "git"));
  const remote = () => !!repoOf(state(), t().projectId)?.remote.trim();
  const [choice, setHow] = createSignal(props.d.how);
  // A pull request needs GitHub CLI, which the machine's overview reports. Until it has, the choice says it is being checked.
  useWantOverview(remote);
  const gh = () => ws.store.mode !== "real" || view().machine.ghAvailable ? "yes" : ws.store.overviewLoaded() ? "no" : "checking";
  const ghWait = () => ws.t(ws.store.overviewIssue() ? "overviewIssue" : "ghChecking");
  const how = () => remote() ? choice() : "squash";
  const [other, setOther] = createSignal<string | null>(null);
  // Generated commit text is English regardless of the task or conversation language.
  const initialMessage = `Complete task changes\n\nRecord reviewed result v${t().result?.version ?? 1}.`;
  const [message, setMessage] = createSignal(initialMessage);
  const [drop, setDrop] = createSignal<boolean>();
  const cleanup = () => t().integrationFacts?.deleteWorktree;
  const cleanupOff = () => ws.demo ? undefined : cleanup()?.available === true && !!checkout() ? undefined
    : cleanup()?.reason === "worktree_dirty" ? "The worktree has uncommitted or untracked files."
    : cleanup()?.reason === "active_execution" ? "Wait for the task and its workers to stop."
    : cleanup()?.reason ? `Safe removal could not be confirmed (${cleanup()!.reason}).` : "Waiting for confirmation that the worktree can be removed safely.";
  const removeFolder = () => !cleanupOff() && (drop() ?? cleanup()?.default ?? true);
  const cleanupReasonId = createUniqueId();
  const [title, setTitle] = createSignal("Complete task changes");
  const initialBody = `${t().result?.summary ?? ""}\n\nChecks: ${checksLine(t().result?.checks ?? [])}.`.trim();
  const [body, setBody] = createSignal(initialBody);
  const [draft, setDraft] = createSignal(false);
  const target = () => view().target;
  const same = () => sameBranch(t(), target());
  // The machine writes these texts from the task's real changes, in the background. Text the owner typed is never replaced.
  const edited = new Set<"message" | "title" | "body">();
  const [written, setWritten] = createSignal("");
  const asked = new Map<string, ReturnType<typeof ws.store.draftIntegrationText>>();
  createEffect(on(() => [same() ? "commit" as const : how(), target()] as const, ([method, branch]) => {
    const key = `${method}:${branch}`;
    let active = true;
    onCleanup(() => { active = false; });
    if (method === "pr") {
      if (!edited.has("title")) setTitle("Complete task changes");
      if (!edited.has("body")) setBody(initialBody);
    } else if (!edited.has("message")) setMessage(initialMessage);
    setWritten(WRITING_CHANGES);
    let request = asked.get(key);
    if (!request) {
      request = ws.store.draftIntegrationText({ taskId: t().id, method, targetBranch: branch, paths: method === "commit" ? t().integrationFacts?.pendingPaths : undefined, summary: t().result?.summary });
      asked.set(key, request);
    }
    void request.then(text => {
      if (!active) return;
      if (method === "pr") {
        const saved = text?.bodySource === "saved_push_summary";
        if (text?.title && !edited.has("title")) setTitle(text.title);
        if (text?.body && !edited.has("body")) setBody(saved ? text.body : `${text.body}\n\nChecks: ${checksLine(t().result?.checks ?? [])}.`);
        setWritten(saved ? "Loaded from the confirmed Push summary. You can edit it." : draftNotice(text, !!text?.body, "Written from the changes. You can edit it."));
      } else {
        if (text?.message && !edited.has("message")) setMessage(text.message);
        setWritten(draftNotice(text, !!text?.message, "Written from the changes. You can edit it."));
      }
    });
  }));
  const def = () => view().project.defaultBranch;
  const setTarget = (branch: string) => ws.run({ type: "set_target", taskId: t().id, branch: branch === def() ? null : branch });
  const behind = () => behindTarget(checkout(), target());
  // Its own confirmation: the update changes the reviewed tree and withdraws an acceptance, so it is never one press here.
  const update = () => ws.open({ kind: "update", taskId: t().id });
  // A result without acceptance is accepted for exactly this version on confirmation, then merged;
  // a refused merge leaves the acceptance on record and the dialog open, and nothing is resent by itself.
  const pending = () => !t().acceptance;
  const [sure, setSure] = createSignal(false);
  const overrides = () => pending() && (t().reviewActions?.acknowledgeOpenChecks ?? needsOverride(t().result?.checks ?? []));
  const why = () => (isBound() && !checkout() ? ws.t("worktreeUnavailable") : undefined)
    ?? finishBlocked(t())
    ?? (same() && !t().integrationFacts?.commitAvailable ? t().integrationFacts?.reason ?? "Refresh the task before committing." : null)
    ?? (!same() && how() === "pr" && t().integration?.method === "pr" ? "A pull request is already open for this version" : null)
    ?? (overrides() && !sure() ? "Tick the box about the open checks first" : null);
  // The target is the one part of the label that can be long, so only it shortens; the whole label stays the button's name.
  const label = () => same() ? `Confirm commit to ${target()}` : pending() ? `Accept v${t().result?.version} and ${how() === "squash" ? `merge into ${target()}` : "create PR"}` : how() === "squash" ? `Confirm merge into ${target()}` : ws.store.mode === "demo" ? "Push and create PR" : "Create PR (no push)";
  const noGh = () => !same() && how() === "pr" && gh() !== "yes";
  const blocked = () => noGh() ? (gh() === "no" ? "GitHub CLI (gh) is unavailable." : ghWait()) : why();
  let working = false;
  const confirm = (e: MouseEvent) => {
    if (working || why()) return;
    const control = e.currentTarget as HTMLElement;
    working = true;
    const destination = target(), method = same() ? "commit" : how();
    const fields: Pick<Extract<WorkspaceCommand, { type: "integrate" }>, "method" | "title" | "message" | "draft" | "deleteWorktree"> = method === "pr" ? { method, title: title(), message: body(), draft: draft() }
      : { method, message: message(), deleteWorktree: isWorktree() && !same() && removeFolder() };
    void ws.busy((async () => {
      let digest = t().acceptance?.digest;
      if (!digest) {
        const r = t().result!;
        const accepted = await ws.run({ type: "accept_result", taskId: t().id, digest: r.digest, ...(overrides() ? { openChecks: true } : {}), now: ws.store.clock() });
        if (!accepted.ok) return;
        digest = r.digest;
      }
      const common = { type: "integrate" as const, taskId: t().id, confirmed: true, digest, targetBranch: destination, now: ws.store.clock() };
      const done = await ws.run({ ...common, ...fields });
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
            <For each={targets()}>{o => <MenuItem label={<span class="mono">{o.name}</span>} sub={o.sub} check={o.name === target()} needs="set_target" onSelect={() => setTarget(o.name)} />}</For>
            <div class="mi-sep"></div>
            <MenuItem label="Other branch…" onSelect={() => setOther("")} />
          </Pop>
          <Show when={other() !== null}><div class="ws-other-b"><input class="inp" aria-label="Other target branch" placeholder="Branch name" value={other()!} onInput={e => setOther(e.currentTarget.value)} ref={el => queueMicrotask(() => el.focus())}
            onKeyDown={e => { if (e.key === "Enter" && other()!.trim()) { e.preventDefault(); void setTarget(other()!).then(r => { if (r.ok) setOther(null); }); } else if (e.key === "Escape") { e.stopPropagation(); setOther(null); } }} /></div></Show>
        </div>
      </div>
      <div class="two">
        <label class="rc"><input type="radio" name="merge-how" checked={how() === "squash"} onChange={() => setHow("squash")} /><span><span class="t">Squash merge locally</span><span class="d">One commit on <span class="mono">{target()}</span>, in this repository.</span></span></label>
        <Show when={remote()}><label class="rc"><input type="radio" name="merge-how" disabled={gh() === "no"} checked={how() === "pr"} onChange={() => setHow("pr")} /><span><span class="t">Create a pull request</span><span class="d">{ws.store.mode === "demo" ? <>Pushes <span class="mono">{t().checkout.branch}</span> to <span class="mono">{repoOf(state(), t().projectId)?.remote ?? "origin"}</span> and opens a PR.</> : <>Uses GitHub CLI. Push the branch yourself first in <button class="lnk" onClick={() => { props.onClose(); ws.go({ name: "project", machine: view().machine.id, project: t().projectId, view: "git", scope: { kind: "all" } }); }}>Git</button>; this action never pushes.</>}<Show when={gh() === "no"}> <span class="st-warn">Install and sign in to GitHub CLI (gh) first.</span></Show><Show when={gh() === "checking"}> <span class="muted" role="status">{ghWait()}</span></Show></span></span></label></Show>
      </div>
      </Show>
      <div class="pre">
        <Show when={!pending()} fallback={<div class="chk"><I n="eye" s={14} class="faint" /><span>Result v{t().result?.version} is accepted as your decision when you confirm</span><span class="src">Not accepted yet</span></div>}>
          <div class="chk"><I n="check-circle" s={14} class={hasOpen() ? "st-warn" : "st-ok"} /><span>Result v{a()?.version} accepted {a()?.automatic ? "automatically" : "by you"}{hasOpen() ? ", with open checks" : ""}</span><span class="src">{a()?.at != null ? clock(a()!.at!) : ""}</span></div>
        </Show>
        <Show when={!same()}>
        <Show when={behind() !== null} fallback={<div class="chk"><I n="help" s={14} class="faint" /><span>Not compared with <span class="mono">{target()}</span> yet</span></div>}>
          <div class="chk"><I n={behind() ? "alert" : "check-circle"} s={14} class={behind() ? "st-warn" : "st-ok"} /><span>{behind() ? <>Behind <span class="mono">{target()}</span> by {behind()} commit{behind() === 1 ? "" : "s"}</> : <>Up to date with <span class="mono">{target()}</span></>}</span>
            {/* Only where the machine offers it, for a task with a worktree of its own; a control that always fails is not shown. */}
            <Show when={behind() && isWorktree() && ws.can(capabilityOf("update_from_target"))}><span class="src"><button class="btn btn-line btn-sm ws-pre-btn" onClick={update}>Update from {target()}…</button></span></Show></div>
        </Show>
        <Show when={files() !== null} fallback={<div class="chk"><I n="help" s={14} class="faint" /><span>{ws.t("noCheckoutState")}</span></div>}>
          <div class="chk"><I n={files() ? "alert" : "check-circle"} s={14} class={files() ? "st-warn" : "st-ok"} /><span>{isWorktree() ? "Worktree" : "Project folder"} {files() ? `has ${files()} uncommitted file${files() === 1 ? "" : "s"}` : "is clean"}</span><span class="src">{files() ? (ws.store.mode === "demo" ? "Not part of this merge" : isBound() ? "Commit the worktree changes before merging" : "Will be committed after confirmation") : "No uncommitted files"}</span></div>
        </Show>
        </Show>
        <div class="chk"><I n={openCheck().icon} s={14} class={openCheck().tone} /><span>Checks: {checksLine(t().result?.checks ?? [], { includeManualSkips: true })}</span><span class="src"><button class="lnk" onClick={seeChecks}>See checks</button></span></div>
      </div>
      <Show when={t().result?.checks.some(c => c.outcome === "not_run")}>
        <div class="pre ws-rows"><For each={t().result?.checks.filter(c => c.outcome === "not_run")}>{c => <ReviewCheckRow check={c} />}</For></div>
      </Show>
      <Show when={overrides()}><label class="ck"><input type="checkbox" checked={sure()} onChange={e => setSure(e.currentTarget.checked)} /><span>I accept v{t().result?.version} knowing these checks did not pass<span class="hint ws-block">{checksLine(t().result?.checks ?? [], { includeManualSkips: true })}. They stay on the record.</span></span></label></Show>
      <p class="cap muted" role="status">{written()}</p>
      <Show when={same() || how() === "squash"} fallback={<div class="ws-fields">
        <label class="fld"><span class="lab">Title</span><input class="inp" value={title()} onInput={e => { edited.add("title"); setTitle(e.currentTarget.value); }} /></label>
        <label class="fld"><span class="lab">Description</span><textarea class="inp" rows="3" ref={el => autoGrow(el, body)} value={body()} onInput={e => { edited.add("body"); setBody(e.currentTarget.value); }} /></label>
        <label class="ck"><input type="checkbox" checked={draft()} onChange={e => setDraft(e.currentTarget.checked)} /><span>Open as a draft pull request</span></label>
      </div>}>
        <div class="ws-fields">
          <label class="fld"><span class="lab">Commit message</span><textarea class="inp" rows="3" ref={el => autoGrow(el, message)} value={message()} onInput={e => { edited.add("message"); setMessage(e.currentTarget.value); }} /></label>
          <Show when={isWorktree() && !same()}><label class="ck"><input type="checkbox" checked={removeFolder()} disabled={!!cleanupOff()} aria-labelledby={`${cleanupReasonId}-label`} aria-describedby={cleanupReasonId} onChange={e => setDrop(e.currentTarget.checked)} /><span><span id={`${cleanupReasonId}-label`}>{ws.t("deleteWorktreeFolder")}</span><span id={cleanupReasonId} class="hint ws-block">{cleanupOff() ?? "Removes the clean folder and keeps its branch."}</span></span></label></Show>
        </div>
      </Show>
    </div>
    <RequestStatus text={ws.store.pendingTask("accept_result", props.d.taskId) ? REQUESTING.accept_result : undefined} />
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
      <Show when={preview()} fallback={<div class="cap muted">{ws.store.canSaveAttachment() ? "There is no preview for this kind of file. Save a copy to open it." : "There is no preview for this kind of file."}</div>}>{src => <img class="ws-att-big" src={src()} alt={props.d.file.name} />}</Show>
      <Show when={saved()}><div class="cap muted" role="status">{saved()}</div></Show>
      <Show when={failure()}><div class="cap st-warn" role="alert">{failure()}</div></Show>
    </div>
    <ActionBar place="dialog"><button class="btn btn-line btn-lg" data-autofocus onClick={() => props.onClose()}>Close</button><Show when={ws.store.canSaveAttachment()}><button class="btn btn-primary btn-lg" aria-disabled={saving() ? "true" : undefined} data-why={saving() ? "Saving this file" : undefined} onClick={() => void save()}>Save a copy</button></Show></ActionBar>
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
  const real = ws.store.mode === "real", conflictId = t().merge?.conflictId;
  const [sending, setSending] = createSignal(false);
  const why = () => files.busy() ? "Wait for the files to finish attaching"
    : sending() ? "Recording the handoff"
    : real && (!conflictId || conflictId !== t().merge?.conflictId || !mergeControls(t()).handoff) ? "This merge is no longer available for handoff" : undefined;
  const go = async () => {
    if (why()) return;
    setSending(true);
    try {
      const receipt = await ws.run({ type: "resolve_with_cortex", taskId: t().id, ...(conflictId ? { conflictId } : {}), guidance: guidance(), attachments: files.files(), now: ws.store.clock() });
      if (receipt.ok || (real && receipt.outcome === "unknown_outcome")) props.onClose();
    } finally { setSending(false); }
  };
  return <>
    <Head title={real ? "Hand this merge to the app?" : "Resolve this conflict automatically?"} close="Keep resolving myself" onClose={props.onClose} />
    <div class="dlg-b">
      <div class="lab">What will happen</div>
      <Show when={real} fallback={<>
      <ul class="conseq">
        <li><I n="file" s={14} /><span>Read both sides, the base and the commits behind them.</span></li>
        <li><I n="pencil" s={14} /><span>Resolve the {f()?.left ?? 0} block{f()?.left === 1 ? "" : "s"} left<Show when={f()?.firstOpen}> in <span class="mono">{fileName(f()!.firstOpen!)}</span></Show>.<Show when={kept()}> Your choice in <span class="mono">{fileName(kept()!.path)}</span> stays as you made it.</Show></span></li>
        <li><I n="check-circle" s={14} /><span>Run the required checks in the worktree <span class="mono">{t().checkout.branch}</span>.</span></li>
        <li><I n="merge" s={14} /><span>Send the result to Ready to merge as a normal diff, for your review.</span></li>
      </ul>
      <div class="note-box plain"><I n="shield" s={14} /><span>Nothing is merged or pushed on its own. Nothing reaches <span class="mono">{view().target}</span> until you confirm it, and Abort merge stays available the whole time.</span></div>
      </>}>
        <p>The app receives the saved merge revisions and a list of conflicting paths, not file previews. Manual merge actions wait while the app has control.</p>
        <p>After resolution, request fresh review and verification. Nothing reaches the target branch until you confirm it separately.</p>
      </Show>
      <div class="fld"><span class="lab">Guidance <span class="faint">Optional</span></span>
        <MessageComposer class="mc-inline" label="Guidance" placeholder="For example: keep the tz argument from main" text={guidance()} onText={setGuidance} files={files} mic={mic} route={<TaskRoute settings={t().settings} />} />
      </div>
    </div>
    <ActionBar place="dialog">
      <button class="btn btn-line btn-lg" data-autofocus onClick={() => props.onClose()}>Cancel</button>
      <button class="btn btn-primary btn-lg" aria-disabled={why() ? "true" : undefined} data-why={why()} {...ws.needs("resolve_with_cortex")} onClick={() => void go()}>{real ? "Hand to the app" : "Resolve automatically"}</button></ActionBar>
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

const Confirm = (props: { title: string; note: JSX.Element; action: string; danger?: boolean; pending?: () => string | undefined; onClose: () => void; onGo: () => void }) => <>
  <Head title={props.title} close="Cancel" onClose={props.onClose} />
  <div class="dlg-b"><div class="cap muted">{props.note}</div><Show when={props.pending}><RequestStatus text={props.pending?.()} /></Show></div>
  <ActionBar place="dialog"><button class="btn btn-line btn-lg" data-autofocus onClick={() => props.onClose()}>Cancel</button><button class={`btn ${props.danger ? "btn-danger" : "btn-primary"} btn-lg`} onClick={() => props.onGo()}>{props.action}</button></ActionBar>
</>;

function Push(props: { d: Of<"push">; onClose: () => void }) {
  const ws = useWs();
  const checkoutId = props.d.checkoutId;
  const find = () => {
    for (const repo of ws.store.state().repos) {
      const checkout = repo.checkouts.find(c => c.id === checkoutId);
      if (checkout && repo.remote.trim() && checkout.branch) return { remote: repo.remote, branch: checkout.branch };
    }
    return null;
  };
  const destination = find();
  const description = createUniqueId();
  const [summary, setSummary] = createSignal("");
  const [note, setNote] = createSignal(destination ? "Writing a summary of the outgoing commits…" : "");
  const [working, setWorking] = createSignal(false);
  let edited = false, active = true;
  onCleanup(() => { active = false; });
  const why = () => {
    const current = find();
    if (!destination || current?.branch !== destination.branch || current?.remote !== destination.remote) return "The Push destination changed or is unavailable. Close and reopen Push.";
    return ws.gate("git_push");
  };
  if (destination) void ws.store.draftPushSummary({ checkoutId, ...destination }).then(text => {
    if (!active || edited || working()) return;
    setSummary(text?.body ?? "");
    setNote(draftNotice(text, !!text?.body, "Written from the outgoing commits. You can edit it."));
  });
  const action = props.d.pullFirst ? "Pull and push" : "Push";
  const go = (e: MouseEvent) => {
    if (!destination || why() || working()) return;
    edited = true; // Freeze what the owner confirmed, even if drafting is still in progress.
    setNote("");
    setWorking(true);
    const command = { type: "git_push" as const, checkoutId, ...destination, confirmed: true, summary: summary() };
    void ws.busy((props.d.pullFirst ? props.d.pullFirst(command) : ws.run(command).then(result => result.ok)).then(done => {
      if (done && active) props.onClose();
    }).finally(() => setWorking(false)), e.currentTarget as HTMLElement);
  };
  return <>
    <Head title={destination ? `${action} ${destination.branch} to ${destination.remote}?` : action} close="Cancel" onClose={props.onClose} />
    <div class="dlg-b">
      <div class="cap muted">{props.d.pullFirst ? "Pulls with fast-forward only. If the pull cannot fast-forward, nothing is pushed. " : ""}{ws.t("pushNote")}</div>
      <label class="fld"><span class="lab">Summary of commits being pushed</span>
        <textarea class="inp" rows="3" aria-describedby={`${description} ${description}-status`} ref={el => autoGrow(el, summary)} value={summary()} disabled={working()} onInput={e => { edited = true; setSummary(e.currentTarget.value); setNote(""); }} />
      </label>
      <div class="cap muted" id={description}>Optional. A successful Push saves this text for the PR description.</div>
      <div class="cap muted" id={`${description}-status`} role="status">{why() ?? note()}</div>
    </div>
    <ActionBar place="dialog">
      <button class="btn btn-line btn-lg" data-autofocus onClick={() => props.onClose()}>Cancel</button>
      <button class="btn btn-primary btn-lg" aria-disabled={why() || working() ? "true" : undefined} data-why={why()} onClick={go}>{action}</button>
    </ActionBar>
  </>;
}

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
  const request = createRequestFeedback();
  const [text, setText] = createSignal(""), [busy, setBusy] = createSignal(false);
  const files = createAttachments(), mic = createDictation();
  const why = () => busy() ? "Creating the task" : props.d.optional || text().trim() ? undefined : "Describe it first";
  const go = () => { if (why()) return; setBusy(true); void Promise.resolve(props.d.go(text(), request.run)).then(done => { setBusy(false); if (done) props.onClose(); }); };
  return <>
    <Head title={props.d.title} close="Cancel" onClose={props.onClose} />
    <div class="dlg-b">
      <MessageComposer class="mc-inline" label={props.d.label} placeholder={props.d.placeholder} text={text()} onText={setText} files={files} mic={mic} autofocus
        effect={props.d.effect} attachOff="This request is text only. Add files to the task once it is created." route={<TaskRoute settings={props.d.settings} />}
        send={{ label: props.d.send, needs: "create_task", why: why(), go }} />
      <RequestStatus text={request.text()} />
    </div>
    <ActionBar place="dialog"><button class="btn btn-line btn-lg" onClick={() => props.onClose()}>Cancel</button></ActionBar>
  </>;
}

function ClearLegacyMerge(props: { d: Of<"clear-legacy-merge">; onClose: () => void }) {
  const ws = useWs();
  const view = useTask(props.d.taskId);
  const [pending, setPending] = createSignal(false);
  const why = () => {
    const task = view()?.task;
    return !task || !mergeControls(task).legacy ? "This saved merge changed. Close and refresh the task." : ws.gate("abort_merge");
  };
  const clear = async () => {
    if (pending() || why()) return;
    setPending(true);
    try {
      const result = await ws.run({ type: "abort_merge", taskId: props.d.taskId, discardLegacy: true });
      if (result.ok && ws.dialog() === props.d) props.onClose();
    } finally { setPending(false); }
  };
  return <>
    <Head title="Clear saved merge record?" close="Cancel" onClose={props.onClose} />
    <div class="dlg-b">
      <div class="cap muted">Finish or abort the merge in Git in the original task checkout first. Clearing the saved record leaves Git unchanged. Earlier checks do not qualify the current checkout.</div>
      <RequestStatus text={pending() ? "Checking the task checkout…" : undefined} />
    </div>
    <ActionBar place="dialog" hint={why() ?? undefined}>
      <button class="btn btn-line btn-lg" data-autofocus onClick={props.onClose}>Cancel</button>
      <button class="btn btn-primary btn-lg" disabled={pending()} aria-disabled={why() ? "true" : undefined} data-why={why()} onClick={() => void clear()}>Clear saved merge record</button>
    </ActionBar>
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
    <Modal open={open("confirm")} onClose={close} label="Confirm"><Show when={of("confirm")} keyed>{x => <Confirm title={x.title} note={x.note} action={x.action} danger={x.danger} pending={x.pending} onClose={close} onGo={() => { void Promise.resolve(x.go()).then(done => { if (done) close(); }); }} />}</Show></Modal>
    <Modal open={open("clear-legacy-merge")} onClose={close} label="Clear saved merge record"><Show when={of("clear-legacy-merge")} keyed>{x => <ClearLegacyMerge d={x} onClose={close} />}</Show></Modal>
    <Modal open={open("prompt")} onClose={close} label="Name"><Show when={of("prompt")} keyed>{x => <Prompt d={x} onClose={close} />}</Show></Modal>
    <Modal open={open("describe")} onClose={close} label={of("describe")?.title ?? "Describe"}><Show when={of("describe")} keyed>{x => <Describe d={x} onClose={close} />}</Show></Modal>
    <Modal open={open("edit-task")} onClose={close} label="Edit task"><Show when={of("edit-task")} keyed>{x => <EditTask taskId={x.taskId} onClose={close} />}</Show></Modal>
    <Modal open={open("accept")} onClose={close} label={of("accept")?.automaticFinish ? "Allow Automatic to finish" : "Accept with open checks"}><Show when={of("accept")} keyed>{x => <Accept d={x} onClose={close} />}</Show></Modal>
    <Modal open={open("changes")} onClose={close} label="Request changes"><Show when={of("changes")} keyed>{x => <Changes d={x} onClose={close} />}</Show></Modal>
    <Modal open={open("attachment")} onClose={close} label="Attachment"><Show when={of("attachment")} keyed>{x => <AttachmentView d={x} onClose={close} />}</Show></Modal>
    <Modal open={open("merge")} onClose={close} label="Finish task" class="wide"><Show when={of("merge")} keyed>{x => <Merge d={x} onClose={close} />}</Show></Modal>
    <Modal open={open("cortex")} onClose={close} label="Resolve automatically"><Show when={of("cortex")} keyed>{x => <AutoResolve d={x} onClose={close} />}</Show></Modal>
    <Modal open={open("delete")} onClose={close} label="Delete worktree"><Show when={of("delete")} keyed>{x => <DeleteWorktree d={x} onClose={close} />}</Show></Modal>
    <Modal open={open("branch")} onClose={close} label="Create a branch"><Show when={of("branch")} keyed>{x => <NewBranch d={x} onClose={close} />}</Show></Modal>
    <Modal open={open("commit")} onClose={close} label="Commit"><Show when={of("commit")} keyed>{x => <CommitDialog checkoutId={x.checkoutId} taskId={x.taskId} onClose={close} />}</Show></Modal>
    <Modal open={open("pull")} onClose={close} label="Pull"><Show when={of("pull")} keyed>{x => <PullDialog checkoutId={x.checkoutId} taskId={x.taskId} onClose={close} />}</Show></Modal>
    <Modal open={open("update")} onClose={close} label="Update from target"><Show when={of("update")} keyed>{x => <UpdateDialog taskId={x.taskId} onClose={close} />}</Show></Modal>
    <Modal open={open("push")} onClose={close} label={of("push")?.pullFirst ? "Pull and push" : "Push"}><Show when={of("push")} keyed>{x => <Push d={x} onClose={close} />}</Show></Modal>
    <Modal open={open("revert")} onClose={close} label="Revert"><Show when={of("revert")} keyed>{x => <Confirm title={x.hunk === undefined ? `Revert ${fileName(x.path)}?` : "Revert this hunk?"} danger action="Revert"
      note={<>This discards your uncommitted change to <span class="mono">{x.path}</span>.{running(x.checkoutId) ? " A task is working in this checkout, so this can block verification." : ""}</>}
      onClose={close} onGo={() => run({ type: "git_revert", checkoutId: x.checkoutId, path: x.path, hunk: x.hunk, confirmed: true, now: ws.store.clock() })} />}</Show></Modal>
    <Modal open={open("revert-commit")} onClose={close} label="Revert commit"><Show when={of("revert-commit")} keyed>{x => <Confirm title="Revert this commit?" action="Revert commit" note={<>Adds a new commit that undoes <span class="mono">{x.sha}</span>, “{x.message}”. History stays as it is.</>}
      onClose={close} onGo={() => run({ type: "revert_commit", checkoutId: x.checkoutId, sha: x.sha, confirmed: true, now: ws.store.clock() })} />}</Show></Modal>
    <Modal open={open("amend")} onClose={close} label="Amend last commit"><Show when={of("amend")} keyed>{x => <Confirm title="Amend the last commit?" danger={x.pushed} action="Amend" note={x.pushed ? "That commit is already pushed. Amending rewrites it, and pushing it again would need a force push, which never happens automatically." : "Replaces the last commit's message and adds the staged files to it."}
      onClose={close} onGo={() => run({ type: "git_commit", checkoutId: x.checkoutId, message: x.message, amend: true, confirmed: true, now: ws.store.clock() })} />}</Show></Modal>
  </>;
}
