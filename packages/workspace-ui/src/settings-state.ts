import { createEffect, createSignal, onCleanup } from "solid-js";
import { diffPaths, type CommandReceipt, type Problem } from "@cortex/workspace-domain";
import { useWs } from "./ctx";
import { parseRoute, type Route } from "./router";

const same = (a: unknown, b: unknown) => JSON.stringify(a) === JSON.stringify(b);
/** How many values differ, leaf by leaf, so the save bar can say "2 unsaved changes". */
export function diffCount(a: unknown, b: unknown): number {
  if (same(a, b)) return 0;
  // A list of plain values is a set of choices: taking one out is one change, not one for every item after it.
  const plain = (x: unknown[]) => x.every(v => typeof v !== "object");
  if (Array.isArray(a) && Array.isArray(b) && plain(a) && plain(b)) return Math.max(1, a.filter(x => !b.includes(x)).length + b.filter(x => !a.includes(x)).length);
  if (Array.isArray(a) && Array.isArray(b)) { let n = Math.abs(a.length - b.length); for (let i = 0; i < Math.min(a.length, b.length); i++) n += diffCount(a[i], b[i]); return n; }
  if (a && b && typeof a === "object" && typeof b === "object") return [...new Set([...Object.keys(a), ...Object.keys(b)])].reduce((n, k) => n + diffCount((a as Record<string, unknown>)[k], (b as Record<string, unknown>)[k]), 0);
  return 1;
}

export type SaveState = "conflict" | "review" | "unknown" | null;
type Entry = { key: string; dirty: () => boolean; save: () => Promise<boolean>; stays?: (to: Route) => boolean };
const NO_NOTE = Symbol("no base value");
/**
 * Unsaved settings. There is one draft per file the machine keeps, not per page, so two pages that write one file can never
 * conflict with each other. A draft only leaves here when the owner saves or discards it, including after a conflict.
 */
export function createSettingsState() {
  const [drafts, setDrafts] = createSignal<Record<string, unknown>>({});
  const [bases, setBases] = createSignal<Record<string, number>>({});
  const [origins, setOrigins] = createSignal<Record<string, unknown>>({});
  const [states, setStates] = createSignal<Record<string, SaveState>>({});
  const [notes, setNotes] = createSignal<Record<string, string>>({});
  const [failures, setFailures] = createSignal<Record<string, string>>({});
  const [scope, setScope] = createSignal<string | null>(null);
  const [pending, setPending] = createSignal<Route | string | null>(null);
  let current: Entry | null = null;
  const put = <T,>(set: (fn: (r: Record<string, T>) => Record<string, T>) => void, key: string, value: T | undefined) => set(r => {
    const next = { ...r };
    if (value === undefined) delete next[key]; else next[key] = value;
    return next;
  });
  const drop = (key: string) => { put(setFailures, key, undefined); put(setDrafts, key, undefined); put(setBases, key, undefined); put(setOrigins, key, undefined); put(setStates, key, undefined); };
  return {
    scope, setScope, pending, setPending,
    draft: (key: string) => drafts()[key], setDraft: (key: string, value: unknown) => put(setDrafts, key, value),
    base: (key: string) => bases()[key] as number | undefined, setBase: (key: string, value: number | undefined) => put(setBases, key, value),
    /** What the draft started from, so a conflict can say which keys moved on each side. */
    origin: (key: string) => key in origins() ? origins()[key] : NO_NOTE, setOrigin: (key: string, value: unknown) => put(setOrigins, key, value),
    state: (key: string) => states()[key] ?? null, setState: (key: string, value: SaveState) => put(setStates, key, value ?? undefined),
    failure: (key: string) => failures()[key] ?? null, setFailure: (key: string, value: string | null) => put(setFailures, key, value ?? undefined),
    note: (key: string) => notes()[key] ?? null, setNote: (key: string, value: string | null) => put(setNotes, key, value ?? undefined),
    register(entry: Entry) { current = entry; },
    unregister(key: string) { if (current?.key === key) current = null; },
    leaving(to: Route | string) {
      if (!current?.dirty()) return false;
      const route = typeof to === "string" ? parseRoute(...(to.split("?") as [string, string?])) : to;
      return !current.stays?.(route);
    },
    saveCurrent: async () => current ? current.save() : true,
    dropCurrent() { if (current) drop(current.key); },
    /** Unsaved drafts of this machine, which a profile switch cannot keep. */
    draftsOn: (machineId: string) => Object.keys(drafts()).filter(k => k.startsWith(`${machineId}:`)).length,
    clearDrafts(machineId: string) { for (const key of Object.keys(drafts())) if (key.startsWith(`${machineId}:`)) drop(key); },
  };
}
export type SettingsState = ReturnType<typeof createSettingsState>;

type DraftProps<T> = {
  key: () => string; saved: () => T; name: () => string; where: () => string;
  commit: (value: T, revision: number | undefined) => Promise<CommandReceipt>;
  /** Files the machine versions. The draft remembers what it was based on, so a later save can be refused. */
  revision?: () => number; onConflict?: () => void; saveNote?: () => string;
  /** Field-level mistakes. Save stays off while there are any, and the command refuses the same list. */
  problems?: (value: T) => Problem[];
  /** Pages that edit this same draft: moving between them is not leaving. */
  stays?: (to: Route) => boolean;
};
/** One editable file: the saved value, the draft over it, and the save rules that apply to every section. */
export function useDraft<T>(props: DraftProps<T>) {
  const ws = useWs();
  const s = ws.settings;
  const value = (): T => (s.draft(props.key()) as T | undefined) ?? props.saved();
  const count = () => diffCount(props.saved(), value());
  const dirty = () => count() > 0;
  const state = () => s.state(props.key());
  const problems = () => props.problems?.(value()) ?? [];
  const clear = () => { const key = props.key(); s.setFailure(key, null); s.setDraft(key, undefined); s.setBase(key, undefined); s.setOrigin(key, undefined); s.setState(key, null); };
  const set = (next: T) => {
    const key = props.key();
    s.setNote(key, null);
    s.setFailure(key, null);
    if (s.base(key) === undefined && props.revision) { s.setBase(key, props.revision()); s.setOrigin(key, props.saved()); }
    if (same(next, props.saved())) clear(); else s.setDraft(key, next);
  };
  const save = async (): Promise<boolean> => {
    if (problems().length) return false;
    const key = props.key(), fault = ws.demo?.takeSaveFault() ?? null;
    if (fault === "conflict") props.onConflict?.();
    const receipt = await props.commit(value(), s.base(key) ?? props.revision?.());
    // The command may or may not have landed: the screen must not claim either until the machine is read again.
    if (fault === "timeout") { s.setState(key, "unknown"); return false; }
    if (!receipt.ok) {
      s.setFailure(key, receipt.message || "The machine refused this change. Nothing was saved; correct the value and try again.");
      if (["revision_conflict", "stale_version"].includes(receipt.outcome)) s.setState(key, "conflict");
      else if (["unknown_outcome", "unknown"].includes(receipt.outcome)) s.setState(key, "unknown");
      return false;
    }
    clear();
    s.setNote(key, props.saveNote?.() ?? `Saved on ${props.where()}`);
    return true;
  };
  /** What moved on the machine and in the draft since it began, by key. A key in both is a collision. */
  const changes = () => {
    const origin = s.origin(props.key());
    if (origin === NO_NOTE) return { there: [] as string[], mine: [] as string[], both: [] as string[] };
    const there = diffPaths(origin, props.saved()), mine = diffPaths(origin, value());
    return { there, mine, both: there.filter(k => mine.includes(k)) };
  };
  const review = () => s.setState(props.key(), "review");
  /** After a review, on purpose: the draft is written against what the machine has now. */
  const reapply = () => { const key = props.key(); s.setBase(key, props.revision?.()); s.setState(key, null); return save(); };
  /** After an unknown result: what the machine says now is the truth. */
  const reread = () => { if (same(value(), props.saved())) clear(); else s.setState(props.key(), null); };
  createEffect(() => {
    const key = props.key();
    s.register({ key, dirty, save, stays: props.stays });
    onCleanup(() => s.unregister(key));
  });
  return { value, set, count, dirty, state, failure: () => s.failure(props.key()), save, discard: clear, review, reapply, reread, changes, problems, note: () => s.note(props.key()), clearNote: () => s.setNote(props.key(), null), name: props.name, where: props.where };
}
export type Draft<T> = ReturnType<typeof useDraft<T>>;
