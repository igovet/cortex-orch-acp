import { createMemo, createSignal, For, Index, Show } from "solid-js";
import { GROUPS, type GroupId, type QueueItem } from "./model";
import { Badge } from "./ui";

type QueueProps = {
  items: QueueItem[]; selectedId: string; loading: boolean; error: string | null; asOf: string | null;
  quietOpen: boolean; onToggleQuiet: () => void; onOpen: (taskId: string) => void; onAllTasks: () => void;
  shortcutsOn: boolean; register: (focusSelected: () => void) => void;
};

/**
 * Attention queue (L0). One Tab stop; arrows or J/K move focus only.
 * Selection changes only on Enter/click, so browsing never changes the focus pane.
 */
export function Queue(p: QueueProps) {
  let nav!: HTMLElement;
  const [active, setActive] = createSignal("");
  const grouped = createMemo(() => {
    const map: Record<GroupId, QueueItem[]> = { decision: [], blocked: [], changed: [], running: [], quiet: [] };
    for (const item of p.items) map[item.group].push(item);
    return map;
  });
  const keys = createMemo(() => {
    const g = grouped();
    const list: string[] = [];
    for (const { id } of GROUPS) {
      if (id !== "quiet") list.push(...g[id].map(i => i.key));
      else {
        if (g.quiet.length) list.push("quiet-toggle");
        if (p.quietOpen) list.push(...g.quiet.map(i => i.key));
      }
    }
    return list;
  });
  const keySet = createMemo(() => new Set(keys()));
  const selectedKey = () => p.items.find(i => i.taskId === p.selectedId)?.key;
  const current = createMemo(() => {
    const set = keySet();
    const a = active();
    if (a && set.has(a)) return a;
    const s = selectedKey();
    if (s && set.has(s)) return s;
    return keys()[0] ?? "";
  });
  p.register(() => {
    const s = selectedKey();
    if (s && keySet().has(s)) setActive(s);
    nav.querySelector<HTMLElement>('[data-roving][tabindex="0"]')?.focus();
  });

  const onKeyDown = (e: KeyboardEvent) => {
    const list = Array.from(nav.querySelectorAll<HTMLElement>("[data-roving]"));
    const i = list.indexOf(document.activeElement as HTMLElement);
    if (i < 0) return;
    const plain = !e.ctrlKey && !e.altKey && !e.metaKey;
    let next = -1;
    if (e.key === "ArrowDown" || (p.shortcutsOn && plain && e.key === "j")) next = Math.min(list.length - 1, i + 1);
    else if (e.key === "ArrowUp" || (p.shortcutsOn && plain && e.key === "k")) next = Math.max(0, i - 1);
    else if (e.key === "Home") next = 0;
    else if (e.key === "End") next = list.length - 1;
    else return;
    e.preventDefault();
    setActive(list[next].dataset.key ?? "");
    list[next].focus();
  };

  const Row = (item: () => QueueItem) => <li>
    <button type="button" class="c-row" data-roving data-key={item().key}
      tabindex={current() === item().key ? 0 : -1}
      aria-current={item().taskId === p.selectedId ? "true" : undefined}
      onFocus={() => setActive(item().key)}
      onClick={() => { setActive(item().key); p.onOpen(item().taskId); }}>
      <span class="c-row-line1"><Badge tone={item().tone} glyph={item().glyph} label={item().badge} />{" "}<span class="c-row-ask">{item().ask}</span></span>
      <span class="c-row-line2"><span class="c-row-title">{item().title}</span>{" "}<span class="c-row-meta">· {item().meta}</span></span>
    </button>
  </li>;

  const asOf = () => p.asOf ? <span class="c-asof"> · as of {p.asOf}</span> : null;

  return <nav ref={nav} class="c-queue" aria-label="Attention queue" onKeyDown={onKeyDown}>
    <Show when={!p.loading} fallback={<div class="c-qsection">
      <p class="c-qhead" role="status">Loading tasks…</p>
      <For each={[1, 2, 3, 4]}>{() => <div class="c-skel" aria-hidden="true"><span /><span /></div>}</For>
    </div>}>
      <Show when={!p.error} fallback={<div class="c-qsection">
        <p class="c-error-text" role="alert"><span aria-hidden="true">✕ </span>Could not load the attention queue (fixture error E_FIXTURE_READ). Nothing was changed.</p>
      </div>}>
        <For each={GROUPS}>{group => <section class="c-qsection" aria-labelledby={`c-q-${group.id}`}>
          <Show when={group.id === "quiet"} fallback={<>
            <h2 id={`c-q-${group.id}`} class="c-qhead">{group.title} ({grouped()[group.id].length}){asOf()}</h2>
            <Show when={grouped()[group.id].length} fallback={group.id === "decision"
              ? <div class="c-qempty"><p>Nothing needs your decision in this view. Last checked: fixture load · observed.</p>
                <button type="button" class="c-link" onClick={() => p.onAllTasks()}>Show all tasks</button></div>
              : <p class="c-qempty">None.</p>}>
              <ul class="c-qlist"><Index each={grouped()[group.id]}>{Row}</Index></ul>
            </Show>
          </>}>
            <Show when={grouped().quiet.length} fallback={<h2 id="c-q-quiet" class="c-qhead">Quiet tasks (0){asOf()}</h2>}>
              <h2 id="c-q-quiet" class="c-qhead">
                <button type="button" class="c-qtoggle" data-roving data-key="quiet-toggle"
                  tabindex={current() === "quiet-toggle" ? 0 : -1} aria-expanded={p.quietOpen} aria-controls="c-quiet-list"
                  onFocus={() => setActive("quiet-toggle")} onClick={() => p.onToggleQuiet()}>
                  <span aria-hidden="true">{p.quietOpen ? "▾ " : "▸ "}</span>Quiet tasks ({grouped().quiet.length}){asOf()}
                </button>
              </h2>
              <ul id="c-quiet-list" class="c-qlist" hidden={!p.quietOpen}>
                <Show when={p.quietOpen}><Index each={grouped().quiet}>{Row}</Index></Show>
              </ul>
            </Show>
          </Show>
        </section>}</For>
      </Show>
    </Show>
  </nav>;
}
