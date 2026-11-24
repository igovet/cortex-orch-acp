import { createMemo, For, onCleanup, onMount, createEffect } from "solid-js";
import { paintMarkdown, type PaintRun } from "./markdown";

/** One drawn line: its classes (block, box and box edge) and its runs. Equal lines keep their object, so their DOM stays. */
type Line = { cls: string; runs: PaintRun[] };

/** What the layer must share with the field so that every character lands where the field puts it. */
const SHARED = ["font-family", "font-size", "font-weight", "font-style", "font-stretch", "font-variant", "font-feature-settings", "font-variation-settings",
  "font-kerning", "font-optical-sizing", "font-synthesis", "line-height", "letter-spacing", "word-spacing", "text-transform", "text-indent", "tab-size",
  "text-rendering", "white-space", "overflow-wrap", "word-break", "line-break", "hyphens", "direction", "unicode-bidi", "box-sizing",
  "padding-top", "padding-bottom", "padding-left", "border-top-width", "border-right-width", "border-bottom-width", "border-left-width"];

function lines(text: string, previous: Line[]): Line[] {
  const painted = paintMarkdown(text);
  const next = painted.map((l, i): Line => {
    const box = l.box ? ` md-box md-box-${l.box.split(":")[0]}${painted[i - 1]?.box !== l.box ? " md-first" : ""}${painted[i + 1]?.box !== l.box ? " md-last" : ""}` : "";
    return { cls: `md-l${l.cls ? ` ${l.cls}` : ""}${box}`, runs: l.runs };
  });
  const pool = new Map<string, Line[]>();
  for (const l of previous) { const key = JSON.stringify(l), list = pool.get(key); if (list) list.push(l); else pool.set(key, [l]); }
  return next.map(l => pool.get(JSON.stringify(l))?.shift() ?? l);
}

/**
 * The Markdown layer under a writing area. The field stays the only place the text lives: it keeps the caret, selection, IME
 * composition, undo, paste and every key, and its own glyphs are transparent. This layer, hidden from assistive technology
 * and from the pointer, draws the same characters from the same reading as the conversation (`markdown.ts`), with the marks
 * dimmed and the content styled, and follows the field's font, padding, width and scroll.
 */
export function MarkdownMirror(props: { text: string; field: () => HTMLTextAreaElement | undefined }) {
  let layer!: HTMLDivElement;
  const shown = createMemo<Line[]>(previous => lines(props.text, previous), []);
  const sync = () => {
    const el = props.field();
    if (!el || !layer || typeof getComputedStyle !== "function") return;
    const style = getComputedStyle(el);
    for (const name of SHARED) { const value = style.getPropertyValue(name); if (value) layer.style.setProperty(name, value); }
    // A scrollbar in the field narrows its text; the layer leaves the same room on the right.
    const borders = (parseFloat(style.borderLeftWidth) || 0) + (parseFloat(style.borderRightWidth) || 0);
    const bar = Math.max(0, el.offsetWidth - el.clientWidth - borders);
    layer.style.paddingRight = `${(parseFloat(style.paddingRight) || 0) + bar}px`;
    layer.scrollTop = el.scrollTop;
  };
  // After the field has taken the value and fitted its height, so the scrollbar and scroll position are the new ones.
  createEffect(() => { shown(); queueMicrotask(sync); });
  onMount(() => {
    const el = props.field();
    if (!el) return;
    sync();
    const follow = () => { layer.scrollTop = el.scrollTop; };
    el.addEventListener("scroll", follow, { passive: true });
    onCleanup(() => el.removeEventListener("scroll", follow));
    if (typeof ResizeObserver === "undefined") return;
    const box = new ResizeObserver(() => sync());
    box.observe(el);
    onCleanup(() => box.disconnect());
  });
  return <div ref={layer} class="mc-mirror" aria-hidden="true">
    <For each={shown()}>{l => <div class={l.cls}>{l.runs.length ? l.runs.map(r => r.cls ? <span class={r.cls}>{r.text}</span> : r.text) : <br />}</div>}</For>
  </div>;
}
