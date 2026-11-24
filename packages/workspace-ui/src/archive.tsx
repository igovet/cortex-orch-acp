import { createMemo, createSignal, For, Show } from "solid-js";
import { previewPurge } from "@cortex/workspace-domain";
import { archiveRow, archiveView, daysLeft, nextCleanup, NO_FILTER, OUTCOME_FILTERS, sizeText, type ArchiveFilter, type ArchiveRow } from "./archive-model";
import { useWs } from "./ctx";
import { createRequestFeedback, RequestStatus } from "./request-feedback";
import { A, I, Keyed, Loading, MenuItem, Modal, OverviewShown, Pop, useOverviewWait, useWantOverview } from "./kit";
import { ActionBar, SectionHead, Toolbar } from "./layout";
import { sprintTitle } from "./selectors";
import { same } from "./stable";

const Row = (props: { row: ArchiveRow; checked: boolean; onCheck: (on: boolean) => void; onMenu: (el: HTMLElement) => void; onRestore: () => void }) => {
  const ws = useWs();
  const r = () => props.row, v = () => props.row.view;
  return <div class="gr ar-cols">
    <input type="checkbox" class="row-ck ws-ck" aria-label={`Select ${v().task.title}`} checked={props.checked} onChange={e => props.onCheck(e.currentTarget.checked)} />
    <span class="trunc"><span class="w500">{v().task.title}</span><span class="sub trunc">{r().sub}</span></span>
    <span class="cap muted trunc">{v().project.name}</span>
    <span class="cap muted trunc ar-opt">{v().machine.name}</span>
    <span class={`st ${r().outcomeTone}`}>{r().outcomeText}</span>
    <span class="cd">
      <Show when={r().status} fallback={<><span class={`cap tnum${r().group === "soon" ? " st-warn w500" : ""}`}>{daysLeft(r().left)}</span><span class="bar"><i class={r().group === "soon" ? "soon" : ""} style={{ width: `${r().percent}%` }}></i></span></>}>
        {s => <span class="st st-warn"><I n={s().icon} s={12} />{s().text}</span>}
      </Show>
    </span>
    <span class="cap muted tnum">{r().size === null ? "" : sizeText(r().size!)}</span>
    <span class="row ws-gap2"><button class="btn btn-line btn-sm" data-why={v().machine.connection === "connected" ? undefined : `${v().machine.name} is offline`} aria-disabled={v().machine.connection === "connected" ? undefined : "true"} {...ws.needs("restore_task")} onClick={() => props.onRestore()}><I n="undo" s={12} />Restore</button></span>
    <button data-tooltip="" class="ib ws-ib" aria-label={`More for ${v().task.title}`} onClick={e => props.onMenu(e.currentTarget)}><I n="more" s={14} /></button>
  </div>;
};

const Head = () => <div class="gr th ar-cols"><span></span><span>Task</span><span>Project</span><span class="ar-opt">Machine</span><span>Outcome</span><span>Cleanup in</span><span>Size</span><span></span><span></span></div>;
const LATER_LIMIT = 8;

/** A preview first: counts and size, and what is skipped because cleanup is blocked. Merged commits are not touched. */
export function PurgeDialog(props: { ids: string[]; onClose: () => void; onDone?: () => void }) {
  const ws = useWs();
  const request = createRequestFeedback();
  const preview = createMemo(() => previewPurge(ws.store.state(), props.ids));
  const n = () => preview().deletable.length;
  const noun = () => `${n()} task${n() === 1 ? "" : "s"}`;
  const size = () => preview().sizeMb > 0 ? `, ${preview().sizeKnown ? "" : "at least "}${sizeText(preview().sizeMb)} freed` : "";
  const go = () => { if (n()) void request.run({ type: "purge_many", taskIds: props.ids, confirmed: true, now: ws.store.clock() }).then(receipt => { if (receipt.ok) { props.onDone?.(); props.onClose(); } }); };
  return <>
    <div class="dlg-h"><h2>Delete from the Archive now?</h2><button data-tooltip="" class="ib" aria-label="Cancel" onClick={() => props.onClose()}><I n="x" /></button></div>
    <div class="dlg-b"><ul class="conseq">
      <Show when={n() > 0}><li><I n="trash" s={14} /><span>{noun()} and their saved history{size()}</span></li></Show>
      <For each={preview().skipped}>{s => <li><I n="alert" s={14} /><span>Skipped: {s.title}. {s.reason}</span></li>}</For>
      <Show when={ws.store.state().tasks.some(t => preview().deletable.includes(t.id) && t.checkout.kind === "worktree")}>
        <li><I n="folder" s={14} /><span>Deleting a task also deletes its clean worktree folder if it was created for the task. Unsafe folders are kept; the main project folder is never deleted.</span></li>
      </Show>
      <li><I n="branch" s={14} /><span>Merged commits, remote branches and pull requests stay in Git</span></li>
    </ul><RequestStatus text={request.text()} /></div>
    <ActionBar place="dialog"><button class="btn btn-line btn-lg" data-autofocus onClick={() => props.onClose()}>Keep them</button>
      <button class="btn btn-danger btn-lg" aria-disabled={n() ? undefined : "true"} data-why={n() ? undefined : "Nothing here can be deleted"} {...ws.needs("purge_many")} onClick={go}>Delete {noun()} permanently</button></ActionBar>
  </>;
}

/** New files after archiving: cleanup stopped so the owner can say what happens. A process in doubt cannot be waved through. */
function BlockedDialog(props: { row: ArchiveRow; onClose: () => void }) {
  const ws = useWs();
  const t = () => props.row.view.task;
  const clearable = () => t().unexpectedChanges && t().process === "stopped" && props.row.view.machine.connection === "connected";
  return <>
    <div class="dlg-h"><h2>Cleanup of {t().title} is blocked</h2><button data-tooltip="" class="ib" aria-label="Decide later" onClick={() => props.onClose()}><I n="x" /></button></div>
    <div class="dlg-b"><div>{props.row.sub}.</div><div class="cap muted">It stays in the Archive until you decide. Nothing is deleted on its own.</div></div>
    <ActionBar place="dialog"><button class="btn btn-line btn-lg" data-autofocus onClick={() => props.onClose()}>Decide later</button>
      <button class="btn btn-line btn-lg" {...ws.needs("restore_task")} onClick={() => { ws.ok({ type: "restore_task", taskId: t().id, now: ws.store.clock() }, () => { props.onClose(); }); }}>Restore {t().archive?.previousStatus === "draft" ? "draft" : "to Done"}</button>
      <Show when={t().work === "unmerged" && props.row.view.machine.connection === "connected"}><button class="btn btn-danger btn-lg" {...ws.needs("confirm_discard")} onClick={() => { ws.ok({ type: "confirm_discard", taskId: t().id, now: ws.store.clock() }, () => { props.onClose(); }); }}>Allow deleting the work at cleanup</button></Show>
      <Show when={clearable()}><button class="btn btn-danger btn-lg" {...ws.needs("clear_purge_block")} onClick={() => { ws.ok({ type: "clear_purge_block", taskId: t().id, confirmed: true }, () => { props.onClose(); }); }}>Discard the new files</button></Show></ActionBar>
  </>;
}

function Picker(props: { id: string; label: string; value: string; options: { id: string; label: string }[]; onPick: (id: string | null) => void }) {
  return <>
    <button class="pick ws-pick" popovertarget={props.id} aria-label={`${props.label}: ${props.value}`}><span class="faint">{props.label}</span><span class="trunc">{props.value}</span><I n="chevron-down" s={12} /></button>
    <Pop id={props.id} class="pop-anchor" align="left" label={props.label}>
      <MenuItem label="All" check={props.value === "All"} onSelect={() => props.onPick(null)} />
      <For each={props.options}>{o => <MenuItem label={o.label} check={o.label === props.value} onSelect={() => props.onPick(o.id)} />}</For>
    </Pop>
  </>;
}

/** Shared across machines, like the Inbox. */
export function Archive() {
  const ws = useWs();
  const [filter, setFilter] = createSignal<ArchiveFilter>(NO_FILTER);
  const [selected, setSelected] = createSignal<Set<string>>(new Set());
  const [menu, setMenu] = createSignal<string | null>(null);
  const [purging, setPurging] = createSignal<string[] | null>(null);
  const [blocked, setBlocked] = createSignal<ArchiveRow | null>(null);
  const [more, setMore] = createSignal(false);
  let menuEl: HTMLDivElement | undefined;
  // Days left, and so which section a task is in, come from the machine's retention: read when the Archive opens.
  useWantOverview();
  const wait = useOverviewWait();
  const view = createMemo(() => archiveView(ws.store.index(), filter()));
  const closedSprints = createMemo(() => ws.store.state().sprints.filter(s => {
    const project = ws.store.state().projects.find(p => p.id === s.projectId), f = filter();
    return s.archivedAt && !s.deletion && project && (!f.machine || project.machineId === f.machine) && (!f.project || s.projectId === f.project)
      && !f.outcome && (!f.text.trim() || sprintTitle(s).toLowerCase().includes(f.text.trim().toLowerCase()));
  }));
  const later = () => more() ? view().later : view().later.slice(0, LATER_LIMIT);
  const filtered = () => filter().text.trim() !== "" || filter().machine !== null || filter().project !== null || filter().outcome !== null;
  const check = (id: string, on: boolean) => { const next = new Set(selected()); if (on) next.add(id); else next.delete(id); setSelected(next); };
  const ids = () => [...selected()].filter(id => view().rows.some(r => r.view.id === id));
  const restore = (id: string) => ws.run({ type: "restore_task", taskId: id, now: ws.store.clock() });
  const restoreAll = () => { for (const id of ids()) restore(id); setSelected(new Set<string>()); };
  // Equal choices stay the same objects, so an open picker keeps its items (and the one under the pointer or focus) on a background read.
  const machines = createMemo(() => ws.store.state().machines.map(m => ({ id: m.id, label: m.name })), undefined, { equals: same });
  const projects = createMemo(() => [...new Map(view().rows.map(r => [r.view.project.id, r.view.project.name])).entries()].map(([id, label]) => ({ id, label })), undefined, { equals: same });
  const menuRow = () => { const id = menu(), v = id ? ws.store.index().byId.get(id) : null; return v ? archiveRow(ws.store.index(), v) : null; };
  const openMenu = (id: string, el: HTMLElement) => {
    setMenu(id);
    if (!menuEl) return;
    const box = el.getBoundingClientRect();
    menuEl.style.inset = "auto"; menuEl.style.right = `${innerWidth - box.right}px`;
    if (box.bottom > innerHeight * .7) menuEl.style.bottom = `${innerHeight - box.top + 4}px`; else menuEl.style.top = `${box.bottom + 4}px`;
    menuEl.showPopover?.();
  };
  // Rows are read through an accessor and keyed by task, so a background read updates them in place: the section, its rows and
  // an opened menu or checked box stay as they were.
  const section = (id: string, title: string, rows: () => ArchiveRow[], tail?: () => unknown) => <Show when={rows().length > 0}>
    <section aria-labelledby={id}>
      <SectionHead id={id} title={title} count={rows().length} />
      <div class="surface"><div class="gt">
        <Head />
        <Keyed each={title === "Later" ? later() : rows()} by={r => r.view.id}>{r => <Row row={r()} checked={selected().has(r().view.id)} onCheck={on => check(r().view.id, on)} onMenu={el => openMenu(r().view.id, el)} onRestore={() => { if (r().view.machine.connection === "connected") restore(r().view.id); }} />}</Keyed>
        {tail?.() as never}
      </div></div>
    </section>
  </Show>;
  return <div class="page"><div class="page-in">
    <div class="page-h"><div><p><OverviewShown inline><>Kept for {ws.machine().retentionDays} days after archiving by default. Sprint drafts aren’t cleaned up automatically.  </></OverviewShown><A class="lnk" to={{ name: "settings", section: "storage", machine: ws.machine().id }}>Change it for each machine.</A> Merged commits stay in Git.</p></div></div>
    <Show when={ids().length > 0} fallback={<Toolbar class="tbar" label="Filter the Archive" start={<>
      <input class="inp ws-filter-wide" placeholder="Filter archived tasks" aria-label="Filter archived tasks" value={filter().text} onInput={e => setFilter({ ...filter(), text: e.currentTarget.value })} />
      <Picker id="m-ar-machine" label="Machine" value={machines().find(m => m.id === filter().machine)?.label ?? "All"} options={machines()} onPick={machine => setFilter({ ...filter(), machine })} />
      <Picker id="m-ar-project" label="Project" value={projects().find(p => p.id === filter().project)?.label ?? "All"} options={projects()} onPick={project => setFilter({ ...filter(), project })} />
      <Picker id="m-ar-outcome" label="Outcome" value={OUTCOME_FILTERS.find(o => o.id === filter().outcome)?.label ?? "All"} options={OUTCOME_FILTERS} onPick={outcome => setFilter({ ...filter(), outcome: outcome as ArchiveFilter["outcome"] })} />
    </>} mid={<span class="cap faint trunc" data-whole="">{nextCleanup(ws.machine(), ws.store.now)}</span>} />}>
      <Toolbar class="sel-bar" label="Selected tasks" start={<span class="w500">{ids().length} selected</span>} end={<>
        <button class="btn btn-sm" onClick={() => setSelected(new Set<string>())}>Clear</button>
        <button class="btn btn-line btn-sm" {...ws.needs("restore_task")} onClick={restoreAll}><I n="undo" s={12} />Restore {ids().length}</button>
        <button class="btn btn-line btn-sm" onClick={() => setPurging(ids())}>Delete now…</button></>} />
    </Show>
    <Show when={closedSprints().length}><section aria-label="Closed sprints">
      <SectionHead title="Closed sprints" count={closedSprints().length} />
      <div class="surface"><For each={closedSprints()}>{s => <div class="srow">
        <span><span class="t">{sprintTitle(s)}</span><span class="d">{s.taskIds.length} tasks retained. Restoring a task returns it to the project without a sprint.</span></span>
        <A class="btn btn-line btn-sm" to={{name:"sprint",machine:ws.store.state().projects.find(p => p.id === s.projectId)!.machineId,project:s.projectId,sprint:s.id,tab:"summary"}}>Open summary</A>
      </div>}</For></div>
    </section></Show>
    <Show when={!wait() || view().rows.length === 0} fallback={<Loading text={wait()!} />}>
    {section("ar-soon", "Cleanup within 3 days", () => view().soon)}
    {section("ar-later", "Later", () => view().later, () => <Show when={view().later.length > LATER_LIMIT && !more()}><button class="gr ws-more" onClick={() => setMore(true)}><span class="cap muted">Show {view().later.length - LATER_LIMIT} more</span></button></Show>)}
    {section("ar-block", "Cleanup blocked", () => view().blocked)}
    </Show>
    <Show when={view().rows.length === 0 && closedSprints().length === 0}>
      <div class="surface ws-empty">{filtered() ? "No archived task matches this filter." : wait() ? "Nothing archived." : `Nothing archived. Tasks move here after ${ws.machine().autoArchiveDays} days in Done.`}</div>
    </Show>
    <Pop id="m-arrow" class="pop-right" label="Archived task" ref={el => { menuEl = el; }}>
      <Show when={menuRow()}>{r => <>
        <MenuItem icon="undo" label="Restore" disabled={r().view.machine.connection === "connected" ? undefined : `${r().view.machine.name} is offline`} needs="restore_task" onSelect={() => restore(r().view.id)} />
        <MenuItem icon="eye" label="Open read-only" onSelect={() => ws.go({ name: "task", machine: r().view.machine.id, task: r().view.id })} />
        <Show when={r().blocker}><MenuItem icon="question" label="Decide…" onSelect={() => setBlocked(r())} /></Show>
        <div class="mi-sep"></div>
        <MenuItem icon="trash" label="Delete now…" danger disabled={r().blocker ?? undefined} onSelect={() => setPurging([r().view.id])} />
      </>}</Show>
    </Pop>
    <Modal open={purging() !== null} onClose={() => setPurging(null)} label="Delete from the Archive"><Show when={purging()} keyed>{list => <PurgeDialog ids={list} onClose={() => setPurging(null)} onDone={() => setSelected(new Set<string>())} />}</Show></Modal>
    <Modal open={blocked() !== null} onClose={() => setBlocked(null)} label="Cleanup blocked"><Show when={blocked()} keyed>{r => <BlockedDialog row={r} onClose={() => setBlocked(null)} />}</Show></Modal>
  </div></div>;
}

/** The title band of the Archive. Numbers come from the same rows the page lists. */
export function ArchiveFacts() {
  const ws = useWs();
  const view = createMemo(() => archiveView(ws.store.index(), NO_FILTER));
  useWantOverview();
  return <div class="row sec muted ws-facts">
    <span class="trunc">{view().total} task{view().total === 1 ? "" : "s"}</span>
    <Show when={ws.store.overviewLoaded() && view().soonTotal > 0}><span class="trunc st-warn">{view().soonTotal} soon</span></Show>
  </div>;
}
export function DeleteArchive() {
  const ws = useWs();
  const [open, setOpen] = createSignal(false);
  const all = () => ws.store.index().archive.map(v => v.id);
  return <>
    <Show when={all().length}><button class="btn btn-line" onClick={() => setOpen(true)}>Delete archive…</button></Show>
    <Modal open={open()} onClose={() => setOpen(false)} label="Delete from the Archive"><Show when={open()}><PurgeDialog ids={all()} onClose={() => setOpen(false)} /></Show></Modal>
  </>;
}
