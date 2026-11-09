import { createEffect, createMemo, createSignal, For, on, onCleanup, onMount, Show, splitProps, useContext, type Accessor, type JSX } from "solid-js";
import type { WorkspaceCommand } from "@cortex/workspace-domain";
import { useWs, WsContext } from "./ctx";
import { hrefOf, type Route } from "./router";
import type { Fact } from "./selectors";

export const I = (props: { n: string; s?: 12 | 14; class?: string }) =>
  <i class={`i ${props.s ? `s${props.s} ` : ""}i-${props.n}${props.class ? ` ${props.class}` : ""}`} aria-hidden="true" />;

/** A count beside a name. Nothing to count says nothing, so no list ever reads "All 0". */
export const Count = (props: { n: number }) => <Show when={props.n > 0}><span class="n">{props.n}</span></Show>;

export const FactLine = (props: { fact: Fact; class?: string }) => <span class={`st ${props.fact.tone}${props.class ? ` ${props.class}` : ""}`}>
  <Show when={props.fact.icon}><I n={props.fact.icon!} s={12} class={props.fact.spin ? "spin" : undefined} /></Show><span>{props.fact.text}</span>
</span>;

/**
 * A list that keeps its rows by name. Read models are rebuilt on every change of the machine, and a plain For would redraw
 * every row for a new object, which reads as a blink under the pointer. Here a row is made once, and only what it says moves.
 */
export function Keyed<T>(props: { each: readonly T[]; by: (item: T) => string; children: (item: Accessor<T>, index: Accessor<number>) => JSX.Element }) {
  const table = createMemo(() => new Map(props.each.map((item, index) => [props.by(item), { item, index }])));
  const keys = createMemo(() => props.each.map(props.by), undefined, { equals: (a, b) => a.length === b.length && a.every((key, i) => key === b[i]) });
  return <For each={keys()}>{key => {
    // A row on its way out still reads what it last held.
    let last = table().get(key)!;
    const entry = () => last = table().get(key) ?? last;
    return props.children(() => entry().item, () => entry().index);
  }}</For>;
}

/** A name is edited where it stands: a click opens it, Enter saves, Escape leaves it as it was. */
export function NameEdit(props: { value: string; label: string; onSave: (name: string) => void; disabled?: string; class?: string; hint?: string }) {
  const [text, setText] = createSignal<string | null>(null);
  const save = () => { const t = text()?.trim(); setText(null); if (t && t !== props.value) props.onSave(t); };
  return <Show when={text() !== null} fallback={<button class={`ws-edit${props.class ? ` ${props.class}` : ""}`} aria-label={`${props.label}: ${props.value}`} aria-disabled={props.disabled ? "true" : undefined} data-why={props.disabled} onClick={() => { if (!props.disabled) setText(props.value); }}>{props.value}</button>}>
    <input class="inp ws-edit-in" aria-label={props.label} value={text()!} onInput={e => setText(e.currentTarget.value)} ref={el => queueMicrotask(() => { el.focus(); el.select(); })} onBlur={() => setText(null)}
      onKeyDown={e => { if (e.key === "Enter") { e.preventDefault(); save(); } else if (e.key === "Escape") { e.stopPropagation(); setText(null); } }} />
  </Show>;
}

/** History link: a plain left click stays in the app, modified clicks keep their browser meaning. */
export function A(props: { to: Route | string } & JSX.AnchorHTMLAttributes<HTMLAnchorElement>) {
  const ws = useWs();
  const [own, rest] = splitProps(props, ["to", "onClick"]);
  const href = () => typeof own.to === "string" ? own.to : hrefOf(own.to);
  return <a {...rest} href={href()} onClick={e => {
    if (typeof own.onClick === "function") (own.onClick as (ev: typeof e) => void)(e);
    if (e.defaultPrevented || e.ctrlKey || e.metaKey || e.shiftKey || e.altKey || e.button !== 0) return;
    e.preventDefault();
    ws.go(own.to);
  }} />;
}

type Align = "left" | "right" | "up";
/** The mockups anchor menus with CSS anchor positioning; placing from the trigger's box behaves the same in any engine. */
export function place(pop: HTMLElement, trigger: HTMLElement, align: Align) {
  const box = trigger.getBoundingClientRect(), s = pop.style;
  s.inset = "auto";
  // A trigger low in the window opens its menu upward, so the last items are never cut off.
  if (align === "up" || box.bottom > innerHeight * .7) s.bottom = `${innerHeight - box.top + 4}px`; else s.top = `${box.bottom + 4}px`;
  if (align === "right") s.right = `${innerWidth - box.right}px`; else s.left = `${box.left}px`;
}
export const closePop = (el: Element | null) => (el?.closest("[popover]") as HTMLElement | null)?.hidePopover?.();

/** Native popover: light dismiss and Escape come from the platform. */
export function Pop(props: { id: string; class?: string; align?: Align; label?: string; style?: JSX.CSSProperties; children: JSX.Element; ref?: (el: HTMLDivElement) => void }) {
  let el!: HTMLDivElement;
  const onBefore = (e: Event) => {
    if ((e as ToggleEvent).newState !== "open" || !props.align) return;
    const trigger = document.querySelector<HTMLElement>(`[popovertarget="${props.id}"]`);
    if (trigger) place(el, trigger, props.align);
  };
  onMount(() => el.addEventListener("beforetoggle", onBefore));
  return <div ref={r => { el = r; props.ref?.(r); }} popover="auto" id={props.id} class={props.class} style={props.style} aria-label={props.label}>{props.children}</div>;
}
export function MenuItem(props: { icon?: string; label: JSX.Element; sub?: string; keys?: string; check?: boolean; danger?: boolean; disabled?: string; needs?: WorkspaceCommand["type"]; onSelect?: () => void }) {
  const ws = useWs();
  // Read through a memo: a conditional prop read in a click handler would otherwise build a computation with no owner.
  const disabled = createMemo(() => props.disabled ?? (props.needs ? ws.gate(props.needs) : undefined));
  return <button class={`mi${props.danger ? " mi-danger" : ""}`} role="menuitem" aria-disabled={disabled() ? "true" : undefined} data-why={disabled()}
    onClick={e => { const at = e.currentTarget; if (disabled()) return; closePop(at); props.onSelect?.(); }}>
    <Show when={props.icon}><I n={props.icon!} /></Show>
    <span class="grow">{props.label}<Show when={props.sub}><span class="sub">{props.sub}</span></Show></span>
    <Show when={props.keys}><span class="kbd">{props.keys}</span></Show>
    <Show when={props.check}><I n="check" s={14} /></Show>
  </button>;
}

/** Native modal: the rest of the app is inert while it is open, and focus returns to where it was. */
export function Modal(props: { open: boolean; onClose: () => void; onDismiss?: () => void; label: string; class?: string; children: JSX.Element }) {
  let el!: HTMLDialogElement, back: HTMLElement | null = null;
  const ws = useContext(WsContext);
  const [priorReceipt, setPriorReceipt] = createSignal(ws?.store.receipt());
  const error = () => { const r = ws?.store.receipt(); return r && r !== priorReceipt() && !r.ok ? r.message : null; };
  const close = () => {
    if (!el.open) return;
    if (typeof el.close === "function") el.close(); else el.removeAttribute("open");
    const target = back?.isConnected ? back : document.querySelector<HTMLElement>("main[tabindex]");
    target?.focus({ preventScroll: true });
  };
  createEffect(on(() => props.open, open => {
    if (open) {
      setPriorReceipt(() => ws?.store.receipt());
      back = document.activeElement as HTMLElement | null;
      if (!el.open) { if (typeof el.showModal === "function") el.showModal(); else el.setAttribute("open", ""); }
      el.querySelector<HTMLElement>("[data-autofocus]")?.focus();
    } else close();
  }));
  onCleanup(close);
  return <dialog ref={el} class={`dlg ${props.class ?? ""}`} aria-label={props.label}
    onCancel={e => { e.preventDefault(); props.onClose(); }}
    onClick={e => { if (e.target === el) (props.onDismiss ?? props.onClose)(); }}>
    <Show when={props.open}>{props.children}<Show when={error()}>{message => <div class="dlg-b ws-err" role="alert">{message()}</div>}</Show></Show>
  </dialog>;
}

export { createFollow } from "./follow";

export type VirtualApi = { el: HTMLDivElement };
/**
 * Windowed list. Rows sit at measured offsets (an estimate until a row has rendered), so a long column only keeps what
 * is in view plus overscan. Keys stay stable across snapshots, and the focused row stays rendered while it scrolls away.
 */
export function VirtualList<T>(props: {
  items: T[]; keyOf: (item: T) => string; estimate: (item: T) => number; gap?: number; overscan?: number;
  class?: string; label?: string; header?: JSX.Element; style?: JSX.CSSProperties; onKeyDown?: (e: KeyboardEvent) => void;
  render: (item: Accessor<T>) => JSX.Element;
}) {
  let scroller!: HTMLDivElement, inner!: HTMLDivElement;
  const [top, setTop] = createSignal(0);
  const [height, setHeight] = createSignal(600);
  const [version, setVersion] = createSignal(0);
  const [held, setHeld] = createSignal<string | null>(null);
  const measured = new Map<string, number>();
  const gap = () => props.gap ?? 0;
  const layout = createMemo(() => {
    version();
    const keys = props.items.map(props.keyOf), offsets = new Float64Array(keys.length + 1);
    keys.forEach((key, i) => { offsets[i + 1] = offsets[i] + (measured.get(key) ?? props.estimate(props.items[i])) + gap(); });
    return { keys, offsets, at: new Map(keys.map((key, i) => [key, i])) };
  });
  const indexAt = (y: number) => {
    const { offsets } = layout();
    let lo = 0, hi = Math.max(0, offsets.length - 2);
    while (lo < hi) { const mid = (lo + hi + 1) >> 1; if (offsets[mid] <= y) lo = mid; else hi = mid - 1; }
    return lo;
  };
  const visible = createMemo(() => {
    const { keys, at } = layout(), over = props.overscan ?? 4;
    const out = keys.slice(Math.max(0, indexAt(top()) - over), indexAt(top() + height()) + 1 + over);
    const pin = held();
    if (pin && at.has(pin) && !out.includes(pin)) out.push(pin);
    return out;
  });
  const sync = () => {
    setTop(Math.max(0, scroller.scrollTop - inner.offsetTop));
    if (scroller.clientHeight) setHeight(scroller.clientHeight);
  };
  const observer = typeof ResizeObserver === "undefined" ? undefined : new ResizeObserver(entries => {
    let changed = false;
    for (const entry of entries) {
      const key = (entry.target as HTMLElement).dataset.key!, size = entry.borderBoxSize?.[0]?.blockSize ?? (entry.target as HTMLElement).offsetHeight;
      const old = measured.get(key) ?? props.estimate(props.items[layout().at.get(key) ?? 0]);
      if (!size || Math.abs(size - old) < .5) continue;
      measured.set(key, size); changed = true;
      // Keep what the reader sees still when a row above the viewport turns out a different height.
      if (layout().offsets[layout().at.get(key) ?? 0] < top()) scroller.scrollTop += size - old;
    }
    if (changed) setVersion(v => v + 1);
  });
  onMount(() => {
    sync();
    if (typeof ResizeObserver !== "undefined") { const box = new ResizeObserver(sync); box.observe(scroller); onCleanup(() => box.disconnect()); }
  });
  onCleanup(() => observer?.disconnect());
  const item = (key: string) => props.items[layout().at.get(key) ?? 0];
  return <div ref={scroller} class={props.class} style={props.style} role="group" aria-label={props.label} onScroll={sync} onKeyDown={e => props.onKeyDown?.(e)}
    onFocusIn={e => setHeld((e.target as HTMLElement).closest<HTMLElement>(".v-row")?.dataset.key ?? null)}
    onFocusOut={e => { if (!scroller.contains(e.relatedTarget as Node | null)) setHeld(null); }}>
    {props.header}
    <div ref={inner} class="v-inner" style={{ height: `${layout().offsets[layout().keys.length] - (layout().keys.length ? gap() : 0)}px` }}>
      <For each={visible()}>{key => <div class="v-row" data-key={key} ref={el => { observer?.observe(el); onCleanup(() => observer?.unobserve(el)); }}
        style={{ transform: `translateY(${layout().offsets[layout().at.get(key) ?? 0]}px)` }}>
        <Show when={layout().at.has(key)}>{props.render(() => item(key))}</Show>
      </div>}</For>
    </div>
  </div>;
}
