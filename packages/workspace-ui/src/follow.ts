import { createEffect, createSignal, onCleanup, onMount, type Accessor } from "solid-js";

/** Normal-flow scrolling: keep the bottom pinned, or keep the reader's visible row still.
 * ResizeObserver runs after Markdown/layout changes, even when no new event arrived.
 * Native anchoring is disabled on these two scrollers so there is only one scroll owner.
 */
export function createFollow(el: () => HTMLElement | undefined, size: Accessor<number>) {
  const [following, setFollowing] = createSignal(true), [missed, setMissed] = createSignal(0);
  let seen = size(), dragging = false, anchor: { node: Element; y: number } | undefined;
  // Where the end was last pinned. Content or clearance that grows after it moves the end away without the reader scrolling up.
  let placed = -1;
  const bottom = (box: HTMLElement) => Math.max(0, box.scrollHeight - box.clientHeight);
  const remember = () => {
    const box = el(), inner = box?.firstElementChild;
    if (!box || !inner) return;
    const y = box.getBoundingClientRect().top;
    // Most scrolls are near the tail. A binary search avoids walking a long history per frame.
    const rows = inner.children;
    let lo = 0, hi = rows.length;
    while (lo < hi) { const mid = (lo + hi) >>> 1; if (rows[mid].getBoundingClientRect().bottom <= y) lo = mid + 1; else hi = mid; }
    const node = rows[lo];
    anchor = node ? { node, y: node.getBoundingClientRect().top } : undefined;
  };
  const sync = () => {
    const box = el();
    if (!box || dragging) return;
    if (following()) { box.scrollTop = bottom(box); placed = box.scrollTop; }
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
    // Not at the end, yet not above where the end was pinned: the layout grew under a reader who stays at the end.
    if (!atEnd && following() && !dragging && placed >= 0 && box.scrollTop >= placed - 1) { remember(); return; }
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
    const down = () => { dragging = true; };
    const up = () => { if (!dragging) return; dragging = false; onScroll(); };
    box.addEventListener("pointerdown", down);
    document.addEventListener("pointerup", up);
    document.addEventListener("pointercancel", up);
    sync();
    onCleanup(() => {
      resize?.disconnect(); box.removeEventListener("pointerdown", down);
      document.removeEventListener("pointerup", up); document.removeEventListener("pointercancel", up);
    });
  });
  return { following, missed, jump, onScroll, set: (on: boolean) => { if (on) jump(); else { setFollowing(false); remember(); } } };
}
