import { plainChoice, classifiedPlanningFailure, stoppedPlanningComposerText } from "./selectors";
import { createEffect, createMemo, createSignal, createUniqueId, For, on, onCleanup, onMount, Show, useContext, type Accessor, type JSX } from "solid-js";
import { capabilityOf, FEATURES, machineEffort, orderedEfforts, resolveEffort, type Attachment, type AttachmentInput, type AttachmentRef, type Message, type SessionOptions, type TaskSettings, type WorkspaceCommand } from "@cortex/workspace-domain";
import { attachmentFile, attachmentName, attachmentPreview, attachmentRef, keepBytes, readAttachments, wireAttachments } from "./attachments";
import { autoGrow } from "./autogrow";
import { useWs } from "./ctx";
import { setDragPreview } from "./drag-preview";
import { REQUESTING, RequestStatus } from "./request-feedback";
import { createUnsent, onScreen, ScreenActive } from "./freshness";
import { createLocalDictation } from "./voice";
import { voiceLanguageName } from "./voice-client";
import { I, MenuItem, Pop } from "./kit";
import { Action, Whole } from "./layout";
import { createSuggest, SUGGEST_HINT, SuggestList, type SlashCommand, type SuggestOptions } from "./suggest";
import { MarkdownMirror } from "./markdown-field";
import { duration, REWORK_LABEL, reworkBinding, resumeState, stopLabel, taskSettings, workingNow, type ComposerMode, type SessionModel } from "./session-model";
import { PlanningPlace, usePlanningSprint, useTaskRoutes } from "./task-place";

/** Attached files are uploaded once, as they are added; the draft holds their references until an explicit send or queue action. */
export function createAttachments() {
  const ws = useWs();
  const [files, setFiles] = createSignal<AttachmentInput[]>([]);
  const [error, setError] = createSignal(""), [busy, setBusy] = createSignal(false);
  let pending = Promise.resolve(), reading = 0;
  const add = (items: AttachmentInput[]) => setFiles([...files(), ...items]);
  // Bytes cross to the machine once, now; the draft keeps the references (and this window the bytes, for thumbnails).
  const uploaded = async (list: Attachment[]) => {
    const refs = await ws.store.uploadAttachments(list);
    refs.forEach((r, i) => { const ref = attachmentRef(r); if (ref && list[i]) keepBytes(ref, list[i].data); });
    return refs;
  };
  const ingest = (items: File[]) => {
    if (!items.length) return;
    reading++; setBusy(true);
    pending = pending.then(async () => {
      try { const added = await readAttachments(items, files()); add(await uploaded(added)); setError(""); }
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
        wireAttachments([...files(), ...added]); add(await uploaded(added)); setError("");
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
    <button data-tooltip="" type="button" class="ib" aria-label="Attach files" aria-disabled={off() ? "true" : undefined} data-why={off()} aria-describedby={off() ? props.describedBy : undefined} onClick={() => { if (!off()) input.click(); }}><I n="clip" /></button>
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
  // A saved file opens by fetching its bytes then; its chip is drawn from its name and type alone.
  const openable = () => props.open ? attachmentFile(props.file) ?? attachmentRef(props.file) : null;
  const [opening, setOpening] = createSignal(false), [openError, setOpenError] = createSignal("");
  const open = async (file: Attachment | AttachmentRef) => {
    if (!("sha256" in file)) { ws.open({ kind: "attachment", file }); return; }
    if (opening()) return;
    setOpening(true); setOpenError("");
    try { const read = await ws.store.readAttachment(file); keepBytes(file, read.data); ws.open({ kind: "attachment", file: read }); }
    catch (e) { setOpenError(e instanceof Error ? e.message : "This file could not be opened."); }
    finally { setOpening(false); }
  };
  // A saved row that kept only the name says so when pressed, instead of looking like a file that opens.
  const nameOnly = () => <Show when={props.open} fallback={<span class="ws-att-face">{face()}</span>}>
    <button class="ws-att-face" aria-label={`${name()}, name only`} aria-disabled="true" data-why="Only the name of this file was kept, so it cannot be opened here.">{face()}</button>
  </Show>;
  return <span class="chip ws-att">
    <Show when={openable()} fallback={nameOnly()}>
      {file => <button class="ws-att-face ws-att-open" aria-label={`Open ${name()}`} aria-busy={opening() ? "true" : undefined} onClick={() => void open(file())}>{face()}</button>}
    </Show>
    <Show when={props.onRemove}><button class="ws-att-x" aria-label={`Remove ${name()}`} onClick={() => props.onRemove!(props.file)}><I n="x" s={12} /></button></Show>
    <Show when={openError()}><span class="cap st-warn" role="alert">{openError()}</span></Show>
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
 * A composer's one bar: its controls only. No words sit beside the send button: a control that cannot be used says why in
 * its own box, opened by a press or keyboard focus (ReasonTip), so nothing beside the plane reads as being about it.
 */
export function CBar(props: { children: JSX.Element }) {
  return <div class="c-bar">{props.children}</div>;
}

/**
 * Model, effort, then Settings for access and language: the same choices on a new task and in every composer that calls an
 * engine. `compact` leaves Settings out where only the model and effort apply.
 */
export function RouteBar(props: { id: string; settings: TaskSettings; onChange: (patch: Partial<TaskSettings>) => void; up?: boolean; disabled?: string; compact?: boolean; needs?: WorkspaceCommand["type"]; options?: SessionOptions }) {
  const ws = useWs();
  const options = () => props.options ?? ws.store.state().options;
  const effortsFor = (model: string) => {
    const offered = options().models.find(m => m.id === model)?.efforts;
    const choices = options().efforts.filter(e => !offered || offered.includes(e.id));
    return orderedEfforts(choices.map(e => e.id)).map(id => choices.find(e => e.id === id)!);
  };
  const efforts = () => effortsFor(props.settings.model);
  const effortFor = (model: string) => resolveEffort(props.settings.effort, machineEffort(ws.machine()),
    effortsFor(model).filter(e => !e.unavailable).map(e => e.id));
  const align = () => props.up ? "up" as const : "left" as const;
  const cls = () => props.up ? "pop-up" : "pop-anchor";
  // A session that offers no list leaves its control off. A choice is never filled in for it.
  const trigger = (kind: string, title: string, icon: string, list: () => readonly { id: string; label: string }[], id: () => string) => {
    const off = () => list().length ? (props.disabled ?? (props.needs && ws.gate(props.needs) ? ws.t("machineRoute") : undefined)) : ws.store.optionsKnown() ? ws.t("noOptions") : ws.t("optionLoading");
    const text = () => list().length && id() ? label(list(), id()) : ws.store.optionsKnown() ? ws.t("optionUnavailable") : ws.t("optionLoading");
    return <button class="sel" popovertarget={off() ? undefined : `m-${kind}-${props.id}`} aria-disabled={off() ? "true" : undefined} data-why={off()} aria-label={`${title}: ${text()}`}>
      <I n={icon} s={14} /><span class="sel-t">{text()}</span><I n="chevron-down" s={12} />
    </button>;
  };
  // Access and language share one Settings control: they are set once per task and rarely changed while writing.
  const settingsOff = () => options().modes.length || options().languages.length ? (props.disabled ?? (props.needs && ws.gate(props.needs) ? ws.t("machineRoute") : undefined)) : ws.t("noOptions");
  const settingsText = () => [options().modes.length ? `Access mode: ${label(options().modes, props.settings.mode)}` : "", options().languages.length ? `Message language: ${label(options().languages, props.settings.language)}` : ""].filter(Boolean).join(", ");
  return <div class="c-route">
    {trigger("model", "Coordinator model", "cpu", () => options().models, () => props.settings.model)}
    {trigger("effort", "Reasoning effort", "gauge", efforts, () => effortFor(props.settings.model))}
    <Show when={!props.compact}>
      <button data-tooltip="" type="button" class="ib c-set" popovertarget={settingsOff() ? undefined : `m-set-${props.id}`} aria-disabled={settingsOff() ? "true" : undefined} data-why={settingsOff()} aria-label={`Settings${settingsText() ? `. ${settingsText()}` : ""}`}><I n="sliders" /></button>
    </Show>
    <Show when={!props.needs || !ws.gate(props.needs)}>
    <Pop id={`m-model-${props.id}`} class={cls()} align={align()} label="Coordinator model" style={{ width: "280px" }}>
      <div class="mi-h">Coordinator model for this task</div>
      <For each={options().models}>{m => <MenuItem label={m.label} sub={m.unavailable ?? m.engine} disabled={m.unavailable} check={m.id === props.settings.model} onSelect={() => props.onChange({ model: m.id, effort: effortFor(m.id) })} />}</For>
    </Pop>
    <Pop id={`m-effort-${props.id}`} class={cls()} align={align()} label="Effort">
      <div class="mi-h">Effort</div>
      <For each={efforts()}>{e => <MenuItem label={e.label} sub={e.unavailable} disabled={e.unavailable} check={e.id === effortFor(props.settings.model)} onSelect={() => props.onChange({ effort: e.id })} />}</For>
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
    phase: () => "recording" as const, progress: () => 100, language: () => undefined,
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
  const language = () => {
    const bound = props.mic.language();
    return bound ? voiceLanguageName(bound) : "";
  };
  const status = () => ({ starting: ws.shell?.kind === "phone" ? "Starting…" : "Starting local dictation…", downloading: "Downloading " + (language() || "speech") + " model: " + props.mic.progress() + "%", loading: "Loading local speech model…", recording: ws.shell?.kind === "phone" ? "Listening" : "Listening · Ctrl+D to stop", stopping: "Finishing transcription…", ready: "Ready to insert", error: "", cancelled: "" }[props.mic.phase()]);
  return <>
    <Show when={props.mic.on()}>
      <div class="micbar" role="group" aria-label={ws.t("voiceLabel")}>
        <I n="mic" s={14} class="st-bad" /><span class="mic-time">{time(props.mic.seconds())}</span>
        <span class="cap muted" role="status">{status()}</span>
        <span class="mic-text" aria-live="polite">{props.mic.text()}</span>
        <span class="cap faint">{ws.voice ? ws.voice.badge ?? ["On-device", language(), "not sent"].filter(Boolean).join(" · ") : ws.t("voiceBadge")}</span>
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
  return <button data-tooltip="" ref={button} type="button" class="ib" aria-label="Voice input" aria-pressed={!off() && props.mic.on() && props.mic.phase() === "recording"}
    aria-disabled={off() ? "true" : undefined} data-why={off()} aria-describedby={off() ? props.describedBy : undefined} onClick={toggle}><I n="mic" /></button>;
}

/**
 * The composer's one action button. Usually the paper plane that sends or saves a draft; where a run is controlled from the
 * composer and the field is empty, it is Start, Stop or Resume. It keeps one size and place whatever it is, named for what it does.
 */
export function SendButton(props: { label: string; why?: string; needs?: WorkspaceCommand["type"]; onClick: () => void; describedBy?: string; icon?: string; shortcut?: string }) {
  const ws = useWs();
  const gated = () => props.needs ? ws.gate(props.needs) : undefined;
  const why = () => gated() || props.why;
  return <button data-tooltip={props.shortcut ? `${props.label} · ${props.shortcut} to send` : ""} type="button" class="ib mc-send c-send" aria-label={props.label} aria-disabled={why() ? "true" : undefined} data-why={why()} aria-describedby={!gated() && why() ? props.describedBy : undefined} onClick={() => { if (!why()) props.onClick(); }}><I n={props.icon ?? "send"} class={props.icon === "loader" ? "spin" : undefined} /></button>;
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
  send?: { label: string; why?: string; needs?: WorkspaceCommand["type"]; go: () => void; insertAndSend?: (text: string) => void;
    /** The button runs the task or planning instead of sending (Start, Stop, Resume): its icon, it works while the field is
     *  closed, and Enter never presses it. */
    run?: { icon: string } };
  /** The composer's group name, where tests and assistive technology find its action button. */
  group?: string;
  /** Enter sends, Shift+Enter breaks the line; or, on a long description, Ctrl/Cmd+Enter sends. */
  sendKey?: "enter" | "mod-enter";
  onEscape?: () => void;
  autofocus?: boolean;
  inputRef?: (el: HTMLTextAreaElement) => void;
  inputId?: string;
  /** `@` files and `/` commands or skills offered while writing; left out where this field has none. */
  suggest?: SuggestOptions;
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
  // Only an empty, closed writing area shows the reason as its placeholder. A retained draft must remain readable.
  const fieldId = props.inputId ?? createUniqueId();
  const offId = createUniqueId();
  const offWithDraft = () => !!off() && props.text.length > 0;
  const said = () => off() ? (offWithDraft() ? offId : fieldId) : undefined;
  const append = (spoken: string) => props.onText(props.text.trim() ? `${props.text.trimEnd()} ${spoken}` : spoken);
  const sendWhy = () => props.send ? (props.send.run ? props.send.why : off() ?? props.send.why) : undefined;
  const submit = () => { if (props.send && !sendWhy()) props.send.go(); };
  let field: HTMLTextAreaElement | undefined;
  // While an input method composes, the field draws its own text (with the composition's underline) over the Markdown layer.
  const [composing, setComposing] = createSignal(false);
  const suggest = createSuggest({ options: () => off() ? undefined : props.suggest, text: () => props.text, setText: v => props.onText(v), field: () => field });
  const onKey = (e: KeyboardEvent) => {
    // An open list takes the keys it uses, so Enter picks an option instead of sending.
    if (suggest.key(e)) return;
    if (e.key === "Escape" && props.onEscape) { e.stopPropagation(); props.onEscape(); return; }
    if (e.key !== "Enter" || e.isComposing || !props.send || props.send.run) return;
    if ((props.sendKey ?? "enter") === "enter" ? e.shiftKey : !(e.ctrlKey || e.metaKey)) return;
    e.preventDefault();
    submit();
  };
  const insertAndSend = () => props.send?.insertAndSend && !off() ? (spoken: string) => props.send!.insertAndSend!(props.text.trim() ? `${props.text.trimEnd()} ${spoken}` : spoken) : undefined;
  return <div class={`composer mc${props.class ? ` ${props.class}` : ""}`} style={props.style} role={props.group ? "group" : undefined} aria-label={props.group} {...attachmentEvents(props.files, () => !off() && !props.attachOff)}>
    <Show when={props.effect}><div class="mc-effect">{props.effect}</div></Show>
    <AttachmentStatus a={props.files} />
    <MicBar mic={props.mic} onInsert={append} onInsertAndSend={insertAndSend()} />
    <AttachChips files={props.files.files()} onRemove={props.files.remove} />
    <Show when={offWithDraft()}><div id={offId} class="mc-effect">{off()}</div></Show>
    {/* The field is the one source of the text; the layer under it only shows that text's Markdown (markdown-field.tsx). */}
    <div class="mc-field" data-composing={composing() ? "" : undefined}>
    <MarkdownMirror text={props.text} field={() => field} />
    <textarea id={fieldId} aria-label={props.label} aria-describedby={offWithDraft() ? offId : undefined} placeholder={off() ?? props.placeholder} disabled={!!off()} value={props.text}
      onInput={e => { props.onText(e.currentTarget.value); suggest.onInput(); }} onKeyDown={onKey}
      onKeyUp={e => { if (["ArrowLeft", "ArrowRight", "Home", "End"].includes(e.key)) suggest.onCaret(); }} onClick={() => suggest.onCaret()} onBlur={() => suggest.close()}
      onCompositionStart={() => { setComposing(true); suggest.onCompositionStart(); }} onCompositionEnd={() => { setComposing(false); suggest.onCompositionEnd(); }}
      aria-autocomplete={props.suggest ? "list" : undefined} aria-controls={suggest.activeId() ? suggest.listId : undefined} aria-activedescendant={suggest.activeId()}
      ref={el => { field = el; autoGrow(el, () => props.text); props.inputRef?.(el); if (props.autofocus) queueMicrotask(() => el.focus()); }} />
    </div>
    <SuggestList s={suggest} field={() => field} />
    <CBar>
      <div class="mc-tools"><AttachButton a={props.files} disabled={off() ?? props.attachOff} describedBy={said()} /><MicButton mic={props.mic} disabled={off() ?? props.voiceOff} describedBy={said()} /></div>
      {props.route}
      <div class="c-end">
        {props.delivery}
        {props.extra}
        <Show when={props.send}>{s => <SendButton label={s().label} why={sendWhy()} needs={s().needs} onClick={submit} describedBy={s().run ? undefined : said()} icon={s().run?.icon} shortcut={s().run ? undefined : props.sendKey === "mod-enter" ? "Ctrl/Cmd+Enter" : "Enter"} />}</Show>
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
    onDragStart={e => { e.dataTransfer?.setData("text/plain", id()); setDragPreview(e); }} onDragOver={e => e.preventDefault()}
    onDrop={e => { e.preventDefault(); const from = e.dataTransfer?.getData("text/plain"); if (from && from !== id()) ws.run({ type: "move_queued", messageId: from, before: id() }); }}>
    <button data-tooltip="Drag to reorder" type="button" class="ib grip" aria-label={`Reorder: ${props.message().text}`}
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
  closed: "This task is closed. Nothing more can be sent to it.", ended: "This run has ended and cannot be resumed. Nothing more can be sent to it; Close task moves it to Done.", archived: "This task is archived. Restore the task first.",
  offline: "Sending is off because the machine is disconnected. Use Re-check state above to reconnect.",
  unconfirmed: "Sending is off because this task's state is not confirmed. Use Re-check state above.",
};

const STOPPING = "Available once every worker is confirmed as stopped";
/** The composer's action button with an empty field: a run control instead of sending, or null. */
type RunControl = { label: string; icon: string; needs?: WorkspaceCommand["type"]; why?: string; go: () => void; effect: string } | null;

/**
 * Queue, beside the plane: off, a message is sent now; on, it waits in the queue. This one visible control, and no hidden key,
 * changes how a message is delivered. Its name is always Queue; a pointer resting on it reads what it does.
 */
function Delivery(props: { value: "send_now" | "queue"; onChange: (value: "send_now" | "queue") => void; off?: string }) {
  const queued = () => props.value === "queue";
  return <div class="mc-delivery" role="group" aria-label="Delivery">
    <button type="button" class="ib" aria-label="Queue" data-tooltip={queued() ? "Queue is on: messages wait until the coordinator is free" : "Queue messages instead of sending them now"} aria-pressed={queued() ? "true" : "false"} aria-disabled={props.off ? "true" : undefined} data-why={props.off}
      onClick={() => { if (!props.off) props.onChange(queued() ? "send_now" : "queue"); }}><I n="queue" /></button>
  </div>;
}

/**
 * The task's message composer, the one place that controls the run. Its action button follows the task and the field: a
 * draft has Start; a running task with an empty field has Stop and a paused one Resume; as soon as the field holds text or
 * files it is the paper plane (Send now or Queue, or Send and resume). It sends, queues or, on a result, requests changes;
 * Stop, Resume and Start stay in the task's "…" menu too. While a question waits, it folds to one line so a general message is
 * never mistaken for an answer. The dock stands below the conversation in the panel's column, never over it.
 */
export function Composer(props: { model: Accessor<SessionModel>; team?: JSX.Element; folded?: boolean }) {
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
  // The task planning a sprint is written to through its sprint's planning: a message asks for changes to the plan and
  // continues the same task, Stop pauses planning and Resume continues it. Sending stays open while the plan waits for
  // approval, and closes once its tasks are created.
  const planning = usePlanningSprint();
  const changingRequest = useContext(PlanningPlace)?.changingRequest ?? (() => false);
  const binding = () => planning() ? null : reworkBinding(m().view);
  const [rework, setRework] = createSignal(false);
  const [reworkPending, setReworkPending] = createSignal(false);
  const [reworkError, setReworkError] = createSignal("");
  const reworking = () => rework() && !!binding();
  const routes = useTaskRoutes(() => ({ machine: m().view.machine.id, id: id() }));
  const planningOff = () => {
    const s = planning();
    if (!s) return undefined;
    if (s.planning === "created") return "Tasks are created. Edit them on the Board.";
    if (mode() === "offline" || mode() === "unconfirmed") return CLOSED_TEXT[mode()];
    if (s.planning === "cancelled") return changingRequest() ? undefined : stoppedPlanningComposerText(s);
    return mode() === "draft" ? "Planning is starting." : undefined;
  };
  const queueIds = createMemo(() => m().queue.map(q => q.id));
  const showQueue = () => !planning() && ["live", "paused", "stopping"].includes(mode()) && queueIds().length + queueSending().length > 0;
  // The message leaves the field as it is sent and reads at once where it went; a send that does not arrive keeps it there,
  // with Retry and Edit, so its text and files are never lost.
  const done = () => { write(""); files.clear(); };
  const send = (value: string) => {
    if (!value.trim() || files.busy()) return;
    const attachments = files.files();
    const target = binding();
    if (target) {
      if (!reworking() || reworkPending() || ws.gate("rework_task") || [...value].length > 32_000) return;
      const taskId = id();
      const submit = () => {
        const current = binding();
        if (reworkPending()) return false;
        if (!current || current.expectedVersion !== target.expectedVersion || current.executionId !== target.executionId || current.ownerEpoch !== target.ownerEpoch || ws.gate("rework_task")) {
          setReworkError("The task changed. Recheck it before sending.");
          return true;
        }
        setReworkError("");
        setReworkPending(true);
        void ws.run({ type: "rework_task", taskId, ...target, text: value, attachments }).then(receipt => {
          if (receipt.ok) { if (text() === value && files.files() === attachments) done(); setRework(false); }
          else setReworkError(receipt.message);
        }).finally(() => {
          setReworkPending(false);
          queueMicrotask(() => { if (shown() && document.activeElement === document.body) input?.focus(); });
        });
        return true;
      };
      if (m().view.task.reworkWarnings?.includes("dependents_started")) ws.open({ kind: "confirm", title: "Return this task for rework?",
        note: "Dependent sprint tasks already started using the old result. Sending reopens this task but does not stop or change those tasks.",
        action: "Send and reopen", go: submit });
      else submit();
      return;
    }
    if (reworkPending() || (rework() && mode() !== "live")) return;
    const sprint = planning();
    // A finished result has nobody to steer: a message sends it back and reopens the task.
    const command: WorkspaceCommand = sprint ? { type: "plan_message", sprintId: sprint.id, text: value, attachments, now: now() }
      : mode() === "review" ? { type: "request_changes", taskId: id(), text: value, attachments, now: now() }
      : { type: "send_message", taskId: id(), text: value, mode: mode() === "live" ? delivery() : "send_now", resume: mode() === "paused", now: now(), attachments };
    done();
    void ws.run(command);
  };
  const queuePlace = () => ({ kind: "queue" as const, taskId: id() });
  const queueSending = () => ws.store.sends.shown(queuePlace());
  // A message taken back from a send that did not arrive returns to this field, beside whatever is written there now.
  createEffect(() => {
    const s = planning();
    const back = (s ? ws.store.sends.returned({ kind: "sprint", sprintId: s.id }) : null) ?? ws.store.sends.returned({ kind: "task", taskId: id() }) ?? ws.store.sends.returned(queuePlace());
    if (!back) return;
    ws.store.sends.take();
    write(text().trim() ? `${text().trimEnd()}\n${back.text}` : back.text); files.add(back.attachments ?? []);
    queueMicrotask(() => input?.focus());
  });
  const readOnly = () => planning() ? planningOff()
    : reworkPending() ? "Sending and reopening…"
    : mode() === "archived" ? CLOSED_TEXT.archived
    : binding() ? reworking() ? undefined : `Use ${REWORK_LABEL} to reopen this task.`
    : rework() && mode() !== "live" ? "Rework is no longer available. Recheck the task before sending." : CLOSED_TEXT[mode()];
  const empty = () => !text().trim() && files.files().length === 0;
  // Something of this task runs now: its coordinator works or a worker does. A task waiting for the owner, or held after an
  // interruption, runs nothing, so it has no Stop here.
  const running = () => mode() === "live" && !props.folded && workingNow(m()) && m().view.task.status !== "needs_you" && m().view.task.observedPhase !== "recovery";
  const resumable = () => ws.can(capabilityOf("continue_task"));
  const stopPlanning = () => { const s = planning(); if (s) void ws.run({ type: "cancel_planning", sprintId: s.id }); };
  const resumePlanning = () => { const s = planning(); if (s) void ws.run({ type: "resume_planning", sprintId: s.id }); };
  /** The planning task's run control: Stop while it works, Resume once stopped, nothing once its tasks are created. */
  const planningControl = (): RunControl => {
    const s = planning()!;
    if (s.planning === "created" || changingRequest() || (s.planning === "cancelled" && s.planningFailure)) return null;
    if (mode() === "stopping" && empty()) return { label: "Stopping", icon: "loader", why: STOPPING, go: () => undefined, effect: "Stopping planning. Messages can be sent once the stop is confirmed." };
    if (s.planning === "cancelled" || (mode() === "paused" && empty())) return { label: "Resume", icon: "play", needs: "resume_planning", go: resumePlanning, effect: "Resume continues planning." };
    if (running() && empty()) return { label: "Stop", icon: "stop", needs: "cancel_planning", go: stopPlanning, effect: "Stop pauses planning and keeps the conversation; Resume continues it. Write a message to send it instead." };
    return null;
  };
  /** The run control the action button is with an empty field, or none: then it sends. */
  const control = (): RunControl =>
    planning() ? planningControl()
    : reworkPending() || reworking() ? null
    : mode() === "draft" ? { label: "Start", icon: "play", needs: "start_task", go: () => void ws.run({ type: "start_task", taskId: id(), now: now() }), effect: "Start runs this task." }
    : mode() === "stopping" && empty() ? { label: "Stopping", icon: "loader", why: STOPPING, go: () => undefined, effect: "Stopping the task. Messages can be sent once the stop is confirmed." }
    : running() && empty() ? { label: stopLabel(resumable()), icon: "stop", needs: "stop_task", go: () => ws.stopTask(id()),
      effect: resumable() ? "Stop pauses the task and keeps everything; Resume continues it. Write a message to send it instead." : "End run stops this task. Write a message to send it instead." }
    : mode() === "paused" && empty() ? resumeState(m().view)?.action === "continue_task"
      ? { label: "Resume", icon: "play", needs: "continue_task", go: () => void ws.run({ type: "continue_task", taskId: id(), now: now() }), effect: "Resume continues the task. Write a message to send it and resume." }
      // A pause whose stop or process exit is not confirmed cannot be resumed yet: Resume stays off and says why.
      : { label: "Resume", icon: "play", why: resumeState(m().view)?.why || "This task cannot be resumed now", go: () => undefined, effect: resumeState(m().view)?.why || "This task cannot be resumed now" }
    : null;
  const plane = (): { label: string; needs: WorkspaceCommand["type"] } => planning() ? { label: changingRequest() ? (classifiedPlanningFailure(planning()!) ? "Send amendment and retry" : "Send and resume planning") : "Send message now", needs: "plan_message" }
    : reworking() ? { label: "Send and reopen", needs: "rework_task" }
    : mode() === "review" ? { label: "Send and reopen", needs: "request_changes" }
    : mode() === "paused" ? { label: "Send and resume", needs: "send_message" }
    : delivery() === "queue" && mode() === "live" ? { label: "Queue message", needs: "send_message" }
    : { label: "Send message now", needs: "send_message" };
  const sendWhy = () => mode() === "stopping" ? STOPPING : files.busy() ? "Attachments are still loading" : text().trim() ? reworking() && [...text()].length > 32_000 ? "Write no more than 32,000 characters" : undefined : "Write a message first";
  const planningEffect = () => control()?.effect ?? (changingRequest() ? (classifiedPlanningFailure(planning()!) ? "Sending your amendment starts planning again." : "Sending your changes resumes planning.") : props.folded ? "To the planner, not an answer to the question."
    : planning()!.planning === "ready" ? "Sending asks for changes to the plan. Create tasks waits for the revised plan." : undefined);
  const effect = () => planning() ? planningEffect() : reworking() ? "Sending reopens this task for rework" : control()?.effect ?? (mode() === "review" ? "Sending requests changes and reopens this task. Acceptance of this result is cleared."
    : mode() === "paused" ? "Sending resumes the task."
    : props.folded ? "To the coordinator, not an answer to the question." : undefined);
  const placeholder = () => mode() === "stopping" ? "Sending returns once the stop is confirmed" : SUGGEST_HINT;
  // `/` runs what this screen already offers: the title band's Stop or Resume, the delivery choice, the task's areas and New task.
  const commands = (): SlashCommand[] => {
    const v = m().view, at = (tab: "workers" | "changes" | "code") => () => ws.go({ name: "task", machine: v.machine.id, task: id(), tab });
    const demo = ws.store.mode === "demo";
    const s = planning();
    // The planning task reads only on its sprint: its Work details are the sprint's Workers tab, and it has no Changes or Code.
    if (s) return [
      ...(s.planning !== "created" && s.planning !== "cancelled" && mode() === "live" ? [{ name: "stop", label: "Stop planning", why: ws.gate("cancel_planning"), run: stopPlanning }] : []),
      ...(s.planning === "cancelled" && !s.planningFailure ? [{ name: "resume", label: "Resume planning", why: ws.gate("resume_planning"), run: resumePlanning }] : []),
      { name: "details", label: "Open Workers", run: () => ws.go(routes.workers()) },
      { name: "new", label: "New task in this project", run: () => ws.go({ name: "new", machine: v.machine.id, kind: "task", project: v.project.id }) },
    ];
    return [
      ...(mode() === "live" ? [{ name: "stop", label: `${stopLabel(ws.can(capabilityOf("continue_task")))} this task`, why: ws.gate("stop_task"), run: () => ws.stopTask(id()) },
        { name: "queue", label: "Queue messages until the coordinator is free", why: ws.gate("send_message"), run: () => setDelivery("queue") },
        { name: "now", label: "Send messages now", why: ws.gate("send_message"), run: () => setDelivery("send_now") }] : []),
      ...(mode() === "paused" ? [{ name: "resume", label: "Resume this task", why: ws.gate("continue_task"), run: () => void ws.run({ type: "continue_task", taskId: id(), now: now() }) }] : []),
      { name: "details", label: "Open Work details", run: at("workers") },
      ...(demo || ws.can(capabilityOf("accept_result")) || ws.can(capabilityOf("request_changes")) ? [{ name: "changes", label: "Open Changes", run: at("changes") }] : []),
      ...(demo || ws.can(capabilityOf("git_fetch")) || ws.can(FEATURES.editFiles) ? [{ name: "code", label: "Open Code", run: at("code") }] : []),
      { name: "new", label: "New task in this project", run: () => ws.go({ name: "new", machine: v.machine.id, kind: "task", project: v.project.id }) },
    ];
  };
  // A draft in progress is never folded away.
  const folded = () => props.folded && !reworking() && !changingRequest() && !opened() && !text().trim() && files.files().length === 0;
  createEffect(() => { if (!props.folded) setOpened(false); });
  let input: HTMLTextAreaElement | undefined;
  const shown = useContext(ScreenActive);
  createEffect(() => {
    if (!shown() || ws.reworkIntent?.()?.taskId !== id() || !binding()) return;
    setRework(true);
    setReworkError("");
    ws.dismissRework();
    queueMicrotask(() => { if (shown()) input?.focus(); });
  });
  createEffect(() => { if (planning() ? readOnly() : mode() !== "live" && mode() !== "paused") mic.stop(); });
  return <>
    <Show when={showQueue()}>
      <div class="queue" role="group" aria-label="Queued messages, not sent">
        <div class="queue-h"><I n="queue" s={14} class="faint" />Queued {queueIds().length + queueSending().length}, not sent yet<span class="warn">Saved on this machine · not sent</span></div>
        <For each={queueIds()}>{qid => <QueueRow message={() => m().queue.find(q => q.id === qid)!} ids={queueIds} mode={mode} onEdit={message => { write(text().trim() ? `${text().trimEnd()}\n${message.text}` : message.text); files.add(message.attachments ?? []); queueMicrotask(() => input?.focus()); }} />}</For>
        {/* A message being queued stands at the end of the queue from the press until the machine's row takes its place. */}
        <For each={queueSending().map(e => e.key)}>{key => <Show when={queueSending().find(e => e.key === key)}>{e => <div class="qrow" data-sent={e().state}>
          <I n={e().state === "failed" ? "alert" : "loader"} s={14} class={e().state === "failed" ? "st-bad" : "spin ws-wait"} />
          <Whole class="qrow-t" text={e().text} />
          <span class="qrow-end">
            <Show when={e().state === "failed"} fallback={<span class="cap faint" aria-live="polite">{e().state === "sent" ? "Queued" : "Queuing…"}</span>}>
              <span class="cap st-bad" role="alert">Not queued: {e().reason}</span>
              <Action kind="line" onClick={() => void ws.store.sends.retry(e().key)}>Retry</Action>
              <Action kind="line" onClick={() => ws.store.sends.edit(e().key)}>Edit</Action>
            </Show>
          </span>
        </div>}</Show>}</For>
      </div>
    </Show>
    {props.team}
    <Show when={!folded()} fallback={<div class="mc-fold"><button type="button" class="btn btn-line" onClick={() => { setOpened(true); queueMicrotask(() => input?.focus()); }}><I n="message" />Send a separate message</button></div>}>
      <Show when={mode() === "draft"}><RequestStatus text={ws.store.pendingTask("start_task", id()) ? REQUESTING.start_task : undefined} /></Show>
      <Show when={reworkError()}><div class="mc-effect st-bad" role="alert">{reworkError()}</div></Show>
      <MessageComposer group="Task composer" label="Message" placeholder={placeholder()} text={text()} onText={write} files={files} mic={mic}
        suggest={{ projectId: m().view.project.id, taskId: id(), commands, skills: true }}
        effect={effect()} off={readOnly()} attachOff={mode() === "stopping" ? STOPPING : undefined} voiceOff={mode() === "stopping" ? STOPPING : undefined}
        inputRef={el => { input = el; }}
        route={<Show when={planning()} fallback={<RouteBar id="task" up needs="set_task_settings" settings={taskSettings(m())} disabled={readOnly() || mode() === "stopping" ? "Unavailable for this task now" : undefined}
          onChange={settings => ws.run({ type: "set_task_settings", taskId: id(), settings: { ...taskSettings(m()), ...settings } })} />}>{s =>
          // The planner's model is the sprint's, changed for the planning task and every later planning pass at once.
          <RouteBar id="task" up needs="set_sprint_settings" settings={taskSettings(m())} disabled={readOnly() || mode() === "stopping" ? "Unavailable for planning now" : undefined}
            onChange={settings => ws.run({ type: "set_sprint_settings", sprintId: s().id, settings })} />}</Show>}
        delivery={<Show when={mode() === "live" && !planning()}><Delivery value={delivery()} onChange={setDelivery} off={ws.gate("send_message")} /></Show>}
        send={control() ? { label: control()!.label, needs: control()!.needs, why: control()!.why, go: control()!.go, run: { icon: control()!.icon } }
          : { label: plane().label, needs: plane().needs, why: sendWhy(), go: () => send(text()), insertAndSend: planning() || mode() === "live" || mode() === "review" || mode() === "paused" ? send : undefined }} />
    </Show>
  </>;
}
