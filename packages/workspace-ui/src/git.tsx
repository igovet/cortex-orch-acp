import { createMemo, createSignal, For, Show } from "solid-js";
import { GitLetter } from "./diff";
import type { Commit, GitFile, ProjectRepo, RepoCheckout } from "@cortex/workspace-domain";
import { useWs } from "./ctx";
import { A, Count, I, MenuItem, place, Pop } from "./kit";
import { conflictFacts, dirName, fileName, fileResolved, inConflict, worktreeRows, worktreeState, type WorktreeRow } from "./review-model";
import type { Route } from "./router";
import { sameBranch, type ProjectSummary } from "./selectors";

const COLS = "minmax(0,1.5fr) minmax(0,1fr) 100px 110px 28px";
const Row = (props: { path: string; status: string; children?: unknown }) => <>
  <GitLetter status={props.status} /><span class="p">{fileName(props.path)} <span class="dir">{dirName(props.path)}</span></span>
</>;

function FileRow(props: { f: GitFile; c: RepoCheckout; go: (path: string) => void }) {
  const ws = useWs();
  const staged = () => props.f.stage === "staged";
  return <div class="gf"><button class="ws-gf" onClick={() => props.go(props.f.path)}><Row path={props.f.path} status={props.f.status} /><Show when={props.f.filter}><span class="cap muted">{props.f.filter === "lfs" ? "Git LFS" : `Filter: ${props.f.filter}`}</span></Show></button>
    <span class="acts">
      <Show when={!staged()}><button class="ib" aria-label={`Revert ${fileName(props.f.path)}`} {...ws.needs("git_revert")} onClick={() => ws.open({ kind: "revert", checkoutId: props.c.id, path: props.f.path })}><I n="undo" s={14} /></button></Show>
      <button class="ib" aria-label={`${staged() ? "Unstage" : "Stage"} ${fileName(props.f.path)}`} {...ws.needs("git_stage")} onClick={() => ws.run({ type: "git_stage", checkoutId: props.c.id, path: props.f.path, staged: !staged() })}><I n={staged() ? "minus" : "plus"} s={14} /></button>
    </span></div>;
}

function Status(props: { repo: ProjectRepo; c: RepoCheckout; codeAt: (path: string) => void }) {
  const ws = useWs();
  const c = () => props.c;
  const task = createMemo(() => c().taskId ? ws.store.index().byId.get(c().taskId!) ?? null : null);
  const merge = () => { const t = task()?.task; return t && inConflict(t) ? t.merge : null; };
  const group = (stage: GitFile["stage"]) => c().files.filter(f => f.stage === stage);
  const staged = () => group("staged").length + (merge()?.files.filter(fileResolved).length ?? 0) + (merge()?.clean.length ?? 0);
  // Nothing staged means everything is committed, as in most Git tools: the button never sits dead beside a list of changes.
  const blocked = () => merge() ? "Resolve the conflict first" : !c().files.length ? "There are no changes to commit" : !c().draft.trim() ? "Write a commit message first" : undefined;
  const stagesAll = () => !merge() && group("staged").length === 0 && c().files.length > 0;
  const remote = () => !!props.repo.remote.trim();
  const noUpstream = () => c().upstream !== undefined ? !c().upstream : !props.repo.remote;
  const pullBlocked = () => noUpstream() ? "No upstream is configured for this checkout" : !c().behind ? "No incoming commits in the last fetched state. Fetch to check the upstream." : undefined;
  let commitButton: HTMLButtonElement | undefined, committing = false;
  // Staging and committing are one action: the button stays busy to the end, and neither a click nor Ctrl+Enter starts it twice.
  const commit = () => {
    if (blocked() || committing || ws.gate("git_commit")) return;
    const id = c().id, message = c().draft, all = stagesAll() ? [...c().files] : [];
    committing = true;
    void ws.busy((async () => {
      for (const f of all) { const r = await ws.run({ type: "git_stage", checkoutId: id, path: f.path, staged: true }); if (!r.ok) return; }
      await ws.run({ type: "git_commit", checkoutId: id, message, now: ws.store.clock() });
    })().finally(() => { committing = false; }), commitButton);
  };
  const generate = () => { const count = group("staged").length || c().files.length; ws.run({ type: "git_draft", checkoutId: c().id, message: count ? `Update ${count} file${count === 1 ? "" : "s"}` : "Update the work in this checkout" }); };
  const last = () => props.repo.commits.find(x => x.branch === c().branch);
  const conflictTo = (): Route => ({ name: "task", machine: task()!.machine.id, task: task()!.id, tab: "changes", sub: "conflict" });
  return <aside class="side-pane" aria-label="Status">
    <div class="git-sum">
      <span class="cap muted grow trunc tnum ws-pl6">{c().ahead} ahead, {c().behind} behind <span class="mono">{c().base}</span><Show when={remote() && noUpstream()}>, no upstream</Show></span>
      <Show when={remote()}>
      <button class="ib" aria-label="Fetch" {...ws.needs("git_fetch")} onClick={() => ws.run({ type: "git_fetch", projectId: props.repo.projectId, now: ws.store.clock() })}><I n="refresh" /></button>
      <button class="ib" data-why={pullBlocked()} aria-label="Pull" {...ws.needs("git_pull")} aria-disabled={pullBlocked() || !ws.can("workspace.git_pull") ? "true" : undefined} onClick={() => { if (!pullBlocked()) ws.run({ type: "git_pull", checkoutId: c().id, now: ws.store.clock() }); }}><I n="download" /></button>
      <button class="ib" aria-label="Push" {...ws.needs("git_push")} onClick={() => ws.open({ kind: "push", checkoutId: c().id })}><I n="upload" /></button>
      </Show>
    </div>
    <div class="gscroll">
      <Show when={merge()}>{m => <>
        <div class="gg">Conflicts<span class="r">{conflictFacts(m()).leftFiles}</span></div>
        <For each={m().files.filter(f => !fileResolved(f))}>{f => <A class="gf" to={conflictTo()}><Row path={f.path} status="C" /><span class="acts on"><span class="btn btn-line btn-sm">Resolve</span></span></A>}</For>
      </>}</Show>
      <Show when={staged() > 0}><div class="gg">Staged<span class="r">{staged()}</span></div></Show>
      <For each={merge()?.files.filter(fileResolved) ?? []}>{f => <div class="gf"><Row path={f.path} status="M" /></div>}</For>
      <For each={merge()?.clean ?? []}>{f => <div class="gf"><Row path={f.path} status={f.status} /></div>}</For>
      <For each={group("staged")}>{f => <FileRow f={f} c={c()} go={props.codeAt} />}</For>
      <Show when={group("unstaged").length > 0}><div class="gg">Unstaged<span class="r">{group("unstaged").length}</span></div></Show>
      <For each={group("unstaged")}>{f => <FileRow f={f} c={c()} go={props.codeAt} />}</For>
      <Show when={group("untracked").length > 0}><div class="gg">Untracked<span class="r">{group("untracked").length}</span></div></Show>
      <For each={group("untracked")}>{f => <FileRow f={f} c={c()} go={props.codeAt} />}</For>
      <Show when={c().files.length === 0 && !merge()}><div class="cap muted ws-pad">No uncommitted changes.</div></Show>
    </div>
    <div class="commit-box">
      <textarea class="inp" rows="2" aria-label="Commit message" placeholder="Commit message" value={c().draft} onInput={e => ws.run({ type: "git_draft", checkoutId: c().id, message: e.currentTarget.value })} onKeyDown={e => { if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) { e.preventDefault(); commit(); } }} />
      <Show when={blocked() && c().files.length > 0} fallback={<Show when={stagesAll()}><div class="cap muted">Nothing is staged, so Commit includes all {c().files.length} change{c().files.length === 1 ? "" : "s"}.</div></Show>}><div class="cap muted">{blocked()}.</div></Show>
      <div class="row"><button class="btn btn-sm" {...ws.needs("git_draft")} onClick={generate}><I n="sparkles" s={12} />Generate</button>
        <button class="ib ws-ib" popovertarget="m-commit" aria-label="More commit options"><I n="more" s={14} /></button>
        <Pop id="m-commit" class="pop-up" align="up" label="Commit options"><MenuItem label="Amend last commit…" disabled={last() && c().draft.trim() ? undefined : "Write a message first"} needs="git_commit" onSelect={() => ws.open({ kind: "amend", checkoutId: c().id, message: c().draft, pushed: remote() && !!last()?.pushed })} /></Pop>
        <span class="grow"></span>
        <button class="btn btn-primary btn-sm" ref={commitButton} aria-disabled={blocked() ? "true" : undefined} data-why={blocked()} {...ws.needs("git_commit")} onClick={commit}>Commit<span class="key">Ctrl+Enter</span></button></div>
    </div>
  </aside>;
}

function History(props: { repo: ProjectRepo; c: RepoCheckout }) {
  const ws = useWs();
  const [sha, setSha] = createSignal<string | null>(null);
  const selected = () => props.repo.commits.find(x => x.sha === sha()) ?? props.repo.commits[0];
  const title = (id: string | null) => id ? ws.store.index().byId.get(id)?.task.title ?? null : null;
  const copy = () => { void navigator.clipboard?.writeText(selected().sha); };
  const row = (x: Commit) => <div class="hrow" role="option" tabindex="0" aria-selected={selected()?.sha === x.sha ? "true" : "false"} onClick={() => setSha(x.sha)} onKeyDown={e => { if (e.key === "Enter") setSha(x.sha); }}>
    <svg viewBox="0 0 56 34" aria-hidden="true"><For each={x.graph.paths}>{p => <path class={`g ${p.cls}`} d={p.d} />}</For><circle class={x.graph.dot.cls} cx={x.graph.dot.x} cy="17" r="3.5" /></svg>
    <span class="row hmsg"><span class="trunc">{x.message}</span><Show when={title(x.taskId)}>{t => <span class="tagc task"><I n="message" s={12} />{t()}</span>}</Show><For each={x.tags}>{tag => <span class="tagc w-wide">{tag}</span>}</For></span>
    <span class="cap muted trunc">{x.author}</span><span class="mono faint trunc">{x.sha}</span><span class="cap faint trunc">{x.when}</span>
  </div>;
  return <>
    <div class="hist" role="listbox" aria-label="History"><For each={props.repo.commits}>{row}</For></div>
    <Show when={selected()}>{x => <div class="cdetail">
      <div class="row"><span class="w500">{x().message}</span><span class="grow"></span>
        <button class="btn btn-sm" {...ws.needs("revert_commit")} onClick={() => ws.open({ kind: "revert-commit", checkoutId: props.c.id, sha: x().sha, message: x().message })}>Revert commit…</button>
        <button class="btn btn-sm" {...ws.needs("create_branch")} onClick={() => ws.open({ kind: "branch", projectId: props.repo.projectId, at: x().sha })}>Create branch here</button>
        <button class="btn btn-sm" onClick={copy}><I n="copy" s={12} />Copy SHA</button></div>
      <div class="cap muted">{x().author}, {x().when}{title(x().taskId) ? `, in ${title(x().taskId)}` : ""}. {x().files} files, <span class="add">+{x().add}</span> <span class="del">−{x().del}</span>{props.repo.remote && !x().pushed ? ". Not pushed" : ""}</div>
    </div>}</Show>
  </>;
}

type Menu = { row: WorktreeRow };
function Worktrees(props: { repo: ProjectRepo | null; summary: ProjectSummary; pick: (id: string) => void }) {
  const ws = useWs();
  const rows = createMemo(() => worktreeRows(ws.store.index(), props.summary, props.repo));
  const main = () => props.repo?.checkouts.find(c => c.kind === "main");
  const [menu, setMenu] = createSignal<Menu | null>(null);
  let pop: HTMLDivElement | undefined;
  const open = (row: WorktreeRow, trigger: HTMLElement) => { setMenu({ row }); if (pop) { place(pop, trigger, "right"); pop.showPopover?.(); } };
  const task = () => menu()?.row.task ?? null;
  const c = () => menu()?.row.checkout ?? null;
  const finished = () => { const t = task()?.task; return !!t && ["review", "ready_to_merge"].includes(t.status) ? undefined : "Available for a finished task"; };
  const mergeable = () => { const t = task()?.task; return t?.status === "ready_to_merge" && t.acceptance ? undefined : "Available in Ready to merge"; };
  const gr = (r: WorktreeRow) => <div class="gr" style={{ "--cols": COLS }}>
    <span class="trunc"><span class="mono ws-fg">{r.branch}</span><span class="sub">{r.path}</span></span>
    <span class="cap muted trunc"><Show when={r.task} fallback={r.label}>{v => <A class="lnk" to={{ name: "task", machine: v().machine.id, task: v().id }}>{r.label}</A>}</Show></span>
    <span class="cap tnum">{r.ahead === null ? "" : `${r.ahead} / ${r.behind}`}</span>
    <span><span class={`st ${r.state.tone}`}><I n={r.state.icon} s={12} class={r.state.spin ? "spin" : undefined} />{r.state.label}</span></span>
    <button class="ib ws-ib" aria-label={`Actions for ${r.branch}`} onClick={e => open(r, e.currentTarget)}><I n="more" s={14} /></button>
  </div>;
  return <div class="hist" aria-label="Branches and worktrees">
    <Show when={main()}>{m => <>
      <div class="wg ws-wg">Main checkout</div>
      <div class="gt"><div class="gr" style={{ "--cols": COLS }}><span class="trunc"><span class="mono ws-fg">{m().branch}</span><span class="sub">{m().path}</span></span><span class="cap muted trunc">Git default branch</span><span class="cap tnum">{m().ahead} / {m().behind}</span><span><span class="st st-idle"><I n="file" s={12} />{m().files.length ? `${m().files.length} uncommitted` : "Clean"}</span></span>
        <button class="ib ws-ib" aria-label="Open the main checkout" onClick={() => props.pick(m().id)}><I n="arrow-right" s={14} /></button></div></div>
    </>}</Show>
    <div class="wg ws-wg">Worktrees<Count n={rows().length} /></div>
    <Show when={rows().length > 0} fallback={<div class="cap muted ws-pad">No worktrees in this project.</div>}><div class="gt"><For each={rows()}>{gr}</For></div></Show>
    <Show when={(props.repo?.branches.length ?? 0) > 0}>
      <div class="wg ws-wg">Other branches<Count n={props.repo!.branches.length} /></div>
      <div class="gt"><For each={props.repo!.branches}>{b => <div class="gr" style={{ "--cols": COLS }}><span class="trunc"><span class="mono ws-fg">{b.name}</span><span class="sub">{b.where}</span></span><span class="cap muted trunc">{b.task ?? "None"}</span><span class="cap tnum">{b.ahead} / {b.behind}</span><span><span class="st st-idle">No checkout</span></span><span></span></div>}</For></div>
    </Show>
    <Pop id="m-worktree" class="pop-right" label="Worktree actions" ref={el => { pop = el; }}>
      <Show when={menu()}>
        <Show when={task()}>{v => <MenuItem icon="arrow-right" label="Open task" onSelect={() => ws.go({ name: "task", machine: v().machine.id, task: v().id })} />}</Show>
        <Show when={c()}>{x => <MenuItem icon="branch" label="Show in Git" onSelect={() => props.pick(x().id)} />}</Show>
        <MenuItem icon="terminal" label="Open worktree terminal" disabled="This service offers project shells only. Open the project terminal from its board." />
        <MenuItem icon="external" label={ws.externalLabel()} disabled={ws.noOpener("workspace.open_editor", "noExternal")} onSelect={() => { const x = c(); if (x) ws.run({ type: "open_editor", projectId: x.projectId, checkoutId: x.id }); }} />
        <Show when={task()}>{v => <>
          <div class="mi-sep"></div>
          <MenuItem icon="refresh" label="Update from base…" disabled={finished()} needs="update_from_target" onSelect={() => { ws.run({ type: "update_from_target", taskId: v().id, now: ws.store.clock() }); }} />
          <Show when={!sameBranch(v().task, v().target)}>
          <MenuItem icon="merge" label="Merge…" disabled={mergeable()} needs="integrate" onSelect={() => ws.open({ kind: "merge", taskId: v().id, how: "squash" })} />
          <Show when={props.repo?.remote.trim()}><MenuItem icon="merge" label="Create PR…" disabled={mergeable() ?? (ws.machine().ghAvailable === false ? "Install and sign in to GitHub CLI (gh) to create a pull request" : undefined)} needs="integrate" onSelect={() => ws.open({ kind: "merge", taskId: v().id, how: "pr" })} /></Show>
          </Show>
        </>}</Show>
        <Show when={!task() && c()}>{x => <><div class="mi-sep"></div><MenuItem icon="plus" label="Attach as task" needs="attach_worktree" onSelect={() => ws.run({ type: "attach_worktree", checkoutId: x().id })} /></>}</Show>
        <div class="mi-sep"></div>
        <MenuItem icon="x" label="Delete worktree…" danger disabled={c() ? undefined : ws.t("noWorktreeRecord")} needs="delete_worktree" onSelect={() => ws.open({ kind: "delete", checkoutId: c()!.id })} />
      </Show>
    </Pop>
  </div>;
}

/** A folder without Git works everywhere else. Here it gets one calm offer, which asks before it creates anything. */
export function NoGit(props: { projectId: string }) {
  const ws = useWs();
  const create = () => ws.open({ kind: "confirm", title: "Create an empty Git repository?", action: "Create repository",
    note: "Runs git init in this project folder. Your files stay as they are, and nothing is committed or pushed.",
    go: () => ws.run({ type: "git_init", projectId: props.projectId, confirmed: true }).then(r => r.ok) });
  return <div class="note-panel"><I n="branch" class="faint" /><span class="w500">This project has no Git repository</span><span>Tasks, reviews and files work without it. Git adds history, branches and worktrees.</span>
    <button class="btn btn-line ws-mt8" {...ws.needs("git_init")} onClick={create}>Create empty repository</button></div>;
}

/** Status of one checkout on the left, history or branches on the right. Everything here changes memory only. */
export function GitView(props: { summary: ProjectSummary; repo: ProjectRepo | null; checkout: RepoCheckout | null; right: "history" | "branches"; onRight: (right: "history" | "branches") => void; onPick: (id: string) => void; codeAt: (path: string) => void }) {
  const ws = useWs();
  const task = createMemo(() => { const id = props.checkout?.taskId; return id ? ws.store.index().byId.get(id) ?? null : null; });
  const stuck = () => { const t = task(); return t && inConflict(t.task) && t.task.conflict !== "resolving" ? t : null; };
  return <Show when={props.summary.project.hasGit !== false && props.repo && props.checkout} fallback={<Show when={props.summary.project.hasGit === false} fallback={<div class="note-panel"><span class="w500">No Git record for this project</span><span>{ws.t("noGitFacts")}</span></div>}><NoGit projectId={props.summary.project.id} /></Show>}>
    <Show when={stuck()}>{v => <div class="strip bad"><I n="alert" s={14} class="st-bad" /><span>Merging <span class="mono">{v().task.merge?.source}</span> into <span class="mono">{v().task.merge?.into}</span>: {conflictFacts(v().task.merge!).leftFiles} file{conflictFacts(v().task.merge!).leftFiles === 1 ? "" : "s"} with conflicts.</span>
      <span class="acts"><A class="btn btn-primary btn-sm" to={{ name: "task", machine: v().machine.id, task: v().id, tab: "changes", sub: "conflict" }}>Resolve</A><button class="btn btn-line btn-sm" {...ws.needs("resolve_with_cortex")} onClick={() => ws.open({ kind: "cortex", taskId: v().id })}>Resolve automatically…</button><button class="btn btn-line btn-sm" {...ws.needs("abort_merge")} onClick={() => ws.run({ type: "abort_merge", taskId: v().id, now: ws.store.clock() })}>Abort merge</button></span></div>}</Show>
    <div class="git-body">
      <Status repo={props.repo!} c={props.checkout!} codeAt={props.codeAt} />
      <section class="git-right" aria-label="History">
        <div class="chg-tools"><div class="seg" role="group" aria-label="Git view">
          <button aria-current={props.right === "history" ? "true" : "false"} onClick={() => props.onRight("history")}>History</button>
          <button aria-current={props.right === "branches" ? "true" : "false"} onClick={() => props.onRight("branches")}>Branches and worktrees</button>
        </div></div>
        <Show when={props.right === "history"} fallback={<Worktrees repo={props.repo} summary={props.summary} pick={props.onPick} />}><History repo={props.repo!} c={props.checkout!} /></Show>
      </section>
    </div>
  </Show>;
}

/** Chooses which checkout the Code and Git views show. */
export function CheckoutButton(props: { repo: ProjectRepo; checkout: RepoCheckout; onPick: (id: string) => void }) {
  const ws = useWs();
  const main = () => props.repo.checkouts.filter(c => c.kind === "main");
  const trees = () => props.repo.checkouts.filter(c => c.kind === "worktree");
  const sub = (c: RepoCheckout) => { const v = c.taskId ? ws.store.index().byId.get(c.taskId) : null; return v ? `${v.task.title}, ${worktreeState(ws.store.state(), c).label.toLowerCase()}` : c.kind === "main" ? `${c.path}, ${c.files.length} uncommitted file${c.files.length === 1 ? "" : "s"}` : "No task"; };
  return <>
    <button class="scope-btn" popovertarget="m-checkout"><I n="branch" s={14} class="faint" /><span class="faint ws-w400">{props.checkout.kind === "main" ? "Main checkout" : "Worktree"}</span><span class="mono trunc ws-scope">{props.checkout.branch}</span><I n="chevron-down" s={12} /></button>
    <Pop id="m-checkout" class="pop-anchor" align="left" label="Checkout" style={{ width: "320px" }}>
      <div class="mi-h">Main checkout</div>
      <For each={main()}>{c => <MenuItem icon="folder" label={<span class="mono">{c.branch}</span>} sub={sub(c)} check={c.id === props.checkout.id} onSelect={() => props.onPick(c.id)} />}</For>
      <div class="mi-h">Worktrees</div>
      <For each={trees()}>{c => <MenuItem icon="branch" label={<span class="mono">{c.branch}</span>} sub={sub(c)} check={c.id === props.checkout.id} onSelect={() => props.onPick(c.id)} />}</For>
    </Pop>
  </>;
}

/** The Inspector's facts about the repository and the checkout in view. */
export function GitPanel(props: { summary: ProjectSummary; repo: ProjectRepo; checkout: RepoCheckout }) {
  const ws = useWs();
  const rows = createMemo(() => worktreeRows(ws.store.index(), props.summary, props.repo));
  const conflicts = () => rows().filter(r => r.state.label === "Conflict").length;
  const task = () => props.checkout.taskId ? ws.store.index().byId.get(props.checkout.taskId) ?? null : null;
  return <>
    <div class="ins-sec"><div class="ins-h"><span>Repository</span></div>
      <dl class="kv"><dt>Remote</dt><dd class="mono">{props.repo.remote}</dd><dt>Default</dt><dd class="mono">{props.summary.project.defaultBranch}</dd><dt>Worktrees</dt><dd>{rows().length}{conflicts() ? `, ${conflicts()} with a conflict` : ""}</dd></dl></div>
    <div class="ins-sec"><div class="ins-h"><span>This checkout</span></div>
      <dl class="kv"><Show when={task()}>{v => <><dt>Task</dt><dd>{v().task.title}</dd></>}</Show><dt>Branch</dt><dd class="mono">{props.checkout.branch}</dd><dt>Ahead</dt><dd>{props.checkout.ahead} of {props.checkout.base}</dd><Show when={props.checkout.behind > 0}><dt>Behind</dt><dd>{props.checkout.behind}</dd></Show>
        <Show when={task()?.task.merge && inConflict(task()!.task)} fallback={<><dt>Uncommitted</dt><dd>{props.checkout.files.length ? `${props.checkout.files.length} files` : "None"}</dd></>}><dt>Merge</dt><dd>In progress, {conflictFacts(task()!.task.merge!).leftFiles} file left</dd></Show></dl></div>
  </>;
}
