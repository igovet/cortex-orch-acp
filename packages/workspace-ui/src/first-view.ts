import { createEffect, createMemo, createSignal, on, onCleanup, type Accessor } from "solid-js";
import type { TaskSession } from "@cortex/workspace-domain";

/**
 * How long a view's first read may take before what has arrived is shown together with one notice naming what has not.
 * One constant, so it is easy to tune once the reads are profiled; new events never restart it.
 */
export const FIRST_VIEW_MS = 5_000;

/** A read the port has begun for a view and not yet confirmed (`loading`), or one that did not complete (`failed`). */
export type ReadState = { state: "loading" } | { state: "failed"; reason: string };
/** What the real port adds to a conversation's session: whether its execution's questions have been read. Absent: they have,
 *  or this port does not read them on their own. Empty decisions before this read are not a confirmed "no questions". */
export type SessionReads = { decisionsRead?: ReadState };
export const decisionsReadOf = (session: TaskSession): ReadState | undefined => (session as TaskSession & SessionReads).decisionsRead;

/**
 * About one screen of a feed: how many entries to read and show first, from the window's height and a short entry's height.
 * It errs on the side of a little more than one screen, so the newest part fills the feed and reads as scrollable.
 */
export const FIRST_WINDOW_ROW_PX = 40;
export function firstWindowEntries(height = typeof globalThis.innerHeight === "number" ? globalThis.innerHeight : 800) {
  const rows = Math.ceil((Number.isFinite(height) && height > 0 ? height : 800) / FIRST_WINDOW_ROW_PX);
  return Math.min(60, Math.max(12, rows));
}

/** One read a view's first reveal waits for, in the owner's words (`section`, as the notice names it). `failed` is settled:
 *  the view shows what it has and names the section. */
export type FirstRead = { section: string; state: "pending" | "settled" | "failed"; reason?: string };

/** The views already revealed in this window, per store: showing one again (a parked tab, a reconnect, a refresh of the same
 *  execution) keeps what it shows and its draft, focus and scroll, with no skeleton flashed over it. */
const revealed = new WeakMap<object, Set<string>>();
const revealedIn = (scope: object) => { let held = revealed.get(scope); if (!held) revealed.set(scope, held = new Set()); return held; };
/** Of those, the views shown before all of their first reads were in (the budget ran out, or the machine was offline): shown
 *  again, they keep naming what is missing, with Retry, until those reads have settled. Never a skeleton again. */
const incomplete = new WeakMap<object, Set<string>>();
const incompleteIn = (scope: object) => { let held = incomplete.get(scope); if (!held) incomplete.set(scope, held = new Set()); return held; };

/**
 * The first-reveal gate of one view. `identity` is the exact thing on view (machine, project, task, execution): while its
 * required `reads` are under way the view shows its skeleton, and once every one has settled it shows all of it at once.
 * After `FIRST_VIEW_MS`, or at once when the machine is offline, the skeleton stops and the view shows what was confirmed with
 * `missing` naming the rest; a late completion then simply clears it. `retry` reads again under a new generation: a reply
 * that answers an older generation or another identity changes nothing here.
 */
export function createFirstView(options: {
  scope: object;
  identity: Accessor<string | null>;
  /** What stays the same view for this mounted screen when `identity` moves on (the task, when its execution changes because
   *  the owner just started it): once it was revealed here, a new identity under it keeps the screen as it is. */
  keep?: Accessor<string>;
  reads: Accessor<FirstRead[]>;
  offline: Accessor<boolean>;
  retry?: () => void;
  budgetMs?: number;
}) {
  const [generation, setGeneration] = createSignal(0);
  // The budget belongs to one identity and one read generation: another identity is never shown as timed out by the last one's.
  const budgetKey = createMemo(() => `${options.identity() ?? ""}\u0000${generation()}`);
  const [expiredKey, setExpiredKey] = createSignal<string | null>(null);
  const expired = () => expiredKey() === budgetKey();
  const [revealedNow, setRevealedNow] = createSignal<string | null>(null);
  const kept = new Set<string>();
  const pending = createMemo(() => options.reads().filter(r => r.state === "pending"));
  const failed = createMemo(() => options.reads().filter(r => r.state === "failed"));
  // Cold: this identity was not revealed before, in this window or on this screen. Decided once per identity (the sets are not
  // signals), so the reveal that follows does not change what the start of this identity was.
  const cold = createMemo(() => {
    const id = options.identity();
    return id !== null && !revealedIn(options.scope).has(id) && !(options.keep && kept.has(options.keep()));
  });
  let timer: ReturnType<typeof setTimeout> | undefined, disposed = false;
  createEffect(on(budgetKey, key => {
    clearTimeout(timer);
    timer = setTimeout(() => { if (!disposed) setExpiredKey(key); }, options.budgetMs ?? FIRST_VIEW_MS);
  }));
  onCleanup(() => { disposed = true; clearTimeout(timer); });
  const loading = createMemo(() => cold() && revealedNow() !== options.identity() && pending().length > 0 && !expired() && !options.offline());
  // Whether this identity was shown before its reads were all in, here or on an earlier mount of the same view. The set is
  // not a signal: `marked` moves when this gate changes it.
  const [marked, setMarked] = createSignal(0);
  const partial = createMemo(() => { marked(); const id = options.identity(); return id !== null && incompleteIn(options.scope).has(id); });
  // Once shown (whole, or after the budget with its notice) this identity is no longer cold: it never shows its skeleton again.
  createEffect(() => {
    const id = options.identity();
    if (id === null || loading()) return;
    revealedIn(options.scope).add(id);
    if (options.keep) kept.add(options.keep());
    if (revealedNow() !== id) {
      // A first reveal with reads still out (the budget ran out, or the machine is offline) is remembered as incomplete.
      if (cold() && pending().length > 0 && !incompleteIn(options.scope).has(id)) { incompleteIn(options.scope).add(id); setMarked(n => n + 1); }
      setRevealedNow(id);
    }
  });
  // Until every read it was shown without has settled, on this mount or a later one.
  createEffect(() => {
    const id = options.identity();
    if (id !== null && !pending().length && incompleteIn(options.scope).delete(id)) setMarked(n => n + 1);
  });
  createEffect(() => { if (!pending().length) clearTimeout(timer); });
  /** The sections still missing once the skeleton has stopped without them; empty while loading and once all of them came.
   *  A view that was already on screen says nothing extra for being offline: the page's own offline line does. */
  const missing = createMemo(() => {
    if (loading()) return [];
    const late = expired() || (options.offline() && cold()) || partial() ? pending() : [];
    return [...late, ...failed()].map(r => r.section).filter((s, i, all) => all.indexOf(s) === i);
  });
  /** Retry was pressed and its reads are under way, within a new budget: the notice says so instead of going away. */
  const retrying = createMemo(() => generation() > 0 && !loading() && !expired() && pending().length > 0);
  return {
    loading, missing, pending, retrying,
    retry: () => { setGeneration(g => g + 1); options.retry?.(); },
  };
}
export type FirstView = ReturnType<typeof createFirstView>;
