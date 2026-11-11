import { createEffect, createMemo, createSignal, For, on, onCleanup, onMount, Show, splitProps, useContext, type Accessor, type JSX } from "solid-js";
import type { WorkspaceCommand } from "@cortex/workspace-domain";
import { useWs, WsContext } from "./ctx";
import { hrefOf, type Route } from "./router";
import type { Fact } from "./selectors";

export const I = (props: { n: string; s?: 12 | 14; class?: string }) =>
  <i class={`i ${props.s ? `s${props.s} ` : ""}i-${props.n}${props.class ? ` ${props.class}` : ""}`} aria-hidden="true" />;

/** Words for something still on its way, with a loader that runs only as long as they are shown. */
export const Loading = (props: { text: string; class?: string }) =>
  <div class={`ws-loading${props.class ? ` ${props.class}` : ""}`} role="status"><I n="loader" s={12} class="spin" /><span>{props.text}</span></div>;

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
/**
 * Keeps an open popover whole inside the window: one placed against a trigger near an edge (or opened from another menu) moves
 * back in by its overflow, and one taller than the window scrolls inside it. Only `translate` and `max-height` are touched, so
 * the placement itself, from CSS or from `place`, stays as it was for the next opening.
 */
export function keepInside(pop: HTMLElement, margin = 8) {
  pop.style.translate = "";
  if (pop.hasAttribute("data-fit")) { pop.style.maxHeight = ""; pop.style.overflowY = ""; pop.removeAttribute("data-fit"); }
  const r = pop.getBoundingClientRect();
  if (!r.width || !r.height) return;
  if (r.height > innerHeight - 2 * margin) { pop.style.maxHeight = `${innerHeight - 2 * margin}px`; pop.style.overflowY = "auto"; pop.setAttribute("data-fit", ""); }
  const h = Math.min(r.height, innerHeight - 2 * margin), w = Math.min(r.width, innerWidth - 2 * margin);
  const dx = r.left + w > innerWidth - margin ? innerWidth - margin - w - r.left : r.left < margin ? margin - r.left : 0;
  const dy = r.top + h > innerHeight - margin ? innerHeight - margin - h - r.top : r.top < margin ? margin - r.top : 0;
  if (dx || dy) pop.style.translate = `${Math.round(dx)}px ${Math.round(dy)}px`;
}

/**
 * Focus that a dialog or a panel puts on its first control as it opens. The quiet focus line belongs to keyboard movement and no
 * press drew this one, so after a pointer opened it the line still marks where Tab, Enter and Escape act, until focus moves on or
 * the next press (workspace.css). A text field keeps only its caret.
 */
export function placeFocus(el: HTMLElement | null | undefined, options?: FocusOptions) {
  if (!el) return;
  el.focus(options);
  // A text field shows its caret instead, as it does for the keyboard.
  if (document.activeElement !== el || document.documentElement.hasAttribute("data-keyboard") || el.matches("textarea, input:not([type='checkbox'], [type='radio']), [contenteditable]")) return;
  el.setAttribute("data-placed-focus", "");
  const clear = () => { el.removeAttribute("data-placed-focus"); el.removeEventListener("blur", clear); window.removeEventListener("pointerdown", clear, true); };
  el.addEventListener("blur", clear);
  window.addEventListener("pointerdown", clear, true);
}

/**
 * Native popover: light dismiss and Escape come from the platform. A popover role carries its label: a menu when it holds menu
 * items, otherwise a non-modal dialog, unless `role` says otherwise.
 */
export function Pop(props: { id: string; class?: string; align?: Align; label?: string; role?: "menu" | "dialog"; style?: JSX.CSSProperties; children: JSX.Element; ref?: (el: HTMLDivElement) => void }) {
  let el!: HTMLDivElement;
  const name = () => { if (!props.role) el.setAttribute("role", el.querySelector("[role='menuitem']") ? "menu" : "dialog"); };
  const onBefore = (e: Event) => {
    if ((e as ToggleEvent).newState !== "open") return;
    name();
    if (!props.align) return;
    const trigger = (el.closest(".ws-pane") ?? document).querySelector<HTMLElement>(`[popovertarget="${props.id}"]`);
    if (trigger) place(el, trigger, props.align);
  };
  const onToggle = (e: Event) => { if ((e as ToggleEvent).newState === "open") keepInside(el); };
  onMount(() => { name(); el.addEventListener("beforetoggle", onBefore); el.addEventListener("toggle", onToggle); });
  return <div ref={r => { el = r; props.ref?.(r); }} popover="auto" id={props.id} class={props.class} style={props.style} role={props.role} aria-label={props.label}>{props.children}</div>;
}
export function MenuItem(props: { icon?: string; label: JSX.Element; sub?: string; keys?: string; check?: boolean; danger?: boolean; disabled?: string; needs?: WorkspaceCommand["type"]; onSelect?: () => void }) {
  const ws = useWs();
  // Read through a memo: a conditional prop read in a click handler would otherwise build a computation with no owner.
  const disabled = createMemo(() => props.disabled ?? (props.needs ? ws.gate(props.needs) : undefined));
  return <button class={`mi${props.danger ? " mi-danger" : ""}`} role="menuitem" aria-disabled={disabled() ? "true" : undefined} data-why={disabled()}
    onClick={e => { const at = e.currentTarget; if (disabled()) return; closePop(at); props.onSelect?.(); }}>
    <Show when={props.icon}><I n={props.icon!} /></Show>
    <span class="grow">{props.label}<Show when={props.sub}><span class="sub">{props.sub}</span></Show><Show when={disabled()}>{why => <span class="sub">{why()}</span>}</Show></span>
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
      placeFocus(el.querySelector<HTMLElement>("[data-autofocus]"));
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

/**
 * A long column of rows in normal flow. Every row is rendered once and the engine paints only what is in view, so scrolling
 * runs no script and changes nothing in the page. (Windowing rows in and out on each scroll event built, measured and
 * re-placed cards while the owner scrolled, which dropped frames in WebKitGTK; test/scroll-perf.py measures it.) Keys stay
 * stable across snapshots, so an updated record redraws inside its row and the focused row keeps its focus.
 */
export function RowList<T>(props: {
  items: T[]; keyOf: (item: T) => string; gap?: number;
  class?: string; label?: string; header?: JSX.Element; style?: JSX.CSSProperties; onKeyDown?: (e: KeyboardEvent) => void;
  render: (item: Accessor<T>) => JSX.Element;
}) {
  const keys = createMemo(() => props.items.map(props.keyOf), undefined, { equals: (a, b) => a.length === b.length && a.every((key, i) => key === b[i]) });
  const at = createMemo(() => new Map(keys().map((key, i) => [key, i])));
  const item = (key: string) => props.items[at().get(key) ?? 0];
  return <div class={props.class} style={props.style} role="group" aria-label={props.label} onKeyDown={e => props.onKeyDown?.(e)}>
    {props.header}
    <div class="v-inner" style={{ gap: `${props.gap ?? 0}px` }}>
      <For each={keys()}>{key => <div class="v-row" data-key={key}><Show when={at().has(key)}>{props.render(() => item(key))}</Show></div>}</For>
    </div>
  </div>;
}
