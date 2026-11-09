import { createMemo, createSignal, For, onCleanup, onMount, Show, splitProps, type JSX } from "solid-js";
import { Portal } from "solid-js/web";
import { useDesk } from "./ctx";
import { Icon, type IconName } from "./icons";
import { EXEC_LABEL, type TaskView } from "./model";

export type Tone = "neutral" | "info" | "warn" | "danger" | "success" | "mock";

/** In-app link: real href (modified clicks open new tabs), plain clicks use the History API. */
export function A(props: JSX.AnchorHTMLAttributes<HTMLAnchorElement> & { href: string }) {
  const desk = useDesk();
  const [own, rest] = splitProps(props, ["href", "onClick"]);
  return <a {...rest} href={own.href} onClick={e => {
    if (typeof own.onClick === "function") (own.onClick as (e: MouseEvent) => void)(e);
    if (e.defaultPrevented || e.ctrlKey || e.metaKey || e.shiftKey || e.altKey || e.button !== 0) return;
    e.preventDefault();
    desk.go(own.href);
  }} />;
}

export function Chip(props: { tone?: Tone; icon?: IconName; children: JSX.Element; title?: string }) {
  return <span class={`chip tone-${props.tone ?? "neutral"}`} title={props.title}>
    <Show when={props.icon}><Icon name={props.icon!} size={14} /></Show>
    <span>{props.children}</span>
  </span>;
}

const EXEC_ICON: Record<string, IconName> = { running: "running", blocked: "octagonPause", waiting: "pause", failed: "xCircle", complete: "complete", unknown: "help" };
const EXEC_TONE: Record<string, Tone> = { running: "info", blocked: "warn", waiting: "warn", failed: "danger", complete: "neutral", unknown: "neutral" };

/** Whether TaskChips renders any chip (not counting the muted "Not started" text). Cards size themselves from this. */
export function hasChips(tv: TaskView, compact = true) {
  return tv.exec !== "not_started" || Boolean(tv.wait && !tv.responded) || tv.responded || tv.check === "failed" || (!compact && tv.check === "not_run") || tv.stale || tv.conflict;
}

/** Execution + wait reason + check chips. "Not started" is muted text, never a chip. Complete is neutral, never success. */
export function TaskChips(props: { tv: TaskView; compact?: boolean; noIdle?: boolean }) {
  const tv = () => props.tv;
  return <span class="chips">
    <Show when={tv().exec !== "not_started"} fallback={props.noIdle ? null : <span class="muted-xs">Not started{tv().created ? " (mock)" : ""}</span>}>
      <Chip tone={EXEC_TONE[tv().exec]} icon={EXEC_ICON[tv().exec]} title={tv().lastKnown ? `Last known: ${EXEC_LABEL[tv().lastKnown!]} · 00:00` : undefined}>
        {EXEC_LABEL[tv().exec]}{tv().lastKnown && !props.compact ? ` (last known ${EXEC_LABEL[tv().lastKnown!]})` : ""}
      </Chip>
    </Show>
    <Show when={tv().wait && !tv().responded}>
      <Chip tone="warn" icon={tv().wait === "permission" ? "key" : "question"}>{tv().wait === "permission" ? "Your permission" : "Your answer"}</Chip>
    </Show>
    <Show when={tv().responded}><Chip tone="mock" icon="flask">Response sent · Mock</Chip></Show>
    <Show when={tv().check === "failed"}><Chip tone="danger" icon="xCircle">Failed check</Chip></Show>
    <Show when={tv().check === "not_run" && !props.compact}><Chip icon="notRun">Not run</Chip></Show>
    <Show when={tv().stale}><Chip tone="warn" icon="history">Result stale</Chip></Show>
    <Show when={tv().conflict}><Chip tone="danger" icon="gitMerge">Conflict</Chip></Show>
  </span>;
}

const HUES = 8;
export function Monogram(props: { name: string; index: number; size?: number }) {
  return <span class={`mono-tile hue-${props.index % HUES}`} style={{ "--tile": `${(props.size ?? 16) / 16}rem` }} aria-hidden="true">{props.name.slice(0, 1)}</span>;
}

type BtnProps = JSX.ButtonHTMLAttributes<HTMLButtonElement> & { variant?: "primary" | "secondary" | "ghost" | "danger" | "danger-ghost"; mock?: boolean; icon?: IconName };
/** Mock-suffixed actions show "· Mock" and carry "(mock)" in their accessible name. */
export function Btn(props: BtnProps) {
  const [own, rest] = splitProps(props, ["variant", "mock", "icon", "children", "class"]);
  return <button type="button" {...rest} class={`btn btn-${own.variant ?? "secondary"} ${own.class ?? ""}`}>
    <Show when={own.icon}><Icon name={own.icon!} /></Show>
    <span>{own.children}</span>
    <Show when={own.mock}><span class="mock-suffix" aria-hidden="true">· Mock</span><span class="sr-only"> (mock)</span></Show>
  </button>;
}
export function IconBtn(props: JSX.ButtonHTMLAttributes<HTMLButtonElement> & { icon: IconName; label: string; badge?: JSX.Element }) {
  const [own, rest] = splitProps(props, ["icon", "label", "class", "badge"]);
  return <button type="button" aria-label={own.label} title={own.label} {...rest} class={`icon-btn ${own.class ?? ""}`}>
    <Icon name={own.icon} />{own.badge}
  </button>;
}

export function Group(props: { label?: JSX.Element; trailing?: JSX.Element; children: JSX.Element; class?: string; id?: string }) {
  return <section class={`group-wrap ${props.class ?? ""}`} aria-labelledby={props.id}>
    <Show when={props.label}><div class="group-label"><h2 id={props.id}>{props.label}</h2>{props.trailing}</div></Show>
    <div class="group">{props.children}</div>
  </section>;
}
export function Row(props: { label: JSX.Element; desc?: JSX.Element; children?: JSX.Element; class?: string }) {
  return <div class={`row ${props.class ?? ""}`}>
    <div class="row-text"><div class="row-label">{props.label}</div><Show when={props.desc}><div class="row-desc">{props.desc}</div></Show></div>
    <Show when={props.children}><div class="row-value">{props.children}</div></Show>
  </div>;
}
export function Notice(props: { tone?: Tone; icon?: IconName; children: JSX.Element; action?: JSX.Element; role?: "status" | "alert" }) {
  return <div class={`notice tone-${props.tone ?? "neutral"}`} role={props.role}>
    <Icon name={props.icon ?? (props.tone === "danger" ? "alert" : "info")} />
    <div class="notice-text">{props.children}</div>
    <Show when={props.action}><div class="notice-action">{props.action}</div></Show>
  </div>;
}
/** One-line inline mock receipt; persists where the action happened until the next action on that item. */
export function Receipt(props: { id: string }) {
  const desk = useDesk();
  return <Show when={desk.local().receipts[props.id]}>
    <p class="receipt" role="status"><Icon name="flask" size={14} /><span>{desk.local().receipts[props.id]}</span></p>
  </Show>;
}
export function Skeleton(props: { w?: string; h?: string; class?: string }) {
  return <span class={`skeleton ${props.class ?? ""}`} style={{ width: props.w ?? "60%", height: props.h ?? "0.75rem" }} aria-hidden="true" />;
}
export function Loading(props: { label: string; rows?: number; h?: string }) {
  return <div class="loading-block" role="status" aria-label={props.label} tabindex="-1">
    <For each={Array.from({ length: props.rows ?? 4 })}>{(_, i) => <Skeleton w={`${90 - (i() % 3) * 18}%`} h={props.h ?? "1rem"} />}</For>
    <span class="sr-only">{props.label}</span>
  </div>;
}
export function ErrorState(props: { text: string; retry?: () => void }) {
  return <div class="state-block" role="alert" tabindex="-1">
    <Notice tone="danger" action={props.retry ? <Btn onClick={props.retry} icon="refresh">Try again</Btn> : undefined}>{props.text}</Notice>
  </div>;
}

/** Stable layout unit: 1rem in px, re-measured when the text size changes. */
export function useRemPx() {
  const [px, setPx] = createSignal(parseFloat(getComputedStyle(document.documentElement).fontSize) || 16);
  onMount(() => {
    const obs = new MutationObserver(() => setPx(parseFloat(getComputedStyle(document.documentElement).fontSize) || 16));
    obs.observe(document.documentElement, { attributes: true, attributeFilter: ["style"] });
    onCleanup(() => obs.disconnect());
  });
  return px;
}

let measureCtx: CanvasRenderingContext2D | null | undefined;
/**
 * Number of lines `text` wraps to at `widthPx` (capped at `max`), measured with a canvas in the UI font.
 * Used to give virtualised cards a content-sized height. Without a canvas (jsdom) it assumes one line.
 */
export function lineCount(text: string, widthPx: number, fontPx: number, max = 2) {
  if (measureCtx === undefined) {
    try { measureCtx = /jsdom/i.test(navigator.userAgent) ? null : document.createElement("canvas").getContext("2d"); } catch { measureCtx = null; }
  }
  const c = measureCtx;
  if (!c || widthPx <= 0) return 1;
  const family = getComputedStyle(document.documentElement).getPropertyValue("--font-ui").trim() || "system-ui, sans-serif";
  c.font = `400 ${fontPx}px ${family}`;
  const space = c.measureText(" ").width, limit = widthPx * 0.98;
  let lines = 1, cur = 0;
  for (const word of text.split(/\s+/).filter(Boolean)) {
    const w = c.measureText(word).width;
    if (cur === 0) cur = w;
    else if (cur + space + w <= limit) cur += space + w;
    else { lines++; cur = w; }
    while (cur > limit) { lines++; cur -= limit; }
    if (lines >= max) return max;
  }
  return lines;
}

export type VirtualApi = { el: HTMLDivElement; scrollToIndex(i: number): void };
/**
 * Virtual list. Rows have a fixed pitch (rem plus px, so it scales with the text-size preset) or, with `sizeOf`,
 * a per-item height in px computed from the list width. Only the rows in view plus overscan exist in the DOM;
 * `pin` keeps the roving-focus item rendered.
 */
export function VirtualList<T>(props: {
  items: readonly T[]; rem: number; px?: number; overscan?: number; class?: string; pin?: number;
  sizeOf?: (item: T, index: number, width: number, remPx: number) => number;
  label?: string; role?: JSX.HTMLAttributes<HTMLDivElement>["role"]; innerRole?: JSX.HTMLAttributes<HTMLDivElement>["role"];
  render: (item: T, index: number, ctx: { width: () => number; remPx: () => number }) => JSX.Element; api?: (api: VirtualApi) => void; onScroll?: (el: HTMLDivElement) => void;
  header?: JSX.Element; tabindex?: number; wrapRole?: JSX.HTMLAttributes<HTMLDivElement>["role"]; onKeyDown?: (e: KeyboardEvent) => void;
}) {
  let el!: HTMLDivElement;
  const remPx = useRemPx();
  const [top, setTop] = createSignal(0);
  const [viewH, setViewH] = createSignal(800);
  const [width, setWidth] = createSignal(0);
  const pitch = () => props.rem * remPx() + (props.px ?? 0);
  /** offsets[i] is the top of row i; offsets[n] is the total height. */
  const offsets = createMemo(() => {
    const n = props.items.length, out = new Float64Array(n + 1), size = props.sizeOf;
    if (size) { const w = width(), r = remPx(); for (let i = 0; i < n; i++) out[i + 1] = out[i] + size(props.items[i], i, w, r); }
    else { const h = pitch(); for (let i = 0; i < n; i++) out[i + 1] = (i + 1) * h; }
    return out;
  });
  const indexAt = (y: number) => {
    const off = offsets();
    let lo = 0, hi = Math.max(0, off.length - 2);
    while (lo < hi) { const mid = (lo + hi + 1) >> 1; if (off[mid] <= y) lo = mid; else hi = mid - 1; }
    return lo;
  };
  const range = createMemo(() => {
    const n = props.items.length, o = props.overscan ?? 4;
    const start = Math.max(0, indexAt(top()) - o), end = Math.min(n, indexAt(top() + viewH()) + 1 + o);
    const out: number[] = [];
    for (let i = start; i < end; i++) out.push(i);
    const pin = props.pin;
    if (pin !== undefined && pin >= 0 && pin < n && (pin < start || pin >= end)) { if (pin < start) out.unshift(pin); else out.push(pin); }
    return out;
  });
  const sync = () => { setTop(el.scrollTop); if (el.clientHeight) setViewH(el.clientHeight); if (el.clientWidth) setWidth(el.clientWidth); };
  onMount(() => {
    sync();
    if (typeof ResizeObserver !== "undefined") {
      const ro = new ResizeObserver(sync);
      ro.observe(el);
      onCleanup(() => ro.disconnect());
    }
    props.api?.({
      el,
      scrollToIndex(i) {
        const off = offsets(), t = off[i] ?? 0, b = off[i + 1] ?? t;
        if (t < el.scrollTop) el.scrollTop = t;
        else if (b > el.scrollTop + el.clientHeight) el.scrollTop = b - el.clientHeight;
        sync();
      },
    });
  });
  return <div class={`vlist ${props.class ?? ""}`} ref={el} role={props.role} aria-label={props.label} tabindex={props.tabindex}
    onScroll={() => { sync(); props.onScroll?.(el); }} onKeyDown={e => props.onKeyDown?.(e)}>
    {props.header}
    <div class="vinner" role={props.innerRole} style={{ height: `${offsets()[props.items.length]}px` }}>
      <For each={range()}>{i => <div class="vrow" role={props.wrapRole} style={{ transform: `translateY(${offsets()[i]}px)`, height: `${offsets()[i + 1] - offsets()[i]}px` }}>
        <Show when={props.items[i] !== undefined}>{props.render(props.items[i], i, { width, remPx })}</Show>
      </div>}</For>
    </div>
  </div>;
}

/** Non-modal popover anchored to its trigger. Esc closes and returns focus; leaving it with Tab or clicking outside closes it. */
export function Popover(props: {
  trigger: (p: { ref: (el: HTMLButtonElement) => void; onClick: () => void; "aria-expanded": boolean; "aria-haspopup": "menu" | "dialog" }) => JSX.Element;
  children: (close: (refocus?: boolean) => void) => JSX.Element; place?: "below-start" | "below-end" | "above-start" | "above-end";
  kind?: "menu" | "dialog"; label: string; width?: string; class?: string;
}) {
  const [open, setOpen] = createSignal(false);
  let trigger: HTMLButtonElement | undefined, wrap!: HTMLDivElement, panel: HTMLDivElement | undefined;
  const close = (refocus = true) => { setOpen(false); if (refocus) trigger?.focus(); };
  const outside = (e: PointerEvent) => { if (!wrap.contains(e.target as Node)) setOpen(false); };
  document.addEventListener("pointerdown", outside);
  onCleanup(() => document.removeEventListener("pointerdown", outside));
  return <div class={`pop-anchor ${props.class ?? ""}`} ref={wrap} onFocusOut={e => { if (open() && e.relatedTarget && !wrap.contains(e.relatedTarget as Node)) setOpen(false); }}>
    {props.trigger({ ref: el => (trigger = el), onClick: () => setOpen(!open()), "aria-expanded": open(), "aria-haspopup": props.kind ?? "menu" })}
    <Show when={open()}>
      <div class={`popover place-${props.place ?? "below-start"}`} ref={el => {
        panel = el;
        queueMicrotask(() => (panel?.querySelector<HTMLElement>("[aria-checked=true], [data-autofocus], button:not([disabled]), [href], select, input, [tabindex='0']"))?.focus());
      }} role={props.kind === "dialog" ? "dialog" : undefined} aria-label={props.label} style={{ width: props.width }}
        onKeyDown={e => { if (e.key === "Escape") { e.preventDefault(); e.stopPropagation(); close(); } }}>
        {props.children(close)}
      </div>
    </Show>
  </div>;
}

export type MenuItem = { label: string; icon?: IconName; hint?: string; checked?: boolean; disabled?: boolean; onSelect: () => void; section?: string; danger?: boolean };
/** Menu anatomy: icon / label / shortcut; ↑/↓/Home/End move focus; radio items expose aria-checked. */
export function Menu(props: { items: MenuItem[]; label: string; close: (refocus?: boolean) => void; radio?: boolean }) {
  let ul!: HTMLDivElement;
  const move = (e: KeyboardEvent) => {
    const items = [...ul.querySelectorAll<HTMLButtonElement>("[role^=menuitem]:not([disabled])")];
    const i = items.indexOf(document.activeElement as HTMLButtonElement);
    const next = e.key === "ArrowDown" ? items[(i + 1) % items.length] : e.key === "ArrowUp" ? items[(i - 1 + items.length) % items.length]
      : e.key === "Home" ? items[0] : e.key === "End" ? items[items.length - 1] : undefined;
    if (next) { e.preventDefault(); next.focus(); }
  };
  return <div class="menu" role="menu" aria-label={props.label} ref={ul} onKeyDown={move}>
    <For each={props.items}>{(item, i) => <>
      <Show when={item.section && (i() === 0 || props.items[i() - 1].section !== item.section)}><div class="menu-section" role="presentation">{item.section}</div></Show>
      <button type="button" class={`menu-item ${item.danger ? "danger" : ""}`} role={props.radio ? "menuitemradio" : "menuitem"} aria-checked={props.radio ? Boolean(item.checked) : undefined}
        disabled={item.disabled} tabindex="-1" onClick={() => { props.close(); item.onSelect(); }}>
        <span class="menu-icon">{props.radio ? (item.checked ? <Icon name="check" /> : null) : item.icon ? <Icon name={item.icon} /> : null}</span>
        <span class="menu-label">{item.label}</span>
        <Show when={item.hint}><kbd class="menu-hint">{item.hint}</kbd></Show>
      </button>
    </>}</For>
  </div>;
}

let openModals = 0;
/**
 * Native modal dialog (showModal): background inert, top layer, Esc fires cancel.
 * The app root also gets `inert` while any modal is open. Focus returns to the invoker on close.
 */
export function Modal(props: { open: boolean; onClose: () => void; label: string; class?: string; children: JSX.Element; initial?: string; describedby?: string }) {
  return <Show when={props.open}>
    {(() => {
      let dlg!: HTMLDialogElement;
      const invoker = document.activeElement as HTMLElement | null;
      onMount(() => {
        openModals++;
        document.getElementById("app")?.setAttribute("inert", "");
        if (typeof dlg.showModal === "function") { try { dlg.showModal(); } catch { dlg.setAttribute("open", ""); } } else dlg.setAttribute("open", "");
        const target = (props.initial ? dlg.querySelector<HTMLElement>(props.initial) : null)
          ?? dlg.querySelector<HTMLElement>("input:not([type=radio]), textarea, select, button:not([disabled])");
        target?.focus();
      });
      onCleanup(() => {
        openModals = Math.max(0, openModals - 1);
        if (openModals === 0) document.getElementById("app")?.removeAttribute("inert");
        if (dlg.open && typeof dlg.close === "function") dlg.close();
        queueMicrotask(() => {
          if (invoker && invoker.isConnected) invoker.focus();
          else document.querySelector<HTMLElement>("#main h1, #main [tabindex='-1']")?.focus();
        });
      });
      return <Portal>
        <dialog ref={dlg} class={`modal ${props.class ?? ""}`} aria-label={props.label} aria-describedby={props.describedby}
          onCancel={e => { e.preventDefault(); props.onClose(); }}
          onKeyDown={e => { if (e.key === "Escape" && typeof dlg.showModal !== "function") { e.preventDefault(); props.onClose(); } }}>
          {props.children}
        </dialog>
      </Portal>;
    })()}
  </Show>;
}

export function Kbd(props: { children: string }) { return <kbd class="kbd">{props.children}</kbd>; }

/** Tab strips never truncate: when a strip overflows, its trailing edge fades and a menu lists every tab. */
export function TabOverflow(props: { strip: () => HTMLElement | undefined; label: string; items: { label: string; href: string; selected: boolean }[] }) {
  const desk = useDesk();
  const [over, setOver] = createSignal(false);
  onMount(() => {
    const el = props.strip();
    if (!el || typeof ResizeObserver === "undefined") return;
    const check = () => { const o = el.scrollWidth > el.clientWidth + 1; setOver(o); el.classList.toggle("overflowing", o); };
    const ro = new ResizeObserver(check);
    ro.observe(el);
    check();
    onCleanup(() => ro.disconnect());
  });
  return <Show when={over()}>
    <Popover label={props.label} place="below-end" trigger={p => <IconBtn {...p} icon="chevronDown" label={props.label} class="sm tab-more" />}>
      {close => <Menu label={props.label} radio close={close} items={props.items.map(i => ({ label: i.label, checked: i.selected, onSelect: () => desk.go(i.href) }))} />}
    </Popover>
  </Show>;
}

/**
 * Select-like menu button (Paseo-style): 32px trigger with a chevron, opening the standard menu with radio items.
 * Replaces the native <select>, whose filled triangle and OS popup read as a web form.
 */
export function SelectMenu<T extends string>(props: {
  id: string; labelId: string; label: string; value: T; options: { value: T; label: string }[]; onChange: (v: T) => void; autofocus?: boolean; describedby?: string;
}) {
  const current = () => props.options.find(o => o.value === props.value);
  return <Popover label={props.label} place="below-start" class="select-anchor" width="100%" trigger={p => <button type="button" {...p} id={props.id} class="select-btn"
    aria-labelledby={`${props.labelId} ${props.id}`} aria-describedby={props.describedby} data-autofocus={props.autofocus ? "" : undefined}>
    <span class="select-value">{current()?.label ?? "—"}</span><Icon name="chevronDown" class="select-chev" />
  </button>}>
    {close => <div class="select-menu"><Menu label={props.label} radio close={close} items={props.options.map(o => ({ label: o.label, checked: o.value === props.value, onSelect: () => props.onChange(o.value) }))} /></div>}
  </Popover>;
}

/** Disabled reason line, always visible (never hover-only). */
export function Reason(props: { text: string | null | undefined; icon?: IconName; id?: string }) {
  return <Show when={props.text}><p class="reason" id={props.id}><Icon name={props.icon ?? "info"} size={14} /><span>{props.text}</span></p></Show>;
}
