import { createSignal } from "solid-js";
import type { FileRead, FileSaveResult } from "@cortex/workspace-domain";

/** The machine's bound for one file, and what one request can carry once the text is escaped. */
const MAX_FILE_BYTES = 512 * 1024, MAX_REQUEST_BYTES = 900_000;
const AUTOSAVE_MS = 1000;
export const UNCONFIRMED_WRITER = "unconfirmed";

export type BufferIo = {
  read(checkoutId: string, path: string): Promise<FileRead>;
  save(checkoutId: string, path: string, text: string, revision: string | null): Promise<FileSaveResult>;
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
  const texts = new Map<string, string>();
  const timers = new Map<string, ReturnType<typeof setTimeout>>();
  const listeners = new Set<(key: string, text: string, undoable: boolean) => void>();
  /** Moves on every write, so a read that started before one cannot be taken for what the disk holds after it. */
  const writes = new Map<string, number>();
  const wrote = (key: string) => writes.set(key, (writes.get(key) ?? 0) + 1);
  let autosave = false;

  const find = (key: string) => list().find(b => b.key === key);
  const update = (key: string, patch: Partial<FileBuffer>) => setList(all => all.map(b => b.key === key ? { ...b, ...patch } : b));
  const cancel = (key: string) => { clearTimeout(timers.get(key)); timers.delete(key); };
  const textOf = (key: string) => texts.get(key) ?? "";
  const canAutosave = (b: FileBuffer) => b.dirty && !b.saving && !b.conflict && !b.unconfirmed && !b.writer && !b.readOnly;
  const schedule = (key: string) => {
    cancel(key);
    const b = find(key);
    if (!autosave || !b || !canAutosave(b)) return;
    timers.set(key, setTimeout(() => { timers.delete(key); const now = find(key); if (autosave && now && canAutosave(now)) void save(key); }, AUTOSAVE_MS));
  };
  const replaceText = (b: FileBuffer, text: string, undoable: boolean) => {
    texts.set(b.key, text);
    for (const listener of listeners) listener(b.key, text, undoable);
  };
  const adopt = (b: FileBuffer, read: FileRead, text: string) => {
    io.remember(b.projectId, b.checkoutId, b.path, text);
    return { revision: read.revision ?? null, base: text, dirty: false, conflict: null, unconfirmed: false, refusal: null, missing: null, readOnly: refusalOf(read, text), writer: read.writer ?? null, eol: lineEnding(text) ?? "\n" };
  };

  async function open(projectId: string, checkoutId: string, path: string) {
    const key = bufferKey(checkoutId, path);
    if (find(key)) return;
    setList(all => [...all, { key, projectId, checkoutId, path, loaded: false, missing: null, revision: null, base: "", dirty: false, saving: false, saved: false, conflict: null, unconfirmed: false, refusal: null, readOnly: null, writer: null, eol: "\n" }]);
    try {
      const read = await io.read(checkoutId, path), b = find(key);
      if (!b) return;
      if (typeof read.text !== "string") { update(key, { loaded: true, missing: read.reason ?? "This file could not be read." }); return; }
      texts.set(key, read.text);
      update(key, { ...adopt(b, read, read.text), loaded: true });
    } catch (error) {
      if (find(key)) update(key, { loaded: true, missing: error instanceof Error ? error.message : "This file could not be read." });
    }
  }

  /** The owner's text changed. Only the flags reach the screens. */
  function edit(key: string, text: string) {
    const b = find(key);
    if (!b) return;
    texts.set(key, text);
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
    const b = find(key);
    if (!b || !b.loaded || b.saving || b.readOnly) return;
    cancel(key);
    const sent = textOf(key);
    if (tooLarge(sent)) { update(key, { refusal: "This file is too large to save here. Your text is kept." }); return; }
    update(key, { saving: true, refusal: null });
    wrote(key);
    const result = await io.save(b.checkoutId, b.path, sent, b.revision).catch((): FileSaveResult => ({ outcome: "unknown" }));
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
    if (!b?.conflict) return;
    update(key, { revision: b.conflict.revision, base: b.conflict.text ?? "", dirty: true, conflict: null, unconfirmed: false });
    await save(key);
  }

  return {
    list, find, open, edit, save, refresh, reload, keepMine,
    text: textOf,
    unsaved: () => list().filter(hasUnsaved),
    /** Drops a buffer. Screens call this only for a clean one or after the owner chose to discard. */
    close(key: string) { cancel(key); texts.delete(key); setList(all => all.filter(b => b.key !== key)); },
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
