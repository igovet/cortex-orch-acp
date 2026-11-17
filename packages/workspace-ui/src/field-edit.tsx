import { createEffect, createMemo, createSignal, createUniqueId, on, onCleanup, Show, type JSX } from "solid-js";
import type { CommandReceipt } from "@cortex/workspace-domain";
import { place } from "./kit";

/** How long a saved text stays in view while the machine's next read has not yet shown it. After that the machine's value is shown. */
export const SETTLE_MS = 5000;
export const NOTHING_TO_SAVE = "No changes to save.";
export const NOTHING_TO_CANCEL = "No changes to cancel.";
export const WAIT_FOR_SAVE = "Wait for the save to finish.";
export const NOT_SAVED = "The machine did not save this change. Your text is kept; try again.";
const filled = (label: string) => (text: string) => text.trim() ? null : `${label} cannot be empty.`;

export type FieldEditProps = {
  /** What the machine last reported. A new value never replaces text the owner is still editing. */
  value: string;
  /** The field's accessible name, such as "Project name". */
  label: string;
  /** Writes the trimmed text. Only an `ok` receipt counts as saved; anything else keeps the text and shows the message. */
  save: (text: string) => Promise<CommandReceipt>;
  /** Why this text cannot be saved, or null. Empty text is refused by default. */
  problem?: (text: string) => string | null;
  /** Why the field cannot be edited at all, or undefined. */
  off?: string;
  placeholder?: string;
  class?: string;
  inputClass?: string;
  /** In the title band, where a row has no room below it: the error opens in a box under the field instead. */
  band?: boolean;
  /**
   * A name edited where it stands: closed, it is a button showing the saved text, and a click opens the field. Without this
   * the field is always open, with Save and Cancel beside it.
   */
  closed?: { text: (saved: string) => JSX.Element; class?: string; label?: (saved: string) => string; start?: () => string };
};

/**
 * One editable text with an explicit save. Typing changes only a local draft; Save or Enter writes it, Cancel or Escape puts the
 * saved text back. Save is off, with its reason in its own box, while nothing changed or the text is not valid. While a save is
 * under way the field and its buttons wait; a refused save keeps the typed text and says why under the field until the next Save
 * or Cancel. A read of the machine never touches an unsaved draft.
 */
export function FieldEdit(props: FieldEditProps) {
  const msgId = `fe-${createUniqueId()}`;
  const [open, setOpen] = createSignal(!props.closed);
  const [draft, setDraft] = createSignal<string | null>(null);
  const [saving, setSaving] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  // A save the machine accepted but has not read back yet: its text stays until the value moves, or SETTLE_MS passes.
  const [held, setHeld] = createSignal<{ sent: string; base: string } | null>(null);
  let box: HTMLSpanElement | undefined, input: HTMLInputElement | undefined, saveButton: HTMLButtonElement | undefined, trigger: HTMLButtonElement | undefined, tip: HTMLDivElement | undefined;
  let settle: ReturnType<typeof setTimeout> | undefined;
  onCleanup(() => clearTimeout(settle));
  createEffect(on(() => props.value, value => { const h = held(); if (h && value !== h.base) { clearTimeout(settle); setHeld(null); } }, { defer: true }));

  const saved = () => held()?.sent ?? props.value;
  // A memo, so the field's value is written only when the text itself changes, never because a read rebuilt the record.
  const text = createMemo(() => draft() ?? saved());
  const dirty = () => text().trim() !== saved().trim();
  const problem = () => (props.problem ?? filled(props.label))(text());
  const saveWhy = () => props.off ?? (dirty() ? problem() ?? undefined : NOTHING_TO_SAVE);
  const cancelWhy = () => saving() ? WAIT_FOR_SAVE : !props.closed && !dirty() && !error() ? NOTHING_TO_CANCEL : undefined;
  /** Focus goes back only when it was in the editor, or was dropped by a press: never away from wherever the owner moved it. */
  const focusWasHere = () => { const at = document.activeElement; return !at || at === document.body || !!box?.contains(at); };
  const finish = (refocus: boolean) => {
    setError(null);
    if (props.closed) { setOpen(false); if (refocus) queueMicrotask(() => trigger?.focus()); }
    else if (refocus) queueMicrotask(() => input?.focus());
  };
  const cancel = () => { if (saving()) return; const refocus = focusWasHere(); setDraft(null); finish(refocus); };
  const save = async () => {
    if (saving() || saveWhy()) return;
    const sent = text().trim(), base = props.value;
    setSaving(true); setError(null);
    let receipt: CommandReceipt | null = null;
    try { receipt = await props.save(sent); } catch { receipt = null; }
    setSaving(false);
    if (!receipt?.ok) { setError(receipt?.message || NOT_SAVED); return; }
    const refocus = focusWasHere();
    if (props.value === base && base !== sent) {
      clearTimeout(settle);
      setHeld({ sent, base });
      settle = setTimeout(() => setHeld(null), SETTLE_MS);
    }
    setDraft(null);
    finish(refocus);
  };
  const begin = () => {
    if (props.off) return;
    setDraft(props.closed?.start?.() ?? saved());
    setError(null);
    setOpen(true);
    queueMicrotask(() => { input?.focus(); input?.select(); });
  };
  const onKeyDown = (e: KeyboardEvent) => {
    if (e.isComposing) return;
    // Enter presses Save, so the press shows its loader, or its reason when there is nothing to save.
    if (e.key === "Enter") { e.preventDefault(); saveButton?.click(); return; }
    if (e.key === "Escape" && (props.closed || dirty() || error())) { e.preventDefault(); e.stopPropagation(); cancel(); }
  };
  // An opened name that was not changed closes when focus leaves it. A changed one stays open until Save or Cancel.
  const onFocusOut = (e: FocusEvent) => {
    if (!props.closed || saving() || error() || dirty()) return;
    const to = e.relatedTarget as Node | null;
    if (to && box?.contains(to)) return;
    setDraft(null); setOpen(false);
  };
  let tipShown = false;
  createEffect(() => {
    const message = error(), shown = open();
    if (!props.band || !tip || typeof tip.showPopover !== "function") return;
    try {
      if (message && shown) { if (!tipShown) { tip.showPopover(); tipShown = true; } if (box) place(tip, box, "left"); }
      else if (tipShown) { tipShown = false; tip.hidePopover(); }
    } catch { tipShown = false; }
  });

  const editor = () => <span ref={box} class={`ws-fe${props.band ? " ws-fe-band" : ""}${props.class ? ` ${props.class}` : ""}`} data-saving={saving() ? "" : undefined} onFocusOut={onFocusOut}>
    <span class="row ws-fe-line">
      <input ref={input} class={`inp ws-fe-in${props.inputClass ? ` ${props.inputClass}` : ""}`} aria-label={props.label} placeholder={props.placeholder} value={text()}
        readOnly={saving() || !!props.off} aria-disabled={props.off ? "true" : undefined} data-why={props.off} aria-invalid={dirty() && problem() ? "true" : undefined} aria-describedby={msgId}
        onInput={e => setDraft(e.currentTarget.value)} onKeyDown={onKeyDown} />
      <button ref={saveButton} type="button" class="btn btn-primary" aria-busy={saving() ? "true" : undefined} aria-disabled={saveWhy() ? "true" : undefined} data-why={saveWhy()} aria-describedby={msgId}
        onClick={() => { if (!saveWhy()) void save(); }}>Save</button>
      <button type="button" class="btn btn-line" aria-disabled={cancelWhy() ? "true" : undefined} data-why={cancelWhy()} aria-describedby={msgId} onClick={() => { if (!cancelWhy()) cancel(); }}>Cancel</button>
    </span>
    <Show when={props.band} fallback={<span id={msgId} class="err ws-fe-err" role="alert">{error() ?? ""}</span>}>
      <div ref={el => { tip = el; tipShown = false; }} id={msgId} class="l-why-tip ws-fe-tip" popover="manual" role="alert">{error() ?? ""}</div>
    </Show>
  </span>;

  return <Show when={!props.closed || open()} fallback={
    <button ref={trigger} type="button" class={props.closed!.class ?? "ws-edit"} aria-label={props.closed!.label?.(saved())} aria-disabled={props.off ? "true" : undefined} data-why={props.off} onClick={begin}>{props.closed!.text(saved())}</button>
  }>{editor()}</Show>;
}

/** A name edited where it stands: a click opens it with Save and Cancel; Enter saves and Escape leaves it as it was. */
export function NameEdit(props: { value: string; label: string; onSave: (name: string) => Promise<CommandReceipt>; disabled?: string; class?: string; hint?: string; problem?: (name: string) => string | null }) {
  return <FieldEdit value={props.value} label={props.label} save={props.onSave} off={props.disabled} placeholder={props.hint} problem={props.problem} inputClass="ws-edit-in"
    closed={{ text: saved => saved, class: `ws-edit${props.class ? ` ${props.class}` : ""}`, label: saved => `${props.label}: ${saved}` }} />;
}
