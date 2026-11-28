import { createMemo, createSignal, onCleanup, type Accessor } from "solid-js";
import type { Decision, Task } from "@cortex/workspace-domain";

const PREFIX = "cw.unsent-text.v1:";
const MAX_TEXT = 32_000;
type Draft = { text: string; version: string };
let edit = 0;

const storage = () => {
  try { return typeof localStorage === "undefined" ? undefined : localStorage; } catch { return undefined; }
};
const validVersion = (value: string | null): value is string => !!value && /^[a-z0-9.-]{1,128}$/.test(value);
const versionKey = (id: string, version: string) => id + ":" + version;

export function createLocalDrafts(namespace: readonly string[]) {
  const volatile = new Map<string, Draft | null>();
  const [change, changed] = createSignal(0);
  const failures = new Set<string>();
  const removed = new Set<string>();
  const storageChanged = (event: StorageEvent) => { if (event.key === null || event.key.startsWith(PREFIX)) changed(n => n + 1); };
  if (typeof window !== "undefined") {
    window.addEventListener("storage", storageChanged);
    onCleanup(() => window.removeEventListener("storage", storageChanged));
  }
  const key = (parts: readonly (string | null)[]) => PREFIX + JSON.stringify([...namespace, ...parts]);
  const read = (id: string): Draft | undefined => {
    change();
    if (volatile.has(id)) return volatile.get(id) ?? undefined;
    try {
      const local = storage(), version = local?.getItem(id) ?? null;
      if (!validVersion(version)) return undefined;
      if (removed.has(versionKey(id, version))) return undefined;
      const text = local?.getItem(versionKey(id, version));
      return text && text.length <= MAX_TEXT ? {text, version} : undefined;
    } catch { return undefined; }
  };
  // Each version has its own content key. Deleting a version can never erase
  // another window's newer text even when the two touches race.
  const remove = (id: string, version: string) => {
    try {
      const local = storage();
      if (!local) throw new Error("storage unavailable");
      local.removeItem(versionKey(id, version));
      removed.delete(versionKey(id, version));
      if (volatile.get(id)?.version === version) { volatile.delete(id); failures.delete(id); }
    } catch {
      removed.add(versionKey(id, version));
      if (volatile.get(id)?.version === version) volatile.delete(id);
      failures.add(id);
    }
    changed(n => n + 1);
  };
  return {
    key, read,
    warning(id: string) { change(); return failures.has(id); },
    set(id: string, text: string) {
      const previous = read(id);
      if (!text) { if (previous) remove(id, previous.version); return; }
      const version = globalThis.crypto?.randomUUID?.() ?? `${Date.now()}-${++edit}-${Math.random()}`;
      try {
        const local = storage();
        if (!local || text.length > MAX_TEXT) throw new Error("draft storage unavailable");
        local.setItem(versionKey(id, version), text);
        local.setItem(id, version);
        volatile.delete(id);
        failures.delete(id);
      } catch {
        volatile.set(id, { text, version });
        failures.add(id);
        try { storage()?.removeItem(versionKey(id, version)); } catch { /* storage may be unavailable */ }
      }
      if (previous) remove(id, previous.version);
      changed(n => n + 1);
    },
    confirmed: remove,
  };
}
export type LocalDrafts = ReturnType<typeof createLocalDrafts>;

/** Remove local copies when the owner explicitly forgets the corresponding phone pairing. */
export function discardPairingDrafts(namespace: readonly string[]) {
  const prefix = PREFIX + JSON.stringify(namespace).slice(0, -1) + ",";
  try {
    const local = storage();
    if (!local) return;
    for (let n = local.length - 1; n >= 0; n--) {
      const key = local.key(n);
      if (key?.startsWith(prefix)) local.removeItem(key);
    }
  } catch { /* storage is optional */ }
}

export function taskDraftScope(task: Task, machineId: string): readonly (string | null)[] {
  const binding = task.draftBinding;
  return [machineId, task.projectId, task.id, task.executionId,
    binding?.runtimeTaskId ?? null, binding?.ownerEpoch ?? null,
    binding?.sessionId ?? null];
}

export function questionDraftScope(base: readonly (string | null)[], ask: Pick<Decision, "id" | "draftBinding">) {
  const binding = ask.draftBinding;
  return [...(binding ? base.slice(0, 3) : base), "question", ask.id, binding?.executionId ?? null, binding?.ownerEpoch ?? null,
    binding?.sessionId ?? null, binding?.requirementRevision ?? null];
}

/** Park the exact submitted version behind the existing in-flight row. The
 * persistent copy stays available until a matching authoritative acknowledgement. */
export function createLocalInput(store: LocalDrafts | undefined, scope: Accessor<readonly (string | null)[] | undefined>) {
  const id = createMemo(() => { const parts = scope(); return parts && store ? store.key(parts) : undefined; });
  const [parked, park] = createSignal<string>();
  const record = () => { const current = id(); return current ? store?.read(current) : undefined; };
  return {
    enabled: () => !!id(),
    text: () => { const saved = record(); return saved && saved.version !== parked() ? saved.text : ""; },
    hasDraft: () => !!record(),
    warning: () => { const key = id(); return key ? store?.warning(key) : undefined; },
    set(text: string) { const key = id(); if (key) store?.set(key, text); },
    snapshot() {
      const key = id(), version = record()?.version;
      if (!key || !version || !store) return undefined;
      return { park: () => park(version), confirm: () => store.confirmed(key, version) };
    },
  };
}
