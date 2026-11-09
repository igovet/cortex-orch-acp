import { createMemo, createSignal, For, onMount, Show } from "solid-js";
import { useDesk } from "./ctx";
import { COLUMNS, cardLabel, columnName, hhmm, sortViews, type ColumnId, type SortKey, type TaskView } from "./model";
import { Badge, ExecBadge, WaitBadge } from "./ui";
import { VirtualList, type VirtualApi } from "./virtual";

export type Pos = { col: number; idx: number };
const CARD_U = 10;
const SKELETONS = [0, 1, 2, 3, 4, 5];

const Meta = (props: { v: TaskView }) => <>
  <span class="a-mono">{props.v.id}<Show when={props.v.created}> · <span class="a-mock-tag">Mock</span></Show></span>
  <span>{props.v.updatedAt ? `updated ${hhmm(props.v.updatedAt)}` : "no events"} · fixture</span>
</>;

/** Board home: seven read-only phase columns, one Tab stop, roving focus between cards. */
export function Board(props: {
  views: TaskView[]; loading: boolean; pos: Pos; setPos: (p: Pos) => void; restoreFocus: boolean; onOpen: (id: string) => void;
}) {
  const desk = useDesk();
  const byCol = createMemo(() => {
    const m = {} as Record<ColumnId, TaskView[]>;
    for (const c of COLUMNS) m[c.id] = [];
    for (const v of props.views) m[v.column].push(v);
    return m;
  });
  const counts = () => COLUMNS.map(c => byCol()[c.id].length);
  const pos = createMemo<Pos | null>(() => {
    const n = counts(), p = props.pos;
    if (n[p.col] > 0) return { col: p.col, idx: Math.min(p.idx, n[p.col] - 1) };
    const c = n.findIndex(x => x > 0);
    return c < 0 ? null : { col: c, idx: 0 };
  });
  const apis: VirtualApi[] = [];
  let grid!: HTMLDivElement;
  const focusCard = (col: number, idx: number) => {
    props.setPos({ col, idx });
    apis[col]?.scrollToIndex(idx);
    const el = grid.querySelector<HTMLElement>(`[data-col="${col}"][data-idx="${idx}"]`);
    el?.focus({ preventScroll: true });
    el?.scrollIntoView?.({ block: "nearest", inline: "nearest" });
  };
  const nextCol = (from: number, dir: 1 | -1) => {
    const n = counts();
    for (let c = from + dir; c >= 0 && c < n.length; c += dir) if (n[c] > 0) return c;
    return -1;
  };
  const onKey = (e: KeyboardEvent) => {
    if (e.altKey || e.ctrlKey || e.metaKey || !(e.target as HTMLElement).classList.contains("a-card")) return;
    const p = pos();
    if (!p) return;
    const n = counts();
    let { col, idx } = p;
    switch (e.key) {
      case "ArrowDown": case "j": idx = Math.min(idx + 1, n[col] - 1); break;
      case "ArrowUp": case "k": idx = Math.max(idx - 1, 0); break;
      case "PageDown": idx = Math.min(idx + 5, n[col] - 1); break;
      case "PageUp": idx = Math.max(idx - 5, 0); break;
      case "ArrowRight": case "l": { const c = nextCol(col, 1); if (c < 0) return; col = c; idx = Math.min(idx, n[c] - 1); break; }
      case "ArrowLeft": case "h": { const c = nextCol(col, -1); if (c < 0) return; col = c; idx = Math.min(idx, n[c] - 1); break; }
      case "Home": idx = 0; break;
      case "End": idx = n[col] - 1; break;
      default: return;
    }
    e.preventDefault();
    focusCard(col, idx);
  };
  const scrollCols = (dir: 1 | -1) => {
    const w = grid.querySelector<HTMLElement>(".a-col")?.offsetWidth ?? 200;
    grid.scrollBy?.({ left: dir * (w + 8) });
  };
  onMount(() => { const p = pos(); if (props.restoreFocus && p) focusCard(p.col, p.idx); });

  const Card = (c: { v: () => TaskView; col: number; idx: number }) => {
    const isPos = () => { const q = pos(); return !!q && q.col === c.col && q.idx === c.idx; };
    const selected = () => c.v().id === desk.store.state().selectedTaskId;
    return <button type="button" class="a-card" classList={{ "a-card--wait": !!c.v().wait }}
      data-col={c.col} data-idx={c.idx} tabindex={isPos() ? 0 : -1} aria-current={selected() ? "true" : undefined}
      aria-label={cardLabel(c.v(), desk.conn())}
      onFocus={() => { if (!isPos()) props.setPos({ col: c.col, idx: c.idx }); }}
      onClick={() => { props.setPos({ col: c.col, idx: c.idx }); props.onOpen(c.v().id); }}>
      <span class="a-card-title">{c.v().title}</span>
      <span class="a-card-badges">
        <ExecBadge v={c.v()} /><WaitBadge v={c.v()} />
        <Show when={c.v().failedCheck}><Badge tone="danger" glyph="x">Check failed</Badge></Show>
      </span>
      <span class="a-card-meta"><Meta v={c.v()} /></span>
    </button>;
  };

  return <div class="a-board-wrap">
    <div class="a-board-tools">
      <p class="a-muted">Columns show the runtime phase (candidate names, fixture projection). They are read-only: no drag and drop.</p>
      <span class="a-actions">
        <button type="button" onClick={() => scrollCols(-1)}>Previous columns</button>
        <button type="button" onClick={() => scrollCols(1)}>Next columns</button>
      </span>
    </div>
    <div class="a-board" ref={grid} role="grid" aria-label="Task board" aria-colcount={COLUMNS.length} aria-readonly="true" onKeyDown={onKey}>
      <div role="row" class="a-board-row a-board-heads">
        <For each={COLUMNS}>{c => <div role="columnheader" class="a-colhead">
          <span>{c.name}</span> <span class="a-count">{props.loading ? "…" : byCol()[c.id].length}</span>
          <Show when={desk.conn() !== "connected"}><span class="a-muted"> · as of {hhmm(desk.store.state().fixture.machines[0].lastConfirmedAt)}</span></Show>
        </div>}</For>
      </div>
      <div role="row" class="a-board-row a-board-cols">
        <For each={COLUMNS}>{(c, col) => <div role="gridcell" class="a-col">
          <Show when={props.loading}>
            <div class="a-skels" aria-hidden="true"><For each={SKELETONS}>{() => <div class="a-skel" />}</For></div>
          </Show>
          <Show when={!props.loading && byCol()[c.id].length === 0}><p class="a-col-empty">No tasks</p></Show>
          <Show when={!props.loading && byCol()[c.id].length > 0}>
            <VirtualList items={byCol()[c.id]} rowU={CARD_U} listRole="list" listLabel={`${c.name} tasks`} rowRole="listitem"
              pin={pos()?.col === col() ? pos()!.idx : undefined}
              api={a => { apis[col()] = a; }} render={(v, idx) => <Card v={v} col={col()} idx={idx} />} />
          </Show>
        </div>}</For>
      </div>
    </div>
  </div>;
}

export type TableApi = { focusTask(id: string): void };
const SORTS: { key: SortKey; label: string }[] = [
  { key: "title", label: "Title" }, { key: "phase", label: "Phase" }, { key: "exec", label: "Execution" },
  { key: "wait", label: "Wait reason" }, { key: "updated", label: "Updated · provenance" },
];

/** List home (sortable table) and the compact task list beside an open task. */
export function TaskTable(props: { views: TaskView[]; compact: boolean; loading: boolean; onOpen: (id: string) => void; api?: (a: TableApi) => void }) {
  const desk = useDesk();
  const [sort, setSort] = createSignal<{ key: SortKey; dir: 1 | -1 } | null>(null);
  const rows = createMemo(() => { const s = sort(); return s ? sortViews(props.views, s.key, s.dir) : props.views; });
  const [focusIdx, setFocusIdx] = createSignal(0);
  const fi = () => Math.min(focusIdx(), Math.max(0, rows().length - 1));
  let vapi: VirtualApi | undefined;
  let wrap!: HTMLDivElement;
  const focusRow = (i: number) => {
    setFocusIdx(i);
    vapi?.scrollToIndex(i);
    wrap.querySelector<HTMLElement>(`[data-row="${i}"]`)?.focus({ preventScroll: true });
  };
  props.api?.({ focusTask(id) { const i = rows().findIndex(v => v.id === id); if (i >= 0) focusRow(i); } });
  const onKey = (e: KeyboardEvent) => {
    if (e.altKey || e.ctrlKey || e.metaKey || !(e.target as HTMLElement).classList.contains("a-row")) return;
    const n = rows().length, i = fi();
    if (!n) return;
    let next = i;
    switch (e.key) {
      case "ArrowDown": case "j": next = Math.min(i + 1, n - 1); break;
      case "ArrowUp": case "k": next = Math.max(i - 1, 0); break;
      case "PageDown": next = Math.min(i + 10, n - 1); break;
      case "PageUp": next = Math.max(i - 10, 0); break;
      case "Home": next = 0; break;
      case "End": next = n - 1; break;
      case "Enter": case " ": e.preventDefault(); props.onOpen(rows()[i].id); return;
      default: return;
    }
    e.preventDefault();
    focusRow(next);
  };
  const toggleSort = (key: SortKey) => {
    const s = sort();
    setSort(s?.key === key ? (s.dir === 1 ? { key, dir: -1 } : null) : { key, dir: 1 });
  };
  const ariaSort = (key: SortKey) => sort()?.key === key ? (sort()!.dir === 1 ? "ascending" : "descending") : "none";

  return <div class="a-table" classList={{ "a-table--compact": props.compact }} role="grid" ref={wrap}
    aria-label={props.compact ? "Task list" : "Tasks"} aria-rowcount={rows().length + 1} aria-colcount={props.compact ? 1 : 5} onKeyDown={onKey}>
    <div role="row" class="a-thead" aria-rowindex={1}>
      <Show when={!props.compact} fallback={<div role="columnheader" class="a-th">Tasks ({rows().length})</div>}>
        <For each={SORTS}>{s => <div role="columnheader" class="a-th" aria-sort={ariaSort(s.key)}>
          <button type="button" class="a-sort" onClick={() => toggleSort(s.key)}>
            {s.label}<span class="a-muted">{sort()?.key === s.key ? (sort()!.dir === 1 ? " (ascending)" : " (descending)") : ""}</span>
          </button>
        </div>}</For>
      </Show>
    </div>
    <Show when={!props.loading} fallback={<div class="a-skels" aria-hidden="true"><For each={SKELETONS}>{() => <div class="a-skel a-skel--row" />}</For></div>}>
      <VirtualList items={rows()} rowU={props.compact ? 3.75 : 2.75} listRole="rowgroup" pin={fi()} api={a => { vapi = a; }}
        render={(v, i) => {
          const selected = () => v().id === desk.store.state().selectedTaskId;
          return <div role="row" class="a-row" classList={{ "a-row--sel": selected(), "a-row--wait": !!v().wait }} data-row={i}
            tabindex={i === fi() ? 0 : -1} aria-rowindex={i + 2} aria-selected={selected()}
            onFocus={() => setFocusIdx(i)} onClick={() => { setFocusIdx(i); props.onOpen(v().id); }}>
            <Show when={!props.compact} fallback={
              <div role="gridcell" class="a-cell a-cell-compact" aria-label={cardLabel(v(), desk.conn())}>
                <span class="a-row-title">{v().title}</span>
                <span class="a-row-badges"><ExecBadge v={v()} /><WaitBadge v={v()} /></span>
              </div>}>
              <div role="gridcell" class="a-cell a-row-title" title={v().title}>{v().title}<Show when={v().created}> <span class="a-mock-tag">Mock</span></Show></div>
              <div role="gridcell" class="a-cell">{columnName(v().column)}</div>
              <div role="gridcell" class="a-cell"><ExecBadge v={v()} /></div>
              <div role="gridcell" class="a-cell"><WaitBadge v={v()} /></div>
              <div role="gridcell" class="a-cell a-muted">{v().updatedAt ? `${hhmm(v().updatedAt!)} · fixture` : "unknown"}</div>
            </Show>
          </div>;
        }} />
    </Show>
  </div>;
}
