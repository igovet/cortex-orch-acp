import { onCleanup } from "solid-js";

/**
 * The one rule of a scroll-driven pager (a conversation's or a worker's earlier rows): it reads more only when the reader's
 * viewport is within `edgeReach` of the edge where more content lies, never because a scroll happened somewhere in the middle.
 * While scrolling, a read starts when the edge is approached (the viewport comes into reach, or moves toward the edge inside
 * it), not on every event inside the reach. A layout pass (content arrived, the box resized) continues only while the
 * viewport is still within reach, which is how a page too small to fill the viewport is followed by the next.
 */
/** How far from the edge a read may start: one viewport height of drawn rows above the reader's viewport. Nearer, a reader
 *  scrolling up reached the top before the page; farther, a read started while most of a screen still lay between the reader
 *  and the edge. Measured by `scroll-perf.mjs <label> earlier`. */
export const edgeReach = (viewport: number) => Math.max(0, viewport);

/** Whether the reader is approaching the edge at the start of the content (scrolling up) between two decisions. */
export function createEdgeApproach() {
  let last = Infinity, toward = false, scrolled = false, wasNear = false;
  return {
    /** A scroll position as it is seen; `scrolling` is true for a scroll event, false for a layout pass. */
    seen(top: number, scrolling: boolean) {
      if (scrolling) { scrolled = true; if (top < last) toward = true; }
      last = top;
    },
    /** Whether to read now, given whether the viewport is within reach of the edge. Settles what was seen so far. */
    decide(near: boolean) {
      const go = near && (!scrolled || toward || !wasNear);
      wasNear = near; toward = false; scrolled = false;
      return go;
    },
  };
}

/**
 * Whether the reader's viewport is within reach of a feed's top edge: the part from `top` (the feed's reserve, or the edge
 * line itself) down to the bottom of `edge` (the line just above the earliest drawn row) lies between `edgeReach` above the
 * viewport and the viewport's bottom. Bounded on both sides: an edge entirely below the viewport (a tall brief fills it)
 * is not read until it comes near, and one far above it (the middle of a long feed) is not read at all. Two rectangles
 * per call, nothing per row.
 */
export function nearEdge(box: HTMLElement, edge: Element | null | undefined, top?: Element | null) {
  if (!edge || !box.clientHeight) return false;
  const view = box.getBoundingClientRect(), end = edge.getBoundingClientRect().bottom;
  const start = top?.isConnected ? Math.min(top.getBoundingClientRect().top, edge.getBoundingClientRect().top) : edge.getBoundingClientRect().top;
  return end > view.top - edgeReach(box.clientHeight) && start < view.bottom;
}

/**
 * Draws rows held above the drawn ones in steps until `hidden` is none: a page of rows drawn at once (and its Markdown) took
 * one long frame. Drawing starts in a microtask (outside any reactive update, before the engine paints) and goes on while it
 * has taken less than `BUDGET_MS` of this frame, then continues in the next frames; each step's own time sets the next
 * step's size. The rows go above the reader into the feed's reserve (FollowEdge in follow.ts); while it has no `room` for
 * another step, drawing waits until it is grown back, so no step has to write the scroll position under a running scroll.
 */
const BUDGET_MS = 6, MIN_STEP = 8;
export function createRevealer(options: { hidden: () => number; reveal: (count: number) => void; room: () => boolean }) {
  let step = MIN_STEP, queued = false, stopped = false, waiting: ReturnType<typeof setTimeout> | undefined;
  const frame = (f: () => void) => typeof requestAnimationFrame === "function" ? requestAnimationFrame(f) : setTimeout(f, 16);
  const run = () => {
    queued = false;
    const started = performance.now();
    while (!stopped && options.hidden() > 0) {
      if (!options.room()) { clearTimeout(waiting); waiting = setTimeout(kick, 120); return; }
      const at = performance.now();
      options.reveal(Math.min(step, options.hidden()));
      const took = performance.now() - at;
      step = took > BUDGET_MS ? Math.max(MIN_STEP, step >> 1) : took < BUDGET_MS / 3 ? Math.min(64, step * 2) : step;
      if (performance.now() - started >= BUDGET_MS) { queued = true; frame(run); return; }
    }
  };
  const kick = () => { if (!queued && !stopped) { queued = true; queueMicrotask(run); } };
  onCleanup(() => { stopped = true; clearTimeout(waiting); });
  return kick;
}
