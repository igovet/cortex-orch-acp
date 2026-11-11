/**
 * The one loading system. Whatever a click or Enter starts, the control that was pressed shows a loader and takes no second
 * press until the work ends. It needs nothing from the control: every action goes through the store, and the store hands
 * its promise here, so a new button is covered the moment it calls an action.
 */
const CONTROL = "button, a.btn, [role=\"menuitem\"], [role=\"tab\"]";
const SUBMIT = ".c-send:not([aria-disabled=\"true\"]), .dlg-f .btn-primary, .dlg-f .btn-danger";

export type Press = {
  /**
   * Holds the pressed control, if any, until the work settles. A multi-step action wraps its whole sequence here, and may
   * name its control, so a keyboard shortcut or a later step never leaves the button free for a second press.
   */
  track: <T>(work: Promise<T>, control?: HTMLElement | null) => Promise<T>;
  /** Names the control a menu opened from, for a menu shared by many triggers. */
  invokedBy: (menu: HTMLElement, trigger: HTMLElement) => void;
  dispose: () => void;
};

export function createPress(): Press {
  const held = new WeakMap<HTMLElement, number>(), invokers = new WeakMap<HTMLElement, HTMLElement>();
  const disabled = new WeakMap<HTMLButtonElement, boolean>();
  let pressed: HTMLElement | null = null, forget: ReturnType<typeof setTimeout> | undefined;

  /** A menu closes as it acts, so what stays on screen is the control that opened it. */
  const visible = (el: HTMLElement) => {
    const menu = el.closest<HTMLElement>("[popover]");
    if (!menu) return el;
    return invokers.get(menu) ?? (menu.closest(".ws-pane") ?? document).querySelector<HTMLElement>(`[popovertarget="${menu.id}"]`) ?? el;
  };
  const remember = (el: HTMLElement | null) => {
    pressed = el && visible(el);
    clearTimeout(forget);
    // A press lasts for the handlers it runs, so nothing started later can be pinned on it.
    forget = setTimeout(() => { pressed = null; });
  };
  const onClick = (e: Event) => {
    const el = (e.target as Element | null)?.closest?.<HTMLElement>(CONTROL) ?? null;
    if (el?.hasAttribute("data-busy")) { e.preventDefault(); e.stopImmediatePropagation(); return; }
    remember(el);
  };
  /** Enter in a field presses that field's send button. */
  const onKey = (e: KeyboardEvent) => {
    if (e.key !== "Enter" || e.shiftKey || e.isComposing) return;
    const field = e.target as HTMLElement | null;
    if (!field?.matches?.("textarea, input")) return;
    const button = field.closest(".composer, dialog")?.querySelector<HTMLElement>(SUBMIT) ?? null;
    if (button?.hasAttribute("data-busy")) { e.preventDefault(); e.stopImmediatePropagation(); return; }
    remember(button);
  };
  window.addEventListener("click", onClick, true);
  window.addEventListener("keydown", onKey, true);

  const hold = (el: HTMLElement) => {
    const n = held.get(el) ?? 0;
    held.set(el, n + 1);
    if (!n) {
      el.setAttribute("data-busy", ""); el.setAttribute("aria-busy", "true");
      if (el instanceof HTMLButtonElement) { disabled.set(el, el.disabled); el.disabled = true; }
    }
  };
  const release = (el: HTMLElement) => {
    const n = (held.get(el) ?? 1) - 1;
    held.set(el, n);
    if (!n) {
      el.removeAttribute("data-busy"); el.removeAttribute("aria-busy");
      if (el instanceof HTMLButtonElement) { el.disabled = disabled.get(el) ?? false; disabled.delete(el); }
    }
  };
  return {
    track(work, control) {
      const el = control ?? pressed;
      if (!el) return work;
      hold(el);
      const done = () => release(el);
      work.then(done, done);
      return work;
    },
    invokedBy: (menu, trigger) => { invokers.set(menu, trigger); },
    dispose() {
      clearTimeout(forget);
      window.removeEventListener("click", onClick, true);
      window.removeEventListener("keydown", onKey, true);
    },
  };
}
