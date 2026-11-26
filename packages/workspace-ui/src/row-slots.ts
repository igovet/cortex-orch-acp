import { createComputed, createSignal, onCleanup, untrack, type Accessor, type Setter } from "solid-js";

/**
 * Each row of a long feed reads its own entry, and is told only when that entry changed. The store keeps an unchanged
 * record the same object through every read (`share` in stable.ts), so a new page put above, or a streamed update to the
 * last row, wakes only the rows whose entries are new or different: the thousands of rows already drawn are not evaluated
 * again. `entries` maps each row's key to its entry; `slot(key)` is called once per drawn row, inside that row's owner.
 */
export function createRowSlots<T>(entries: Accessor<ReadonlyMap<string, T>>) {
  const slots = new Map<string, { get: Accessor<T | undefined>; set: Setter<T | undefined> }>();
  createComputed(() => { for (const [key, entry] of entries()) slots.get(key)?.set(() => entry); });
  return (key: string): Accessor<T | undefined> => {
    let held = slots.get(key);
    if (!held) {
      const [get, set] = createSignal<T | undefined>(untrack(entries).get(key));
      slots.set(key, held = { get, set });
      onCleanup(() => { if (slots.get(key) === held) slots.delete(key); });
    }
    return held.get;
  };
}

/** Equal lists of the same items, in order: a list rebuilt around unchanged records is not a change. */
export const sameItems = <T,>(a: readonly T[], b: readonly T[]) => a.length === b.length && a.every((item, i) => item === b[i]);
