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
  const out = next.map((item, i) => { const id = idOf(item); return share(id !== undefined && byId.has(id) ? byId.get(id) : prev[i], item); });
  return out.length === prev.length && out.every((item, i) => item === prev[i]) ? prev : out;
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
