import { plainChoice } from "./selectors";
import { createEffect, createMemo, createSignal, createUniqueId, For, on, onCleanup, onMount, Show, useContext, type Accessor, type JSX } from "solid-js";
import { FEATURES, type AttachmentInput, type Message, type TaskSettings, type WorkspaceCommand } from "@cortex/workspace-domain";
import { attachmentFile, attachmentName, attachmentPreview, readAttachments, wireAttachments } from "./attachments";
import { autoGrow, reportHeight } from "./autogrow";
import { useWs } from "./ctx";
import { createUnsent, onScreen, ScreenActive } from "./freshness";
import { createLocalDictation } from "./voice";
import { I, MenuItem, Pop } from "./kit";
import { Action, ReasonSlot, Whole } from "./layout";
import { duration, taskSettings, type ComposerMode, type SessionModel } from "./session-model";

/** Draft bytes stay in memory until an explicit send or queue action. */
export function createAttachments() {
  const ws = useWs();
  const [files, setFiles] = createSignal<AttachmentInput[]>([]);
  const [error, setError] = createSignal(""), [busy, setBusy] = createSignal(false);
  let pending = Promise.resolve(), reading = 0;
  const add = (items: AttachmentInput[]) => setFiles([...files(), ...items]);
  const ingest = (items: File[]) => {
    if (!items.length) return;
    reading++; setBusy(true);
    pending = pending.then(async () => {
      try { const added = await readAttachments(items, files()); add(added); setError(""); }
      catch (e) { setError(e instanceof Error ? e.message : "This file could not be read."); }
    }).finally(() => { reading--; setBusy(reading > 0); });
  };
  /** `refused` keeps the bytes out and says why; `report` says so when the machine finds nothing to attach. */
  const pasteNative = (drop = false, how: { refused?: string; report?: boolean } = {}) => {
    reading++; setBusy(true);
    pending = pending.then(async () => {
      try {
        const added = await (drop ? ws.store.takeDroppedFiles() : ws.store.readClipboardImage());
        if (how.refused) { if (added.length) setError(how.refused); return; }
        if (!added.length) { if (how.report) setError("The clipboard has no image to attach."); return; }
        wireAttachments([...files(), ...added]); add(added); setError("");
      } catch (e) { setError(e instanceof Error ? e.message : "Clipboard is unavailable."); }
    }).finally(() => { reading--; setBusy(reading > 0); });
  };
  return { files, error, busy, ingest, pasteNative, refuse: setError, set: setFiles, add, remove: (item: AttachmentInput) => { setFiles(files().filter(f => f !== item)); setError(""); }, clear: () => { setFiles([]); setError(""); } };
}
export type Attachments = ReturnType<typeof createAttachments>;
export function attachmentEvents(a: Attachments, enabled: () => boolean = () => true) {
  const ws = useWs();
  const can = () => enabled() && ws.can(FEATURES.attachments);
  const refusal = () => ws.can(FEATURES.attachments) ? "Attachments can't be added here right now." : "Attachments are not available on this machine.";
  return {
    onDragOver: (e: DragEvent) => { if (e.dataTransfer?.types.some(t => t === "Files" || t === "text/uri-list")) { e.preventDefault(); if (e.dataTransfer) e.dataTransfer.dropEffect = can() ? "copy" : "none"; } },
    onDrop: (e: DragEvent) => { if (!e.dataTransfer) return; const files = [...e.dataTransfer.files]; if (!files.length && !e.dataTransfer.types.includes("text/uri-list")) return; e.preventDefault(); e.stopPropagation(); if (!can()) { a.refuse(refusal()); return; } if (files.length) a.ingest(files); else a.pasteNative(true); },
    onPaste: (e: ClipboardEvent) => {
      const clip = e.clipboardData;
      if (!clip) return;
      const types = [...(clip.types ?? [])];
      // Ordinary text stays an ordinary paste, whatever else the clipboard offers.
      if (types.includes("text/plain") || clip.getData?.("text/plain")) return;
      const named = [...(clip.files ?? [])], images = named.length ? [] : [...(clip.items ?? [])].flatMap(item => { const file = item.kind === "file" && item.type.startsWith("image/") ? item.getAsFile() : null; return file ? [file] : []; });
      const found = [...named, ...images];
      // No text will land in the field. WebKitGTK reports a clipboard image as nothing at all, or as only its text/html, so what it did not report is asked of the machine.
      e.preventDefault();
      if (found.length) { if (can()) a.ingest(found); else a.refuse(refusal()); return; }
      if (can()) a.pasteNative(false, { report: types.length > 0 }); else a.pasteNative(false, { refused: refusal() });
    },
  };
}
export const AttachmentStatus = (props: { a: Attachments }) => <>
  <Show when={props.a.busy()}><div class="cap muted" role="status">Reading attachments…</div></Show>
  <Show when={props.a.error()}><div class="cap st-warn" role="alert">{props.a.error()}</div></Show>
</>;
/** Attach keeps its place in every composer. When files cannot be added it stays, unusable, with the reason. */
export function AttachButton(props: { a: Attachments; disabled?: string; describedBy?: string }) {
  const ws = useWs();
  let input!: HTMLInputElement;
  const off = () => props.disabled ?? (ws.can(FEATURES.attachments) ? undefined : "Attachments are not available on this machine.");
  return <>
    <Show when={!off()}><input ref={input} type="file" multiple hidden aria-label="Choose files" onChange={e => { props.a.ingest([...(e.currentTarget.files ?? [])]); e.currentTarget.value = ""; }} /></Show>
    <button type="button" class="ib" aria-label="Attach files" aria-disabled={off() ? "true" : undefined} data-why={off()} aria-describedby={off() ? props.describedBy : undefined} onClick={() => { if (!off()) input.click(); }}><I n="clip" /></button>
  </>;
}
function AttachChip(props: { file: AttachmentInput; onRemove?: (name: AttachmentInput) => void; open?: boolean }) {
  const ws = useWs();
  const name = () => attachmentName(props.file);
  const preview = () => attachmentPreview(props.file);
  const face = () => <>
    <Show when={preview()} fallback={<I n="file" s={12} />}>{src => <img class="ws-att-thumb" src={src()} alt="" />}</Show>
    <span class="trunc">{name()}</span>
  </>;
  const openable = () => props.open ? attachmentFile(props.file) : null;
  // A saved row that kept only the name says so when pressed, instead of looking like a file that opens.
  const nameOnly = () => <Show when={props.open} fallback={<span class="ws-att-face">{face()}</span>}>
    <button class="ws-att-face" aria-label={`${name()}, name only`} aria-disabled="true" data-why="Only the name of this file was kept, so it cannot be opened here.">{face()}</button>
  </Show>;
  return <span class="chip ws-att">
    <Show when={openable()} fallback={nameOnly()}>
      {file => <button class="ws-att-face ws-att-open" aria-label={`Open ${name()}`} onClick={() => ws.open({ kind: "attachment", file: file() })}>{face()}</button>}
    </Show>
    <Show when={props.onRemove}><button class="ws-att-x" aria-label={`Remove ${name()}`} onClick={() => props.onRemove!(props.file)}><I n="x" s={12} /></button></Show>
  </span>;
}
export const AttachChips = (props: { files: AttachmentInput[] | undefined; onRemove?: (name: AttachmentInput) => void; open?: boolean }) => <Show when={props.files?.length}>
  <span class="ws-atts"><For each={props.files}>{f => <AttachChip file={f} onRemove={props.onRemove} open={props.open} />}</For></span>
</Show>;

/** What was attached to a saved task. The bytes are read on request, so a board never carries them. */
export function TaskAttachments(props: { taskId: string }) {
  const ws = useWs();
  const [files, setFiles] = createSignal<AttachmentInput[]>([]);
  const [failed, setFailed] = createSignal(false);
  let asked = 0;
  const read = () => {
    const at = ++asked;
    setFailed(false);
    ws.store.readTaskAttachments(props.taskId).then(list => { if (at === asked) setFiles(list); }, () => { if (at === asked) setFailed(true); });
  };
  const savedKey = createMemo(() => { const task = ws.store.index().byId.get(props.taskId)?.task; return `${props.taskId}:${task?.executionId ? "started" : task?.sourceVersion ?? "draft"}`; });
  createEffect(on(savedKey, read));
  return <>
    <Show when={failed()}><div class="cap st-warn" role="status">Attachments could not be read. <button class="lnk" onClick={read}>Retry</button></div></Show>
    <AttachChips files={files()} open />
  </>;
}
const label = (list: readonly { id: string; label: string }[], id: string) => list.find(x => x.id === id)?.label ?? plainChoice(id);

/**
 * A composer's one bar: its controls, and in the room between the choices and the send controls the reason one of them
 * cannot be used, read from the controls themselves. The words never change the bar's height or move a control.
 */
export function CBar(props: { children: JSX.Element }) {
  let bar: HTMLDivElement | undefined;
  return <div class="c-bar" ref={bar}>{props.children}<ReasonSlot bar={() => bar} /></div>;
}

/**
 * Model, effort, then Settings for access and language: the same choices on a new task and in every composer that calls an
 * engine. `compact` leaves Settings out where only the model and effort apply.
 */
export function RouteBar(props: { id: string; settings: TaskSettings; onChange: (patch: Partial<TaskSettings>) => void; up?: boolean; disabled?: string; compact?: boolean; needs?: WorkspaceCommand["type"] }) {
  const ws = useWs();
  const options = () => ws.store.state().options;
  const efforts = () => { const selected = options().models.find(m => m.id === props.settings.model); return options().efforts.filter(e => !selected?.efforts || selected.efforts.includes(e.id)); };
  const align = () => props.up ? "up" as const : "left" as const;
  const cls = () => props.up ? "pop-up" : "pop-anchor";
  // A session that offers no list leaves its control off. A choice is never filled in for it.
  const trigger = (kind: string, title: string, icon: string, list: () => readonly { id: string; label: string }[], id: () => string) => {
    const off = () => list().length ? (props.disabled ?? (props.needs && ws.gate(props.needs) ? ws.t("machineRoute") : undefined)) : ws.t("noOptions");
    const text = () => list().length && id() ? label(list(), id()) : ws.t("optionUnavailable");
    return <button class="sel" popovertarget={off() ? undefined : `m-${kind}-${props.id}`} aria-disabled={off() ? "true" : undefined} data-why={off()} aria-label={`${title}: ${text()}`}>
      <I n={icon} s={14} /><span class="sel-t">{text()}</span><I n="chevron-down" s={12} />
    </button>;
  };
  // Access and language share one Settings control: they are set once per task and rarely changed while writing.
  const settingsOff = () => options().modes.length || options().languages.length ? (props.disabled ?? (props.needs && ws.gate(props.needs) ? ws.t("machineRoute") : undefined)) : ws.t("noOptions");
  const settingsText = () => [options().modes.length ? `Access mode: ${label(options().modes, props.settings.mode)}` : "", options().languages.length ? `Message language: ${label(options().languages, props.settings.language)}` : ""].filter(Boolean).join(", ");
  return <div class="c-route">
    {trigger("model", "Coordinator model", "cpu", () => options().models, () => props.settings.model)}
    {trigger("effort", "Reasoning effort", "gauge", efforts, () => props.settings.effort)}
    <Show when={!props.compact}>
      <button type="button" class="ib c-set" popovertarget={settingsOff() ? undefined : `m-set-${props.id}`} aria-disabled={settingsOff() ? "true" : undefined} data-why={settingsOff()} aria-label={`Settings${settingsText() ? `. ${settingsText()}` : ""}`}><I n="sliders" /></button>
    </Show>
    <Show when={!props.needs || !ws.gate(props.needs)}>
    <Pop id={`m-model-${props.id}`} class={cls()} align={align()} label="Coordinator model" style={{ width: "280px" }}>
      <div class="mi-h">Coordinator model for this task</div>
      <For each={options().models}>{m => <MenuItem label={m.label} sub={m.unavailable ?? m.engine} disabled={m.unavailable} check={m.id === props.settings.model} onSelect={() => props.onChange({ model: m.id, ...(m.efforts ? {effort:m.efforts.includes(props.settings.effort) ? props.settings.effort : m.efforts[0] ?? ""} : {}) })} />}</For>
    </Pop>
    <Pop id={`m-effort-${props.id}`} class={cls()} align={align()} label="Effort">
      <div class="mi-h">Effort</div>
      <For each={efforts()}>{e => <MenuItem label={e.label} sub={e.unavailable} disabled={e.unavailable} check={e.id === props.settings.effort} onSelect={() => props.onChange({ effort: e.id })} />}</For>
    </Pop>
    <Show when={!props.compact}>
      <Pop id={`m-set-${props.id}`} class={cls()} align={align()} label="Settings" style={{ width: "300px" }}>
        <Show when={options().modes.length}>
          <div class="mi-h">Access mode</div>
          <For each={options().modes}>{m => <MenuItem label={m.label} sub={m.sub} check={m.id === props.settings.mode} onSelect={() => props.onChange({ mode: m.id as TaskSettings["mode"] })} />}</For>
        </Show>
        <Show when={options().modes.length && options().languages.length}><div class="mi-sep"></div></Show>
        <Show when={options().languages.length}>
          <div class="mi-h">Message language</div>
          <For each={options().languages}>{l => <MenuItem label={l.label} check={l.id === props.settings.language} onSelect={() => props.onChange({ language: l.id as TaskSettings["language"] })} />}</For>
        </Show>
      </Pop>
    </Show>
    </Show>
  </div>;
}

const time = (s: number) => duration(s * 1000);

/** Dictation lands only in the editable draft. Sending takes a separate click, so voice can never send on its own. */
export function createDictation() {
  const ws = useWs();
  if (ws.voice) return createLocalDictation(ws.voice);
  const [on, setOn] = createSignal(false);
  const [seconds, setSeconds] = createSignal(0);
  const [words, setWords] = createSignal(0);
  const [fault, setFault] = createSignal<string | null>(null);
  let clock: ReturnType<typeof setInterval> | undefined, speech: ReturnType<typeof setInterval> | undefined;
  const spoken = () => ws.demo?.dictation.spoken ?? [];
  const stop = () => { clearInterval(clock); clearInterval(speech); setOn(false); };
  onCleanup(stop);
  return {
    on, seconds, fault, words, stop,
    phase: () => "recording" as const, progress: () => 100,
    pause: async () => { clearInterval(clock); clearInterval(speech); },
    finish: async () => { const text = spoken().slice(0, words()).join(" "); stop(); return text; },
    toggle() { if (on()) { clearInterval(clock); clearInterval(speech); } else this.start(); },
    text: () => spoken().slice(0, words()).join(" "),
    finished: () => spoken().join(" "),
    clearFault: () => setFault(null),
    start() {
      const voice = ws.demo?.dictation;
      if (!voice) return;
      const reason = voice.fault();
      if (reason) { setFault(reason); return; }
      setFault(null); setSeconds(0); setWords(1); setOn(true);
      clock = setInterval(() => setSeconds(s => s + 1), 1000);
      speech = setInterval(() => setWords(w => Math.min(spoken().length, w + 1)), 450);
    },
  };
}
export type Dictation = ReturnType<typeof createDictation>;

export function MicBar(props: { mic: Dictation; onInsert: (text: string) => void; onInsertAndSend?: (text: string) => void }) {
  const ws = useWs();
  // A parked tab listens to nothing. What was heard so far stays ready to insert on return.
  createEffect(on(useContext(ScreenActive), shown => { if (!shown && props.mic.on()) void props.mic.pause(); }, { defer: true }));
  const [finishing, setFinishing] = createSignal(false);
  const finish = async (use?: (text: string) => void) => {
    if (finishing()) return;
    setFinishing(true);
    try { const text = await props.mic.finish(); if (text) use?.(text); }
    finally { setFinishing(false); }
  };
  const status = () => ({ starting: "Starting local dictation…", downloading: "Downloading Russian model: " + props.mic.progress() + "%", loading: "Loading local speech model…", recording: "Listening · Ctrl+D to stop", stopping: "Finishing transcription…", ready: "Ready to insert", error: "", cancelled: "" }[props.mic.phase()]);
  return <>
    <Show when={props.mic.on()}>
      <div class="micbar" role="group" aria-label={ws.t("voiceLabel")}>
        <I n="mic" s={14} class="st-bad" /><span class="mic-time">{time(props.mic.seconds())}</span>
        <span class="cap muted" role="status">{status()}</span>
        <span class="mic-text" aria-live="polite">{props.mic.text()}</span>
        <span class="cap faint">{ws.voice ? "On-device · Russian · not sent" : ws.t("voiceBadge")}</span>
        <button class="btn btn-sm" onClick={() => props.mic.stop()}>Cancel</button>
        <button class="btn btn-line btn-sm" disabled={finishing() || !props.mic.text()} onClick={() => void finish(props.onInsert)}>Insert</button>
        <Show when={props.onInsertAndSend}><button class="btn btn-primary btn-sm" disabled={finishing() || !props.mic.text()} onClick={() => void finish(props.onInsertAndSend)}>Insert and send</button></Show>
      </div>
    </Show>
    <Show when={props.mic.fault()}>{f => <div class="micbar" role="alert"><I n="mic" s={14} class="st-warn" /><span class="mic-text">{f()}</span><button class="btn btn-sm" onClick={() => props.mic.clearFault()}>Dismiss</button></div>}</Show>
  </>;
}
export function MicButton(props: { mic: Dictation; disabled?: string; describedBy?: string }) {
  const ws = useWs();
  const off = () => props.disabled ?? (ws.voice || ws.can(FEATURES.voice) ? undefined : ws.t("noVoice"));
  let button!: HTMLButtonElement;
  const toggle = () => { if (!off()) props.mic.toggle(); };
  const shown = useContext(ScreenActive);
  onMount(() => {
    const key = (e: KeyboardEvent) => {
      if (!shown() || e.defaultPrevented || e.repeat || !(e.ctrlKey || e.metaKey) || e.altKey || e.shiftKey || e.key.toLowerCase() !== "d") return;
      if (document.activeElement?.closest(".terminal")) return;
      const composer = document.activeElement?.closest(".composer");
      const first = [...onScreen().querySelectorAll<HTMLButtonElement>('[aria-label="Voice input"]')].find(b => b.getAttribute("aria-disabled") !== "true" && b.getClientRects().length);
      if ((composer ? composer === button.closest(".composer") : first === button) && !off()) { e.preventDefault(); toggle(); }
    };
    document.addEventListener("keydown", key); onCleanup(() => document.removeEventListener("keydown", key));
  });
  // Voice keeps its place beside Attach. Where dictation cannot run, it stays, unusable, with the reason.
  return <button ref={button} type="button" class="ib" aria-label="Voice input" aria-pressed={!off() && props.mic.on() && props.mic.phase() === "recording"}
    aria-disabled={off() ? "true" : undefined} data-why={off()} aria-describedby={off() ? props.describedBy : undefined} onClick={toggle}><I n="mic" /></button>;
}

/** The paper plane: the one control that sends or saves a draft, named for what it really does. */
export function SendButton(props: { label: string; why?: string; needs?: WorkspaceCommand["type"]; onClick: () => void; describedBy?: string }) {
  const ws = useWs();
  const gated = () => props.needs ? ws.gate(props.needs) : undefined;
  const why = () => gated() || props.why;
  return <button type="button" class="ib mc-send c-send" aria-label={props.label} aria-disabled={why() ? "true" : undefined} data-why={why()} aria-describedby={!gated() && why() ? props.describedBy : undefined} onClick={() => { if (!why()) props.onClick(); }}><I n="send" /></button>;
}

export type MessageComposerProps = {
  /** The writing area's accessible name. */
  label: string;
  placeholder: string;
  text: string;
  onText: (value: string) => void;
  files: Attachments;
  mic: Dictation;
  /** Who receives the draft, or what sending changes, said before anything is sent. */
  effect?: JSX.Element;
  /** Why nothing can be written here now. The draft and its tools stay in place. */
  off?: string;
  attachOff?: string;
  voiceOff?: string;
  /** Model, effort and Settings, only where an engine is called. */
  route?: JSX.Element;
  /** A delivery choice beside the plane, where delivery is a choice. */
  delivery?: JSX.Element;
  /** Controls before the plane, such as Cancel. */
  extra?: JSX.Element;
  /** The plane. Left out only where a confirmation outside the composer does the sending. */
  send?: { label: string; why?: string; needs?: WorkspaceCommand["type"]; go: () => void; insertAndSend?: (text: string) => void };
  /** Enter sends, Shift+Enter breaks the line; or, on a long description, Ctrl/Cmd+Enter sends. */
  sendKey?: "enter" | "mod-enter";
  onEscape?: () => void;
  autofocus?: boolean;
  inputRef?: (el: HTMLTextAreaElement) => void;
  inputId?: string;
  class?: string;
  style?: JSX.CSSProperties;
};

/**
 * Every message draft in the app: an optional effect line, then files and dictation, the writing area, one row of tools
 * (Attach, Voice, model, effort, Settings, delivery and the plane, in that order) and the bar's reason slot.
 * A control that cannot be used stays in its place with its reason, so no composer ever looks like a different one.
 */
export function MessageComposer(props: MessageComposerProps) {
  const off = () => props.off;
  // A closed composer says why once, in its writing area; its disabled tools point there instead of repeating it below.
  const fieldId = props.inputId ?? createUniqueId();
  const said = () => off() ? fieldId : undefined;
  const append = (spoken: string) => props.onText(props.text.trim() ? `${props.text.trimEnd()} ${spoken}` : spoken);
  const sendWhy = () => props.send ? (off() ?? props.send.why) : undefined;
  const submit = () => { if (props.send && !sendWhy()) props.send.go(); };
  const onKey = (e: KeyboardEvent) => {
    if (e.key === "Escape" && props.onEscape) { e.stopPropagation(); props.onEscape(); return; }
    if (e.key !== "Enter" || e.isComposing || !props.send) return;
    if ((props.sendKey ?? "enter") === "enter" ? e.shiftKey : !(e.ctrlKey || e.metaKey)) return;
    e.preventDefault();
    submit();
  };
  const insertAndSend = () => props.send?.insertAndSend && !off() ? (spoken: string) => props.send!.insertAndSend!(props.text.trim() ? `${props.text.trimEnd()} ${spoken}` : spoken) : undefined;
  return <div class={`composer mc${props.class ? ` ${props.class}` : ""}`} style={props.style} {...attachmentEvents(props.files, () => !off() && !props.attachOff)}>
    <Show when={props.effect}><div class="mc-effect">{props.effect}</div></Show>
    <AttachmentStatus a={props.files} />
    <MicBar mic={props.mic} onInsert={append} onInsertAndSend={insertAndSend()} />
    <AttachChips files={props.files.files()} onRemove={props.files.remove} />
    <textarea id={fieldId} aria-label={props.label} placeholder={off() ?? props.placeholder} disabled={!!off()} value={props.text}
      onInput={e => props.onText(e.currentTarget.value)} onKeyDown={onKey}
      ref={el => { autoGrow(el, () => props.text); props.inputRef?.(el); if (props.autofocus) queueMicrotask(() => el.focus()); }} />
    <CBar>
      <div class="mc-tools"><AttachButton a={props.files} disabled={off() ?? props.attachOff} describedBy={said()} /><MicButton mic={props.mic} disabled={off() ?? props.voiceOff} describedBy={said()} /></div>
      {props.route}
      <div class="c-end">
        {props.delivery}
        {props.extra}
        <Show when={props.send}>{s => <SendButton label={s().label} why={sendWhy()} needs={s().needs} onClick={submit} describedBy={said()} />}</Show>
      </div>
    </CBar>
  </div>;
}

/**
 * One unsent message: reorder, send or remove. Edit takes it out of the queue and back into the composer, to be sent again.
 * The row is one line: its text shortens, and its files stay with the message but are shown only in Edit and, once sent, in the conversation.
 */
function QueueRow(props: { message: Accessor<Message>; ids: Accessor<string[]>; mode: Accessor<ComposerMode>; onEdit: (message: Message) => void }) {
  const ws = useWs();
  const id = () => props.message().id;
  const canSend = () => props.mode() === "stopping" ? "Available once every worker is confirmed as stopped" : undefined;
  const edit = () => { const message = props.message(); ws.ok({ type: "remove_queued", messageId: id() }, () => { props.onEdit(message); }); };
  const move = (by: number) => {
    const ids = props.ids(), at = ids.indexOf(id()), to = at + by;
    if (to < 0 || to >= ids.length) return;
    ws.run({ type: "move_queued", messageId: id(), before: by < 0 ? ids[to] : ids[to + 1] ?? null });
    queueMicrotask(() => onScreen().querySelector<HTMLElement>(`[data-q="${id()}"] .grip`)?.focus());
  };
  return <div class="qrow" data-q={id()} draggable={true}
    onDragStart={e => e.dataTransfer?.setData("text/plain", id())} onDragOver={e => e.preventDefault()}
    onDrop={e => { e.preventDefault(); const from = e.dataTransfer?.getData("text/plain"); if (from && from !== id()) ws.run({ type: "move_queued", messageId: from, before: id() }); }}>
    <button type="button" class="ib grip" aria-label={`Reorder: ${props.message().text}`}
      onKeyDown={e => { const by = e.key === "ArrowUp" ? -1 : e.key === "ArrowDown" ? 1 : 0; if (by) { e.preventDefault(); move(by); } }}><I n="grip" /></button>
    <Whole class="qrow-t" text={props.message().text} />
    {/* One row size: Edit, Send now and remove share the composer's control height, so no control looks smaller than its neighbour. */}
    <span class="qrow-end">
      <Action kind="line" {...ws.needs("remove_queued")} onClick={edit}>Edit</Action>
      <Action kind="line" why={canSend()} {...ws.needs("send_queued")} onClick={() => ws.run({ type: "send_queued", messageId: id(), resume: true, now: ws.store.clock() })}><I n="send" />Send now</Action>
      <Action kind="icon" aria-label={`Remove from queue: ${props.message().text}`} {...ws.needs("remove_queued")} onClick={() => ws.run({ type: "remove_queued", messageId: id() })}><I n="x" /></Action>
    </span>
  </div>;
}

const CLOSED_TEXT: Partial<Record<ComposerMode, string>> = {
  draft: "Start the task to send a message.",
  closed: "This task is closed. Nothing more can be sent to it.", ended: "This run has ended. Nothing more can be sent to it.", archived: "This task is archived. Restore it to continue.",
  offline: "Sending is off because the machine is disconnected. Use Re-check state above to reconnect.",
  unconfirmed: "Sending is off because this task's state is not confirmed. Use Re-check state above.",
};

const STOPPING = "Available once every worker is confirmed as stopped";

/** Send now or Queue: a visible choice beside the plane. Nothing else, no hidden key, changes how a message is delivered. */
function Delivery(props: { value: "send_now" | "queue"; onChange: (value: "send_now" | "queue") => void; off?: string }) {
  const choice = (value: "send_now" | "queue", label: string) =>
    <button type="button" aria-pressed={props.value === value ? "true" : "false"} aria-disabled={props.off ? "true" : undefined} data-why={props.off} onClick={() => { if (!props.off) props.onChange(value); }}>{label}</button>;
  return <div class="seg mc-delivery" role="group" aria-label="Delivery">{choice("send_now", "Send now")}{choice("queue", "Queue")}</div>;
}

/**
 * The task's message composer. It sends, queues or, on a result, requests changes; it never starts, resumes or stops the
 * task, which are task actions in the title band. While a question waits, it folds to one line so a general message is
 * never mistaken for an answer. The dock floats over the conversation; its margins and the gap to the queue stay transparent.
 */
export function Composer(props: { model: Accessor<SessionModel>; onHeight: (px: number) => void; pill?: JSX.Element; folded?: boolean }) {
  const ws = useWs();
  const m = () => props.model();
  const id = () => m().view.id;
  const mode = () => m().mode;
  const [text, setText] = createSignal(ws.drafts.get(id()) ?? "");
  const write = (value: string) => { setText(value); ws.drafts.set(id(), value); };
  const mic = createDictation();
  const files = createAttachments();
  createUnsent(() => !!text().trim() || files.files().length > 0);
  const now = () => ws.store.clock();
  const [delivery, setDelivery] = createSignal<"send_now" | "queue">("send_now");
  const [opened, setOpened] = createSignal(false);
  const queueIds = createMemo(() => m().queue.map(q => q.id));
  const showQueue = () => ["live", "paused", "stopping"].includes(mode()) && queueIds().length > 0;
  const done = () => { write(""); files.clear(); };
  const send = (value: string) => {
    if (!value.trim() || files.busy()) return;
    // A finished result has nobody to steer: a message sends it back and reopens the task.
    if (mode() === "review") { ws.ok({ type: "request_changes", taskId: id(), text: value, attachments: files.files(), now: now() }, done); return; }
    ws.ok({ type: "send_message", taskId: id(), text: value, mode: mode() === "live" ? delivery() : "send_now", resume: mode() === "paused", now: now(), attachments: files.files() }, done);
  };
  const readOnly = () => CLOSED_TEXT[mode()];
  const plane = () => mode() === "review" ? { label: "Send and reopen", needs: "request_changes" as const }
    : mode() === "paused" ? { label: "Send and resume", needs: "send_message" as const }
    : delivery() === "queue" && mode() === "live" ? { label: "Queue message", needs: "send_message" as const }
    : { label: "Send message now", needs: "send_message" as const };
  const sendWhy = () => mode() === "stopping" ? STOPPING : files.busy() ? "Attachments are still loading" : text().trim() ? undefined : "Write a message first";
  const effect = () => mode() === "review" ? "Sending requests changes and reopens this task. Acceptance of this result is cleared."
    : mode() === "paused" ? "Sending resumes the task."
    : props.folded ? "To the coordinator, not an answer to the question." : undefined;
  const placeholder = () => mode() === "paused" ? "Write a message. Sending resumes the task." : mode() === "stopping" ? "Sending returns once the stop is confirmed" : mode() === "review" ? "Ask about this result, or describe what to change…" : "Write a message…";
  // A draft in progress is never folded away.
  const folded = () => props.folded && !opened() && !text().trim() && files.files().length === 0;
  createEffect(() => { if (!props.folded) setOpened(false); });
  let dock!: HTMLDivElement, input: HTMLTextAreaElement | undefined;
  reportHeight(() => dock, px => props.onHeight(px));
  createEffect(() => { if (mode() !== "live" && mode() !== "paused") mic.stop(); });
  return <div class="dock" ref={dock}>
    {props.pill}
    <Show when={showQueue()}>
      <div class="queue" role="group" aria-label="Queued messages, not sent">
        <div class="queue-h"><I n="queue" s={14} class="faint" />Queued {queueIds().length}, not sent yet<span class="warn">Saved on this machine · not sent</span></div>
        <For each={queueIds()}>{qid => <QueueRow message={() => m().queue.find(q => q.id === qid)!} ids={queueIds} mode={mode} onEdit={message => { write(text().trim() ? `${text().trimEnd()}\n${message.text}` : message.text); files.add(message.attachments ?? []); queueMicrotask(() => input?.focus()); }} />}</For>
      </div>
    </Show>
    <Show when={!folded()} fallback={<div class="mc-fold"><button type="button" class="btn btn-line" onClick={() => { setOpened(true); queueMicrotask(() => input?.focus()); }}><I n="message" />Send a separate message</button></div>}>
      <MessageComposer label="Message" placeholder={placeholder()} text={text()} onText={write} files={files} mic={mic}
        effect={effect()} off={readOnly()} attachOff={mode() === "stopping" ? STOPPING : undefined} voiceOff={mode() === "stopping" ? STOPPING : undefined}
        inputRef={el => { input = el; }}
        route={<RouteBar id="task" up needs="set_task_settings" settings={taskSettings(m())} disabled={readOnly() || mode() === "stopping" ? "Unavailable for this task now" : undefined}
          onChange={settings => ws.run({ type: "set_task_settings", taskId: id(), settings: { ...taskSettings(m()), ...settings } })} />}
        delivery={<Show when={mode() === "live"}><Delivery value={delivery()} onChange={setDelivery} off={ws.gate("send_message")} /></Show>}
        send={{ label: plane().label, needs: plane().needs, why: sendWhy(), go: () => send(text()), insertAndSend: mode() === "live" || mode() === "review" || mode() === "paused" ? send : undefined }} />
    </Show>
  </div>;
}
