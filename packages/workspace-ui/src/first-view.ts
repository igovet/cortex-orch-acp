import { createEffect, createMemo, createSignal, type Accessor } from "solid-js";
import type { TaskSession } from "@cortex/workspace-domain";

/**
 * How long the board's own reads let a screen's first reads go first (`shownFirst` in the real port). It is not a deadline
 * of the view: a slow first read is waited for, with the skeleton, however long it takes (`createFirstView`).
 */
export const FIRST_VIEW_MS = 5_000;

/** Codes and words of a read that only took long (an older service's deadline, a busy lane), never a failure. */
const SLOW_CODES = /^(service_timeout|machine_busy|machine_tool_timeout|git_timeout|busy)$/;
/** The store diagnostic for a store held by another Cortex operation (a migration, an initialization): it is waited out. */
const STORE_BUSY_WORDS = "The project store is busy.";
const SLOW_WORDS = "The machine is busy and did not answer in time. Try again.";
/**
 * Whether a read failed only for being slow. Such a read is not an error: the view keeps its skeleton (or what it last
 * confirmed) and reads again. A store held by another Cortex operation (`busy`) is waited out the same way. A disconnect, a refusal or an unreadable store is a genuine failure and is shown.
 */
export function slowRead(error: unknown) {
  const code = (error as { code?: unknown } | null)?.code;
  const text = (error instanceof Error ? error.message : String(error)).replace(/^refused:/, "");
  return (typeof code === "string" && SLOW_CODES.test(code)) || SLOW_CODES.test(text) || text === SLOW_WORDS || text.startsWith(STORE_BUSY_WORDS);
}

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
 *  the view shows what it has and names the section. A `pending` read is waited for, however long it takes. */
export type FirstRead = { section: string; state: "pending" | "settled" | "failed"; reason?: string };

/** The views already revealed in this window, per store: showing one again (a parked tab, a reconnect, a refresh of the same
 *  execution) keeps what it shows and its draft, focus and scroll, with no skeleton flashed over it. */
const revealed = new WeakMap<object, Set<string>>();
const revealedIn = (scope: object) => { let held = revealed.get(scope); if (!held) revealed.set(scope, held = new Set()); return held; };
/** Of those, the views shown before all of their first reads were in (the machine was offline): shown again, they keep
 *  naming what is missing, with Retry, until those reads have settled. Never a skeleton again. */
const incomplete = new WeakMap<object, Set<string>>();
const incompleteIn = (scope: object) => { let held = incomplete.get(scope); if (!held) incomplete.set(scope, held = new Set()); return held; };

/**
 * The first-reveal gate of one view. `identity` is the exact thing on view (machine, project, task, execution): while its
 * required `reads` are under way the view shows its skeleton, and once every one has settled it shows all of it at once.
 * A slow read is never an error: the skeleton stays until it arrives, however long that takes. Only a read that genuinely
 * failed (`failed`), or the machine going offline before the first reveal, ends the skeleton early: the view then shows what
 * was confirmed with `missing` naming the rest, and a late completion simply clears it. `retry` reads again under a new
 * generation: a reply that answers an older generation or another identity changes nothing here.
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
}) {
  const [generation, setGeneration] = createSignal(0);
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
  // No budget: a pending first read keeps the skeleton until it settles, however slow it is.
  const loading = createMemo(() => cold() && revealedNow() !== options.identity() && pending().length > 0 && !options.offline());
  // Whether this identity was shown before its reads were all in, here or on an earlier mount of the same view. The set is
  // not a signal: `marked` moves when this gate changes it.
  const [marked, setMarked] = createSignal(0);
  const partial = createMemo(() => { marked(); const id = options.identity(); return id !== null && incompleteIn(options.scope).has(id); });
  // Once shown (whole, or offline with its notice) this identity is no longer cold: it never shows its skeleton again.
  createEffect(() => {
    const id = options.identity();
    if (id === null || loading()) return;
    revealedIn(options.scope).add(id);
    if (options.keep) kept.add(options.keep());
    if (revealedNow() !== id) {
      // A first reveal with reads still out (the machine is offline) is remembered as incomplete.
      if (cold() && pending().length > 0 && !incompleteIn(options.scope).has(id)) { incompleteIn(options.scope).add(id); setMarked(n => n + 1); }
      setRevealedNow(id);
    }
  });
  // Until every read it was shown without has settled, on this mount or a later one.
  createEffect(() => {
    const id = options.identity();
    if (id !== null && !pending().length && incompleteIn(options.scope).delete(id)) setMarked(n => n + 1);
  });
  /** The sections still missing once the skeleton has stopped without them; empty while loading and once all of them came.
   *  A view that was already on screen says nothing extra for being offline: the page's own offline line does. */
  const missing = createMemo(() => {
    if (loading()) return [];
    const late = (options.offline() && cold()) || partial() ? pending() : [];
    return [...late, ...failed()].map(r => r.section).filter((s, i, all) => all.indexOf(s) === i);
  });
  /** Retry was pressed and its reads are under way: the notice says so instead of going away. */
  const retrying = createMemo(() => generation() > 0 && !loading() && pending().length > 0);
  return {
    loading, missing, pending, retrying,
    retry: () => { setGeneration(g => g + 1); options.retry?.(); },
  };
}
export type FirstView = ReturnType<typeof createFirstView>;
