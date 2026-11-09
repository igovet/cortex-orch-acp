import { createMemo, createSignal, For, onCleanup, onMount, Show, type JSX } from "solid-js";

type Role = JSX.HTMLAttributes<HTMLDivElement>["role"];

export type VirtualApi = {
  el: HTMLDivElement;
  scrollToIndex(index: number, align?: "nearest" | "start" | "end"): void;
  atEnd(): boolean;
};

/**
 * Fixed-row virtual list. Row height is given in text units (`--u`, the
 * concept's scaled rem) and measured live, so 200 % text keeps rows unclipped.
 * Only the rows in view plus a small overscan exist in the DOM.
 */
export function VirtualList<T>(props: {
  items: readonly T[]; rowU: number; overscan?: number; class?: string;
  /** Index kept rendered even when scrolled out of view (roving focus target). */
  pin?: number;
  listRole?: Role; listLabel?: string; rowRole?: Role;
  render: (item: () => T, index: number) => JSX.Element;
  api?: (api: VirtualApi) => void; onScrollChange?: (atEnd: boolean) => void;
}) {
  let el!: HTMLDivElement;
  let measure!: HTMLSpanElement;
  const [top, setTop] = createSignal(0);
  const [viewH, setViewH] = createSignal(0);
  const [unit, setUnit] = createSignal(16);
  const rowPx = () => props.rowU * unit();
  const range = createMemo(() => {
    const h = rowPx(), n = props.items.length, o = props.overscan ?? 3, vh = viewH() || 600;
    const start = Math.max(0, Math.floor(top() / h) - o), end = Math.min(n, Math.ceil((top() + vh) / h) + o);
    const out: number[] = [];
    for (let i = start; i < end; i++) out.push(i);
    const pin = props.pin;
    if (pin !== undefined && pin >= 0 && pin < n && (pin < start || pin >= end)) pin < start ? out.unshift(pin) : out.push(pin);
    return out;
  });
  const sync = () => { setTop(el.scrollTop); setViewH(el.clientHeight); };
  const atEnd = () => el.scrollTop + el.clientHeight >= el.scrollHeight - 2;
  onMount(() => {
    const measureUnit = () => setUnit(measure.getBoundingClientRect().height || 16);
    measureUnit();
    sync();
    if (typeof ResizeObserver !== "undefined") {
      const ro = new ResizeObserver(() => { measureUnit(); sync(); });
      ro.observe(el);
      ro.observe(measure);
      onCleanup(() => ro.disconnect());
    }
    props.api?.({
      el, atEnd,
      scrollToIndex(index, align = "nearest") {
        const h = rowPx(), t = index * h, vh = el.clientHeight;
        if (align === "start" || t < el.scrollTop) el.scrollTop = t;
        else if (align === "end" || t + h > el.scrollTop + vh) el.scrollTop = t + h - vh;
        sync();
      },
    });
  });
  return <div class={`a-vlist ${props.class ?? ""}`} ref={el} onScroll={() => { sync(); props.onScrollChange?.(atEnd()); }}>
    <span class="a-vmeasure" ref={measure} aria-hidden="true" />
    <div class="a-vinner" role={props.listRole} aria-label={props.listLabel} style={{ height: `${props.items.length * rowPx()}px` }}>
      <For each={range()}>{i => {
        const item = () => props.items[i];
        return <div class="a-vrow" role={props.rowRole ?? "none"} style={{ top: `${i * rowPx()}px`, height: `${rowPx()}px` }}>
          <Show when={item() !== undefined}>{props.render(item as () => T, i)}</Show>
        </div>;
      }}</For>
    </div>
  </div>;
}
