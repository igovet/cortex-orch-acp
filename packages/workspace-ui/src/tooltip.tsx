import { createUniqueId, onCleanup, onMount } from "solid-js";

export const TOOLTIP_DELAY = 500;
const TRIGGER = "[data-tooltip], [data-whole], [data-tooltip-cut]";
const CONTROL = "button, a[href], input, textarea, select, [tabindex], [contenteditable='true']";
type Rect = Pick<DOMRect, "left" | "right" | "top" | "bottom">;
const overlaps = (a: Rect, b: Rect) => a.left < b.right && a.right > b.left && a.top < b.bottom && a.bottom > b.top;

export function tooltipPlacement(anchor: Rect, width: number, height: number, vw: number, vh: number, controls: Rect[]) {
  const margin = 8, gap = 6;
  if (width > vw - 2 * margin || height > vh - 2 * margin) return null;
  const shift = (n: number, end: number) => Math.min(Math.max(n, margin), end - margin);
  const preferredX = shift(anchor.left, vw - width), preferredY = shift(anchor.top, vh - height);
  const candidates = [
    ...[anchor.bottom + gap, anchor.top - gap - height].flatMap((top, side) => {
      if (top < margin || top + height > vh - margin) return [];
      const xs = [preferredX, ...controls.filter(r => r.top < top + height && r.bottom > top)
        .flatMap(r => [r.left - width - 4, r.right + 4])];
      return xs.map(x => {
        const left = shift(x, vw - width);
        return { left, top, score: Math.abs(left - preferredX) + side };
      });
    }),
    ...[anchor.right + gap, anchor.left - gap - width].flatMap((left, side) => {
      if (left < margin || left + width > vw - margin) return [];
      const ys = [preferredY, ...controls.filter(r => r.left < left + width && r.right > left)
        .flatMap(r => [r.top - height - 4, r.bottom + 4])];
      return ys.map(y => {
        const top = shift(y, vh - height);
        return { left, top, score: Math.abs(top - preferredY) + 16 + side };
      });
    }),
  ].sort((a, b) => a.score - b.score);
  const at = candidates.find(({ left, top }) => {
    const rect = { left, top, right: left + width, bottom: top + height };
    return !overlaps(rect, anchor) && !controls.some(control => overlaps(rect, control));
  });
  return at ? { left: at.left, top: at.top } : null;
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
    const hide = () => {
      clearTimeout(timer);
      timer = undefined;
      trigger = null;
      focused = null;
      if (!owner) return;
      const ids = (owner.getAttribute("aria-describedby") ?? "").split(/\s+/).filter(part => part && part !== id);
      if (ids.length) owner.setAttribute("aria-describedby", ids.join(" "));
      else owner.removeAttribute("aria-describedby");
      owner = null;
      tip.hidePopover?.();
      tip.hidden = true;
      home.append(tip);
    };
    const start = (el: Element, described: Element = el, force = false) => {
      if (trigger === el && !force) return;
      hide();
      if (el.closest('[data-shell="phone"], [aria-hidden="true"]') || !textFor(el)) return;
      trigger = el;
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
        const anchor = el.getBoundingClientRect(), rect = tip.getBoundingClientRect();
        const controls = [...document.querySelectorAll(CONTROL)]
          .filter(control => !control.contains(el) && !el.contains(control) && !control.closest("[inert]"))
          .map(control => control.getBoundingClientRect()).filter(box => box.width > 0 && box.height > 0);
        // A cut label within a button must not cover that button either.
        if (described !== el) controls.push(described.getBoundingClientRect());
        else {
          const parent = el.parentElement?.closest(CONTROL);
          if (parent) controls.push(parent.getBoundingClientRect());
        }
        const at = tooltipPlacement(anchor, rect.width, rect.height, innerWidth, innerHeight, controls);
        if (!at) return hide();
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
      if (el && el !== targetOf(e.relatedTarget)) start(el);
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
