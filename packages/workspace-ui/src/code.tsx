import { createEffect, createMemo, createSignal, createUniqueId, For, lazy, on, onCleanup, Show, Suspense, untrack, useContext } from "solid-js";
import type { ProjectRepo, RepoCheckout } from "@cortex/workspace-domain";
import { useWs } from "./ctx";
import { Code, Counts, Diff, GitLetter, type HunkActions } from "./diff";
import { bufferKey, hasUnsaved, saveWord, UNCONFIRMED_WRITER, type FileBuffer } from "./editor-buffers";
import { ScreenActive } from "./freshness";
import { languageOf } from "./highlight";
import { A, I, Modal } from "./kit";
import { ActionBar, revealCutIn, Toolbar } from "./layout";
import { addedLines, allDirs, checkoutFiles, dirName, fileName, searchFiles, sourceOf, splitMatch, treeRows } from "./review-model";
import type { Route } from "./router";

/** Its own chunk, so the first window pays nothing for an editor until a file opens. */
const CodeEditor = lazy(() => import("./editor"));
/** How often the open file is compared with the disk, so a worker's change shows before the owner saves over it. */
const DISK_POLL_MS = 3000;

/** Why Save does nothing right now, or nothing when it would save. */
function saveBlocked(b: FileBuffer) {
  if (b.readOnly) return b.readOnly;
  if (b.writer) return "Saving waits until the task stops. Your text stays here.";
  if (b.conflict) return "Choose what to keep first.";
  if (b.saving) return "Saving…";
  return hasUnsaved(b) ? undefined : "No changes to save";
}

/** The one question a dirty tab asks before it goes. Discarding is always the owner's own choice. */
function CloseFile(props: { buffer: FileBuffer | null; onCancel: () => void; onDiscard: () => void; onSave: () => void }) {
  return <Modal open={!!props.buffer} onClose={props.onCancel} label="Unsaved changes">
    <Show when={props.buffer}>{b => <>
      <div class="dlg-h"><h2>Save changes to {fileName(b().path)}?</h2><button class="ib" aria-label="Cancel" onClick={() => props.onCancel()}><I n="x" /></button></div>
      <div class="dlg-b"><p>Closing this file without saving drops the text you typed.</p></div>
      <ActionBar place="dialog" start={<button class="btn" onClick={() => props.onCancel()}>Cancel</button>}><button class="btn btn-line" onClick={() => props.onDiscard()}>Discard changes</button><button class="btn btn-primary" data-autofocus aria-disabled={saveBlocked(b()) ? "true" : undefined} data-why={saveBlocked(b())} onClick={() => { if (!saveBlocked(b())) props.onSave(); }}>Save</button></ActionBar>
    </>}</Show>
  </Modal>;
}

/** Every way of closing the window lands here while unsaved text exists. */
export function UnsavedOnClose(props: { open: boolean; onClose: () => void }) {
  const ws = useWs();
  const files = () => ws.buffers.unsaved();
  const [saving, setSaving] = createSignal(false);
  const saveAll = async () => {
    setSaving(true);
    for (const b of files()) await ws.buffers.save(b.key);
    setSaving(false);
    if (!files().length) ws.chrome?.closeNow();
  };
  return <Modal open={props.open} onClose={props.onClose} label="Unsaved files">
    <div class="dlg-h"><h2>{files().length === 1 ? "1 file has" : `${files().length} files have`} unsaved changes</h2><button class="ib" aria-label="Keep working" onClick={() => props.onClose()}><I n="x" /></button></div>
    <div class="dlg-b"><ul class="ws-unsaved"><For each={files()}>{b => <li><span class="mono trunc">{b.path}</span><span class="cap faint">{b.conflict || b.writer || b.refusal ? saveBlocked(b) ?? b.refusal : saveWord(b)}</span></li>}</For></ul></div>
    <ActionBar place="dialog" start={<button class="btn" onClick={() => props.onClose()}>Keep working</button>}><button class="btn btn-line" onClick={() => ws.chrome?.closeNow()}>Close without saving</button><button class="btn btn-primary" data-autofocus aria-disabled={saving() ? "true" : undefined} data-why={saving() ? "Saving…" : undefined} onClick={() => { if (!saving()) void saveAll(); }}>Save all and close</button></ActionBar>
  </Modal>;
}

function GoToFile(props: { open: boolean; paths: string[]; onClose: () => void; onPick: (path: string) => void }) {
  const [text, setText] = createSignal("");
  const found = () => props.paths.filter(p => p.toLowerCase().includes(text().trim().toLowerCase())).slice(0, 12);
  const pick = (path: string | undefined) => { if (path) { props.onClose(); props.onPick(path); } };
  return <Modal open={props.open} onClose={props.onClose} label="Go to file" class="ws-palette wide">
    <div class="ws-pal-h"><I n="search" class="faint" /><input class="ws-pal-in" data-autofocus aria-label="Go to file" placeholder="Type a file name" value={text()} onInput={e => setText(e.currentTarget.value)} onKeyDown={e => { if (e.key === "Enter") { e.preventDefault(); pick(found()[0]); } }} /></div>
    <div class="ws-pal-b"><For each={found()} fallback={<p class="ws-pal-none">No matches</p>}>{p => <button class="mi" onClick={() => pick(p)}><I n="file" /><span class="grow trunc">{p}</span></button>}</For></div>
  </Modal>;
}

/** Tree, tabs, the file and its diff for one checkout. `at` builds the address, so the same view serves a project and a task. */
export function CodeView(props: { repo: ProjectRepo; checkout: RepoCheckout; file?: string; line?: number; at: (file: string, line?: number) => Route }) {
  const ws = useWs();
  const state = () => ws.store.state();
  const data = createMemo(() => checkoutFiles(state(), props.repo, props.checkout));
  const [tabs, setTabs] = createSignal<{ path: string; preview: boolean }[]>([]);
  const [onlyChanged, setOnlyChanged] = createSignal(false);
  const [collapsed, setCollapsed] = createSignal<Set<string>>(new Set());
  const [query, setQuery] = createSignal("");
  const [matchCase, setMatchCase] = createSignal(false);
  const [edit, setEdit] = createSignal(false);
  const [goTo, setGoTo] = createSignal(false);
  const [asking, setAsking] = createSignal<string | null>(null);
  const left = () => ws.codeLeft();
  const mode = () => ws.codeMode();
  const editable = () => ws.store.canEdit();
  const diffOf = (path: string) => data().diffs.find(d => d.path === path);
  const working = (path: string) => props.checkout.files.find(f => f.path === path);
  const paths = createMemo(() => { const changed = new Set(data().diffs.map(d => d.path)); return onlyChanged() ? data().tree.filter(p => changed.has(p)) : data().tree; });
  const rows = createMemo(() => treeRows(paths(), collapsed()));
  const open = () => props.file ?? null;
  const bufferOf = (path: string) => ws.buffers.list().find(b => b.key === bufferKey(props.checkout.id, path)) ?? null;
  const buffer = createMemo(() => open() ? bufferOf(open()!) : null);
  const task = createMemo(() => props.checkout.taskId ? ws.store.index().byId.get(props.checkout.taskId) ?? null : null);
  const live = () => task()?.task.process === "live";
  // The machine's answer outranks the task list: it is what a save will meet, and it also covers a main-checkout task.
  const writer = () => { const b = buffer(); return b?.loaded && !b.missing ? b.writer : live() ? task()!.id : null; };
  const writerTask = () => { const id = writer(); return !id || id === UNCONFIRMED_WRITER ? null : ws.store.index().byId.get(id) ?? task(); };
  const locked = (b: FileBuffer) => !!b.readOnly || (!!writer() && !edit() && !hasUnsaved(b));
  const source = createMemo(() => open() ? sourceOf(props.repo, props.checkout, data().diffs, open()!) : null);
  const added = createMemo(() => addedLines(open() ? diffOf(open()!) : undefined));
  const found = createMemo(() => searchFiles(props.repo, props.checkout, data().diffs, data().tree, query(), matchCase()));
  const toggleDir = (dir: string) => { const next = new Set(collapsed()); if (next.has(dir)) next.delete(dir); else next.add(dir); setCollapsed(next); };
  const show = (file: string, line?: number) => ws.go(props.at(file, line));
  const ensure = (path: string, preview: boolean) => setTabs(list => {
    if (list.some(t => t.path === path)) return preview ? list : list.map(t => t.path === path ? { ...t, preview: false } : t);
    return [...list.filter(t => !t.preview), { path, preview }];
  });
  const drop = (path: string) => {
    const b = bufferOf(path);
    if (b) ws.buffers.close(b.key);
    const next = tabs().filter(t => t.path !== path);
    setTabs(next);
    if (path === open()) { const last = next.at(-1)?.path ?? data().diffs[0]?.path ?? data().tree[0]; if (last) ws.go(props.at(last), { replace: true }); }
  };
  const close = (path: string) => { const b = bufferOf(path); if (b && hasUnsaved(b)) setAsking(path); else drop(path); };
  const saveNow = () => { const b = buffer(); if (b && !saveBlocked(b)) void ws.buffers.save(b.key); };
  const saveAndDrop = async (path: string) => {
    const b = bufferOf(path);
    setAsking(null);
    if (!b) return;
    await ws.buffers.save(b.key);
    const after = bufferOf(path);
    if (after && !hasUnsaved(after)) drop(path);
  };
  // A real checkout keeps its files on the machine, so one is read as it opens. The diff's lines stand in until it answers.
  createEffect(() => {
    const f = props.file;
    if (!f) return;
    if (editable()) void ws.buffers.open(props.repo.projectId, props.checkout.id, f);
    else if (ws.store.mode === "real") void ws.store.loadSource(props.repo.projectId, props.checkout.id, f);
  });
  // Only the open file is compared; any other is checked by its own save, which never overwrites.
  const shown = useContext(ScreenActive);
  let parked = false;
  createEffect(() => {
    const f = props.file;
    if (!f || !editable()) return;
    if (!shown()) { parked = true; return; }
    const key = bufferKey(props.checkout.id, f), look = () => { if (document.visibilityState !== "hidden") void ws.buffers.refresh(key); };
    // Back on view: the file may have moved on meanwhile.
    if (parked) { parked = false; look(); }
    const timer = setInterval(look, DISK_POLL_MS);
    window.addEventListener("focus", look);
    onCleanup(() => { clearInterval(timer); window.removeEventListener("focus", look); });
  });
  // Opening a checkout lands on its first changed file, so the work in progress is what shows first.
  createEffect(() => { if (!props.file) { const first = data().diffs[0]?.path ?? data().tree[0]; if (first) ws.go(props.at(first), { replace: true }); } });
  // Memos, because `on` fires on every route change otherwise, even when the value is the same.
  const checkoutId = createMemo(() => props.checkout.id);
  const spot = createMemo(() => `${props.file ?? ""}:${props.line ?? ""}`);
  createEffect(on(checkoutId, () => setTabs([]), { defer: true }));
  createEffect(() => { const f = props.file; if (f) ensure(f, !diffOf(f)); });
  // Unsaved text keeps its tab, also after leaving this screen and coming back.
  createEffect(() => { for (const b of ws.buffers.unsaved()) if (b.checkoutId === props.checkout.id) ensure(b.path, false); });
  // A preview replaced by the next file leaves nothing behind unless the owner typed in it.
  createEffect(() => {
    const kept = new Set(tabs().map(t => t.path));
    for (const b of untrack(ws.buffers.list)) if (b.checkoutId === props.checkout.id && b.path !== props.file && !kept.has(b.path) && !hasUnsaved(b)) ws.buffers.close(b.key);
  });
  createEffect(on(spot, () => {
    const line = props.line;
    if (line === undefined) return;
    ws.setCodeMode("file");
    queueMicrotask(() => document.getElementById(`code-line-${line}`)?.scrollIntoView?.({ block: "center" }));
  }));
  const onKey = (e: KeyboardEvent) => {
    if (!shown() || document.querySelector("dialog[open]") || !(e.ctrlKey || e.metaKey) || e.altKey) return;
    if (e.key.toLowerCase() === "p" && !e.shiftKey) { e.preventDefault(); setGoTo(true); }
    else if (e.key.toLowerCase() === "s" && !e.shiftKey && buffer()) { e.preventDefault(); saveNow(); }
    else if (e.key.toLowerCase() === "f" && e.shiftKey) { e.preventDefault(); ws.setCodeLeft("search"); }
  };
  document.addEventListener("keydown", onKey);
  onCleanup(() => document.removeEventListener("keydown", onKey));
  const actions = (path: string): HunkActions => {
    const file = working(path);
    if (!file) return { reason: "Committed already. Stage and Revert act on uncommitted changes.", stage: () => undefined, revert: () => undefined, staged: () => false };
    return { stage: i => ws.run({ type: "git_stage", checkoutId: props.checkout.id, path, staged: !file.hunks[i]?.staged, hunk: i }), revert: i => ws.open({ kind: "revert", checkoutId: props.checkout.id, path, hunk: i }), staged: i => !!file.hunks[i]?.staged };
  };
  const diff = () => open() ? diffOf(open()!) : undefined;
  // The crumb bar has no room for words: the file's saved state beside Save is what says why Save is off.
  const saveState = createUniqueId();
  return <>
    <Show when={writer()}>
      <div class="strip warn"><I n="alert" s={14} class="st-warn" /><span>{writer() === UNCONFIRMED_WRITER ? ws.t("writerUnconfirmed") : edit() ? ws.t("writerChosen") : ws.t("writerWorking")}</span>
        <Show when={!edit() && writerTask()}>{t => <span class="acts"><button class="btn btn-line btn-sm" {...ws.needs("stop_task")} onClick={() => ws.stopTask(t().id)}>Stop the task first</button><button class="btn btn-sm" onClick={() => setEdit(true)}>Edit anyway</button></span>}</Show></div>
    </Show>
    <div class="code-body">
      <aside class="side-pane" aria-label="Files" ref={revealCutIn}>
        <Toolbar small class="side-h" start={<div class="seg" role="group" aria-label="Left pane"><button aria-current={left() === "files" ? "true" : "false"} onClick={() => ws.setCodeLeft("files")}>Files</button><button aria-current={left() === "search" ? "true" : "false"} onClick={() => ws.setCodeLeft("search")}>Search</button></div>}
          end={<button class="ib" aria-label="Collapse all" onClick={() => setCollapsed(new Set(allDirs(paths())))}><I n="chevrons-v" s={14} /></button>} />
        <Show when={left() === "files"} fallback={<div class="tree" aria-label="Search results">
          <div class="ws-search"><input class="inp ws-find-in" aria-label="Search files" placeholder="Search in files" value={query()} onInput={e => setQuery(e.currentTarget.value)} ref={el => queueMicrotask(() => el.focus())} />
            <div class="row ws-mt6"><span class="cap faint">{query() ? `${found().total} match${found().total === 1 ? "" : "es"} in ${found().files.length} file${found().files.length === 1 ? "" : "s"}` : "Type to search"}</span><span class="grow"></span>
              <button class="ib cap ws-aa" aria-label="Match case" aria-pressed={matchCase()} onClick={() => setMatchCase(!matchCase())}>Aa</button></div></div>
          <For each={found().files}>{f => <>
            <div class="res-file"><I n="file" s={12} class="faint" /><span class="trunc">{f.path}</span><span class="faint ws-res-n">{f.hits.length}</span></div>
            <For each={f.hits}>{h => { const part = () => splitMatch(h.text.trim(), query(), matchCase()); return <button class="res-hit" onClick={() => show(f.path, h.line)}><span class="n">{h.line}</span><span>{part().before}<mark>{part().hit}</mark>{part().after}</span></button>; }}</For>
          </>}</For>
        </div>}>
          <Toolbar small class="side-opts" start={<label class="sw-l"><span class="sw"><input type="checkbox" aria-label="Only changed files" checked={onlyChanged()} onChange={e => setOnlyChanged(e.currentTarget.checked)} /><i></i></span>Only changed</label>} end={<span class="cap faint">{data().diffs.length} changed</span>} />
          <div class="tree" role="tree" aria-label="Files">
            <For each={rows()}>{row => row.kind === "dir"
              ? <button class="tn" role="treeitem" style={{ "--d": row.depth }} aria-expanded={!collapsed().has(row.path)} onClick={() => toggleDir(row.path)}><I n={collapsed().has(row.path) ? "chevron-right" : "chevron-down"} s={14} /><span class="trunc">{row.name}</span></button>
              : <button class="tn" role="treeitem" style={{ "--d": row.depth }} aria-current={open() === row.path ? "true" : undefined} onClick={() => show(row.path)}><I n="file" s={14} /><span class="trunc">{row.name}</span><Show when={diffOf(row.path)}>{d => <GitLetter status={d().status} />}</Show></button>}</For>
          </div>
        </Show>
      </aside>
      <section class="ed" aria-label="Editor" ref={revealCutIn}>
        <div class="etabs">
          <div class="etab-list" role="tablist" aria-label="Open files"><For each={tabs()}>{t => { const unsaved = () => { const b = bufferOf(t.path); return !!b && hasUnsaved(b); }; return <span class="etab" role="tab" aria-label={unsaved() ? `${fileName(t.path)}, unsaved` : fileName(t.path)} aria-selected={open() === t.path ? "true" : "false"} tabindex="0" onClick={() => show(t.path)} onDblClick={() => ensure(t.path, false)} onKeyDown={e => { if (e.key === "Enter") show(t.path); }}>
            <span class="trunc" data-whole={t.path}><Show when={t.preview} fallback={fileName(t.path)}><em>{fileName(t.path)}</em></Show></span>
            <button class="x" classList={{ "ws-dirty": unsaved() }} aria-label={`Close ${fileName(t.path)}`} onClick={e => { e.stopPropagation(); close(t.path); }}><I n="x" s={12} /></button></span>; }}</For></div>
          <span class="etab-tools"><div class="seg" role="group" aria-label="View">
            <button aria-current={mode() === "file" ? "true" : "false"} onClick={() => ws.setCodeMode("file")}>File</button>
            <button aria-current={mode() === "inline" ? "true" : "false"} onClick={() => ws.setCodeMode("inline")}>Inline diff</button>
            <button aria-current={mode() === "split" ? "true" : "false"} onClick={() => ws.setCodeMode("split")}>Side by side</button>
          </div></span>
        </div>
        <Show when={open()} fallback={<div class="note-panel"><span class="w500">No file open</span></div>}>{path => <>
          <Toolbar small class="ecrumb" label="File" mid={<span class="ws-crumb"><Show when={dirName(path())}><I n="folder" s={12} class="faint" /><span class="trunc ws-crumb-d" data-whole={dirName(path())}>{dirName(path()).split("/").join(" / ")}</span><I n="chevron-right" s={12} class="faint" /></Show><span class="ws-fg trunc" data-whole={path()}>{fileName(path())}</span>
              <Show when={diff()}>{d => <span class="tnum trunc ws-against l-compact-hide"><Counts add={d().add} del={d().del} /> against <span class="mono">{props.checkout.base}</span></span>}</Show></span>}
            end={<>
              <Show when={buffer()?.loaded && !buffer()?.missing && buffer()}>{b => <><span id={saveState} class="ws-save-state" role="status">{saveWord(b()) || "No changes"}</span>
                <button class="btn btn-sm" aria-label={`Save ${fileName(path())}`} aria-disabled={saveBlocked(b()) ? "true" : undefined} data-why={saveBlocked(b())} aria-describedby={saveBlocked(b()) ? saveState : undefined} onClick={saveNow}>Save</button></>}</Show>
              <button class="btn btn-sm" onClick={() => setGoTo(true)}>Go to file</button>
              <button class="btn btn-sm ws-ext" aria-label={ws.externalLabel()} aria-disabled={ws.noOpener("workspace.open_editor", "noExternal") ? "true" : undefined} data-why={ws.noOpener("workspace.open_editor", "noExternal")} onClick={() => { if (!ws.noOpener("workspace.open_editor", "noExternal")) ws.run({ type: "open_editor", projectId: props.repo.projectId, checkoutId: props.checkout.id, file: path(), line: props.line }); }}><I n="external" s={14} /><span class="ws-ext-t">{ws.externalLabel()}</span></button>
            </>} />
          <Show when={buffer()}>{b => <>
            <Show when={b().conflict}>{c => <div class="strip warn" role="alert"><I n="alert" s={14} class="st-warn" /><span>{c().text === null ? "This file is no longer on disk." : "This file changed on disk after you opened it."} Your text is kept here.</span>
              <span class="acts"><Show when={c().text !== null} fallback={<button class="btn btn-sm" onClick={() => setAsking(b().path)}>Close file</button>}><button class="btn btn-sm" onClick={() => ws.buffers.reload(b().key)}>Reload from disk</button></Show><button class="btn btn-line btn-sm" onClick={() => void ws.buffers.keepMine(b().key)}>Keep mine</button></span></div>}</Show>
            <Show when={b().unconfirmed}><div class="strip warn" role="alert"><I n="alert" s={14} class="st-warn" /><span>The machine did not confirm this save. Your text is kept here, and Save asks again.</span></div></Show>
            <Show when={b().refusal}>{message => <div class="strip bad" role="alert"><I n="alert" s={14} /><span>{message()}</span></div>}</Show>
            <Show when={b().readOnly}>{message => <div class="strip info"><I n="lock" s={14} class="faint" /><span>{message()}</span></div>}</Show>
          </>}</Show>
          <Show when={working(path())?.filter}>{driver => <div class="strip info"><span>{driver() === "lfs" ? "Git LFS" : `Git filter: ${driver()}`} · Content comparison is unavailable; no filter is run.</span></div>}</Show>
          <Show when={mode() === "file"} fallback={<div class="ebody ws-flush"><Show when={diff()} fallback={<div class="note-panel"><span class="w500">No changes in this file</span><span>It matches {props.checkout.base}.</span></div>}>{d => <Diff file={d()} layout={mode() === "split" ? "split" : "unified"} actions={actions(path())} />}</Show></div>}>
            <Show when={buffer()?.loaded && !buffer()?.missing && buffer()} fallback={<Show when={!editable() || buffer()?.missing} fallback={<div class="ebody"><div class="note-panel"><span>Opening…</span></div></div>}>
              <div class="ebody"><Show when={source()} fallback={<div class="note-panel"><span class="w500">{buffer()?.missing ?? ws.t("noFileCopy")}</span></div>}>{s => <>
                <Show when={s().partial}><div class="cap faint ws-pad">{ws.t("partialFile")}</div></Show>
                <Show when={buffer()?.missing}>{reason => <div class="cap faint ws-pad">{reason()}</div>}</Show>
                <div class="code"><For each={s().lines}>{(line, i) => <div id={`code-line-${i() + 1}`} class={`cl${added().has(i() + 1) ? " chg" : ""}${props.line === i() + 1 ? " hit" : ""}`}><span class="n">{i() + 1}</span><span class="m"></span><span class="c"><Code path={path()} text={line} /></span></div>}</For></div>
              </>}</Show></div>
            </Show>}>{b => <div class="ebody ws-flush ws-editor"><Suspense fallback={<div class="note-panel"><span>Opening…</span></div>}><CodeEditor buffer={b()} locked={locked(b())} line={props.line} changed={added()} onSave={saveNow} /></Suspense></div>}</Show>
          </Show>
        </>}</Show>
      </section>
    </div>
    <GoToFile open={goTo()} paths={data().tree} onClose={() => setGoTo(false)} onPick={show} />
    <CloseFile buffer={asking() ? bufferOf(asking()!) : null} onCancel={() => setAsking(null)} onDiscard={() => { const path = asking(); setAsking(null); if (path) drop(path); }} onSave={() => { const path = asking(); if (path) void saveAndDrop(path); }} />
  </>;
}

/** The Inspector's facts about the open file and its checkout. */
export function CodePanel(props: { repo: ProjectRepo; checkout: RepoCheckout; file?: string }) {
  const ws = useWs();
  const data = createMemo(() => checkoutFiles(ws.store.state(), props.repo, props.checkout));
  const diff = () => data().diffs.find(d => d.path === props.file);
  const lines = () => props.file ? sourceOf(props.repo, props.checkout, data().diffs, props.file)?.lines.length : undefined;
  const task = () => props.checkout.taskId ? ws.store.index().byId.get(props.checkout.taskId) ?? null : null;
  return <>
    <Show when={props.file}>{path => <div class="ins-sec"><div class="ins-h"><span>File</span></div>
      <dl class="kv"><dt>Path</dt><dd class="mono">{path()}</dd><dt>Language</dt><dd>{languageOf(path())}</dd><Show when={lines() !== undefined}><dt>Lines</dt><dd>{lines()}</dd></Show><Show when={diff()?.changedBy}><dt>Changed by</dt><dd>{diff()!.changedBy}</dd></Show></dl>
      <Show when={diff()}><button class="btn btn-sm ws-mt8" onClick={() => ws.setCodeMode("inline")}>Show diff</button></Show></div>}</Show>
    <div class="ins-sec"><div class="ins-h"><span>Checkout</span></div>
      <dl class="kv"><Show when={task()}>{v => <><dt>Task</dt><dd><A class="lnk" to={{ name: "task", machine: v().machine.id, task: v().id }}>{v().task.title}</A></dd></>}</Show><dt>Branch</dt><dd class="mono">{props.checkout.branch}</dd><dt>Autosave</dt><dd><A class="lnk" to={{ name: "settings", section: "appearance", machine: null }}>{ws.panels.autosave() ? "On" : "Off"}</A></dd></dl></div>
  </>;
}
