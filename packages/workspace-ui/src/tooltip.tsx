import { createUniqueId, onCleanup, onMount } from "solid-js";

export const TOOLTIP_DELAY = 500;
const TRIGGER = "[data-tooltip], [data-whole], [data-tooltip-cut]";
type Rect = Pick<DOMRect, "left" | "right" | "top" | "bottom">;
type Point = { x: number; y: number };
const MARGIN = 8;
const clamp = (n: number, size: number, end: number) => Math.max(MARGIN, Math.min(n, end - MARGIN - size));

/** Pointer tooltips sit just below-right of the cursor; at an edge they flip to the other side so the hotspot stays uncovered. */
export function pointerPlacement(at: Point, width: number, height: number, vw: number, vh: number) {
  const dx = 12, dy = 16;
  const left = at.x + dx + width > vw - MARGIN ? at.x - dx - width : at.x + dx;
  const top = at.y + dy + height > vh - MARGIN ? at.y - 8 - height : at.y + dy;
  return { left: clamp(left, width, vw), top: clamp(top, height, vh) };
}

/** Keyboard tooltips stay at the element's start edge, below it (above when there is no room). */
export function anchorPlacement(anchor: Rect, width: number, height: number, vw: number, vh: number) {
  const gap = 6;
  const below = anchor.bottom + gap;
  const top = below + height > vh - MARGIN ? anchor.top - gap - height : below;
  return { left: clamp(anchor.left, width, vw), top: clamp(top, height, vh) };
}

const cutText = (el: Element) => {
  if (el.scrollWidth <= el.clientWidth + 1 && el.scrollHeight <= el.clientHeight + 1) return "";
  return (el.getAttribute("data-whole") || el.textContent || "").trim();
};
const textFor = (el: Element) => {
  const explicit = el.getAttribute("data-tooltip");
  return (explicit === null ? cutText(el) : explicit || el.getAttribute("aria-label") || "").trim();
};
const targetOf = (target: EventTarget | null) => target instanceof Element ? target.closest(TRIGGER) : null;

/** One desktop layer. Triggers opt in with data-tooltip (empty uses the accessible name), or mark potentially cut text with data-whole. */
export function Tooltip() {
  const id = `ws-tooltip-${createUniqueId()}`;
  let home!: HTMLDivElement, tip!: HTMLDivElement;

  onMount(() => {
    let timer: ReturnType<typeof setTimeout> | undefined;
    let trigger: Element | null = null, owner: Element | null = null;
    // Keyboard focus is tracked apart from the pointer so leaving a hovered trigger never drops the focused one's tip.
    let focused: Element | null = null;
    // Latest pointer position for a hover-started tip; null when focus started it.
    let point: Point | null = null;
    const hide = () => {
      clearTimeout(timer);
      timer = undefined;
      trigger = null;
      focused = null;
      point = null;
      if (!owner) return;
      const ids = (owner.getAttribute("aria-describedby") ?? "").split(/\s+/).filter(part => part && part !== id);
      if (ids.length) owner.setAttribute("aria-describedby", ids.join(" "));
      else owner.removeAttribute("aria-describedby");
      owner = null;
      tip.hidePopover?.();
      tip.hidden = true;
      home.append(tip);
    };
    const start = (el: Element, described: Element = el, force = false, at: Point | null = null) => {
      if (trigger === el && !force) return;
      hide();
      if (el.closest('[data-shell="phone"], [aria-hidden="true"]') || !textFor(el)) return;
      trigger = el;
      point = at;
      timer = setTimeout(() => {
        const message = textFor(el);
        if (!el.isConnected || !described.isConnected || !message) return hide();
        owner = described;
        // A modal makes everything outside it inert, including descriptions.
        (el.closest("dialog[open]") ?? home).append(tip);
        tip.textContent = message;
        tip.hidden = false;
        tip.style.visibility = "hidden";
        tip.showPopover?.();
        const rect = tip.getBoundingClientRect();
        const at = point && focused !== el ? pointerPlacement(point, rect.width, rect.height, innerWidth, innerHeight)
          : anchorPlacement(el.getBoundingClientRect(), rect.width, rect.height, innerWidth, innerHeight);
        tip.style.left = `${at.left}px`;
        tip.style.top = `${at.top}px`;
        const ids = owner.getAttribute("aria-describedby");
        owner.setAttribute("aria-describedby", ids ? `${ids} ${id}` : id);
        tip.style.visibility = "";
      }, TOOLTIP_DELAY);
    };
    const hover = (e: PointerEvent) => {
      if ((trigger && trigger === focused) || e.pointerType === "touch" ||
        window.matchMedia?.("(hover: hover) and (pointer: fine)")?.matches === false) return;
      const el = targetOf(e.target);
      if (el && el !== targetOf(e.relatedTarget)) start(el, el, false, { x: e.clientX, y: e.clientY });
    };
    const move = (e: PointerEvent) => {
      if (point && trigger && trigger !== focused && e.pointerType !== "touch") point = { x: e.clientX, y: e.clientY };
    };
    const leave = (e: PointerEvent) => {
      if (trigger && trigger !== focused && e.pointerType !== "touch" &&
        targetOf(e.target) === trigger && targetOf(e.relatedTarget) !== trigger) hide();
    };
    const focus = (e: FocusEvent) => {
      const el = e.target;
      if (!(el instanceof Element) || !document.documentElement.hasAttribute("data-keyboard")) return;
      // A disabled reason is more useful than the action's hint; keep its existing keyboard box unobscured.
      if (el.matches('[aria-disabled="true"][data-why]')) return;
      const target = targetOf(el) ?? [...el.querySelectorAll(TRIGGER)].find(item => textFor(item));
      if (target) {
        // Keep the hover timer or visible tip when it already describes the focused element.
        start(target, el, target !== el);
        if (trigger === target) focused = target;
      }
    };
    const escape = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      const visible = !!owner;
      hide();
      if (visible) { e.preventDefault(); e.stopImmediatePropagation(); }
    };
    const changes = new MutationObserver(() => {
      if (trigger && (!trigger.isConnected || (owner && textFor(trigger) !== tip.textContent))) hide();
    });
    changes.observe(document.body, { subtree: true, childList: true, characterData: true, attributes: true,
      attributeFilter: ["data-tooltip", "data-whole", "aria-label", "aria-hidden"] });
    const events = new AbortController();
    const opts = { capture: true, signal: events.signal };
    document.addEventListener("pointerover", hover, opts);
    document.addEventListener("pointermove", move, opts);
    document.addEventListener("pointerout", leave, opts);
    document.addEventListener("focusin", focus, opts);
    document.addEventListener("focusout", e => {
      if (focused || (trigger && e.target instanceof Element && e.target.contains(trigger))) hide();
    }, opts);
    for (const event of ["pointerdown", "click", "scroll", "wheel", "resize"]) window.addEventListener(event, hide, opts);
    // Element blur is handled by focusout; only loss of window focus dismisses globally.
    window.addEventListener("blur", hide, { signal: events.signal });
    window.addEventListener("keydown", escape, opts);
    onCleanup(() => { events.abort(); changes.disconnect(); hide(); });
  });

  return <div ref={home} class="ws-tooltip-host"><div ref={tip} id={id} class="ws-tooltip" role="tooltip" popover="manual" hidden /></div>;
}
