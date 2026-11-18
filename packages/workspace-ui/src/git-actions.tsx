import { createEffect, createMemo, createSignal, For, on, onCleanup, Show } from "solid-js";
import { capabilityOf, type RepoCheckout } from "@cortex/workspace-domain";
import { useWs, type Ws } from "./ctx";
import { A, I, Keyed, MenuItem, place, Pop } from "./kit";
import { autoGrow } from "./autogrow";
import { ActionBar, Strip } from "./layout";
import { gitItems, gitPrimary, LIVE_WRITER, syncCounts, type GitFacts, type GitItemId } from "./git-actions-model";
import { repoOf, taskCheckout } from "./review-model";
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
  const update = () => { const v = props.view; if (v) void step("update", async () => { const ok = await command({ type: "update_from_target", taskId: v.id, now: ws.store.clock() }, update); if (ok) ws.go({ name: "task", machine: v.machine.id, task: v.id, tab: "changes" }); return ok; }); };
  const act = (id: GitItemId) => {
    const c = s.checkout(), v = props.view;
    if (id === "commit" && c) return ws.open({ kind: "commit", checkoutId: c.id, ...(v ? { taskId: v.id } : {}) });
    if (id === "fetch") return fetch();
    if (id === "pull") return pull();
    if (id === "push" && c) return ws.open({ kind: "push", checkoutId: c.id });
    if (id === "pull_push") return pullAndPush();
    if (id === "update") return update();
    if (!v) return;
    if (id === "merge") return ws.open({ kind: "merge", taskId: v.id, how: "squash" });
    if (id === "pr") return ws.open({ kind: "merge", taskId: v.id, how: "pr" });
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
  const sep = (id: GitItemId) => { const i = at(id); return i > 0 && (id === "fetch" || id === "archive" || ((id === "update" || id === "merge") && !["update", "merge"].includes(items()[i - 1].id))); };
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
        <MenuItem icon={i().icon} label={i().label} disabled={i().why} onSelect={() => act(i().id)} />
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
  // A task works in this checkout: its own worktree, or the project folder that every project-folder task shares.
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
  const [message, setMessage] = createSignal(c()?.draft.trim() ? c()!.draft : fallback());
  let edited = !!c()?.draft.trim();
  const [writing, setWriting] = createSignal<"writing" | "model" | "fallback" | null>(edited ? null : "fallback");
  // Offered once as the dialog opens, written from these files for the task they belong to. Typed text is never replaced.
  if (!edited && props.taskId && files().length) {
    const v = ws.store.index().byId.get(props.taskId);
    setWriting("writing");
    void ws.store.draftIntegrationText({ taskId: props.taskId, method: "commit", targetBranch: c()!.branch, paths: files().map(f => f.path), summary: v?.task.result?.summary }).then(text => {
      if (text?.message && !edited) setMessage(text.message);
      setWriting(text ? text.source : "fallback");
    });
  }
  const note = () => writing() === "writing" ? "Writing from the changes…" : writing() === "model" ? "Written from the changes, you can edit it" : writing() === "fallback" ? "Listed from the changed files, you can edit it" : "Your saved draft";
  const why = () => !c() ? "This checkout is no longer in the last Git read" : !files().length ? "Nothing to commit" : !message().trim() ? "Write a commit message first" : writer() && !anyway() ? `${LIVE_WRITER} Stop it first, or choose Commit anyway.` : ws.gate("git_commit");
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
        <textarea class="inp" rows="3" data-autofocus ref={el => autoGrow(el, message)} value={message()} onInput={e => { edited = true; setWriting(null); setMessage(e.currentTarget.value); }} /></label>
    </div>
    <ActionBar place="dialog" start={<Show when={gitPage()}>{to => <A class="lnk" to={to()} onClick={() => props.onClose()}>Stage files in Git</A>}</Show>} hint="Nothing is committed until you confirm. Nothing is pushed.">
      <button class="btn btn-line btn-lg" onClick={() => props.onClose()}>Cancel</button>
      <button class="btn btn-primary btn-lg" aria-disabled={why() ? "true" : undefined} data-why={why()} onClick={commit}>Commit</button>
    </ActionBar>
  </>;
}
