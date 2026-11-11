import { createEffect, onCleanup, onMount } from "solid-js";

/** Keeps a textarea as tall as its text, from the height its stylesheet gives it up to the `textarea[data-grow]` max-height, and scrolling only past that. Measured, because WebKitGTK has no `field-sizing: content`. */
export function autoGrow(el: HTMLTextAreaElement, value: () => string) {
  el.dataset.grow = "";
  let width = 0, height = 0, pending = 0;
  const fit = () => {
    // A field that is not laid out reads 0. It keeps its height, and the observer fits it once it shows.
    if (!el.getClientRects().length) { width = 0; return; }
    // Measuring folds the field for a moment, which can pull a scrolled page back: each scrolled box around it is put back where it was.
    // Nothing around the field is held to its height, because a held grid, like a dialog's .fld, stretches the field it measures.
    const kept: [Element, number][] = [];
    for (let box = el.parentElement; box; box = box.parentElement) if (box.scrollTop) kept.push([box, box.scrollTop]);
    // Measured without a scrollbar, whose width could wrap one more line than the field will show.
    el.style.overflowY = "hidden";
    el.style.height = "";
    const needed = el.scrollHeight + el.offsetHeight - el.clientHeight;
    el.style.height = `${needed}px`;
    for (const [box, top] of kept) box.scrollTop = top;
    width = el.offsetWidth; height = el.offsetHeight;
    el.style.overflowY = height < needed ? "auto" : "hidden";
  };
  // Every change of the value, typed or not. It runs after the value reaches the field.
  createEffect(() => { value(); fit(); });
  if (typeof ResizeObserver === "undefined") return;
  // Anything but this field's own fit: shown again, a new width, or a window whose height moved the cap. A frame later, so the fit is not a second change inside this observation.
  const box = new ResizeObserver(() => {
    if (el.offsetWidth === width && el.offsetHeight === height) return;
    cancelAnimationFrame(pending);
    pending = requestAnimationFrame(fit);
  });
  box.observe(el);
  onCleanup(() => { box.disconnect(); cancelAnimationFrame(pending); });
}

/** Reports a floating dock's height, so the feed under it can leave that much room at its end. */
export function reportHeight(el: () => HTMLElement, report: (px: number) => void) {
  onMount(() => {
    report(el().offsetHeight);
    if (typeof ResizeObserver === "undefined") return;
    const box = new ResizeObserver(() => report(el().offsetHeight));
    box.observe(el()); onCleanup(() => box.disconnect());
  });
}
