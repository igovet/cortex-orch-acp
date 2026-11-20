/**
 * Structural sharing. A read of the machine rebuilds every record, even when nothing in it changed, and a new object is
 * enough to make a list redraw its rows. This keeps every part of `next` that equals `prev` as the old object, so a poll
 * that says nothing new changes nothing on the screen, and one that says something changes only that.
 */
const plain = (v: unknown): v is Record<string, unknown> => { if (typeof v !== "object" || v === null) return false; const p = Object.getPrototypeOf(v); return p === Object.prototype || p === null; };
const idOf = (v: unknown) => plain(v) ? v.id ?? v.taskId ?? v.key : undefined;

function shareList(prev: unknown[], next: unknown[]) {
  const byId = new Map<unknown, unknown>();
  for (const item of prev) { const id = idOf(item); if (id !== undefined) byId.set(id, item); }
  // Sequence is a separate key space and is safe only when unique on both sides.
  const sequenceOf = (item: unknown) => idOf(item) === undefined && plain(item) && typeof item.sequence === "number" ? item.sequence : undefined;
  const counts = (items: unknown[]) => { const m = new Map<number, number>(); for (const item of items) { const s = sequenceOf(item); if (s !== undefined) m.set(s, (m.get(s) ?? 0) + 1); } return m; };
  const prevCounts = counts(prev), nextCounts = counts(next), bySequence = new Map<number, unknown>();
  for (const item of prev) { const s = sequenceOf(item); if (s !== undefined && prevCounts.get(s) === 1) bySequence.set(s, item); }
  const out = next.map((item, i) => {
    const id = idOf(item), s = sequenceOf(item);
    if (id !== undefined) return share(byId.has(id) ? byId.get(id) : prev[i], item);
    if (s !== undefined) return share(prevCounts.get(s) === 1 && nextCounts.get(s) === 1 ? bySequence.get(s) : undefined, item);
    return share(prev[i], item);
  });
  return out.length === prev.length && out.every((item, i) => item === prev[i]) ? prev : out;
}

/** Equal as data, plain values all the way down. It stops at the first difference and builds no text, so comparing a
 *  repository's facts with the last ones costs a walk, not a serialization of megabytes of diffs. */
export function same(a: unknown, b: unknown): boolean {
  if (Object.is(a, b)) return true;
  if (Array.isArray(a)) {
    if (!Array.isArray(b) || a.length !== b.length) return false;
    for (let i = 0; i < a.length; i++) if (!same(a[i], b[i])) return false;
    return true;
  }
  if (!plain(a) || !plain(b)) return false;
  const keys = Object.keys(a);
  if (keys.length !== Object.keys(b).length) return false;
  for (const key of keys) if (!(key in b) || !same(a[key], b[key])) return false;
  return true;
}

export function share<T>(prev: T, next: T): T {
  if (Object.is(prev, next)) return prev;
  if (Array.isArray(prev) && Array.isArray(next)) return shareList(prev, next) as T;
  if (!plain(prev) || !plain(next)) return next;
  const keys = Object.keys(next);
  let same = keys.length === Object.keys(prev).length;
  const out: Record<string, unknown> = {};
  for (const key of keys) {
    out[key] = key in prev ? share(prev[key], next[key]) : next[key];
    if (out[key] !== prev[key] || !(key in prev)) same = false;
  }
  return (same ? prev : out) as T;
}
