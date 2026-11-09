import { createMemo, createSignal, For, onCleanup, onMount, Show, type JSX } from "solid-js";
import type { BModel, Tone } from "./model";

export type GlyphName = "files" | "search" | "branch" | "mission" | "board" | "gear" | "check" | "x" | "dash" | "question" | "half"
  | "pause" | "minus" | "slash" | "plug" | "plugoff" | "plugzap" | "history" | "clock" | "flask" | "key" | "message" | "filecheck"
  | "lock" | "warn" | "close" | "terminal";

const circle = () => <circle cx="8" cy="8" r="6" />;
function glyphBody(name: GlyphName): JSX.Element {
  switch (name) {
    case "files": return <path d="M3 2h6l3 3v9H3zM9 2v3h3" />;
    case "search": return <><circle cx="7" cy="7" r="4.5" /><path d="M10.5 10.5L14 14" /></>;
    case "branch": return <><circle cx="4.5" cy="3.5" r="1.5" /><circle cx="4.5" cy="12.5" r="1.5" /><circle cx="11.5" cy="5.5" r="1.5" /><path d="M4.5 5v6M11.5 7c0 3-7 2-7 4" /></>;
    case "mission": return <path d="M2 3h12v10H2zM2 6.5h12M6 6.5V13" />;
    case "board": return <path d="M2 3h3.5v10H2zM6.25 3h3.5v7h-3.5zM10.5 3H14v5h-3.5z" />;
    case "gear": return <><circle cx="8" cy="8" r="2.5" /><path d="M8 1.5v2M8 12.5v2M1.5 8h2M12.5 8h2M3.4 3.4l1.4 1.4M11.2 11.2l1.4 1.4M3.4 12.6l1.4-1.4M11.2 4.8l1.4-1.4" /></>;
    case "check": return <>{circle()}<path d="M5 8.2l2 2 4-4.2" /></>;
    case "x": return <>{circle()}<path d="M6 6l4 4M10 6l-4 4" /></>;
    case "dash": return <>{circle()}<path d="M5.5 8h5" /></>;
    case "question": return <>{circle()}<path d="M6.3 6.3a1.8 1.8 0 1 1 2.4 1.7c-.5.2-.7.6-.7 1V10M8 11.6v.4" /></>;
    case "half": return <>{circle()}<path d="M8 2a6 6 0 0 1 0 12z" fill="currentColor" /></>;
    case "pause": return <path d="M6 4v8M10 4v8" />;
    case "minus": return <path d="M3 8h10" />;
    case "slash": return <>{circle()}<path d="M3.8 12.2l8.4-8.4" /></>;
    case "plug": return <path d="M6 2v3M10 2v3M4.5 5h7v2.5a3.5 3.5 0 0 1-7 0zM8 11v3" />;
    case "plugoff": return <path d="M6 2v3M10 2v3M4.5 5h7v2.5a3.5 3.5 0 0 1-7 0zM8 11v3M2 2l12 12" />;
    case "plugzap": return <path d="M6 2v3M10 2v3M4.5 5h7v2.5a3.5 3.5 0 0 1-7 0zM8 11v3M13 9.5l-1.5 2h2L12 13.5" />;
    case "history": return <path d="M2.5 8a5.5 5.5 0 1 0 1.6-3.9M2.5 2.5V5H5M8 5v3l2 1.5" />;
    case "clock": return <>{circle()}<path d="M8 5v3l2 1.5" /></>;
    case "flask": return <path d="M6 2h4M6.5 2v4L3 13.5h10L9.5 6V2" />;
    case "key": return <><circle cx="5.5" cy="10.5" r="2.5" /><path d="M7.3 8.7L13 3M11 5l1.5 1.5" /></>;
    case "message": return <path d="M2.5 3h11v8h-6l-3 2.5V11h-2z" />;
    case "filecheck": return <path d="M4 2h5l3 3v9H4zM9 2v3h3M6 10l1.5 1.5 3-3" />;
    case "lock": return <path d="M4.5 7h7v6h-7zM6 7V5a2 2 0 0 1 4 0v2" />;
    case "warn": return <path d="M8 2.5l6 11H2zM8 6.5v3M8 11.3v.4" />;
    case "close": return <path d="M4 4l8 8M12 4l-8 8" />;
    case "terminal": return <path d="M2 3h12v10H2zM4.5 6l2 2-2 2M8 10h3" />;
  }
}

export function Glyph(props: { name: GlyphName }) {
  return <svg class="cb-glyph" viewBox="0 0 16 16" width="16" height="16" aria-hidden="true" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">{glyphBody(props.name)}</svg>;
}

export function Badge(props: { tone: Tone; glyph?: GlyphName; text: string; prov?: string }) {
  return <span class="cb-badge-wrap">
    <span class={`cb-badge t-${props.tone}`} title={props.text}>
      <Show when={props.glyph}><Glyph name={props.glyph!} /></Show>
      <span class="cb-badge-label">{props.text}</span>
    </span>
    <Show when={props.prov}><span class="cb-prov">· {props.prov}</span></Show>
  </span>;
}

/** Mutating prototype control: visible "Mock" tag, "(mock)" in the accessible name. */
export function MockButton(props: { label: string; onClick?: () => void; disabled?: boolean; primary?: boolean; type?: "button" | "submit"; id?: string }) {
  return <button id={props.id} type={props.type ?? "button"} class="cb-btn" classList={{ primary: !!props.primary }} disabled={props.disabled}
    aria-label={`${props.label} (mock)`} onClick={() => props.onClick?.()}>
    {props.label} <span class="cb-mock-tag" aria-hidden="true">· Mock</span>
  </button>;
}

export function ActionRow(props: { reason?: string | null; children: JSX.Element }) {
  return <div class="cb-actions">
    <div class="cb-actions-btns">{props.children}</div>
    <Show when={props.reason}><p class="cb-reason"><Glyph name="lock" /> {props.reason}</p></Show>
  </div>;
}

export function Receipts(props: { m: BModel; where: string; last?: number }) {
  const list = () => { const all = props.m.receiptsFor(props.where); return props.last ? all.slice(-props.last) : all; };
  return <div class="cb-receipts" role="status" tabindex="-1" data-receipts={props.where}>
    <For each={list()}>{r => <p class="cb-receipt"><Glyph name="flask" /> <span>{r.text}</span></p>}</For>
  </div>;
}

export function Skeleton(props: { label: string; rows?: number }) {
  return <div class="cb-skeleton">
    <p role="status">{props.label}</p>
    <For each={Array.from({ length: props.rows ?? 5 }, (_, i) => i)}>{() => <div class="cb-skel-row" aria-hidden="true" />}</For>
  </div>;
}

export function ConnectionBanner(props: { m: BModel }) {
  const m = props.m;
  return <>
    <Show when={m.conn() === "disconnected"}>
      <div class="cb-banner t-danger" role="note">
        <Glyph name="plugoff" />
        <p>Machine disconnected. Last confirmed {m.lastConfirmed()}. Current states are unknown. Actions are unavailable.</p>
        <div class="cb-banner-actions">
          <MockButton label="Reconnect" onClick={m.reconnect} disabled={m.ui.loading} />
          <button type="button" class="cb-btn" onClick={() => m.openSettings()}>Connection settings</button>
        </div>
      </div>
    </Show>
    <Show when={m.conn() === "offline"}>
      <div class="cb-banner t-neutral" role="note">
        <Glyph name="clock" />
        <p>As of {m.lastConfirmed()}: showing the last confirmed state (cached). Read-only; mutating actions are disabled.</p>
      </div>
    </Show>
  </>;
}

export function TaskListState(props: { m: BModel; children: JSX.Element; label?: string }) {
  const m = props.m;
  return <Show when={!m.ui.loading} fallback={<Skeleton label="Loading tasks…" />}>
    <Show when={!m.store.error()} fallback={
      <div class="cb-banner t-danger" role="note"><Glyph name="x" />
        <p>Could not load {props.label ?? "tasks"} (fixture error: {m.store.error()}). Nothing was changed.</p>
        <button type="button" class="cb-btn" onClick={() => m.store.reset()}>Reset mock store to recover</button>
      </div>}>
      <Show when={m.tasks().length > 0} fallback={
        <div class="cb-empty">
          <p class="cb-empty-h">No tasks in this project yet.</p>
          <p>Create a task to start a discussion.</p>
          <button type="button" class="cb-btn" onClick={() => m.openNewTask()}>New task…</button>
        </div>}>
        {props.children}
      </Show>
    </Show>
  </Show>;
}

/** Virtualised single-select listbox: only rows near the viewport are in the DOM. */
export function VirtualListbox<T>(props: {
  id: string; label: string; items: T[]; rowRem: number; rootPx: number;
  keyOf: (item: T) => string; selectedKey?: string | null;
  render: (item: T, index: number) => JSX.Element; onActivate: (item: T, pin: boolean) => void;
}) {
  let el!: HTMLDivElement;
  const [scrollTop, setScrollTop] = createSignal(0);
  const [height, setHeight] = createSignal(600);
  const [active, setActive] = createSignal(0);
  const rowPx = () => props.rowRem * props.rootPx;
  const range = createMemo(() => {
    const start = Math.max(0, Math.floor(scrollTop() / rowPx()) - 6);
    const end = Math.min(props.items.length, Math.ceil((scrollTop() + height()) / rowPx()) + 6);
    return { start, end };
  });
  const indices = createMemo(() => { const { start, end } = range(); return Array.from({ length: Math.max(0, end - start) }, (_, k) => start + k); });
  onMount(() => {
    const measure = () => setHeight(el.clientHeight || 600);
    measure();
    if (typeof ResizeObserver !== "undefined") { const ro = new ResizeObserver(measure); ro.observe(el); onCleanup(() => ro.disconnect()); }
  });
  const ensureVisible = (i: number) => {
    const top = i * rowPx(), bottom = top + rowPx();
    if (top < el.scrollTop) el.scrollTop = top;
    else if (bottom > el.scrollTop + el.clientHeight) el.scrollTop = bottom - el.clientHeight;
    setScrollTop(el.scrollTop);
  };
  const move = (i: number) => {
    const n = props.items.length; if (!n) return;
    const next = Math.max(0, Math.min(n - 1, i));
    setActive(next); ensureVisible(next);
  };
  const onKey = (e: KeyboardEvent) => {
    const page = Math.max(1, Math.floor(height() / rowPx()) - 1);
    switch (e.key) {
      case "ArrowDown": move(active() + 1); break;
      case "ArrowUp": move(active() - 1); break;
      case "PageDown": move(active() + page); break;
      case "PageUp": move(active() - page); break;
      case "Home": move(0); break;
      case "End": move(props.items.length - 1); break;
      case "Enter": { const it = props.items[active()]; if (it !== undefined) props.onActivate(it, true); break; }
      case " ": { const it = props.items[active()]; if (it !== undefined) props.onActivate(it, false); break; }
      default: return;
    }
    e.preventDefault();
  };
  const activeInRange = () => active() >= range().start && active() < range().end && active() < props.items.length;
  return <div ref={el} id={props.id} class="cb-vlist" role="listbox" tabindex="0" aria-label={props.label}
    aria-activedescendant={activeInRange() ? `${props.id}-${active()}` : undefined}
    onScroll={() => setScrollTop(el.scrollTop)} onKeyDown={onKey}>
    <div class="cb-vlist-inner" style={{ height: `${props.items.length * rowPx()}px` }}>
      <For each={indices()}>{index => {
        const item = () => props.items[index];
        return <Show when={item() !== undefined}>
          <div id={`${props.id}-${index}`} role="option" class="cb-vrow" classList={{ active: index === active() }}
            aria-selected={!!props.selectedKey && props.keyOf(item()) === props.selectedKey}
            aria-setsize={props.items.length} aria-posinset={index + 1}
            style={{ top: `${index * rowPx()}px`, height: `${rowPx()}px` }}
            onClick={() => { setActive(index); props.onActivate(item(), false); }}
            onDblClick={() => props.onActivate(item(), true)}>
            {props.render(item(), index)}
          </div>
        </Show>;
      }}</For>
    </div>
  </div>;
}

/** Modal dialog: native showModal traps focus and Escape closes; focus returns to the opener. */
export function Modal(props: { label: string; onClose: () => void; children: JSX.Element; wide?: boolean }) {
  let dialog!: HTMLDialogElement;
  const opener = document.activeElement as HTMLElement | null;
  onMount(() => {
    try { dialog.showModal(); } catch { dialog.setAttribute("open", ""); }
    dialog.querySelector<HTMLElement>("[data-autofocus]")?.focus();
  });
  onCleanup(() => {
    try { dialog.close(); } catch { /* jsdom without dialog support */ }
    if (opener?.isConnected) opener.focus();
  });
  return <dialog ref={dialog} class="cb-dialog" classList={{ wide: !!props.wide }} aria-label={props.label}
    onCancel={e => { e.preventDefault(); props.onClose(); }}
    onKeyDown={e => { if (e.key === "Escape") { e.preventDefault(); props.onClose(); } }}>
    {props.children}
  </dialog>;
}
