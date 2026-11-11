import { children, createSignal, createUniqueId, For, onCleanup, onMount, Show, splitProps, type Accessor, type JSX } from "solid-js";
import { Count } from "./kit";

/*
 * The layout primitives: the only sanctioned way to build bars, headings, rows, action groups, dialog footers and bottom bars.
 * Their geometry lives in layout.css; a reason never changes a control's box, and test/layout-fit.mjs proves it in a real engine.
 */

const cx = (...names: (string | false | undefined)[]) => names.filter(Boolean).join(" ");
const DISABLED = '[aria-disabled="true"][data-why]';
const reasonOf = (el: Element) => el.getAttribute("data-why") ?? "";
const isCut = (el: Element) => el.scrollWidth > el.clientWidth + 1 || el.scrollHeight > el.clientHeight + 1;
/** Enter and Space press a cut box as a click does, as Row's do, so the keyboard opens what a press opens. */
const pressByKey = (e: KeyboardEvent) => {
  const el = e.target as HTMLElement;
  if (e.defaultPrevented || (e.key !== "Enter" && e.key !== " ") || !el.matches?.("[data-whole][role='button']") || el.matches("a, button")) return;
  e.preventDefault();
  el.click();
};

/**
 * For a ref: a box whose text may be cut. While it is cut it can be reached by keyboard, and a press, Enter, Space or keyboard focus opens
 * the whole text beside it (ReasonTip). Never a hover. `data-whole` holds the whole text, or is empty to use what is shown.
 */
export function revealWhenCut(el: HTMLElement) {
  const mark = () => { if (isCut(el)) { el.tabIndex = 0; el.setAttribute("role", "button"); } else { el.removeAttribute("tabindex"); el.removeAttribute("role"); } };
  onMount(() => {
    mark();
    el.addEventListener("keydown", pressByKey);
    onCleanup(() => el.removeEventListener("keydown", pressByKey));
    if (typeof ResizeObserver === "undefined") return;
    const size = new ResizeObserver(mark), text = new MutationObserver(mark);
    size.observe(el); text.observe(el, { subtree: true, childList: true, characterData: true });
    onCleanup(() => { size.disconnect(); text.disconnect(); });
  });
}

/**
 * For a ref on a band or a panel that lists names: each name or value in it that is cut (`.trunc`, `.cur`, a `dd`) and is not
 * already part of a link or a button becomes a box a press, or keyboard focus, opens whole, as with revealWhenCut.
 */
export function revealCutIn(box: HTMLElement) {
  const mark = () => {
    for (const el of box.querySelectorAll<HTMLElement>(".trunc, .cur, dd")) {
      if (el.parentElement?.closest("a, button, [role='button']") || el.matches("a, button")) continue;
      if (isCut(el)) { el.setAttribute("data-whole", ""); el.tabIndex = 0; el.setAttribute("role", "button"); }
      else if (el.hasAttribute("data-whole")) { el.removeAttribute("data-whole"); el.removeAttribute("tabindex"); el.removeAttribute("role"); }
    }
  };
  onMount(() => {
    mark();
    box.addEventListener("keydown", pressByKey);
    onCleanup(() => box.removeEventListener("keydown", pressByKey));
    if (typeof ResizeObserver === "undefined") return;
    let queued = false;
    const later = () => { if (!queued) { queued = true; requestAnimationFrame(() => { queued = false; mark(); }); } };
    const size = new ResizeObserver(later), text = new MutationObserver(later);
    size.observe(box); text.observe(box, { subtree: true, childList: true, characterData: true });
    onCleanup(() => { size.disconnect(); text.disconnect(); });
  });
}

/** One line of text that may be cut, such as a path or a branch: a press shows the whole of it. */
export function Whole(props: { text: string; class?: string }) {
  return <span ref={revealWhenCut} class={cx("l-whole", props.class)} data-whole={props.text}>{props.text}</span>;
}

/** Head, status lane, body and foot. Only the body scrolls, unless it is laid out as Regions (`scroll="none"`). */
export function Page(props: { head?: JSX.Element; status?: JSX.Element; foot?: JSX.Element; children: JSX.Element; scroll?: "vertical" | "sideways" | "none"; class?: string }) {
  return <div class={cx("l-page", props.class)}>
    <div class="l-page-head">{props.head}</div>
    <div class="l-page-status">{props.status}</div>
    <div class="l-page-body" data-scroll={props.scroll ?? "vertical"}>{props.children}</div>
    <div class="l-page-foot">{props.foot}</div>
  </div>;
}

/** One fixed-height line in three zones. It has no room for a reason: that goes to the Strip below it, or into a menu row. */
export function Toolbar(props: { start?: JSX.Element; mid?: JSX.Element; end?: JSX.Element; small?: boolean; class?: string; label?: string }) {
  return <div class={cx("l-toolbar", props.class)} data-bar={props.small ? "toolbar-sm" : "toolbar"} role={props.label ? "group" : undefined} aria-label={props.label}>
    <div class="l-z-start">{props.start}</div>
    <div class="l-z-mid">{props.mid}</div>
    <div class="l-z-end">{props.end}</div>
  </div>;
}

/**
 * Says why a control in its bar cannot be used, reading the declaration the control already carries (`data-why`, set by hand
 * or by `ws.needs`). The primary's reason shows first; focusing or pressing another swaps the words and moves nothing.
 */
export function ReasonSlot(props: { bar: Accessor<HTMLElement | undefined>; hint?: JSX.Element }) {
  const id = createUniqueId();
  const [reason, setReason] = createSignal<string | null>(null);
  let picked: Element | null = null;
  const described = new Set<Element>();
  const read = () => {
    const bar = props.bar();
    if (!bar) return;
    // A control that a strip beside the bar already explains, with its way out, is left to that strip.
    const controls = [...bar.querySelectorAll(DISABLED)].filter(c => { const by = c.getAttribute("aria-describedby"); return !by || by === id; });
    // The primary's reason first; in a composer the paper plane is its primary.
    const shown = (picked && controls.includes(picked) ? picked : controls.find(c => c.matches(".btn-primary, .btn-danger, .mc-send")) ?? controls[0]) ?? null;
    setReason(shown ? reasonOf(shown) : null);
    for (const c of controls) if (!c.getAttribute("aria-describedby")) { c.setAttribute("aria-describedby", id); described.add(c); }
    for (const c of described) if (!controls.includes(c)) { if (c.getAttribute("aria-describedby") === id) c.removeAttribute("aria-describedby"); described.delete(c); }
  };
  const pick = (e: Event) => { const el = (e.target as Element).closest?.(DISABLED); if (el) { picked = el; read(); } };
  const leave = (e: FocusEvent) => { if (!props.bar()?.contains(e.relatedTarget as Node | null)) { picked = null; read(); } };
  onMount(() => {
    const bar = props.bar()!;
    read();
    const watch = new MutationObserver(read);
    watch.observe(bar, { subtree: true, childList: true, attributes: true, attributeFilter: ["aria-disabled", "data-why", "aria-describedby"] });
    bar.addEventListener("focusin", pick); bar.addEventListener("click", pick); bar.addEventListener("focusout", leave);
    onCleanup(() => { watch.disconnect(); bar.removeEventListener("focusin", pick); bar.removeEventListener("click", pick); bar.removeEventListener("focusout", leave); });
  });
  return <div id={id} ref={revealWhenCut} class="l-reason" data-whole="">{reason() ?? props.hint}</div>;
}

/** Left group, reason slot, actions: the one component for a card's actions, a page's bottom bar and a dialog's footer. */
export function ActionBar(props: { place?: "page" | "dialog" | "card"; start?: JSX.Element; hint?: JSX.Element; children: JSX.Element; class?: string; label?: string }) {
  let bar: HTMLDivElement | undefined;
  return <div ref={bar} class={cx("l-bar", props.class)} data-bar={props.place === "dialog" ? "dialog" : props.place === "card" ? "card" : "action"} role={props.label ? "group" : undefined} aria-label={props.label}>
    <div class="l-bar-start">{props.start}</div>
    <ReasonSlot bar={() => bar} hint={props.hint} />
    <div class="l-bar-actions">{props.children}</div>
  </div>;
}

type ActionProps = Omit<JSX.ButtonHTMLAttributes<HTMLButtonElement>, "onClick"> & {
  kind?: "primary" | "line" | "danger" | "plain" | "icon"; why?: string; steady?: boolean;
  onClick?: (e: MouseEvent & { currentTarget: HTMLButtonElement; target: Element }) => void; "aria-disabled"?: "true"; "data-why"?: string;
};
/** A button with one box in every state. A reason keeps it focusable and describes it; a press then does nothing. `steady` holds its width when its label swaps. Spread `ws.needs(...)` as before. */
export function Action(props: ActionProps) {
  const [own, rest] = splitProps(props, ["kind", "why", "steady", "class", "onClick", "aria-disabled", "data-why", "children"]);
  const why = () => own.why ?? own["data-why"];
  const look = () => own.kind === "icon" ? "ib" : cx("btn", own.kind && own.kind !== "plain" && `btn-${own.kind}`);
  return <button type="button" {...rest} class={cx("l-action", look(), own.steady && "l-act-slot", own.class)} aria-disabled={why() ? "true" : own["aria-disabled"]} data-why={why()}
    onClick={e => { if (e.currentTarget.getAttribute("aria-disabled") !== "true") own.onClick?.(e); }}>{own.children}</button>;
}

/**
 * The only heading inside a panel. Its side inset equals its rows'; a count and any actions sit at its end, and `lead`, such
 * as a panel toggle, before the title. `id` names the whole head, count included, for aria-labelledby.
 */
export function SectionHead(props: { title: JSX.Element; count?: number; lead?: JSX.Element; children?: JSX.Element; class?: string; id?: string }) {
  return <div id={props.id} class={cx("l-sect", props.class)}>
    <Show when={props.lead}><div class="l-sect-lead">{props.lead}</div></Show>
    <h3 class="l-sect-t"><span ref={revealWhenCut} class="l-whole" data-whole="">{props.title}</span></h3>
    <Show when={props.count !== undefined}><Count n={props.count!} /></Show>
    <div class="l-sect-end">{props.children}</div>
  </div>;
}

export type StripTone = "fail" | "warn" | "info" | "ok";
/** `domId` names the strip for a control it describes, through aria-describedby. */
export type StripItem = { id: string; tone: StripTone; icon?: JSX.Element; text: JSX.Element; action?: JSX.Element; role?: "status" | "alert"; domId?: string };

/** A notice of at most two lines with its one action pinned right. */
export function Strip(props: { tone?: StripTone; icon?: JSX.Element; action?: JSX.Element; children: JSX.Element; id?: string; role?: "status" | "alert"; class?: string }) {
  return <div id={props.id} class={cx("l-strip", props.class)} data-tone={props.tone ?? "info"} role={props.role}>
    <span class="l-strip-i">{props.icon}</span>
    <div ref={revealWhenCut} class="l-strip-t" data-whole="">{props.children}</div>
    <div class="l-strip-a">{props.action}</div>
  </div>;
}

const TONE_ORDER: Record<StripTone, number> = { fail: 0, warn: 1, info: 2, ok: 3 };
/** Under a page head: the two most urgent strips, and the rest folded behind a count. */
export function StatusLane(props: { items: readonly StripItem[] }) {
  const [open, setOpen] = createSignal(false);
  const ordered = () => [...props.items].sort((a, b) => TONE_ORDER[a.tone] - TONE_ORDER[b.tone]);
  const shown = () => open() ? ordered() : ordered().slice(0, 2);
  const folded = () => ordered().length - 2;
  return <div class="l-lane" data-open={open() ? "" : undefined}>
    <For each={shown()}>{item => <Strip id={item.domId} role={item.role} tone={item.tone} icon={item.icon} action={item.action}>{item.text}</Strip>}</For>
    <Show when={folded() > 0}><button class="lnk l-lane-more" aria-expanded={open()} onClick={() => setOpen(v => !v)}>{open() ? "Show less" : `${folded()} more`}</button></Show>
  </div>;
}

/**
 * Lead, one-line title, up to two lines of detail and a right-aligned end. A cut title or detail opens with a click and shows
 * the whole row; `line` fixes a single-line height.
 */
export function Row(props: { lead?: JSX.Element; title: JSX.Element; detail?: JSX.Element; end?: JSX.Element; line?: boolean; class?: string }) {
  const detail = children(() => props.detail);
  const [open, setOpen] = createSignal(false);
  const [cut, setCut] = createSignal(false);
  const [cutTitle, setCutTitle] = createSignal(false);
  let box: HTMLDivElement | undefined, head: HTMLDivElement | undefined;
  // While open nothing is cut, so the last measure stands until it closes again.
  const measure = () => { if (open()) return; if (box) setCut(box.scrollHeight > box.clientHeight + 1); if (head) setCutTitle(head.scrollWidth > head.clientWidth + 1); };
  const toggle = () => { if (cut() || cutTitle() || open()) setOpen(v => !v); };
  const key = (e: KeyboardEvent) => { if (e.key === "Enter" || e.key === " ") { e.preventDefault(); toggle(); } };
  onMount(() => {
    measure();
    // A face that arrives after the first measure, or new words in the same box, change how much text fits without resizing it.
    const fonts = typeof document !== "undefined" ? document.fonts : undefined;
    let live = true;
    const remeasure = () => { if (live) measure(); };
    fonts?.addEventListener?.("loadingdone", remeasure);
    void fonts?.ready?.then(remeasure);
    onCleanup(() => { live = false; fonts?.removeEventListener?.("loadingdone", remeasure); });
    if (typeof MutationObserver !== "undefined" && head) {
      const words = new MutationObserver(measure);
      words.observe(head, { childList: true, characterData: true, subtree: true });
      onCleanup(() => words.disconnect());
    }
    if (typeof ResizeObserver === "undefined") return;
    const watch = new ResizeObserver(measure);
    if (box) watch.observe(box);
    if (head) watch.observe(head);
    onCleanup(() => watch.disconnect());
  });
  return <div class={cx("l-row", props.line && "l-row-line", props.class)} data-open={open() ? "" : undefined}>
    <span class="l-row-lead">{props.lead}</span>
    <div ref={head} class={cutTitle() ? "l-row-t ws-pointer" : "l-row-t"} role={cutTitle() ? "button" : undefined} tabindex={cutTitle() ? 0 : undefined} aria-expanded={cutTitle() ? open() : undefined}
      onClick={() => { if (cutTitle()) toggle(); }} onKeyDown={e => { if (cutTitle()) key(e); }}>{props.title}</div>
    <Show when={detail()}>
      <div ref={box} class={cut() ? "l-row-d ws-pointer" : "l-row-d"} data-open={open() ? "" : undefined} role={cut() ? "button" : undefined} tabindex={cut() ? 0 : undefined} aria-expanded={cut() ? open() : undefined}
        onClick={() => { if (cut()) toggle(); }} onKeyDown={e => { if (cut()) key(e); }}>{detail()}</div>
    </Show>
    <div class="l-row-end">{props.end}</div>
  </div>;
}

/** The only inner scroller, for when something beside it must stay in view. */
export function Region(props: JSX.HTMLAttributes<HTMLDivElement>) {
  const [own, rest] = splitProps(props, ["class", "children"]);
  return <div {...rest} class={cx("l-region", own.class)}>{own.children}</div>;
}

/**
 * The whole reason for a control that has no slot showing all of it: a press, or moving to it by keyboard, opens it beside the
 * control. It never reacts to hover, never takes room from anything, and does not take focus.
 */
export function ReasonTip() {
  const TARGET = `${DISABLED}, [data-whole]`;
  let tip!: HTMLDivElement, owner: HTMLElement | null = null;
  const [text, setText] = createSignal("");
  const hide = () => { if (!owner) return; owner = null; if (typeof tip.hidePopover === "function") tip.hidePopover(); };
  const shownInFull = (el: Element, why: string) => {
    const described = document.getElementById(el.getAttribute("aria-describedby") ?? "");
    // A strip says more than the reason, around it; a slot holds exactly the reason.
    const slot = described?.querySelector(".l-strip-t") ?? described;
    return !!slot && !!slot.textContent?.includes(why) && slot.scrollHeight <= slot.clientHeight + 1;
  };
  const show = (el: HTMLElement) => {
    let why: string;
    if (el.matches(DISABLED)) {
      why = reasonOf(el);
      // A menu row already carries its reason as its second line.
      if (el.matches(".mi") || !why || shownInFull(el, why)) { hide(); return; }
    } else {
      // Text that is cut: the whole of it, and nothing when all of it is already in view.
      why = isCut(el) ? (el.getAttribute("data-whole") || el.textContent || "").trim() : "";
      if (!why) { hide(); return; }
    }
    setText(why);
    if (owner === null && typeof tip.showPopover === "function") tip.showPopover();
    owner = el;
    const box = el.getBoundingClientRect(), s = tip.style;
    s.inset = "auto";
    s.left = `${Math.max(8, Math.min(box.left, innerWidth - tip.offsetWidth - 8))}px`;
    s.top = `${box.bottom + 4 + tip.offsetHeight > innerHeight - 8 ? Math.max(8, box.top - 4 - tip.offsetHeight) : box.bottom + 4}px`;
  };
  const target = (e: Event) => (e.target as Element | null)?.closest?.<HTMLElement>(TARGET) ?? null;
  // On the window, so a bar's own slot has already taken this press into account.
  const onClick = (e: MouseEvent) => { const el = target(e); if (el) show(el); };
  const onFocus = (e: FocusEvent) => { const el = target(e); if (el && document.documentElement.hasAttribute("data-keyboard")) show(el); };
  const onBlur = (e: FocusEvent) => { if (owner && e.target === owner) hide(); };
  const onPress = (e: Event) => { if (owner && !owner.contains(e.target as Node)) hide(); };
  const onKey = (e: KeyboardEvent) => { if (e.key === "Escape") hide(); };
  onMount(() => {
    window.addEventListener("click", onClick);
    document.addEventListener("focusin", onFocus);
    document.addEventListener("focusout", onBlur);
    window.addEventListener("pointerdown", onPress, true);
    window.addEventListener("scroll", hide, true);
    window.addEventListener("resize", hide);
    window.addEventListener("keydown", onKey, true);
    onCleanup(() => {
      window.removeEventListener("click", onClick);
      document.removeEventListener("focusin", onFocus);
      document.removeEventListener("focusout", onBlur);
      window.removeEventListener("pointerdown", onPress, true);
      window.removeEventListener("scroll", hide, true);
      window.removeEventListener("resize", hide);
      window.removeEventListener("keydown", onKey, true);
    });
  });
  return <div ref={tip} class="l-why-tip" popover="manual" role="status">{text()}</div>;
}
