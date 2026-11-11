import { createMemo, createSignal, For, Show } from "solid-js";
import { previewPurge } from "@cortex/workspace-domain";
import { archiveRow, archiveView, daysLeft, nextCleanup, NO_FILTER, OUTCOME_FILTERS, sizeText, type ArchiveFilter, type ArchiveRow } from "./archive-model";
import { useWs } from "./ctx";
import { A, I, MenuItem, Modal, Pop } from "./kit";
import { ActionBar, SectionHead, Toolbar } from "./layout";
import { sprintTitle, type TaskView } from "./selectors";

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
    <button class="ib ws-ib" aria-label={`More for ${v().task.title}`} onClick={e => props.onMenu(e.currentTarget)}><I n="more" s={14} /></button>
  </div>;
};

const Head = () => <div class="gr th ar-cols"><span></span><span>Task</span><span>Project</span><span class="ar-opt">Machine</span><span>Outcome</span><span>Cleanup in</span><span>Size</span><span></span><span></span></div>;
const LATER_LIMIT = 8;

/** Unmerged work never reaches the Archive without the owner's word: Keep is the default, and delete names what goes. */
export function UnmergedDialog(props: { taskId: string; onClose: () => void }) {
  const ws = useWs();
  const [choice, setChoice] = createSignal<"keep" | "drop">("keep");
  const view = createMemo(() => ws.store.index().byId.get(props.taskId)!);
  const t = () => view().task, f = () => t().workFacts ?? null;
  const days = () => view().machine.autoArchiveDays;
  const what = () => f() ? `${f()!.commits} commits and ${f()!.files} files` : "the work";
  const answer = (discard: boolean) => { ws.ok({ type: "answer_archive", taskId: props.taskId, discard, now: ws.store.clock() }, () => { props.onClose(); }); };
  return <>
    <div class="dlg-h"><h2>{t().title} has unmerged work</h2><button class="ib" aria-label="Keep it" onClick={() => props.onClose()}><I n="x" /></button></div>
    <div class="dlg-b">
      <div>This task is due for the Archive. Its work is not merged, and archiving would delete that work when it is cleaned up. What should happen?</div>
      <Show when={f()}>{x => <div class="pre">
        <div class="chk"><I n="commit" s={14} class="faint" /><span>{x().commits} commits on <span class="mono">{t().checkout.branch}</span></span><span class="src">Not in any other branch</span></div>
        <div class="chk"><I n="file" s={14} class="faint" /><span>{x().files} uncommitted files in the worktree</span></div>
        <div class="chk"><I n="drive" s={14} class="faint" /><span>Worktree size</span><span class="src">{sizeText(x().sizeMb)}</span></div>
      </div>}</Show>
      <div class="ws-fields">
        <label class="rc"><input type="radio" name="unmerged" checked={choice() === "keep"} onChange={() => setChoice("keep")} /><span><span class="t">Keep the work, do not archive</span><span class="d">The task stays in Done with its worktree. Ask again in {days()} days.</span></span></label>
        <label class="rc danger"><input type="radio" name="unmerged" checked={choice() === "drop"} onChange={() => setChoice("drop")} /><span><span class="t">Delete the work and archive</span><span class="d">Discards {what()} at cleanup. This cannot be undone after that.</span></span></label>
      </div>
    </div>
    <ActionBar place="dialog" start={<A class="lnk" to={{ name: "task", machine: view().machine.id, task: props.taskId }} onClick={() => props.onClose()}>Open the task</A>}>
      <Show when={choice() === "keep"} fallback={<button class="btn btn-danger btn-lg" {...ws.needs("answer_archive")} onClick={() => answer(true)}>Delete {what()}</button>}><button class="btn btn-primary btn-lg" data-autofocus {...ws.needs("answer_archive")} onClick={() => answer(false)}>Keep the work</button></Show>
    </ActionBar>
  </>;
}

/** A preview first: counts and size, and what is skipped because cleanup is blocked. Merged commits are not touched. */
export function PurgeDialog(props: { ids: string[]; onClose: () => void; onDone?: () => void }) {
  const ws = useWs();
  const preview = createMemo(() => previewPurge(ws.store.state(), props.ids));
  const n = () => preview().deletable.length;
  const noun = () => `${n()} task${n() === 1 ? "" : "s"}`;
  const size = () => preview().sizeMb > 0 ? `, ${preview().sizeKnown ? "" : "at least "}${sizeText(preview().sizeMb)} freed` : "";
  const go = () => { if (n()) ws.ok({ type: "purge_many", taskIds: props.ids, confirmed: true, now: ws.store.clock() }, () => { props.onDone?.(); props.onClose(); }); };
  return <>
    <div class="dlg-h"><h2>Delete from the Archive now?</h2><button class="ib" aria-label="Cancel" onClick={() => props.onClose()}><I n="x" /></button></div>
    <div class="dlg-b"><ul class="conseq">
      <Show when={n() > 0}><li><I n="trash" s={14} /><span>{noun()} and their saved history{size()}</span></li></Show>
      <For each={preview().skipped}>{s => <li><I n="alert" s={14} /><span>Skipped: {s.title}. {s.reason}</span></li>}</For>
      <li><I n="branch" s={14} /><span>Merged commits, remote branches and pull requests stay in Git</span></li>
    </ul></div>
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
    <div class="dlg-h"><h2>Cleanup of {t().title} is blocked</h2><button class="ib" aria-label="Decide later" onClick={() => props.onClose()}><I n="x" /></button></div>
    <div class="dlg-b"><div>{props.row.sub}.</div><div class="cap muted">It stays in the Archive until you decide. Nothing is deleted on its own.</div></div>
    <ActionBar place="dialog"><button class="btn btn-line btn-lg" data-autofocus onClick={() => props.onClose()}>Decide later</button>
      <button class="btn btn-line btn-lg" {...ws.needs("restore_task")} onClick={() => { ws.ok({ type: "restore_task", taskId: t().id, now: ws.store.clock() }, () => { props.onClose(); }); }}>Restore to Done</button>
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
  const [asking, setAsking] = createSignal<string | null>(null);
  const [more, setMore] = createSignal(false);
  let menuEl: HTMLDivElement | undefined;
  const view = createMemo(() => archiveView(ws.store.index(), filter()));
  const closedSprints = createMemo(() => ws.store.state().sprints.filter(s => {
    const project = ws.store.state().projects.find(p => p.id === s.projectId), f = filter();
    return s.archivedAt && project && (!f.machine || project.machineId === f.machine) && (!f.project || s.projectId === f.project)
      && !f.outcome && (!f.text.trim() || sprintTitle(s).toLowerCase().includes(f.text.trim().toLowerCase()));
  }));
  const later = () => more() ? view().later : view().later.slice(0, LATER_LIMIT);
  const filtered = () => filter().text.trim() !== "" || filter().machine !== null || filter().project !== null || filter().outcome !== null;
  const check = (id: string, on: boolean) => { const next = new Set(selected()); if (on) next.add(id); else next.delete(id); setSelected(next); };
  const ids = () => [...selected()].filter(id => view().rows.some(r => r.view.id === id));
  const restore = (id: string) => ws.run({ type: "restore_task", taskId: id, now: ws.store.clock() });
  const restoreAll = () => { for (const id of ids()) restore(id); setSelected(new Set<string>()); };
  const machines = () => ws.store.state().machines.map(m => ({ id: m.id, label: m.name }));
  const projects = () => [...new Map(view().rows.map(r => [r.view.project.id, r.view.project.name])).entries()].map(([id, label]) => ({ id, label }));
  const menuRow = () => { const id = menu(), v = id ? ws.store.index().byId.get(id) : null; return v ? archiveRow(ws.store.index(), v) : null; };
  const openMenu = (id: string, el: HTMLElement) => {
    setMenu(id);
    if (!menuEl) return;
    const box = el.getBoundingClientRect();
    menuEl.style.inset = "auto"; menuEl.style.right = `${innerWidth - box.right}px`;
    if (box.bottom > innerHeight * .7) menuEl.style.bottom = `${innerHeight - box.top + 4}px`; else menuEl.style.top = `${box.bottom + 4}px`;
    menuEl.showPopover?.();
  };
  const section = (id: string, title: string, rows: ArchiveRow[], tail?: () => unknown) => <Show when={rows.length > 0}>
    <section aria-labelledby={id}>
      <SectionHead id={id} title={title} count={rows.length} />
      <div class="surface"><div class="gt">
        <Head />
        <For each={title === "Later" ? later() : rows}>{r => <Row row={r} checked={selected().has(r.view.id)} onCheck={on => check(r.view.id, on)} onMenu={el => openMenu(r.view.id, el)} onRestore={() => { if (r.view.machine.connection === "connected") restore(r.view.id); }} />}</For>
        {tail?.() as never}
      </div></div>
    </section>
  </Show>;
  const asked = (v: TaskView) => { const f = v.task.workFacts; return `${v.project.name}, unmerged work${f ? `: ${f.commits} commits and ${f.files} uncommitted files on ` : " on "}`; };
  return <div class="page"><div class="page-in">
    <div class="page-h"><div><p>Kept for {ws.machine().retentionDays} days after archiving by default. <A class="lnk" to={{ name: "settings", section: "storage", machine: ws.machine().id }}>Change it for each machine.</A> Merged commits stay in Git.</p></div></div>
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
    <Show when={view().waiting.length > 0}>
      <section aria-labelledby="ar-wait">
        <SectionHead id="ar-wait" title={<><I n="question" s={14} class="st-warn" /> Waiting for your decision</>} count={view().waiting.length} />
        <div class="surface ws-warn"><For each={view().waiting}>{v => <div class="srow">
          <span><span class="t">{v.task.title}</span><span class="d">{asked(v)}<span class="mono">{v.task.checkout.branch}</span>. It stays out of the Archive until you decide.</span></span>
          <span class="ctl"><button class="btn btn-primary btn-sm" onClick={() => setAsking(v.id)}>Decide…</button></span>
        </div>}</For></div>
      </section>
    </Show>
    <Show when={closedSprints().length}><section aria-label="Closed sprints">
      <SectionHead title="Closed sprints" count={closedSprints().length} />
      <div class="surface"><For each={closedSprints()}>{s => <div class="srow">
        <span><span class="t">{sprintTitle(s)}</span><span class="d">{s.taskIds.length} tasks retained. Unfinished tasks remain in the project.</span></span>
        <A class="btn btn-line btn-sm" to={{name:"sprint",machine:ws.store.state().projects.find(p => p.id === s.projectId)!.machineId,project:s.projectId,sprint:s.id,tab:"summary"}}>Open summary</A>
      </div>}</For></div>
    </section></Show>
    {section("ar-soon", "Cleanup within 3 days", view().soon)}
    {section("ar-later", "Later", view().later, () => <Show when={view().later.length > LATER_LIMIT && !more()}><button class="gr ws-more" onClick={() => setMore(true)}><span class="cap muted">Show {view().later.length - LATER_LIMIT} more</span></button></Show>)}
    {section("ar-block", "Cleanup blocked", view().blocked)}
    <Show when={view().rows.length === 0 && view().waiting.length === 0 && closedSprints().length === 0}>
      <div class="surface ws-empty">{filtered() ? "No archived task matches this filter." : `Nothing archived. Tasks move here after ${ws.machine().autoArchiveDays} days in Done.`}</div>
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
    <Modal open={asking() !== null} onClose={() => setAsking(null)} label="Unmerged work"><Show when={asking()} keyed>{id => <UnmergedDialog taskId={id} onClose={() => setAsking(null)} />}</Show></Modal>
  </div></div>;
}

/** The title band of the Archive. Numbers come from the same rows the page lists. */
export function ArchiveFacts() {
  const ws = useWs();
  const view = createMemo(() => archiveView(ws.store.index(), NO_FILTER));
  return <div class="row sec muted ws-facts">
    <span class="trunc">{view().total} task{view().total === 1 ? "" : "s"}</span>
    <Show when={view().soonTotal > 0}><span class="trunc st-warn">{view().soonTotal} soon</span></Show>
    <Show when={view().waiting.length > 0}><span class="trunc">{view().waiting.length} wait{view().waiting.length === 1 ? "s" : ""} for you</span></Show>
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
