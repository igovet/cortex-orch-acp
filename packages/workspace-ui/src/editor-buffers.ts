import { createSignal } from "solid-js";
import type { FileRead, FileSaveResult } from "@cortex/workspace-domain";
import { COPY } from "./copy";
import { CodeRefusal, leaseMessage, type CodeLeaseIo } from "./code-support";
import { same } from "./stable";

/** The machine's bound for one file, and what one request can carry once the text is escaped. */
const MAX_FILE_BYTES = 512 * 1024, MAX_REQUEST_BYTES = 900_000;
const AUTOSAVE_MS = 1000;
export const UNCONFIRMED_WRITER = "unconfirmed";

export type BufferIo = {
  read(checkoutId: string, path: string): Promise<FileRead>;
  lease?: CodeLeaseIo;
  taskTitle?(id: string): string | undefined;
  /** What the rest of the app shows for this file, such as search. */
  remember(projectId: string, checkoutId: string, path: string, text: string): void;
};
export type DiskConflict = { revision: string | null; text: string | null };
/** A record replaced on each change, so a screen sees it move. `text` alone stays out of that: a keystroke re-renders nothing. */
export type FileBuffer = {
  key: string; projectId: string; checkoutId: string; path: string;
  loaded: boolean;
  /** Why there is no text to edit, as the machine said. */
  missing: string | null;
  revision: string | null; base: string; dirty: boolean;
  saving: boolean; saved: boolean;
  lease: "idle" | "acquiring" | "held";
  leaseNotice: string | null;
  conflict: DiskConflict | null;
  /** The reply was lost and a fresh read did not show the owner's text. It is never treated as clean. */
  unconfirmed: boolean;
  refusal: string | null; readOnly: string | null; writer: string | null;
  /** How this file ends its lines, so saving never rewrites them. */
  eol: "\n" | "\r\n";
};
export const bufferKey = (checkoutId: string, path: string) => `${checkoutId}|${path}`;
export const hasUnsaved = (b: FileBuffer) => b.dirty || b.unconfirmed;

function lineEnding(text: string): "\n" | "\r\n" | null {
  const crlf = text.match(/\r\n/g)?.length ?? 0, breaks = text.match(/\r\n|\r|\n/g)?.length ?? 0, lf = text.match(/(?<!\r)\n/g)?.length ?? 0;
  if (crlf === 0) return breaks === lf ? "\n" : null;
  return crlf === breaks ? "\r\n" : null;
}
/** One line ending throughout is what the editor can hold without changing bytes it was not asked to change. */
const refusalOf = (read: FileRead, text: string) => read.readOnly ?? (lineEnding(text) === null ? "This file mixes line endings, so saving it here would rewrite them." : null);

function tooLarge(text: string) {
  const encode = new TextEncoder();
  return encode.encode(text).length > MAX_FILE_BYTES || encode.encode(JSON.stringify(text)).length > MAX_REQUEST_BYTES;
}

/**
 * Open files and the owner's unsaved text. They live above every screen, so moving around, closing a panel or a Cortex
 * restart never touches them. Nothing here becomes clean until the machine confirms the bytes.
 */
export function createBuffers(io: BufferIo) {
  const [list, setList] = createSignal<FileBuffer[]>([]);
  const active = new Map<string, { token: string; deadline: number; timer?: ReturnType<typeof setTimeout>; expiry?: ReturnType<typeof setTimeout> }>();
  const pending = new Map<string, { cancelled: boolean; done: Promise<void> }>();
  const releasing = new Map<string, Promise<unknown>>();
  const texts = new Map<string, string>();
  const timers = new Map<string, ReturnType<typeof setTimeout>>();
  const textListeners = new Set<(key: string) => void>();
  const listeners = new Set<(key: string, text: string, undoable: boolean) => void>();
  /** Moves on every write, so a read that started before one cannot be taken for what the disk holds after it. */
  const writes = new Map<string, number>();
  const wrote = (key: string) => writes.set(key, (writes.get(key) ?? 0) + 1);
  let autosave = false;

  const find = (key: string) => list().find(b => b.key === key);
  // A patch that changes nothing keeps the record, so the disk poll on a quiet file wakes no screen. A conflict is compared
  // as data: each poll of a file changed on disk reads it as a new object with the same revision and text.
  const update = (key: string, patch: Partial<FileBuffer>) => {
    const b = find(key);
    if (!b || (Object.keys(patch) as (keyof FileBuffer)[]).every(k => same(b[k], patch[k]))) return;
    setList(all => all.map(b => b.key === key ? { ...b, ...patch } : b));
  };
  const cancel = (key: string) => { clearTimeout(timers.get(key)); timers.delete(key); };
  const textOf = (key: string) => texts.get(key) ?? "";
  const canAutosave = (b: FileBuffer) => b.lease === "held" && b.dirty && !b.saving && !b.conflict && !b.unconfirmed && !b.readOnly;
  const schedule = (key: string) => {
    cancel(key);
    const b = find(key);
    if (!autosave || !b || !canAutosave(b)) return;
    timers.set(key, setTimeout(() => { timers.delete(key); const now = find(key); if (autosave && now && canAutosave(now)) void save(key); }, AUTOSAVE_MS));
  };
  function release(key: string, notice?: string) {
    const request = pending.get(key);
    if (request) request.cancelled = true;
    const held = active.get(key), b = find(key);
    cancel(key);
    if (held) {
      clearTimeout(held.timer);
      clearTimeout(held.expiry);
      active.delete(key);
      if (b) void releaseToken(b, held.token);
    }
    update(key, { lease: "idle", ...(notice ? { leaseNotice: notice } : {}) });
  }

  async function releaseToken(b: FileBuffer, token: string) {
    const result = io.lease!.release(b, token).catch(() => undefined);
    releasing.set(b.key, result);
    await result;
    if (releasing.get(b.key) === result) releasing.delete(b.key);
  }

  function reason(error: unknown) {
    const holder = error instanceof CodeRefusal ? error.holderTaskId : undefined;
    return leaseMessage(error, holder ? io.taskTitle?.(holder) : undefined);
  }

  async function acquire(key: string) {
    const b = find(key), lease = io.lease;
    if (!lease || !b?.loaded || b.missing || b.readOnly || pending.has(key) || active.has(key)) return;
    let finish!: () => void;
    const done = new Promise<void>(resolve => { finish = resolve; });
    const request = { cancelled: false, done };
    pending.set(key, request);
    update(key, { lease: "acquiring", leaseNotice: null });
    try {
      if (releasing.has(key)) await releasing.get(key);
      if (request.cancelled) return;
      const start = Date.now(), grant = await lease.acquire(b);
      if (request.cancelled || start + grant.expiresInMs <= Date.now()) {
        await releaseToken(b, grant.leaseToken);
        if (!request.cancelled) update(key, { lease: "idle", leaseNotice: COPY.codeLeaseLost });
        return;
      }
      const held = { token: grant.leaseToken, deadline: 0 } as NonNullable<ReturnType<typeof active.get>>;
      active.set(key, held);
      const arm = (issuedAt: number, expiresInMs: number) => {
        held.deadline = issuedAt + expiresInMs;
        clearTimeout(held.expiry);
        held.expiry = setTimeout(() => { if (active.get(key) === held) release(key, COPY.codeLeaseLost); }, Math.max(0, held.deadline - Date.now()));
        held.timer = setTimeout(async () => {
          if (active.get(key) !== held) return;
          try {
            const tick = Date.now();
            const renewed = await lease.renew(b, held.token);
            if (active.get(key) === held) {
              if (Date.now() >= held.deadline) release(key, COPY.codeLeaseLost);
              else arm(tick, renewed.expiresInMs);
            }
          } catch (error) {
            if (active.get(key) === held) release(key, error instanceof CodeRefusal && error.code === "task_file_lease_held" ? reason(error) : COPY.codeLeaseLost);
          }
        }, Math.max(0, issuedAt + grant.renewAfterMs - Date.now()));
      };
      arm(start, grant.expiresInMs);
      update(key, { lease: "held", leaseNotice: null });
      // Compare the disk before editing or saving resumed text. No automatic resubmission on reacquire.
      void refresh(key);
    } catch (error) {
      if (!request.cancelled) update(key, { lease: "idle", leaseNotice: reason(error) });
    } finally {
      pending.delete(key);
      finish();
    }
  }

  const replaceText = (b: FileBuffer, text: string, undoable: boolean) => {
    texts.set(b.key, text);
    for (const listener of listeners) listener(b.key, text, undoable);
    for (const listener of textListeners) listener(b.key);
  };
  const adopt = (b: FileBuffer, read: FileRead, text: string) => {
    io.remember(b.projectId, b.checkoutId, b.path, text);
    return { revision: read.revision ?? null, base: text, dirty: false, conflict: null, unconfirmed: false, refusal: null, missing: null, readOnly: refusalOf(read, text), writer: read.writer ?? null, eol: lineEnding(text) ?? "\n" };
  };

  async function open(projectId: string, checkoutId: string, path: string) {
    const key = bufferKey(checkoutId, path);
    if (find(key)) return;
    setList(all => [...all, { key, projectId, checkoutId, path, loaded: false, missing: null, revision: null, base: "", dirty: false, saving: false, saved: false, conflict: null, unconfirmed: false, refusal: null, readOnly: null, writer: null, lease: "idle", leaseNotice: null, eol: "\n" }]);
    const opened = writes.get(key);
    try {
      const read = await io.read(checkoutId, path), b = find(key);
      if (!b || writes.get(key) !== opened) return;
      if (typeof read.text !== "string") { update(key, { loaded: true, missing: read.reason ?? "This file could not be read." }); return; }
      texts.set(key, read.text);
      update(key, { ...adopt(b, read, read.text), loaded: true });
    } catch (error) {
      if (find(key) && writes.get(key) === opened) update(key, { loaded: true, missing: error instanceof Error ? error.message : "This file could not be read." });
    }
  }

  /** The owner's text changed. Only the flags reach the screens. */
  function edit(key: string, text: string) {
    const b = find(key);
    if (!b || b.lease !== "held") return;
    texts.set(key, text);
    for (const listener of textListeners) listener(key);
    const dirty = text !== b.base;
    if (dirty !== b.dirty || b.saved || b.refusal) update(key, { dirty, saved: false, refusal: null });
    schedule(key);
  }

  /** Sees what the disk holds now. A clean buffer follows it; owner text is never replaced from here. */
  async function refresh(key: string) {
    const b = find(key);
    if (!b || !b.loaded || b.saving) return;
    const seen = writes.get(key);
    let read: FileRead;
    try { read = await io.read(b.checkoutId, b.path); } catch { return; }
    const now = find(key);
    if (!now || now.saving || writes.get(key) !== seen) return;
    if (typeof read.text !== "string") {
      if (now.dirty) update(key, { conflict: { revision: null, text: null } });
      else if (!now.missing) update(key, { missing: read.reason ?? "This file could not be read." });
      return;
    }
    const facts = { writer: read.writer ?? null, readOnly: refusalOf(read, read.text) };
    if (read.revision === now.revision && !now.missing) {
      update(key, { ...facts, conflict: null });
      if (now.writer && !facts.writer) schedule(key);
      return;
    }
    if (read.text === textOf(key) || !hasUnsaved(now)) {
      const same = read.text === textOf(key);
      if (!same) replaceText(now, read.text, false);
      texts.set(key, read.text);
      update(key, { ...adopt(now, read, read.text), saved: false });
      return;
    }
    update(key, { ...facts, conflict: { revision: read.revision ?? null, text: read.text } });
  }

  async function save(key: string) {
    const b = find(key), held = active.get(key);
    if (!b || !b.loaded || b.saving || b.readOnly || b.conflict || !hasUnsaved(b) || !held || !io.lease) return;
    if (Date.now() >= held.deadline) { release(key, COPY.codeLeaseLost); return; }
    cancel(key);
    const sent = textOf(key);
    if (tooLarge(sent)) { update(key, { refusal: "This file is too large to save here. Your text is kept." }); return; }
    update(key, { saving: true, refusal: null });
    wrote(key);
    let result: FileSaveResult;
    try { result = await io.lease.save(b, sent, b.revision, held.token); }
    catch (error) {
      if (error instanceof CodeRefusal) {
        if (error.code === "edit_lease_lost" || error.code === "task_file_lease_held") {
          if (active.get(key) === held) release(key, reason(error));
          update(key, { saving: false });
          return;
        }
        result = { outcome: "refused", message: error.message };
      } else result = { outcome: "unknown" };
    }
    wrote(key);
    const settle = (patch: Partial<FileBuffer>) => update(key, { saving: false, ...patch });
    if (!find(key)) return;
    if (result.outcome === "saved") {
      io.remember(b.projectId, b.checkoutId, b.path, sent);
      const dirty = textOf(key) !== sent;
      settle({ revision: result.revision, base: sent, dirty, saved: !dirty, conflict: null, unconfirmed: false });
      if (dirty) schedule(key);
    } else if (result.outcome === "conflict") {
      settle({ conflict: { revision: result.revision, text: null } });
      await refresh(key);
    } else if (result.outcome === "writer_active") settle({ writer: result.writer });
    else if (result.outcome === "refused") settle({ refusal: result.message });
    else await confirm(key, sent);
  }

  /** A lost reply says nothing about the write, so only the disk can. It is read once and never written again. */
  async function confirm(key: string, sent: string) {
    const read = await io.read(find(key)!.checkoutId, find(key)!.path).catch(() => null);
    const b = find(key);
    if (!b) return;
    if (read?.text === sent) {
      const dirty = textOf(key) !== sent;
      update(key, { saving: false, revision: read.revision ?? null, base: sent, dirty, saved: !dirty, unconfirmed: false });
    } else update(key, { saving: false, unconfirmed: true });
  }

  /** The owner took the disk's version. Their text stays one undo away, so even this loses nothing. */
  function reload(key: string) {
    const b = find(key);
    if (!b?.conflict || b.conflict.text === null) return;
    const { revision, text } = b.conflict;
    replaceText(b, text, true);
    update(key, { revision, base: text, dirty: false, saved: false, conflict: null, unconfirmed: false, refusal: null, eol: lineEnding(text) ?? "\n" });
    io.remember(b.projectId, b.checkoutId, b.path, text);
  }

  /** The owner keeps their text over what they were just shown. It is still a save bound to that exact revision. */
  async function keepMine(key: string) {
    const b = find(key);
    if (!b?.conflict || b.lease !== "held" || b.saving || b.readOnly) return;
    update(key, { revision: b.conflict.revision, base: b.conflict.text ?? "", dirty: true, conflict: null, unconfirmed: false });
    await save(key);
  }

  return {
    list, find, open, edit, save, refresh, reload, keepMine, acquire, release,
    hasPendingLease: () => pending.size > 0,
    async releaseAll() {
      for (const key of new Set([...active.keys(), ...pending.keys()])) release(key);
      await Promise.all([...pending.values()].map(request => request.done).concat([...releasing.values()].map(result => result.then(() => undefined))));
    },
    subscribeText(listener: (key: string) => void) { textListeners.add(listener); return () => { textListeners.delete(listener); }; },
    text: textOf,
    unsaved: () => list().filter(hasUnsaved),
    /** Drops a buffer. Screens call this only for a clean one or after the owner chose to discard. */
    close(key: string) { if (find(key)?.saving) return; release(key); wrote(key); texts.delete(key); setList(all => all.filter(b => b.key !== key)); },
    setAutosave(on: boolean) { autosave = on; for (const b of list()) schedule(b.key); },
    subscribe(listener: (key: string, text: string, undoable: boolean) => void) { listeners.add(listener); return () => { listeners.delete(listener); }; },
  };
}
export type Buffers = ReturnType<typeof createBuffers>;

/** A few words for where this file stands, in the order that matters to the owner. */
export function saveWord(b: FileBuffer) {
  if (b.saving) return "Saving…";
  if (b.unconfirmed) return "Save not confirmed";
  if (b.conflict) return "Changed on disk";
  if (b.refusal) return "Not saved";
  if (b.dirty) return "Unsaved";
  return b.saved ? "Saved" : "";
}
