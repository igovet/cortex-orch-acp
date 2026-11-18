import { createEffect, createSignal, onCleanup, onMount, type Accessor } from "solid-js";

/** Normal-flow scrolling: keep the bottom pinned, or keep the reader's visible row still.
 * ResizeObserver runs after Markdown/layout changes, even when no new event arrived, and when the scroller itself resizes:
 * the dock is the scroller's flow sibling below it, so the dock growing or shrinking changes the scroller's own height.
 * Native anchoring is disabled on these scrollers so there is only one scroll owner.
 */
export function createFollow(el: () => HTMLElement | undefined, size: Accessor<number>) {
  const [following, setFollowing] = createSignal(true), [missed, setMissed] = createSignal(0);
  let seen = size(), dragging = false, anchor: { node: Element; y: number } | undefined;
  // When the reader last pressed a key or moved focus inside the feed: a scroll soon after is theirs (Tab onto a control above).
  let intent = -Infinity;
  const bottom = (box: HTMLElement) => Math.max(0, box.scrollHeight - box.clientHeight);
  const remember = () => {
    const box = el(), inner = box?.firstElementChild;
    if (!box || !inner) return;
    const y = box.getBoundingClientRect().top;
    // Most scrolls are near the tail. A binary search avoids walking a long history per frame.
    const rows = inner.children;
    let lo = 0, hi = rows.length;
    while (lo < hi) { const mid = (lo + hi) >>> 1; if (rows[mid].getBoundingClientRect().bottom <= y) lo = mid + 1; else hi = mid; }
    // A row marked `data-follow-skip` stays put while rows are put below it (the top of a conversation read in part, as
    // its earlier part arrives): the reading position holds on the first row after it instead.
    while (rows[lo]?.hasAttribute("data-follow-skip") && lo + 1 < rows.length) lo++;
    const node = rows[lo];
    anchor = node ? { node, y: node.getBoundingClientRect().top } : undefined;
  };
  const sync = () => {
    const box = el();
    if (!box || dragging) return;
    if (following()) box.scrollTop = bottom(box);
    else if (anchor?.node.isConnected) {
      const delta = anchor.node.getBoundingClientRect().top - anchor.y;
      if (Math.abs(delta) > .1) box.scrollTop += delta;
    }
    remember();
  };
  const jump = () => { setFollowing(true); setMissed(0); sync(); };
  const onScroll = () => {
    const box = el();
    if (!box) return;
    const atEnd = Math.abs(bottom(box) - box.scrollTop) <= 2;
    // While following, only the reader leaves the end. Wheel, key and touch input pause before the browser scrolls and a
    // scrollbar drag is `dragging`; any other scroll away from the end is the engine's own: content or the dock's height that changed,
    // or WebKitGTK moving scrollTop by itself when rows near the end are replaced in place (it has no `overflow-anchor`).
    // Taken as the reader's, that stopped following at the end and showed Latest. The end is pinned again instead.
    if (!atEnd && following() && !dragging && performance.now() - intent > 1000) { box.scrollTop = bottom(box); remember(); return; }
    setFollowing(atEnd);
    if (atEnd) setMissed(0);
    remember();
  };
  createEffect(() => {
    const n = size();
    if (n > seen && !following()) setMissed(m => m + n - seen);
    seen = n;
  });
  onMount(() => {
    const box = el(), inner = box?.firstElementChild;
    if (!box || !inner) return;
    const resize = typeof ResizeObserver === "undefined" ? undefined : new ResizeObserver(sync);
    resize?.observe(box); resize?.observe(inner);
    const mark = () => { intent = performance.now(); };
    // A smooth or kinetic scroll can first land within the end, which follows again: the marked intent lets it leave again.
    const pause = () => { mark(); setFollowing(false); remember(); };
    const wheel = (e: WheelEvent) => { if (e.deltaY < 0) pause(); };
    // Focus a pointer press gave is not a move by the keyboard.
    const focus = () => { if (!dragging) mark(); };
    const key = (e: KeyboardEvent) => {
      if ((e.target as Element)?.closest("input, textarea, select, [contenteditable=true]")) return;
      mark();
      if (["ArrowUp", "PageUp", "Home"].includes(e.key) || (e.key === " " && e.shiftKey)) pause();
    };
    let touchY = 0;
    const touchStart = (e: TouchEvent) => { touchY = e.touches[0]?.clientY ?? 0; };
    const touchMove = (e: TouchEvent) => { const y = e.touches[0]?.clientY ?? touchY; if (y > touchY) pause(); touchY = y; };
    const down = () => { dragging = true; };
    const up = () => { if (!dragging) return; dragging = false; onScroll(); };
    box.addEventListener("wheel", wheel, { passive: true });
    box.addEventListener("keydown", key);
    box.addEventListener("focusin", focus);
    box.addEventListener("touchstart", touchStart, { passive: true });
    box.addEventListener("touchmove", touchMove, { passive: true });
    box.addEventListener("pointerdown", down);
    document.addEventListener("pointerup", up);
    document.addEventListener("pointercancel", up);
    sync();
    onCleanup(() => {
      resize?.disconnect(); box.removeEventListener("pointerdown", down);
      box.removeEventListener("wheel", wheel); box.removeEventListener("keydown", key); box.removeEventListener("focusin", focus);
      box.removeEventListener("touchstart", touchStart); box.removeEventListener("touchmove", touchMove);
      document.removeEventListener("pointerup", up); document.removeEventListener("pointercancel", up);
    });
  });
  /** The reader opened `node`, a row: it stays where they can read it. Following stops, so the pinned end no longer pulls the
   *  opened row up and out of view as it grows, and what opened is scrolled into view only as far as the row's own heading
   *  stays visible. `inset` is what covers the bottom of the box once following stops, such as the Latest band; given as
   *  a function it is read after following stopped, so it measures what that showed. */
  const reveal = (node: Element, inset: number | (() => number) = 0) => {
    const box = el();
    if (!box) return;
    setFollowing(false);
    const cover = typeof inset === "function" ? inset() : inset;
    const view = box.getBoundingClientRect(), row = node.getBoundingClientRect();
    const above = row.top - view.top, below = row.bottom - (view.bottom - cover);
    if (above < 0) box.scrollTop += above;
    else if (below > 0) box.scrollTop += Math.min(below, above);
    remember();
  };
  return { following, missed, jump, onScroll, reveal, set: (on: boolean) => { if (on) jump(); else { setFollowing(false); remember(); } } };
}
