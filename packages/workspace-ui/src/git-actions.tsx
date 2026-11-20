import { createEffect, createMemo, createSignal, For, on, onCleanup, Show } from "solid-js";
import { capabilityOf, type RepoCheckout } from "@cortex/workspace-domain";
import { useWs, type Ws } from "./ctx";
import { A, I, Keyed, MenuItem, place, Pop } from "./kit";
import { autoGrow } from "./autogrow";
import { ActionBar, Strip } from "./layout";
import { DIRTY, gitItems, gitPrimary, LIVE_WRITER, operationWord, syncCounts, trackedChanges, updateBlocked, type GitFacts, type GitItemId } from "./git-actions-model";
import { behindTarget, repoOf, taskCheckout } from "./review-model";
import type { Route } from "./router";
import type { ProjectSummary, TaskView } from "./selectors";
import { sessionModel } from "./session-model";
import { persisted } from "./theme";

/** Where the title band can open the checkout on view. Only what this computer and machine can do is listed. */
export type OpenTarget = { id: "editor" | "code" | "terminal"; label: string; icon: string };
export function openTargets(ws: Pick<Ws, "can" | "externalLabel" | "terminalAvailable">, checkout: RepoCheckout | null, worktree: boolean): OpenTarget[] {
  const out: OpenTarget[] = [];
  // A task's worktree opens only from its own record; the project folder opens without one.
  if (ws.can("workspace.open_editor") && (checkout || !worktree)) out.push({ id: "editor", label: ws.externalLabel(), icon: ws.can("workspace.open_vscode") ? "vscode" : "external" });
  out.push({ id: "code", label: "Open in Code tab", icon: "code" });
  // The service offers project shells only, so a worktree has no terminal of its own here.
  if (ws.terminalAvailable() && !worktree) out.push({ id: "terminal", label: "Open terminal here", icon: "terminal" });
  return out;
}
const [lastOpen, setLastOpen] = persisted<OpenTarget["id"] | "">("cw.ws.openIn", "", v => v === "" || v === "editor" || v === "code" || v === "terminal");

/** What the band's two split buttons act on: the task on view, or the project folder when no task is open. */
type Scope = { view: TaskView | null; summary: ProjectSummary };
function useScope(props: Scope) {
  const ws = useWs();
  const projectId = () => props.view?.project.id ?? props.summary.project.id;
  const repo = createMemo(() => repoOf(ws.store.state(), projectId()));
  const checkout = createMemo(() => props.view ? taskCheckout(ws.store.state(), props.view.task) : repo()?.checkouts.find(c => c.kind === "main") ?? null);
  return { projectId, repo, checkout };
}

export function OpenInButton(props: Scope) {
  const ws = useWs();
  const s = useScope(props);
  const worktree = () => props.view?.task.checkout.kind === "worktree";
  const targets = createMemo(() => openTargets(ws, s.checkout(), worktree()));
  const primary = () => targets().find(t => t.id === lastOpen()) ?? targets()[0];
  const codeRoute = (): Route => props.view ? { name: "task", machine: props.view.machine.id, task: props.view.id, tab: "code" } : { name: "project", machine: props.summary.machine.id, project: props.summary.project.id, view: "code", scope: { kind: "all" } };
  const open = (t: OpenTarget) => {
    setLastOpen(t.id);
    if (t.id === "code") return ws.go(codeRoute());
    if (t.id === "terminal") { if (!ws.panels.terminalOpen()) ws.toggleTerminal(); return; }
    const c = s.checkout();
    void ws.run({ type: "open_editor", projectId: s.projectId(), ...(c ? { checkoutId: c.id } : {}) });
  };
  return <span class="split hs hs-open" role="group" aria-label="Open">
    <button class="btn btn-line hs-a" aria-label={primary().label} onClick={() => open(primary())}><I n={primary().icon} s={14} /></button>
    <button class="btn btn-line hs-m" popovertarget="m-open-in" aria-label="Open in…"><I n="chevron-down" s={12} /></button>
    <Pop id="m-open-in" class="pop-right" align="right" label="Open in">
      {/* Kept by id: the targets are rebuilt whenever the checkout record changes, and an open menu must not redraw its rows. */}
      <Keyed each={targets()} by={t => t.id}>{t => <MenuItem icon={t().icon} label={t().label} check={t().id === primary().id} onSelect={() => open(t())} />}</Keyed>
    </Pop>
  </span>;
}

/** Pull and push, or one of them, with the service's own words for anything that stops it. */
type Failure = { message: string; retry?: () => void };

/**
 * The checkout's Git state at a glance and its one next action, with every other Git and task action one click away in a menu
 * that always lists the same items in the same order. Every action is an existing service operation behind its own
 * confirmation; nothing here forces, resets or discards.
 */
export function GitButton(props: Scope & { whyId?: string }) {
  const ws = useWs();
  const s = useScope(props);
  // A Git status line, not a Git screen: read as this opens, with the task's own events, on return to the window and after
  // any Git action of the owner, never on a timer. Opening the menu reads it again, and the last state shows meanwhile.
  const interest = createMemo(() => `${s.projectId()}|${props.view?.id ?? ""}`);
  createEffect(on(interest, key => { const [project, task] = key.split("|"); onCleanup(ws.store.interest(project, "status", task || undefined)); }));
  const reread = (e: Event) => { if ((e as ToggleEvent).newState === "open") void ws.store.refreshProjectFacts(s.projectId(), true); };
  const [running, setRunning] = createSignal<GitItemId | null>(null);
  const [progress, setProgress] = createSignal<string | undefined>();
  const [failure, setFailure] = createSignal<Failure | null>(null);
  let split: HTMLSpanElement | undefined, failPop: HTMLDivElement | undefined, menu: HTMLDivElement | undefined;
  const mode = createMemo(() => props.view ? sessionModel(ws.store.index(), props.view.id)?.mode : undefined);
  const facts = (): GitFacts => ({ checkout: s.checkout(), repo: s.repo(), task: props.view, mode: mode(), running: running(), progress: progress(), gate: ws.gate, offered: type => ws.can(capabilityOf(type)) });
  const items = createMemo(() => gitItems(facts()));
  const primary = createMemo(() => gitPrimary(facts(), items()));
  const counts = createMemo(() => syncCounts({ repo: s.repo(), checkout: s.checkout() }));
  const fail = (message: string, retry?: () => void) => {
    setFailure({ message, retry });
    queueMicrotask(() => { if (failPop && split) { place(failPop, split, "right"); failPop.showPopover?.(); } });
  };
  // One action at a time from here. The service refuses at once, with its reason, when another Git action of the project runs.
  const step = async (id: GitItemId, run: () => Promise<boolean>) => {
    if (running()) return false;
    setFailure(null); failPop?.hidePopover?.();
    setRunning(id);
    try { return await run(); } finally { setRunning(null); setProgress(undefined); }
  };
  const command = async (c: Parameters<Ws["run"]>[0], retry: () => void) => {
    const r = await ws.run(c);
    if (!r.ok) fail(r.message, retry);
    return r.ok;
  };
  const fetch = () => void step("fetch", () => command({ type: "git_fetch", projectId: s.projectId(), now: ws.store.clock() }, fetch));
  const pull = () => { const c = s.checkout(); if (c) void step("pull", () => command({ type: "git_pull", checkoutId: c.id, now: ws.store.clock() }, pull)); };
  const target = () => { const c = s.checkout(), r = s.repo(); return c && r ? { remote: r.remote, branch: c.branch } : null; };
  // Pull first, fast-forward only. If it cannot, nothing is pushed and the pull's own reason is what the owner reads.
  const pullAndPush = () => {
    const c = s.checkout(), to = target();
    if (!c || !to) return;
    ws.open({ kind: "confirm", title: `Pull and push ${to.branch}?`, action: "Pull and push",
      note: `Pulls with fast-forward only, then pushes ${to.branch} to ${to.remote}. If the pull cannot fast-forward, nothing is pushed. ${ws.t("pushNote")}`,
      go: () => step("pull_push", async () => {
        setProgress("Pulling…");
        const pulled = await ws.run({ type: "git_pull", checkoutId: c.id, now: ws.store.clock() });
        if (!pulled.ok) { fail(`Nothing was pushed. ${pulled.message}`, pullAndPush); return true; }
        setProgress("Pushing…");
        return command({ type: "git_push", checkoutId: c.id, remote: to.remote, branch: to.branch, confirmed: true }, pullAndPush).then(() => true);
      }) });
  };
  const gitPage = (checkout: string): Route => ({ name: "project", machine: props.view?.machine.id ?? props.summary.machine.id, project: s.projectId(), view: "git", scope: { kind: "all" }, checkout });
  // Finishing a stopped pull: the merge commit, or the rest of the rebase, which can stop again at its next conflict.
  const resume = () => { const c = s.checkout(); if (c) void step("continue", () => command({ type: "git_pull_continue", checkoutId: c.id, now: ws.store.clock() }, resume)); };
  const abort = () => {
    const c = s.checkout(), op = c?.operation;
    if (!c || !op) return;
    const word = operationWord(op);
    ws.open({ kind: "confirm", title: `Abort the ${word}?`, action: `Abort ${word}`, danger: true,
      note: `Returns ${c.branch || op.branch || "this checkout"} to exactly where it was before the ${word}: the same commit, staged files and tracked files. Any conflict you resolved here is discarded. Untracked files stay. Nothing is stashed or reset beyond that.`,
      go: () => step("abort_pull", () => command({ type: "git_pull_abort", checkoutId: c.id, confirmed: true, now: ws.store.clock() }, abort)) });
  };
  const act = (id: GitItemId) => {
    const c = s.checkout(), v = props.view;
    if (id === "commit" && c) return ws.open({ kind: "commit", checkoutId: c.id, ...(v ? { taskId: v.id } : {}) });
    if (id === "fetch") return fetch();
    // Diverged histories: the owner chooses merge or rebase; otherwise Pull only fast-forwards.
    if (id === "pull" && c && c.ahead > 0 && c.behind > 0) return ws.open({ kind: "pull", checkoutId: c.id, ...(v ? { taskId: v.id } : {}) });
    if (id === "pull") return pull();
    if (id === "push" && c) return ws.open({ kind: "push", checkoutId: c.id });
    if (id === "pull_push") return pullAndPush();
    if (id === "resolve" && c) return ws.go(gitPage(c.id));
    if (id === "continue") return resume();
    if (id === "abort_pull") return abort();
    if (!v) return;
    if (id === "update") return ws.open({ kind: "update", taskId: v.id });
    if (id === "merge") return ws.open({ kind: "merge", taskId: v.id, how: "squash" });
    if (id === "pr") return ws.open({ kind: "merge", taskId: v.id, how: "pr" });
    if (id === "view_pr") return void command({ type: "open_pull_request", projectId: v.project.id, taskId: v.id }, () => act("view_pr"));
    if (id === "archive") return ws.archiveTask(v.id);
  };
  // Only a merge the task page explains is ever off. Anything else with nothing to do now, or with a reason it cannot run,
  // opens the menu, where every item says why it is on or off; a running action only shows its progress.
  const off = () => primary().id === "merge" && !!primary().why && !running();
  const press = (e: MouseEvent) => {
    const p = primary();
    if (running() || off()) return;
    if (p.id && !p.why) return act(p.id);
    if (menu) { place(menu, e.currentTarget as HTMLElement, "right"); menu.showPopover?.(); }
  };
  const sync = () => { const n = counts(); if (!n) return ""; return [n.ahead ? `${n.ahead} ahead` : "", n.behind ? `${n.behind} behind` : "", n.files ? `${n.files} uncommitted file${n.files === 1 ? "" : "s"}` : ""].filter(Boolean).join(", "); };
  const at = (id: GitItemId) => items().findIndex(i => i.id === id);
  const sep = (id: GitItemId) => { const i = at(id); return i > 0 && (id === "fetch" || id === "archive" || id === "resolve" || ((id === "update" || id === "merge" || id === "view_pr") && !["update", "merge", "pr"].includes(items()[i - 1].id))); };
  return <>
    <Show when={sync()}><span class="hg-sync tnum" role="note" aria-label={sync()}>
      <Show when={counts()?.ahead}><span>↑{counts()!.ahead}</span></Show><Show when={counts()?.behind}><span>↓{counts()!.behind}</span></Show>
      <Show when={counts()?.files}><span class="hg-dirty"><i class="hg-dot" aria-hidden="true" />{counts()!.files}</span></Show>
    </span></Show>
    <span class="split hs" role="group" aria-label="Git" ref={split}>
      <button class={`btn ${primary().strong ? "btn-primary" : "btn-line"} hs-a hg-a`} aria-label={primary().label} aria-busy={running() ? "true" : undefined}
        aria-haspopup={!running() && !off() && (!primary().id || primary().why) ? "menu" : undefined}
        aria-disabled={off() ? "true" : undefined} data-why={off() ? primary().why : undefined} aria-describedby={off() ? props.whyId : undefined} onClick={press}>
        <I n={primary().icon} s={14} class={running() ? "spin" : undefined} /><span class="hs-t">{primary().label}</span></button>
      <button class={`btn ${primary().strong ? "btn-primary" : "btn-line"} hs-m`} popovertarget="m-git" aria-label="Git actions"><I n="branch" s={14} class="hs-n" /><I n="chevron-down" s={12} /></button>
    </span>
    <Pop id="m-git" class="pop-right" align="right" label="Git actions" style={{ "min-width": "260px" }} ref={el => { menu = el; el.addEventListener("toggle", reread); }}>
      {/* Kept by id: the list is rebuilt on every Git read, and its rows must not be redrawn under the pointer. */}
      <Keyed each={items()} by={i => i.id}>{i => <>
        <Show when={sep(i().id)}><div class="mi-sep"></div></Show>
        <MenuItem icon={i().icon} label={i().label} sub={i().why ? undefined : i().sub} disabled={i().why} onSelect={() => act(i().id)} />
      </>}</Keyed>
    </Pop>
    <Pop id="m-git-fail" class="pop-right" label="Git action failed" style={{ width: "320px", padding: "10px 12px" }} ref={el => { failPop = el; }}>
      <Show when={failure()}>{f => <div role="alert" class="hg-fail"><span>{f().message}</span>
        <span class="row ws-mt6"><Show when={f().retry}><button class="btn btn-line btn-sm" onClick={() => { failPop?.hidePopover?.(); f().retry!(); }}>Retry</button></Show>
          <button class="btn btn-sm" onClick={() => { setFailure(null); failPop?.hidePopover?.(); }}>Dismiss</button></span></div>}</Show>
    </Pop>
  </>;
}

/** The title band's two split buttons, desktop only: open the checkout somewhere, and its Git actions. A folder without Git has only the first. */
export function BandActions(props: Scope & { whyId?: string }) {
  const ws = useWs();
  return <Show when={ws.shell.kind !== "phone"}>
    <OpenInButton view={props.view} summary={props.summary} />
    <Show when={(props.view?.project ?? props.summary.project).hasGit !== false}><GitButton view={props.view} summary={props.summary} whyId={props.whyId} /></Show>
  </Show>;
}

/**
 * Commit from the title band: what will be taken in, and a message the owner confirms. The message is drafted from the changes
 * when the machine can write one for this task; nothing is committed until Commit is pressed. A task still working here is
 * stopped first, or committed around on purpose, as the Code screen asks before saving.
 */
export function CommitDialog(props: { checkoutId: string; taskId?: string; onClose: () => void }) {
  const ws = useWs();
  const found = createMemo(() => { for (const r of ws.store.state().repos) { const c = r.checkouts.find(x => x.id === props.checkoutId); if (c) return { repo: r, c }; } return null; });
  const c = () => found()?.c ?? null;
  // A task works in this checkout: its own worktree, or the project folder that every project-folder task shares. A sprint's
  // planning task is not among them (`views` leaves it out): it only reads the project, so it writes nothing to commit
  // around, and no commit or branch action is ever offered for it.
  const writer = createMemo(() => {
    const at = c(), project = found()?.repo.projectId;
    if (!at) return null;
    return ws.store.index().views.find(v => v.task.process === "live" && v.project.id === project && (at.taskId === v.id || (at.kind === "main" && v.task.checkout.kind === "main"))) ?? null;
  });
  const [anyway, setAnyway] = createSignal(false);
  const files = () => c()?.files ?? [];
  const count = (stage: string) => files().filter(f => f.stage === stage).length;
  const staged = () => count("staged");
  const fallback = () => { const n = staged() || files().length; return n ? `Update ${n} file${n === 1 ? "" : "s"}` : ""; };
  let edited = !!c()?.draft.trim();
  // While the message is being written from the changes the field stays empty and Commit stays off: a placeholder
  // message is never what gets committed. The file-list fallback appears only once writing has finished without text.
  const drafting = !edited && !!props.taskId && files().length > 0;
  const [message, setMessage] = createSignal(edited ? c()!.draft : drafting ? "" : fallback());
  const [writing, setWriting] = createSignal<"writing" | "model" | "fallback" | null>(edited ? null : drafting ? "writing" : "fallback");
  // Offered once as the dialog opens, written from these files for the task they belong to. Typed text is never replaced.
  if (drafting) {
    const v = ws.store.index().byId.get(props.taskId!);
    void ws.store.draftIntegrationText({ taskId: props.taskId!, method: "commit", targetBranch: c()!.branch, paths: files().map(f => f.path), summary: v?.task.result?.summary })
      .catch(() => null)
      .then(text => {
        if (edited) return;
        setMessage(text?.message || fallback());
        setWriting(text?.message ? text.source : "fallback");
      });
  }
  const note = () => writing() === "writing" ? "Writing from the changes…" : writing() === "model" ? "Written from the changes, you can edit it" : writing() === "fallback" ? "Listed from the changed files, you can edit it" : "Your saved draft";
  const why = () => !c() ? "This checkout is no longer in the last Git read" : !files().length ? "Nothing to commit" : writing() === "writing" ? "The commit message is still being written" : !message().trim() ? "Write a commit message first" : writer() && !anyway() ? `${LIVE_WRITER} Stop it first, or choose Commit anyway.` : ws.gate("git_commit");
  let working = false;
  const commit = (e: MouseEvent) => {
    if (why() || working) return;
    working = true;
    void ws.busy(ws.run({ type: "git_commit", checkoutId: props.checkoutId, message: message(), now: ws.store.clock() }).then(r => { if (r.ok) props.onClose(); }).finally(() => { working = false; }), e.currentTarget as HTMLElement);
  };
  const gitPage = (): Route | null => { const r = found()?.repo; const m = ws.store.state().projects.find(p => p.id === r?.projectId); return r && m ? { name: "project", machine: m.machineId, project: r.projectId, view: "git", scope: { kind: "all" } } : null; };
  const shown = () => files().slice(0, 8);
  return <>
    <div class="dlg-h"><h2>Commit in <span class="mono">{c()?.branch ?? ""}</span></h2><button class="ib" aria-label="Cancel" onClick={() => props.onClose()}><I n="x" /></button></div>
    <div class="dlg-b">
      <Show when={writer()}>{w => <Strip tone="warn" icon={<I n="alert" s={14} class="st-warn" />} action={<Show when={!anyway()}><span class="acts"><button class="btn btn-line btn-sm" {...ws.needs("stop_task")} onClick={() => ws.stopTask(w().id)}>Stop the task first</button><button class="btn btn-sm" onClick={() => setAnyway(true)}>Commit anyway</button></span></Show>}>
        {anyway() ? `“${w().task.title}” is still working here. Its later changes stay uncommitted.` : `“${w().task.title}” is working in this checkout and may still change these files.`}</Strip>}</Show>
      <div class="lab">{staged() ? `${staged()} staged` : "Nothing staged"}{count("unstaged") ? `, ${count("unstaged")} unstaged` : ""}{count("untracked") ? `, ${count("untracked")} untracked` : ""}</div>
      <Show when={!staged() && files().length}><div class="cap muted">Nothing is staged, so Commit includes all {files().length} change{files().length === 1 ? "" : "s"}.</div></Show>
      <div class="pre"><For each={shown()}>{f => <div class="mono trunc">{f.status} {f.path}</div>}</For>
        <Show when={files().length > shown().length}><div class="cap faint">{files().length - shown().length} more</div></Show></div>
      <label class="fld"><span class="lab">Commit message <span class="faint">{note()}</span></span>
        <textarea class="inp" rows="3" data-autofocus ref={el => autoGrow(el, message)} value={message()} placeholder={writing() === "writing" ? "Writing the message from the changes…" : undefined} onInput={e => { edited = true; setWriting(null); setMessage(e.currentTarget.value); }} /></label>
    </div>
    <ActionBar place="dialog" start={<Show when={gitPage()}>{to => <A class="lnk" to={to()} onClick={() => props.onClose()}>Stage files in Git</A>}</Show>} hint="Nothing is committed until you confirm. Nothing is pushed.">
      <button class="btn btn-line btn-lg" onClick={() => props.onClose()}>Cancel</button>
      <button class="btn btn-primary btn-lg" aria-disabled={why() ? "true" : undefined} data-why={why()} onClick={commit}>Commit</button>
    </ActionBar>
  </>;
}

/** Where a checkout of the current state is, with its project's repository. */
const useCheckout = (id: string) => {
  const ws = useWs();
  return createMemo(() => { for (const r of ws.store.state().repos) { const c = r.checkouts.find(x => x.id === id); if (c) return { repo: r, c }; } return null; });
};
const n = (count: number, word: string) => `${count} ${word}${count === 1 ? "" : "s"}`;

/**
 * Pull on diverged histories: the local branch and its upstream both have commits the other lacks, so the owner chooses how
 * the remote commits come in. Merge is the default; nothing is pushed, stashed, reset or forced, and a pull that stops at a
 * conflict stays stopped in the checkout until the owner continues or aborts it.
 */
export function PullDialog(props: { checkoutId: string; taskId?: string; onClose: () => void }) {
  const ws = useWs();
  const found = useCheckout(props.checkoutId);
  const c = () => found()?.c ?? null;
  const [mode, setMode] = createSignal<"merge" | "rebase">("merge");
  const [failure, setFailure] = createSignal("");
  const writer = createMemo(() => { const at = c(), project = found()?.repo.projectId; return at ? ws.store.index().views.find(v => v.task.process === "live" && v.project.id === project && (at.taskId === v.id || (at.kind === "main" && v.task.checkout.kind === "main"))) ?? null : null; });
  const upstream = () => c()?.upstream || c()?.base || "the upstream";
  const why = () => !c() ? "This checkout is no longer in the last Git read" : c()!.operation ? "A pull is already stopped in this checkout. Continue or abort it first." : !c()!.branch ? "This checkout is not on a branch (detached HEAD)" : trackedChanges(c()!) ? DIRTY : writer() ? `${LIVE_WRITER} Stop it first.` : ws.gate("git_pull");
  const fetched = () => { const at = found()?.repo.fetchedAt; return at ? `Counted at the last fetch, ${new Date(at).toLocaleString()}.` : "Counted at the last fetch."; };
  const gitPage = (): Route | null => { const r = found()?.repo; const p = ws.store.state().projects.find(x => x.id === r?.projectId); return r && p ? { name: "project", machine: p.machineId, project: r.projectId, view: "git", scope: { kind: "all" }, checkout: props.checkoutId } : null; };
  let working = false;
  const go = (e: MouseEvent) => {
    if (why() || working) return;
    working = true; setFailure("");
    void ws.busy(ws.run({ type: "git_pull", checkoutId: props.checkoutId, mode: mode(), now: ws.store.clock() }).then(r => { if (r.ok) props.onClose(); else setFailure(r.message); }).finally(() => { working = false; }), e.currentTarget as HTMLElement);
  };
  return <>
    <div class="dlg-h"><h2>Pull into <span class="mono">{c()?.branch ?? ""}</span></h2><button class="ib" aria-label="Cancel" onClick={() => props.onClose()}><I n="x" /></button></div>
    <div class="dlg-b">
      <Show when={c()}>{x => <div class="cap muted">The histories diverged: this branch has {n(x().ahead, "commit")} that <span class="mono">{upstream()}</span> does not, and <span class="mono">{upstream()}</span> has {n(x().behind, "commit")} that this branch does not. {fetched()}</div>}</Show>
      <div class="ws-fields" role="radiogroup" aria-label="How to bring in the remote commits">
        <label class="rc"><input type="radio" name="pull-how" checked={mode() === "merge"} onChange={() => setMode("merge")} /><span><span class="t">Merge</span><span class="d">One merge commit joins both sides. Nothing is rewritten, so the push afterwards is a plain push.</span></span></label>
        <label class="rc"><input type="radio" name="pull-how" checked={mode() === "rebase"} onChange={() => setMode("rebase")} /><span><span class="t">Rebase</span><span class="d">Replays this branch's {n(c()?.ahead ?? 0, "commit")} on top of <span class="mono">{upstream()}</span> for a straight history. Those commits get new identities; they are not on <span class="mono">{upstream()}</span>, so nothing published there is rewritten.</span></span></label>
      </div>
      <div class="cap muted">Pull fetches <span class="mono">{upstream()}</span> again first, so it can bring in commits newer than these counts. If both sides changed the same lines it stops at the conflict: resolve it in Git and continue, or abort, which returns this checkout exactly to where it is now. Nothing is pushed, stashed, reset or forced.</div>
      <Show when={c() && trackedChanges(c()!) > 0}><Strip tone="warn" icon={<I n="alert" s={14} class="st-warn" />} action={<button class="btn btn-line btn-sm" onClick={() => ws.open({ kind: "commit", checkoutId: props.checkoutId, ...(props.taskId ? { taskId: props.taskId } : {}) })}>Commit…</button>}>{DIRTY}</Strip></Show>
      <Show when={failure()}><div role="alert" class="cap st-warn">{failure()} <Show when={c()?.operation && gitPage()}>{to => <A class="lnk" to={to()} onClick={() => props.onClose()}>Resolve conflicts</A>}</Show></div></Show>
    </div>
    <ActionBar place="dialog" hint="Nothing is pulled until you confirm.">
      <button class="btn btn-line btn-lg" data-autofocus onClick={() => props.onClose()}>Cancel</button>
      <button class="btn btn-primary btn-lg" aria-disabled={why() ? "true" : undefined} data-why={why()} onClick={go}>{mode() === "merge" ? "Pull with merge" : "Pull with rebase"}</button>
    </ActionBar>
  </>;
}

/**
 * Update from target: the target branch's new commits merged into the task's own branch, in its worktree. The target is not
 * changed and nothing is rewritten. The tree the owner reviewed changes, so the reviewed result stops being current and any
 * acceptance of it is withdrawn; a conflict stays in the task, with its own conflict view and Abort merge.
 */
export function UpdateDialog(props: { taskId: string; onClose: () => void }) {
  const ws = useWs();
  const view = createMemo(() => ws.store.index().byId.get(props.taskId) ?? null);
  const checkout = createMemo(() => { const v = view(); return v ? taskCheckout(ws.store.state(), v.task) : null; });
  const [failure, setFailure] = createSignal("");
  const target = () => view()?.target ?? "";
  const behind = () => behindTarget(checkout(), target());
  const why = () => { const v = view(); return !v ? "This task is no longer on this machine" : updateBlocked(checkout(), v) ?? ws.gate("update_from_target"); };
  const conflictAt = (): Route | null => { const v = view(); return v && v.task.conflict !== "none" ? { name: "task", machine: v.machine.id, task: v.id, tab: "changes", sub: "conflict" } : null; };
  let working = false;
  const go = (e: MouseEvent) => {
    const v = view();
    if (!v?.task.result || why() || working) return;
    working = true; setFailure("");
    void ws.busy(ws.run({ type: "update_from_target", taskId: v.id, confirmed: true, digest: v.task.result.digest, now: ws.store.clock() }).then(r => {
      if (r.ok) { props.onClose(); ws.go({ name: "task", machine: v.machine.id, task: v.id, tab: "changes" }); } else setFailure(r.message);
    }).finally(() => { working = false; }), e.currentTarget as HTMLElement);
  };
  return <Show when={view()}>{v => <>
    <div class="dlg-h"><h2>Update <span class="mono">{v().task.checkout.branch}</span> from <span class="mono">{target()}</span>?</h2><button class="ib" aria-label="Cancel" onClick={() => props.onClose()}><I n="x" /></button></div>
    <div class="dlg-b">
      <ul class="conseq">
        <li><I n="merge" s={14} /><span>Merges {behind() ? n(behind()!, "new commit") : "the new commits"} of <span class="mono">{target()}</span> into <span class="mono">{v().task.checkout.branch}</span>, in the task's worktree. <span class="mono">{target()}</span> is not changed, and no commit is rewritten.</span></li>
        <li><I n="alert" s={14} /><span>Result v{v().task.result?.version} was reviewed on the tree before this, so it is no longer current afterwards{v().task.acceptance ? <>: your acceptance of v{v().task.acceptance!.version} is withdrawn</> : ""}. Request changes for fresh checks before it can be merged.</span></li>
        <li><I n="pencil" s={14} /><span>If both changed the same lines, the update stops at the conflict in this task: resolve it there, or abort it, which returns the worktree exactly to how it is now.</span></li>
      </ul>
      <Show when={checkout() && trackedChanges(checkout()!) > 0}><Strip tone="warn" icon={<I n="alert" s={14} class="st-warn" />} action={<button class="btn btn-line btn-sm" onClick={() => ws.open({ kind: "commit", checkoutId: checkout()!.id, taskId: v().id })}>Commit…</button>}>The task's worktree has uncommitted changes. Commit them first; a merge over them could not be aborted exactly, and nothing is stashed.</Strip></Show>
      <Show when={failure()}><div role="alert" class="cap st-warn">{failure()} <Show when={conflictAt()}>{to => <A class="lnk" to={to()} onClick={() => props.onClose()}>Open the conflict</A>}</Show></div></Show>
    </div>
    <ActionBar place="dialog" hint="Nothing is merged until you confirm.">
      <button class="btn btn-line btn-lg" data-autofocus onClick={() => props.onClose()}>Cancel</button>
      <button class="btn btn-primary btn-lg" aria-disabled={why() ? "true" : undefined} data-why={why()} {...(why() ? {} : ws.needs("update_from_target"))} onClick={go}>Update from <span class="l-branch">{target()}</span></button>
    </ActionBar>
  </>}</Show>;
}
