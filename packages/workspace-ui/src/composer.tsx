import { plainChoice } from "./selectors";
import { createEffect, createMemo, createSignal, For, onCleanup, onMount, Show, type Accessor, type JSX } from "solid-js";
import { capabilityOf, FEATURES, type AttachmentInput, type Message, type TaskSettings, type WorkspaceCommand } from "@cortex/workspace-domain";
import { attachmentName, readAttachments, wireAttachments } from "./attachments";
import { useWs } from "./ctx";
import { createLocalDictation } from "./voice";
import { I, MenuItem, Pop } from "./kit";
import { duration, contextOf, stopLabel, taskSettings, tokens, type ComposerMode, type SessionModel } from "./session-model";

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
  const pasteNative = (drop = false) => {
    reading++; setBusy(true);
    pending = pending.then(async () => {
      try {
        const added = await (drop ? ws.store.takeDroppedFiles() : ws.store.readClipboardImage());
        wireAttachments([...files(), ...added]); add(added); setError("");
      } catch (e) { setError(e instanceof Error ? e.message : "Clipboard is unavailable."); }
    }).finally(() => { reading--; setBusy(reading > 0); });
  };
  return { files, error, busy, ingest, pasteNative, set: setFiles, add, remove: (item: AttachmentInput) => setFiles(files().filter(f => f !== item)), clear: () => { setFiles([]); setError(""); } };
}
export type Attachments = ReturnType<typeof createAttachments>;
export function attachmentEvents(a: Attachments, enabled: () => boolean = () => true) {
  const ws = useWs();
  const can = () => enabled() && ws.can(FEATURES.attachments);
  return {
    onDragOver: (e: DragEvent) => { if (e.dataTransfer?.types.some(t => t === "Files" || t === "text/uri-list")) { e.preventDefault(); if (e.dataTransfer) e.dataTransfer.dropEffect = can() ? "copy" : "none"; } },
    onDrop: (e: DragEvent) => { if (!e.dataTransfer) return; const files = [...e.dataTransfer.files]; if (!files.length && !e.dataTransfer.types.includes("text/uri-list")) return; e.preventDefault(); e.stopPropagation(); if (can()) { if (files.length) a.ingest(files); else a.pasteNative(true); } },
    onPaste: (e: ClipboardEvent) => { const files = [...(e.clipboardData?.files ?? [])]; if (files.length) { e.preventDefault(); if (can()) a.ingest(files); } else if (can() && e.clipboardData?.types.length === 0) { e.preventDefault(); a.pasteNative(); } },
  };
}
export const AttachmentStatus = (props: { a: Attachments }) => <>
  <Show when={props.a.busy()}><div class="cap muted" role="status">Reading attachments…</div></Show>
  <Show when={props.a.error()}><div class="cap st-warn" role="alert">{props.a.error()}</div></Show>
</>;
export function AttachButton(props: { a: Attachments; disabled?: string }) {
  const ws = useWs();
  let input!: HTMLInputElement;
  return <Show when={!props.disabled && ws.can(FEATURES.attachments)}>
    <input ref={input} type="file" multiple hidden aria-label="Choose files" onChange={e => { props.a.ingest([...(e.currentTarget.files ?? [])]); e.currentTarget.value = ""; }} />
    <button class="ib" aria-label="Attach files" onClick={() => input.click()}><I n="plus" /></button>
  </Show>;
}
export const AttachChips = (props: { files: AttachmentInput[] | undefined; onRemove?: (name: AttachmentInput) => void }) => <Show when={props.files?.length}>
  <span class="ws-atts"><For each={props.files}>{f => <span class="chip ws-att"><I n="file" s={12} /><span class="trunc">{attachmentName(f)}</span>
    <Show when={props.onRemove}><button class="ws-att-x" aria-label={`Remove ${attachmentName(f)}`} onClick={() => props.onRemove!(f)}><I n="x" s={12} /></button></Show>
  </span>}</For></span>
</Show>;
export const withFiles = (text: string, files: AttachmentInput[]) => files.length ? `${text.trimEnd()}\nAttached: ${files.map(attachmentName).join(", ")}` : text;
const label = (list: readonly { id: string; label: string }[], id: string) => list.find(x => x.id === id)?.label ?? plainChoice(id);

/** Model, effort, access and language: the same four choices on a new task and in the composer. */
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
  return <div class="c-route">
    {trigger("model", "Coordinator model", "cpu", () => options().models, () => props.settings.model)}
    {trigger("effort", "Reasoning effort", "gauge", efforts, () => props.settings.effort)}
    <Show when={!props.compact}>
      {trigger("mode", "Access mode", "shield", () => options().modes, () => props.settings.mode)}
      {trigger("lang", "Message language", "globe", () => options().languages, () => props.settings.language)}
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
    <Pop id={`m-mode-${props.id}`} class={cls()} align={align()} label="Access mode" style={{ width: "300px" }}>
      <div class="mi-h">Access mode</div>
      <For each={options().modes}>{m => <MenuItem label={m.label} sub={m.sub} check={m.id === props.settings.mode} onSelect={() => props.onChange({ mode: m.id as TaskSettings["mode"] })} />}</For>
    </Pop>
    <Pop id={`m-lang-${props.id}`} class={cls()} align={align()} label="Language">
      <div class="mi-h">Message language</div>
      <For each={options().languages}>{l => <MenuItem label={l.label} check={l.id === props.settings.language} onSelect={() => props.onChange({ language: l.id as TaskSettings["language"] })} />}</For>
    </Pop>
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
export function MicButton(props: { mic: Dictation; disabled?: string }) {
  const ws = useWs();
  const off = () => props.disabled ?? (ws.voice || ws.can(FEATURES.voice) ? undefined : ws.t("noVoice"));
  let button!: HTMLButtonElement;
  const toggle = () => { if (!off()) props.mic.toggle(); };
  onMount(() => {
    const key = (e: KeyboardEvent) => {
      if (e.defaultPrevented || e.repeat || !(e.ctrlKey || e.metaKey) || e.altKey || e.shiftKey || e.key.toLowerCase() !== "d") return;
      if (document.activeElement?.closest(".terminal")) return;
      const composer = document.activeElement?.closest(".composer");
      const first = [...document.querySelectorAll<HTMLButtonElement>('[aria-label="Voice input"]')].find(b => b.getAttribute("aria-disabled") !== "true" && b.getClientRects().length);
      if ((composer ? composer === button.closest(".composer") : first === button) && !off()) { e.preventDefault(); toggle(); }
    };
    document.addEventListener("keydown", key); onCleanup(() => document.removeEventListener("keydown", key));
  });
  return <Show when={!off()}><button ref={button} class="ib" aria-label="Voice input" aria-pressed={props.mic.on() && props.mic.phase() === "recording"}
    onClick={toggle}><I n="mic" /></button></Show>;
}

/** The free-form answer to a question. It takes what the chat composer takes: Shift+Enter for a new line, and the same attach button. */
export function OtherAnswer(props: { onSend: (text: string, attachments?: AttachmentInput[]) => void; onCancel: () => void }) {
  const [text, setText] = createSignal("");
  const files = createAttachments(), mic = createDictation();
  const append = (spoken: string) => setText(text().trim() ? text().trimEnd() + " " + spoken : spoken);
  const sendText = (value: string) => { if (value.trim() && !files.busy()) props.onSend(value, files.files()); };
  const send = () => sendText(text());
  return <div class="composer ws-dc-other" {...attachmentEvents(files)}>
    <MicBar mic={mic} onInsert={append} onInsertAndSend={spoken => sendText(text().trim() ? text().trimEnd() + " " + spoken : spoken)} />
    <AttachmentStatus a={files} />
    <AttachChips files={files.files()} onRemove={files.remove} />
    <textarea aria-label="Your answer" placeholder="Your answer. Shift+Enter for a new line." value={text()} onInput={e => setText(e.currentTarget.value)} ref={el => queueMicrotask(() => el.focus())}
      onKeyDown={e => { if (e.key === "Enter" && !e.shiftKey && !e.isComposing) { e.preventDefault(); send(); } else if (e.key === "Escape") { e.stopPropagation(); props.onCancel(); } }} />
    <div class="c-bar">
      <AttachButton a={files} />
      <div class="c-end">
        <MicButton mic={mic} />
        <button class="btn btn-sm" onClick={() => props.onCancel()}>Cancel</button>
        <button class="btn btn-primary btn-sm" aria-disabled={text().trim() && !files.busy() ? undefined : "true"} data-why={text().trim() ? (files.busy() ? "Attachments are still loading" : undefined) : "Write an answer first"} onClick={send}>Send answer</button>
      </div>
    </div>
  </div>;
}

/** One unsent message: reorder, send or remove. Edit takes it out of the queue and back into the composer, to be sent again. */
function QueueRow(props: { message: Accessor<Message>; ids: Accessor<string[]>; mode: Accessor<ComposerMode>; onEdit: (message: Message) => void }) {
  const ws = useWs();
  const id = () => props.message().id;
  const canSend = () => props.mode() === "stopping" ? "Available once every worker is confirmed as stopped" : undefined;
  const edit = () => { const message = props.message(); ws.ok({ type: "remove_queued", messageId: id() }, () => { props.onEdit(message); }); };
  const move = (by: number) => {
    const ids = props.ids(), at = ids.indexOf(id()), to = at + by;
    if (to < 0 || to >= ids.length) return;
    ws.run({ type: "move_queued", messageId: id(), before: by < 0 ? ids[to] : ids[to + 1] ?? null });
    queueMicrotask(() => document.querySelector<HTMLElement>(`[data-q="${id()}"] .grip`)?.focus());
  };
  return <div class="qrow" data-q={id()} draggable={true}
    onDragStart={e => e.dataTransfer?.setData("text/plain", id())} onDragOver={e => e.preventDefault()}
    onDrop={e => { e.preventDefault(); const from = e.dataTransfer?.getData("text/plain"); if (from && from !== id()) ws.run({ type: "move_queued", messageId: from, before: id() }); }}>
    <button class="ib grip" style={{ width: "22px", height: "24px" }} aria-label={`Reorder: ${props.message().text}`}
      onKeyDown={e => { const by = e.key === "ArrowUp" ? -1 : e.key === "ArrowDown" ? 1 : 0; if (by) { e.preventDefault(); move(by); } }}><I n="grip" s={14} /></button>
    <span class="grow trunc">{props.message().text}</span>
    <AttachChips files={props.message().attachments} />
    <button class="btn btn-sm" {...ws.needs("remove_queued")} onClick={edit}>Edit</button>
    <button class="btn btn-sm" aria-disabled={canSend() ? "true" : undefined} data-why={canSend()} {...ws.needs("send_queued")} onClick={() => { if (!canSend()) ws.run({ type: "send_queued", messageId: id(), resume: true, now: ws.store.clock() }); }}>Send now</button>
    <button class="ib" style={{ width: "24px", height: "24px" }} aria-label={`Remove from queue: ${props.message().text}`} {...ws.needs("remove_queued")} onClick={() => ws.run({ type: "remove_queued", messageId: id() })}><I n="x" s={14} /></button>
  </div>;
}

const CLOSED_TEXT: Partial<Record<ComposerMode, string>> = {
  draft: "Start the task to send a message.",
  closed: "This task is closed. Nothing more can be sent to it.", ended: "This run has ended. Nothing more can be sent to it.", archived: "This task is archived. Restore it to continue.",
  offline: "This machine's state is not confirmed. Sending stays off until it reconnects.",
};

/** The dock floats over the conversation. Its margins and the gap to the queue stay transparent. */
export function Composer(props: { model: Accessor<SessionModel>; onHeight: (px: number) => void; pill?: JSX.Element }) {
  const ws = useWs();
  const m = () => props.model();
  const id = () => m().view.id;
  const mode = () => m().mode;
  const [text, setText] = createSignal(ws.drafts.get(id()) ?? "");
  const write = (value: string) => { setText(value); ws.drafts.set(id(), value); };
  const mic = createDictation();
  const files = createAttachments();
  const empty = () => !text().trim() || files.busy();
  const now = () => ws.store.clock();
  const queueIds = createMemo(() => m().queue.map(q => q.id));
  const showQueue = () => ["live", "paused", "stopping"].includes(mode()) && queueIds().length > 0;
  const send = (value: string, how: "send_now" | "queue") => {
    if (!value.trim() || files.busy()) return;
    // A finished result has nobody to steer: a message sends it back to Cortex and reopens the task.
    if (mode() === "review") { ws.ok({ type: "request_changes", taskId: id(), text: value, attachments: files.files(), now: now() }, () => { write(""); files.clear(); }); return; }
    const resume = mode() === "paused";
    ws.ok({ type: "send_message", taskId: id(), text: value, mode: how, resume, now: now(), attachments: files.files() }, () => { write(""); files.clear(); });
  };
  const append = (spoken: string) => write(text().trim() ? `${text().trimEnd()} ${spoken}` : spoken);
  const resume = () => { if (empty()) ws.run({ type: "continue_task", taskId: id(), now: now() }); else send(text(), "send_now"); };
  const onKey = (e: KeyboardEvent) => {
    if (e.key !== "Enter" || e.isComposing || e.shiftKey) return;
    e.preventDefault();
    if (mode() === "paused") { if (!empty()) resume(); return; }
    if (mode() === "review") { send(text(), "send_now"); return; }
    if (mode() !== "live") return;
    send(text(), e.ctrlKey || e.metaKey ? "queue" : "send_now");
  };
  const readOnly = () => CLOSED_TEXT[mode()];
  const context = () => contextOf(m().session.context);
  const placeholder = () => readOnly() ?? (mode() === "paused" ? "Any message resumes the task…" : mode() === "stopping" ? "Sending returns once the stop is confirmed" : mode() === "review" ? "Ask about this result, or describe what to change…" : ws.can(FEATURES.attachments) ? "Message about this task, @ to attach files" : "Message about this task");
  let dock!: HTMLDivElement, input: HTMLTextAreaElement | undefined;
  onMount(() => {
    props.onHeight(dock.offsetHeight);
    if (typeof ResizeObserver === "undefined") return;
    const box = new ResizeObserver(() => props.onHeight(dock.offsetHeight));
    box.observe(dock); onCleanup(() => box.disconnect());
  });
  createEffect(() => { if (mode() !== "live" && mode() !== "paused") mic.stop(); });
  return <div class="dock" ref={dock}>
    {props.pill}
    <Show when={showQueue()}>
      <div class="queue" role="group" aria-label="Queued messages, not sent">
        <div class="queue-h"><I n="queue" s={14} class="faint" />Queued {queueIds().length}, not sent yet<span class="warn">Saved on this machine · not sent</span></div>
        <For each={queueIds()}>{qid => <QueueRow message={() => m().queue.find(q => q.id === qid)!} ids={queueIds} mode={mode} onEdit={message => { write(text().trim() ? `${text().trimEnd()}\n${message.text}` : message.text); files.add(message.attachments ?? []); queueMicrotask(() => input?.focus()); }} />}</For>
      </div>
    </Show>
    <div class="composer" {...attachmentEvents(files, () => !readOnly())}>
      <AttachmentStatus a={files} />
      <MicBar mic={mic} onInsert={append} onInsertAndSend={mode() === "live" ? value => send(text().trim() ? `${text().trimEnd()} ${value}` : value, "send_now") : undefined} />
      <AttachChips files={files.files()} onRemove={files.remove} />
      <textarea ref={input} aria-label="Message" placeholder={placeholder()} disabled={!!readOnly()} value={text()} onInput={e => write(e.currentTarget.value)} onKeyDown={onKey} />
      <div class="c-bar">
        <Show when={!readOnly()}><AttachButton a={files} /></Show>
        <RouteBar id="task" up needs="set_task_settings" settings={taskSettings(m())} disabled={mode() === "closed" || mode() === "ended" || mode() === "archived" || mode() === "offline" ? "Unavailable for this task now" : undefined}
          onChange={settings => ws.run({ type: "set_task_settings", taskId: id(), settings: { ...taskSettings(m()), ...settings } })} />
        <div class="c-end">
          <Show when={context()}>{c => <span class="ctx" aria-label={`Context ${c().percent}%`}>
            <span class="ring" style={{ "--p": `${c().percent}%` }}></span><span class="ctx-t">{c().percent}%</span>
          </span>}</Show>
          <Show when={m().session.tokens !== null}><span class="ctx ctx-n tnum" aria-label={`Usage ${tokens(m().session.tokens!)}`}>{tokens(m().session.tokens!)}</span></Show>
          <MicButton mic={mic} disabled={readOnly() ? "Voice input is off for this task now" : undefined} />
          <Show when={mode() === "live"}>
            <button class="btn btn-line c-send" aria-disabled={empty() ? "true" : undefined} data-why={empty() ? "Type a message first" : undefined} {...ws.needs("send_message")} onClick={() => send(text(), "queue")}>Queue<span class="key">Ctrl+Enter</span></button>
            <Show when={!empty()} fallback={<button class="btn btn-line c-send ws-send" {...ws.needs("stop_task")} onClick={() => ws.stopTask(id())}><I n="stop" s={12} class="st-bad" />{stopLabel(ws.can(capabilityOf("continue_task")))}</button>}>
              <button class="btn btn-primary c-send ws-send" {...ws.needs("send_message")} onClick={() => send(text(), "send_now")}>Steer<span class="key">Enter</span></button>
            </Show>
          </Show>
          <Show when={mode() === "review"}><button class="btn btn-primary btn-sm c-send ws-round" aria-disabled={empty() ? "true" : undefined} data-why={empty() ? "Type a message first" : undefined} aria-label="Send" {...ws.needs("request_changes")} onClick={() => send(text(), "send_now")}><I n="arrow-up" s={14} /></button></Show>
          <Show when={mode() === "paused"}><button class="btn btn-primary c-send ws-send" {...ws.needs("continue_task")} onClick={resume}><I n="play" s={12} />Resume</button></Show>
          <Show when={mode() === "stopping"}><button class="btn btn-line c-send ws-send" aria-disabled="true" data-why="Available once every worker is confirmed as stopped"><I n="play" s={12} />Resume</button></Show>
          <Show when={mode() === "draft"}><button class="btn btn-primary c-send ws-send" {...ws.needs("start_task")} onClick={() => ws.run({ type: "start_task", taskId: id(), now: now() })}><I n="play" s={12} />Start</button></Show>
        </div>
      </div>
    </div>
  </div>;
}
