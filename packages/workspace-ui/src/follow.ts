import { createEffect, createSignal, onCleanup, onMount, type Accessor } from "solid-js";

/** An undefined offset means follow the current end, not the old bottom offset. The offset is taken below the feed's
 *  reserve (see `FollowEdge`), so it lands on the same row whatever height the reserve has when the feed is shown again. */
export type FollowPosition = { top?: number };

/**
 * The room a feed keeps above its rows while earlier ones may still come (a conversation read in part, a worker's held or
 * unread activity): a block of quiet skeleton shapes, `reserve`, standing directly above the earliest drawn row, kept
 * (`keep`) while more exists. Rows put above the reader, a page or a line that changed, take their height from the reserve
 * instead of pushing the rows below them down, so no row the reader sees moves and the scroll position is not written.
 * WebKitGTK has no scroll anchoring (`overflow-anchor`): a written scroll position stops the wheel's or the touchpad's
 * own scroll animation, so with the position corrected for each page the feed stood still for a frame or two, and a page that
 * came while the reader was held at the very top was undone by that animation, a jump by the whole page. Measured by
 * `test/scroll-perf.mjs <label> earlier`. The position is written only when the reserve has to grow (it is grown again
 * while the reader is still, or in one step with a page larger than what is left of it) and when it goes, which waits for
 * the reader to be still too.
 */
export type FollowEdge = { reserve: () => HTMLElement | undefined; keep: Accessor<boolean> };
/** The reserve holds at least this many viewports, or this many times the most it gave room to between two growths (what
 *  the reader climbed through without holding still), whichever is more, and never more than `RESERVE_MOST` viewports. */
const RESERVE_SCREENS = 8, RESERVE_PAGES = 4, RESERVE_MOST = 60;
/** How long the reader holds still before the reserve is grown back or taken away. */
const STILL_MS = 200;
/** How long after a wheel notch or a touch the engine may still be animating the scroll it started. */
const WHEEL_MS = 400;

type Feed = { scrollTo(node: Element, block?: ScrollLogicalPosition): void };
/** Each mounted feed's controller, by its scroller, so a control outside the feed scrolls it through its controller. */
const feeds = new WeakMap<Element, Feed>();

/** Brings `node` into view. Inside a feed this goes through that feed's controller: following stops first, so the pinned end
 *  does not pull the feed back down and undo the move. Anywhere else it is an ordinary scroll into view. */
export function scrollIntoFeed(node: Element, block: ScrollLogicalPosition = "center") {
  for (let at = node.parentElement; at; at = at.parentElement) {
    const feed = feeds.get(at);
    if (feed) { feed.scrollTo(node, block); return; }
  }
  node.scrollIntoView?.({ block });
}

/** One rule for every feed (a task's or a sprint's conversation, a worker's or the coordinator's activity): at the very end the
 * feed follows, and keeps the end pinned as content grows; once the reader moves away from the end it stops following and
 * nothing moves what they read. Back at the end it follows again, and the reader's own message or answer brings it back (`jump`).
 * Normal-flow scrolling: keep the bottom pinned, or keep the reader's visible row still.
 * ResizeObserver runs after Markdown/layout changes, even when no new event arrived, and when the scroller itself resizes:
 * the dock is the scroller's flow sibling below it, so the dock growing or shrinking changes the scroller's own height.
 * Native anchoring is disabled on these scrollers so there is only one scroll owner.
 */
export function createFollow(el: () => HTMLElement | undefined, size: Accessor<number>, restore?: { position: FollowPosition; ready: Accessor<boolean> }, afterSync?: () => void, edge?: FollowEdge) {
  let pendingTop = restore?.position.top;
  // The reserve's height as last set here (0: none), and the largest page it gave room to.
  const [reserved, setReserved] = createSignal(0);
  let largest = 0, last = 0, batch = 0, still: ReturnType<typeof setTimeout> | undefined, disposed = false;
  const [following, setFollowing] = createSignal(pendingTop === undefined), [missed, setMissed] = createSignal(0);
  // The reading anchor is a row and its place in the content (from the content's top, not the viewport's): rows put or grown
  // above it move that place, the reader's own scrolling does not. Taken against the viewport, an anchor remembered before a
  // scroll the engine had not reported yet undid that scroll when rows arrived above it (the feed stood still for a frame).
  let seen = size(), anchor: { node: Element; offset: number } | undefined;
  // A pointer is down in the feed (`pressing`), and that press is on the feed's own scrollbar (`dragging`). Only a scrollbar
  // press holds the feed still: a click or a text selection in the rows leaves following as it is.
  let pressing = false, dragging = false;
  // When the reader last pressed a key or moved focus inside the feed: a scroll soon after is theirs (Tab onto a control above).
  let intent = -Infinity;
  const mark = () => { intent = performance.now(); };
  const bottom = (box: HTMLElement) => Math.max(0, box.scrollHeight - box.clientHeight);
  const atEnd = (box: HTMLElement) => Math.abs(bottom(box) - box.scrollTop) <= 2;
  const offsetOf = (box: HTMLElement, node: Element, top: number) => node.getBoundingClientRect().top - top + box.scrollTop;
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
    anchor = node ? { node, offset: offsetOf(box, node, y) } : undefined;
  };
  const reserveOf = () => { const r = edge?.reserve(); return r?.isConnected ? r : undefined; };
  const heightOf = (r: HTMLElement) => { const h = parseFloat(r.style.height); return Number.isFinite(h) ? h : r.getBoundingClientRect().height; };
  const setReserve = (r: HTMLElement, h: number) => { batch = 0; const px = Math.max(0, Math.round(h * 10) / 10); r.style.height = `${px}px`; setReserved(px); return px; };
  const target = (box: HTMLElement) => Math.max(box.clientHeight, Math.min(RESERVE_MOST * box.clientHeight, Math.max(RESERVE_SCREENS * box.clientHeight, RESERVE_PAGES * largest)));
  /** Sizes a reserve met for the first time. Pages are read in one size, so the first page put above is about as tall as
   *  the rows held below the reserve now: it is sized for that from the start, not first used up and grown mid-scroll. */
  const sizeNew = (box: HTMLElement, r: HTMLElement) => {
    largest = Math.max(largest, box.scrollHeight - (offsetOf(box, r, box.getBoundingClientRect().top) + heightOf(r)));
    return setReserve(r, target(box));
  };
  /** Gives `delta` (how far the anchor row moved in the content) to the reserve above it, and returns what the scroll position
   *  must still move by: nothing, unless the reserve was new or has to grow. */
  const absorb = (box: HTMLElement, delta: number) => {
    const r = reserveOf();
    if (!r || !anchor || !(r.compareDocumentPosition(anchor.node) & Node.DOCUMENT_POSITION_FOLLOWING)) return delta;
    const h = heightOf(r), sized = r.style.height !== "", next = h - delta, most = target(box);
    const climbed = sized && delta > 0 ? batch + delta : 0;
    if (sized && delta > 0) { largest = Math.max(largest, climbed); last = delta; }
    // Down to none of it: it is grown back once the reader holds still (`settle`), never while a scroll may be under way.
    if (sized && next >= 0 && next <= 2 * most) { setReserve(r, next); batch = climbed; return 0; }
    // New, too small for this page, or grown past twice its size: set to its size in one step, and move the position by the
    // difference. Only this writes the position while the reader may be scrolling; reserving several pages keeps it rare.
    return delta + (sized ? setReserve(r, target(box)) : sizeNew(box, r)) - h;
  };
  /** Once the feed holds still: the reserve is grown back to its size, or taken away when nothing more can come. Nothing on
   *  screen moves (the position moves with it). Still means no scroll for `STILL_MS` (none at all at the very top, where a
   *  reader who outran the reserve waits for it) and the position the same across two frames: a position written while the
   *  engine's own scroll animation runs was undone by that animation (measured: a jump by the whole reserve), and a busy
   *  page can hold scroll events back for longer than `STILL_MS`. A wheel turned against the top moves nothing. */
  let moved = -Infinity, wheeled = -Infinity;
  const moving = () => { moved = performance.now(); };
  /** Whether the engine may be running a scroll animation of its own: a wheel notch or a touch moved the feed lately. Other
   *  scrolling (a scrollbar drag, keys, a script) sets the position directly, and a position written under it holds. */
  const animating = () => performance.now() - wheeled < WHEEL_MS;
  /** Grows the reserve back to its size now, the position moving with it. */
  const grow = (box: HTMLElement, r: HTMLElement) => {
    const before = heightOf(r), grew = setReserve(r, edge?.keep() ? target(box) : 0) - before;
    if (following()) box.scrollTop = bottom(box);
    else if (pendingTop === undefined && Math.abs(grew) > .1) box.scrollTop += grew;
    remember();
  };
  const settle = () => {
    clearTimeout(still);
    if (!edge || disposed) return;
    still = setTimeout(() => {
      const box = el(), r = reserveOf();
      if (!box || !r || disposed) return;
      const h = heightOf(r), want = edge.keep() ? target(box) : 0;
      if (edge.keep() ? r.style.height !== "" && h >= .75 * want : h === 0) return;
      if (pressing || dragging || (box.scrollTop >= 1 && performance.now() - moved < STILL_MS)) { settle(); return; }
      const at = box.scrollTop;
      const frame = typeof requestAnimationFrame === "function" ? requestAnimationFrame : (f: () => void) => setTimeout(f, 16);
      frame(() => frame(() => {
        if (disposed || !r.isConnected || box.scrollTop !== at || (at >= 1 && performance.now() - moved < STILL_MS)) { settle(); return; }
        grow(box, r);
      }));
    }, STILL_MS);
  };
  /** What a saved offset is measured from: the bottom of the reserve. */
  const below = () => { const r = reserveOf(); return r ? heightOf(r) : 0; };
  const sync = () => {
    const box = el();
    if (!box) return;
    if (dragging) {
      // Under a scrollbar drag the position is the reader's: rows put above still take the reserve's room, nothing else moves.
      if (anchor?.node.isConnected && !following()) absorb(box, offsetOf(box, anchor.node, box.getBoundingClientRect().top) - anchor.offset);
      remember();
      return;
    }
    const r = reserveOf();
    // While following, or before a saved offset is applied, the reserve takes its size without being seen.
    if (r && edge?.keep() && r.style.height === "" && (following() || pendingTop !== undefined)) sizeNew(box, r);
    if (pendingTop !== undefined) {
      // Applying the offset to only the recent tail would clamp it before the older rows arrive.
      if (!restore?.ready()) return;
      box.scrollTop = pendingTop + below();
      pendingTop = undefined;
    } else if (following()) box.scrollTop = bottom(box);
    else if (anchor?.node.isConnected) {
      let delta = offsetOf(box, anchor.node, box.getBoundingClientRect().top) - anchor.offset;
      if (Math.abs(delta) > .1) delta = absorb(box, delta);
      if (Math.abs(delta) > .1) box.scrollTop += delta;
    }
    remember();
    settle();
    afterSync?.();
  };
  const jump = () => { pendingTop = undefined; setFollowing(true); setMissed(0); sync(); };
  const onScroll = () => {
    const box = el();
    if (!box) return;
    if (pendingTop !== undefined) {
      if (!dragging && performance.now() - intent > 1000) return;
      pendingTop = undefined;
    }
    const end = atEnd(box);
    // While following, only the reader leaves the end. Wheel, key and touch input pause before the browser scrolls and a
    // scrollbar drag is `dragging`; any other scroll away from the end is the engine's own: content or the dock's height that changed,
    // or WebKitGTK moving scrollTop by itself when rows near the end are replaced in place (it has no `overflow-anchor`).
    // Taken as the reader's, that stopped following at the end and showed Latest. The end is pinned again instead.
    if (!end && following() && !dragging && performance.now() - intent > 1000) { box.scrollTop = bottom(box); remember(); return; }
    setFollowing(end);
    if (end) setMissed(0);
    remember();
    moving(); settle();
  };
  createEffect(() => {
    const n = size();
    // What arrives while the reader holds the scrollbar is not followed either.
    if (n > seen && (!following() || dragging)) setMissed(m => m + n - seen);
    seen = n;
  });
  /** The reader opened `node`, a row: it stays where they can read it. When the feed follows and the whole opened row is
   *  visible with the end pinned (a short feed, or a row that fits above the end), nothing needs to move: following goes on
   *  and the end stays pinned as the feed grows. Otherwise following stops, so the pinned end no longer pulls the opened row
   *  up and out of view as it grows, and what opened is scrolled into view only as far as the row's own heading stays
   *  visible. `inset` is what covers the bottom of the box once following stops, such as the Latest band; given as a
   *  function it is read after following stopped, so it measures what that showed. */
  const reveal = (node: Element, inset: number | (() => number) = 0) => {
    const box = el();
    if (!box) return;
    pendingTop = undefined;
    if (following() && !dragging) {
      // Where the row sits once the end is pinned again: the content that opened below the old end moves it up by as much.
      // A box with no area shows nothing, so a row in it is never taken as visible.
      const view = box.getBoundingClientRect(), row = node.getBoundingClientRect(), shift = bottom(box) - box.scrollTop;
      if (view.bottom - view.top > 0 && row.top - shift >= view.top - .5 && row.bottom - shift <= view.bottom + .5) {
        box.scrollTop = bottom(box);
        remember();
        return;
      }
    }
    setFollowing(false);
    const cover = typeof inset === "function" ? inset() : inset;
    const view = box.getBoundingClientRect(), row = node.getBoundingClientRect();
    const above = row.top - view.top, below = row.bottom - (view.bottom - cover);
    if (above < 0) box.scrollTop += above;
    else if (below > 0) box.scrollTop += Math.min(below, above);
    remember();
  };
  /** Brings `node` into view for a control outside the feed (Answer, See what is needed, a notification). Following stops
   *  first, so the end does not take the feed back; landing at the end follows again, as reaching it any other way does. */
  const scrollTo = (node: Element, block: ScrollLogicalPosition = "center") => {
    const box = el();
    pendingTop = undefined;
    mark(); setFollowing(false);
    node.scrollIntoView?.({ block });
    if (box && atEnd(box)) { setFollowing(true); setMissed(0); }
    remember();
  };
  onMount(() => {
    const box = el(), inner = box?.firstElementChild;
    if (!box || !inner) return;
    const resize = typeof ResizeObserver === "undefined" ? undefined : new ResizeObserver(sync);
    resize?.observe(box); resize?.observe(inner);
    feeds.set(box, { scrollTo });
    // A smooth or kinetic scroll can first land within the end, which follows again: the marked intent lets it leave again.
    const pause = () => { mark(); setFollowing(false); remember(); };
    // Whether a move up that starts at `target` scrolls the feed itself: the feed has somewhere to go, and no box inside it on
    // the way (a command's output, a block's steps) takes the move instead. A feed shorter than its box never stops following.
    const feedMoves = (target: EventTarget | null) => {
      if (bottom(box) < 1) return false;
      for (let n = target instanceof Element ? target : null; n && n !== box; n = n.parentElement) {
        if (!(n instanceof HTMLElement) || n.scrollHeight <= n.clientHeight) continue;
        const style = getComputedStyle(n);
        if (!/^(auto|scroll)$/.test(style.overflowY)) continue;
        if (n.scrollTop > 0 || /^(contain|none)$/.test(style.overscrollBehaviorY ?? "")) return false;
      }
      return true;
    };
    const wheel = (e: WheelEvent) => { wheeled = performance.now(); if (e.deltaY < 0) { if (feedMoves(e.target)) pause(); } else if (pendingTop !== undefined) mark(); };
    // Focus a pointer press gave is not a move by the keyboard.
    const focus = () => { if (!pressing) mark(); };
    const key = (e: KeyboardEvent) => {
      if ((e.target as Element)?.closest?.("input, textarea, select, [contenteditable=true]")) return;
      mark();
      if ((["ArrowUp", "PageUp", "Home"].includes(e.key) || (e.key === " " && e.shiftKey)) && feedMoves(e.target)) pause();
    };
    let touchY = 0;
    const touchStart = (e: TouchEvent) => { touchY = e.touches[0]?.clientY ?? 0; };
    const touchMove = (e: TouchEvent) => { wheeled = performance.now(); const y = e.touches[0]?.clientY ?? touchY; if (y > touchY && feedMoves(e.target)) pause(); touchY = y; };
    // The feed's own scrollbar lies past its content box on the right; a press on a row or in the text is not on it.
    const onScrollbar = (e: PointerEvent) => {
      if (e.target !== box) return false;
      return e.clientX - box.getBoundingClientRect().left >= box.clientLeft + box.clientWidth;
    };
    const down = (e: PointerEvent) => { pressing = true; if (onScrollbar(e)) dragging = true; };
    // The reader let go of the scrollbar: where the feed is now is theirs. Away from the end it stays there, and nothing
    // that arrived while it was held pulls it down.
    const release = () => {
      pressing = false;
      if (!dragging) return;
      dragging = false;
      mark(); onScroll();
    };
    const move = (e: PointerEvent) => {
      if (!pressing) return;
      // A release this document did not see (outside the window, or taken by the engine's scrollbar).
      if (e.buttons === 0) { release(); return; }
      // Selecting text past the feed's top edge scrolls it up: that is the reader's move.
      if (!dragging && e.clientY < box.getBoundingClientRect().top) mark();
    };
    // A row or block the reader opens stays in view, in every feed: following the end would otherwise scroll it up and away
    // as it grows. A plain listener, not a click target of its own: it only watches the rows' own toggles, after they have run.
    // What covers the feed's bottom once following stops is the Latest band in the dock below the feed.
    const cover = () => box.nextElementSibling?.querySelector(".pill-band")?.getBoundingClientRect().height ?? 0;
    const opened = (e: MouseEvent) => {
      const toggle = (e.target as Element | null)?.closest?.("button[aria-expanded]");
      if (!toggle || !box.contains(toggle)) return;
      queueMicrotask(() => {
        if (toggle.isConnected && toggle.getAttribute("aria-expanded") === "true") reveal(toggle.closest(".step, .fold-block") ?? toggle, cover);
      });
    };
    box.addEventListener("wheel", wheel, { passive: true });
    box.addEventListener("keydown", key);
    box.addEventListener("focusin", focus);
    box.addEventListener("touchstart", touchStart, { passive: true });
    box.addEventListener("touchmove", touchMove, { passive: true });
    box.addEventListener("pointerdown", down);
    box.addEventListener("click", opened);
    document.addEventListener("pointermove", move, { passive: true });
    document.addEventListener("pointerup", release);
    document.addEventListener("pointercancel", release);
    document.addEventListener("lostpointercapture", release);
    window.addEventListener("blur", release);
    sync();
    if (pendingTop !== undefined && restore) createEffect(() => { if (restore.ready()) sync(); });
    onCleanup(() => {
      disposed = true; clearTimeout(still);
      if (restore) restore.position.top = following() ? undefined : pendingTop ?? box.scrollTop - below();
      if (feeds.get(box)?.scrollTo === scrollTo) feeds.delete(box);
      resize?.disconnect(); box.removeEventListener("pointerdown", down); box.removeEventListener("click", opened);
      box.removeEventListener("wheel", wheel); box.removeEventListener("keydown", key); box.removeEventListener("focusin", focus);
      box.removeEventListener("touchstart", touchStart); box.removeEventListener("touchmove", touchMove);
      document.removeEventListener("pointermove", move); document.removeEventListener("pointerup", release);
      document.removeEventListener("pointercancel", release); document.removeEventListener("lostpointercapture", release);
      window.removeEventListener("blur", release);
    });
  });
  /** Whether rows may be put above the reader now without writing the scroll position: the reserve holds a viewport and
   *  twice the last step it gave room to. While following (the end is pinned) or with no reserve, there is nothing to keep. */
  const room = () => {
    const box = el(), r = reserveOf();
    if (!box || !r || following() || heightOf(r) >= Math.max(box.clientHeight, 2 * last)) return true;
    // Used up under a scroll with no animation of the engine's own (a drag, keys, a script): grown now, the rows go on.
    if (!dragging && !animating()) { grow(box, r); return true; }
    return false;
  };
  /** Whether the reserve stands in the feed: while more may come, and after that until it has been taken away unseen. */
  const reserving = () => !!edge?.keep() || reserved() > 0;
  return { following, missed, jump, onScroll, reveal, scrollTo, sync, reserving, room };
}
