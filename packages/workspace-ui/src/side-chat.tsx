import { createEffect, createMemo, createSignal, For, on, onCleanup, Show, useContext, type Accessor } from "solid-js";
import { orderedEfforts, type SessionOptions, type SideChatEntry, type SideChatPort, type TaskSettings } from "@cortex/workspace-domain";
import { autoGrow } from "./autogrow";
import { useWs } from "./ctx";
import { ScreenActive } from "./freshness";
import { I, Pop, closePop } from "./kit";
import { ResultText } from "./result-text";
import type { SideChatPlace } from "./prefs";
import "./side-chat.css";

export type Rect = { x: number; y: number; w: number; h: number };
export type Viewport = { w: number; h: number };
/** The smallest panel that still holds a question and an answer, and the gap kept from the window's edges. */
export const SIDE_MIN = { w: 280, h: 220 }, SIDE_MARGIN = 12;
/** How often a running answer is read again, and for how long at most. */
export const SIDE_POLL_MS = 1200, SIDE_POLL_FOR_MS = 6 * 60_000;
/** Ctrl+Shift+J opens the side chat, or collapses and expands it. */
export const SIDE_CHAT_KEYS = "Ctrl+Shift+J";

/**
 * The panel's rectangle inside the window: never smaller than its minimum, never larger than the window less its margins,
 * and always wholly on screen. A place below 0 is the default corner (bottom right).
 */
export function clampPlace(r: Rect, view: Viewport): Rect {
  const maxW = Math.max(SIDE_MIN.w, view.w - 2 * SIDE_MARGIN), maxH = Math.max(SIDE_MIN.h, view.h - 2 * SIDE_MARGIN);
  const w = Math.round(Math.min(Math.max(r.w, SIDE_MIN.w), maxW)), h = Math.round(Math.min(Math.max(r.h, SIDE_MIN.h), maxH));
  const fit = (v: number, size: number, room: number) => Math.round(Math.min(Math.max(v, SIDE_MARGIN), Math.max(SIDE_MARGIN, room - size - SIDE_MARGIN)));
  return { w, h, x: r.x < 0 ? Math.max(SIDE_MARGIN, view.w - w - SIDE_MARGIN) : fit(r.x, w, view.w), y: r.y < 0 ? Math.max(SIDE_MARGIN, view.h - h - SIDE_MARGIN) : fit(r.y, h, view.h) };
}
/** The collapsed chat's label: its name, and how many exchanges it holds once it holds any. */
export const pillLabel = (name: string, count: number) => count > 0 ? `${name} (${count})` : name;
/** Why there is no answer, in words. */
export function sideChatReason(entry: SideChatEntry): string {
  if (entry.state === "interrupted") return "Interrupted: the machine stopped before the answer was written. Ask again.";
  const words: Record<string, string> = {
    timeout: "No answer in time. Ask again, or pick a faster model.",
    tool_call: "The answer was dropped: the model tried a tool that is not read-only.",
    no_read_only_mode: "This engine offers no read-only mode, so it cannot answer here.",
    codex_hardening_unavailable: "This Codex connection cannot be checked as hardened, so it cannot answer here.",
    engine_unavailable: "The engine could not be started.",
    engine_failed: "The engine stopped before answering.",
    model_refused: "The model did not answer.",
    no_answer: "The model gave no answer.",
    context_unavailable: "The task could not be read for this answer.",
    no_checkout: "The task's folder is unavailable.",
  };
  return words[entry.reason ?? ""] ?? "No answer could be written. Ask again.";
}

/** The side chat's own model and effort lists: the task's coordinator choices, unavailable ones left out. */
function choices(options: SessionOptions, model: string) {
  const models = options.models.filter(m => !m.unavailable);
  const offered = options.models.find(m => m.id === model)?.efforts;
  const efforts = options.efforts.filter(e => !e.unavailable && (!offered || offered.includes(e.id)));
  return { models, efforts: orderedEfforts(efforts.map(e => e.id)).map(id => efforts.find(e => e.id === id)!) };
}

export type SideChatProps = {
  taskId: string;
  port: SideChatPort;
  /** The task's own coordinator model and effort: where the chat starts, and where it returns for another task. */
  defaults: Accessor<Pick<TaskSettings, "model" | "effort">>;
  options: Accessor<SessionOptions>;
  place: Accessor<SideChatPlace>;
  setPlace: (place: SideChatPlace) => void;
  phone?: boolean;
  title: string;
  note: string;
  viewport?: () => Viewport;
};

/**
 * The task's floating side chat. It asks the machine (`SideChatPort`) and nothing else: it sends no task command, writes
 * nothing the workspace store holds and adds nothing to the task's conversation. Dragged by its header, resized by its
 * corner, collapsed to a pill (Esc or the minus) and closed with the cross; on a phone it is a bottom sheet.
 */
export function SideChatPanel(props: SideChatProps) {
  const viewport = () => props.viewport?.() ?? { w: window.innerWidth, h: window.innerHeight };
  const [entries, setEntries] = createSignal<SideChatEntry[]>([]);
  const [text, setText] = createSignal("");
  const [refusal, setRefusal] = createSignal<string | null>(null);
  const [sending, setSending] = createSignal(false);
  const [picked, setPicked] = createSignal<{ model?: string; effort?: string }>({});
  const model = () => picked().model ?? props.defaults().model;
  const lists = createMemo(() => choices(props.options(), model()));
  const effort = () => {
    const wanted = picked().effort ?? props.defaults().effort, offered = lists().efforts.map(e => e.id);
    return offered.length === 0 || offered.includes(wanted) ? wanted : offered[0];
  };
  const [view, setView] = createSignal(viewport());
  // While a drag lasts the panel follows the pointer; the place is remembered once, when it is let go.
  const [dragged, setDragged] = createSignal<Rect | null>(null);
  const rect = createMemo(() => dragged() ?? clampPlace(props.place(), view()));
  const running = () => entries().some(e => e.state === "running");
  let log: HTMLDivElement | undefined, field: HTMLTextAreaElement | undefined, box: HTMLElement | undefined, alive = true;
  const polls = new Set<string>();
  onCleanup(() => { alive = false; });

  const keep = (entry: SideChatEntry) => setEntries(list => list.some(e => e.id === entry.id) ? list.map(e => e.id === entry.id ? entry : e) : [...list, entry]);
  const follow = (entryId: string, task: string) => {
    if (polls.has(entryId)) return;
    polls.add(entryId);
    const until = Date.now() + SIDE_POLL_FOR_MS;
    const tick = async () => {
      if (!alive || task !== props.taskId) { polls.delete(entryId); return; }
      const read = await props.port.poll(task, entryId).catch(() => null);
      if (!alive || task !== props.taskId) { polls.delete(entryId); return; }
      if (read) keep(read);
      if ((read?.state ?? "running") === "running" && Date.now() < until) setTimeout(tick, SIDE_POLL_MS);
      else polls.delete(entryId);
    };
    setTimeout(tick, SIDE_POLL_MS);
  };
  // Each task has its own history and starts on its own route. A closed chat asks the machine nothing.
  const isOpen = createMemo(() => props.place().open);
  createEffect(on([() => props.taskId, isOpen], ([task, open]) => {
    setEntries([]); setPicked({}); setRefusal(null); polls.clear();
    if (!open) return;
    void props.port.history(task).then(list => {
      if (!alive || task !== props.taskId) return;
      setEntries(list);
      for (const e of list) if (e.state === "running") follow(e.id, task);
    });
  }));
  createEffect(on(() => entries().length, () => queueMicrotask(() => { if (log) log.scrollTop = log.scrollHeight; })));
  const onResize = () => setView(viewport());
  window.addEventListener("resize", onResize);
  onCleanup(() => window.removeEventListener("resize", onResize));

  const save = (patch: Partial<SideChatPlace>) => props.setPlace({ ...props.place(), ...patch });
  const collapse = () => save({ collapsed: true });
  const close = () => save({ open: false, collapsed: false });
  const send = async () => {
    const question = text().trim();
    if (!question || sending()) return;
    setSending(true); setRefusal(null);
    const task = props.taskId;
    try {
      const reply = await props.port.ask({ taskId: task, question, ...(model() ? { model: model() } : {}), ...(effort() ? { effort: effort() } : {}) });
      if (!alive || task !== props.taskId) return;
      if (reply.ok) { setText(""); keep(reply.entry); if (reply.entry.state === "running") follow(reply.entry.id, task); }
      else setRefusal(reply.message || "The question could not be asked.");
    } finally { setSending(false); }
  };
  const clear = async () => {
    const task = props.taskId;
    if (await props.port.clear(task) && alive && task === props.taskId) setEntries(list => list.filter(e => e.state === "running"));
  };

  /** Drags move the panel, the corner resizes it; both stay inside the window and are remembered when released. */
  const pointer = (kind: "move" | "size") => (e: PointerEvent) => {
    if (props.phone || e.button !== 0 || (kind === "move" && (e.target as Element).closest("button, select, textarea, input"))) return;
    e.preventDefault();
    // The press takes no focus (it would select text); the panel takes it, so Esc still collapses it after a drag.
    if (box && !box.contains(document.activeElement)) box.focus({ preventScroll: true });
    const start = { px: e.clientX, py: e.clientY, ...rect() };
    const move = (m: PointerEvent) => {
      const dx = m.clientX - start.px, dy = m.clientY - start.py;
      setDragged(clampPlace(kind === "move" ? { x: start.x + dx, y: start.y + dy, w: start.w, h: start.h } : { x: start.x, y: start.y, w: start.w + dx, h: start.h + dy }, view()));
    };
    const up = () => {
      window.removeEventListener("pointermove", move); window.removeEventListener("pointerup", up); window.removeEventListener("pointercancel", up);
      const done = dragged();
      setDragged(null);
      if (done) save(done);
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
    window.addEventListener("pointercancel", up);
  };
  const style = () => props.phone ? {} : { left: `${rect().x}px`, top: `${rect().y}px`, width: `${rect().w}px`, height: `${rect().h}px` };
  const label = (list: readonly { id: string; label: string }[], id: string) => list.find(x => x.id === id)?.label ?? id;
  /** The app's own picker (the composer's): a trigger that opens a menu of choices, the current one checked. */
  const picker = (kind: "model" | "effort", title: string, icon: string, list: () => readonly { id: string; label: string; sub?: string }[], current: () => string, choose: (id: string) => void) => {
    const popId = `sc-${kind}-${props.taskId}`;
    const text = () => current() ? label(list(), current()) : "Task default";
    return <>
      <button type="button" class="sel" popovertarget={popId} aria-haspopup="menu" aria-label={`Side chat ${kind}: ${text()}`}>
        <I n={icon} s={14} /><span class="sel-t">{text()}</span><I n="chevron-down" s={12} />
      </button>
      <Pop id={popId} class="pop-anchor" align="left" label={title} role="menu">
        <div class="mi-h">{title}</div>
        <For each={list()}>{x => <button type="button" class="mi" role="menuitem" aria-checked={x.id === current()} onClick={e => { closePop(e.currentTarget); choose(x.id); }}>
          <span class="grow">{x.label}<Show when={x.sub}><span class="sub">{x.sub}</span></Show></span>
          <Show when={x.id === current()}><I n="check" s={14} /></Show>
        </button>}</For>
      </Pop>
    </>;
  };

  return <Show when={props.place().open}>
    <Show when={!props.place().collapsed} fallback={
      <div class="sidechat-pill" data-shell={props.phone ? "phone" : undefined}>
        <button class="btn btn-line sidechat-pill-open" aria-label={`Expand ${props.title}`} onClick={() => save({ collapsed: false })}>
          <I n="message" s={14} /><span>{pillLabel(props.title, entries().length)}</span><Show when={running()}><I n="loader" s={12} class="spin" /></Show>
        </button>
        <button class="ib" aria-label={`Close ${props.title}`} onClick={close}><I n="x" s={14} /></button>
      </div>
    }>
      {/* Focusable itself, so a press anywhere in the panel keeps focus inside it and Esc collapses it from there too. */}
      <section ref={box} tabindex="-1" class="sidechat" classList={{ "sidechat-sheet": !!props.phone }} role="dialog" aria-modal="false" aria-label={props.title} style={style()}
        onKeyDown={e => { if (e.key === "Escape" && !e.defaultPrevented) { e.preventDefault(); e.stopPropagation(); collapse(); } }}>
        <header class="sidechat-head" onPointerDown={pointer("move")}>
          <I n="message" s={14} />
          <span class="sidechat-title">{props.title}</span>
          <span class="sidechat-note">{props.note}</span>
          <button class="ib" aria-label={`Collapse ${props.title}`} onClick={collapse}><I n="minus" s={14} /></button>
          <button class="ib" aria-label={`Close ${props.title}`} onClick={close}><I n="x" s={14} /></button>
        </header>
        <div class="sidechat-route">
          {picker("model", "Side chat model", "cpu", () => lists().models.map(m => ({ id: m.id, label: m.label, sub: m.engine })), model, id => setPicked({ model: id }))}
          <Show when={lists().efforts.length > 0}>
            {picker("effort", "Effort", "gauge", () => lists().efforts, effort, id => setPicked({ ...picked(), effort: id }))}
          </Show>
        </div>
        <div class="sidechat-log" ref={log} aria-live="polite">
          <Show when={entries().length > 0} fallback={<p class="sidechat-empty">Ask about this task. Answers read the task, its reports, recent messages, its changes and files, and change nothing.</p>}>
            <For each={entries()}>{entry => <div class="sidechat-entry" data-state={entry.state}>
              <p class="sidechat-q">{entry.question}</p>
              <Show when={entry.state === "running"}><p class="sidechat-wait" role="status"><I n="loader" s={12} class="spin" />Answering…</p></Show>
              {/* The answer is Markdown, rendered as task results are. */}
              <Show when={entry.state === "done"}><div class="sidechat-a"><ResultText breaks text={entry.answer ?? ""} /></div></Show>
              <Show when={entry.state === "failed" || entry.state === "interrupted"}><p class="sidechat-why">{sideChatReason(entry)}</p></Show>
              <Show when={entry.route?.model}>{m => <p class="sidechat-by">{label(props.options().models, m())}{entry.route?.effort ? ` · ${label(props.options().efforts, entry.route.effort)}` : ""}</p>}</Show>
            </div>}</For>
          </Show>
        </div>
        <Show when={refusal()}>{why => <p class="sidechat-why" role="alert">{why()}</p>}</Show>
        <form class="sidechat-form" onSubmit={e => { e.preventDefault(); void send(); }}>
          <textarea ref={el => { field = el; autoGrow(el, text); }} class="inp" rows={2} aria-label="Side chat question" placeholder="Ask about this task" value={text()} onInput={e => setText(e.currentTarget.value)}
            onKeyDown={e => { if (e.key === "Enter" && !e.shiftKey && !e.isComposing) { e.preventDefault(); void send(); } }} />
          <div class="sidechat-actions">
            <button type="button" class="btn btn-sm" aria-disabled={entries().length === 0 ? "true" : undefined} data-why={entries().length === 0 ? "Nothing to clear yet" : undefined} onClick={() => { if (entries().length) void clear(); }}>Clear</button>
            <button type="submit" class="btn btn-primary btn-sm" aria-disabled={!text().trim() || sending() ? "true" : undefined} data-why={!text().trim() ? "Write a question first" : sending() ? "Sending…" : undefined}><I n="send" s={12} />Ask</button>
          </div>
        </form>
        <Show when={!props.phone}><span class="sidechat-grip" aria-hidden="true" onPointerDown={pointer("size")} /></Show>
      </section>
    </Show>
  </Show>;
}

/** Opens the side chat, or collapses and expands it. */
export function toggleSideChat(place: SideChatPlace): SideChatPlace {
  if (!place.open) return { ...place, open: true, collapsed: false };
  return { ...place, collapsed: !place.collapsed };
}

/** The side chat of the task on view, with its shortcut. Shown only while its screen is shown and the machine offers it. */
export function SideChat(props: { taskId: string; defaults: Accessor<Pick<TaskSettings, "model" | "effort">> }) {
  const ws = useWs();
  const shown = useContext(ScreenActive);
  const port = () => ws.store.sideChat;
  createEffect(() => {
    if (!shown() || !port() || ws.shell.kind === "phone") return;
    const onKey = (e: KeyboardEvent) => {
      if (!(e.ctrlKey || e.metaKey) || !e.shiftKey || e.altKey || e.code !== "KeyJ" || document.querySelector("dialog[open]")) return;
      e.preventDefault();
      ws.panels.setSideChat(toggleSideChat(ws.panels.sideChat()));
    };
    document.addEventListener("keydown", onKey);
    onCleanup(() => document.removeEventListener("keydown", onKey));
  });
  return <Show when={shown() && port()}>{p => <SideChatPanel taskId={props.taskId} port={p()} defaults={props.defaults}
    options={() => ws.store.state().options} place={ws.panels.sideChat} setPlace={ws.panels.setSideChat}
    phone={ws.shell.kind === "phone"} title={ws.t("sideChat")} note={ws.t("sideChatNote")} />}</Show>;
}
