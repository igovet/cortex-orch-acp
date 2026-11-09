import { createEffect, createMemo, createSignal, For, on, onCleanup, onMount, Show, type JSX } from "solid-js";
import { useC, uid } from "./ctx";
import { OUTCOME_GLYPH, OUTCOME_LABEL, OUTCOME_ORDER, OUTCOME_TONE, type Check, type Outcome, type Tone } from "./model";

/** Status badge: visible text + decorative glyph + colour role. Colour is never the only signal. */
export function Badge(p: { tone: Tone; glyph: string; label: string }) {
  return <span class={`c-badge tone-${p.tone}`}><span class="c-glyph" aria-hidden="true">{p.glyph}</span>{p.label}</span>;
}

export function OutcomeBadge(p: { outcome: Outcome }) {
  return <Badge tone={OUTCOME_TONE[p.outcome]} glyph={OUTCOME_GLYPH[p.outcome]} label={OUTCOME_LABEL[p.outcome]} />;
}

/**
 * Every mutating prototype control carries a visible "Mock" tag and "(mock)" in its accessible name.
 * Unavailable controls stay focusable (aria-disabled) and describe their reason.
 */
export function MockButton(p: {
  label: string; onAct: () => void; tone?: "primary" | "danger";
  reason?: string | null; reasonId?: string; id?: string;
}) {
  return <button type="button" id={p.id} class="c-btn"
    classList={{ "c-primary": p.tone === "primary", "c-danger-btn": p.tone === "danger" }}
    aria-label={`${p.label} (mock)`} aria-disabled={p.reason ? true : undefined}
    aria-describedby={p.reason ? p.reasonId : undefined}
    onClick={() => { if (!p.reason) p.onAct(); }}>
    {p.label}<span class="c-mock-tag" aria-hidden="true"> · Mock</span>
  </button>;
}

export function Actions(p: { reason?: string | null; children: (reasonId: string) => JSX.Element }) {
  const id = uid("reason");
  return <div class="c-actions-wrap">
    <div class="c-actions">{p.children(id)}</div>
    <Show when={p.reason}><p id={id} class="c-reason">{p.reason}</p></Show>
  </div>;
}

/** Inline, persistent mock receipt where the action was taken (never a toast). */
export function Receipt(p: { rkey: string }) {
  const c = useC();
  return <Show when={c.receipts()[p.rkey]}>{message =>
    <p class="c-receipt" role="status"><Badge tone="mock" glyph="⚗" label="Mock" />{" "}{message()}</p>
  }</Show>;
}

export function CheckTable(p: { checks: readonly Check[] }) {
  const sorted = () => [...p.checks].sort((a, b) => OUTCOME_ORDER[a.outcome] - OUTCOME_ORDER[b.outcome]);
  const notRun = () => p.checks.filter(c => c.outcome === "not_run");
  return <div class="c-checks">
    <table class="c-table">
      <caption class="c-sr">Checks, sorted Failed, Not run, Unverified, Passed</caption>
      <thead><tr><th scope="col">Check</th><th scope="col">Outcome</th><th scope="col">Source</th><th scope="col">Duration</th><th scope="col">Details</th></tr></thead>
      <tbody><For each={sorted()}>{check => <tr>
        <th scope="row">{check.name}</th><td><OutcomeBadge outcome={check.outcome} /></td>
        <td>fixture record</td><td>unknown</td><td>{check.detail}</td>
      </tr>}</For></tbody>
    </table>
    <h4 class="c-h4">Required but not run</h4>
    <Show when={notRun().length} fallback={<p class="c-muted">None in the fixture.</p>}>
      <ul class="c-plain"><For each={notRun()}>{check => <li><OutcomeBadge outcome="not_run" />{" "}{check.name} (required). {check.detail}</li>}</For></ul>
    </Show>
  </div>;
}

/** Long messages collapse to 12 lines with an explicit expand control. */
export function LongText(p: { text: string; label: string }) {
  const [open, setOpen] = createSignal(false);
  const id = uid("long");
  const long = () => p.text.length > 600;
  return <div class="c-longtext">
    <p id={id} class="c-prose" classList={{ "c-clamp12": long() && !open() }}>{p.text}</p>
    <Show when={long()}>
      <button type="button" class="c-link" aria-expanded={open()} aria-controls={id} onClick={() => setOpen(!open())}>
        {open() ? `Show less of the ${p.label}` : `Show full ${p.label} (${p.text.length.toLocaleString("en-US")} characters)`}
      </button>
    </Show>
  </div>;
}

/** Roving tabindex for any container marked data-roving-box: one Tab stop, arrows (and J/K when enabled) inside. */
export function rovingKeyDown(e: KeyboardEvent, jk = false) {
  const box = (e.target as HTMLElement).closest<HTMLElement>("[data-roving-box]");
  if (!box) return;
  const list = Array.from(box.querySelectorAll<HTMLElement>("[data-roving]"));
  const i = list.indexOf(document.activeElement as HTMLElement);
  if (i < 0) return;
  const plain = !e.ctrlKey && !e.altKey && !e.metaKey;
  let next = -1;
  if (e.key === "ArrowDown" || (jk && plain && e.key === "j")) next = Math.min(list.length - 1, i + 1);
  else if (e.key === "ArrowUp" || (jk && plain && e.key === "k")) next = Math.max(0, i - 1);
  else if (e.key === "Home") next = 0;
  else if (e.key === "End") next = list.length - 1;
  else return;
  e.preventDefault();
  for (const el of list) el.tabIndex = el === list[next] ? 0 : -1;
  list[next].focus();
}
export function rovingFocusIn(e: FocusEvent) {
  const target = (e.target as HTMLElement).closest<HTMLElement>("[data-roving]");
  const box = target?.closest<HTMLElement>("[data-roving-box]");
  if (!target || !box) return;
  for (const el of box.querySelectorAll<HTMLElement>("[data-roving]")) el.tabIndex = el === target ? 0 : -1;
}

/** Fixed-row windowed list; row height in em so it follows the text-size preset. */
export function VirtualList<T>(p: {
  items: readonly T[]; rowEm: number; heightRem: number; label: string; scale: number;
  row: (item: T, index: number) => JSX.Element;
}) {
  let el!: HTMLDivElement;
  const [top, setTop] = createSignal(0);
  const [rowPx, setRowPx] = createSignal(p.rowEm * 16);
  const [viewPx, setViewPx] = createSignal(p.heightRem * 16);
  const measure = () => {
    const size = parseFloat(getComputedStyle(el).fontSize) || 16;
    setRowPx(p.rowEm * size);
    setViewPx(el.clientHeight || p.heightRem * size);
  };
  onMount(measure);
  createEffect(on(() => p.scale, () => measure(), { defer: true }));
  const indices = createMemo(() => {
    const rp = rowPx();
    const start = Math.max(0, Math.floor(top() / rp) - 8);
    const end = Math.min(p.items.length, Math.ceil((top() + viewPx()) / rp) + 8);
    return Array.from({ length: Math.max(0, end - start) }, (_, k) => start + k);
  });
  return <div ref={el} class="c-vlist" role="region" aria-label={p.label} tabindex="0"
    style={{ height: `${p.heightRem}rem` }} onScroll={() => { setTop(el.scrollTop); measure(); }}>
    <ol class="c-vlist-inner" style={{ height: `${p.items.length * p.rowEm}em` }}>
      <For each={indices()}>{i =>
        <li class="c-vrow" style={{ top: `${i * p.rowEm}em`, height: `${p.rowEm}em` }}
          aria-setsize={p.items.length} aria-posinset={i + 1}>{p.row(p.items[i], i)}</li>
      }</For>
    </ol>
  </div>;
}

/** Modal dialog: native top layer when available, focus to the safe control, Escape cancels, focus restored. */
export function Modal(p: { label: string; onClose: () => void; initialFocus?: string; children: JSX.Element }) {
  let dialog!: HTMLDialogElement;
  const previous = document.activeElement as HTMLElement | null;
  const cancel = (e: Event) => { e.preventDefault(); p.onClose(); };
  onMount(() => {
    dialog.addEventListener("cancel", cancel);
    if (typeof dialog.showModal === "function") dialog.showModal(); else dialog.setAttribute("open", "");
    const target = p.initialFocus ? dialog.querySelector<HTMLElement>(p.initialFocus) : null;
    (target ?? dialog.querySelector<HTMLElement>("button, input, select, textarea"))?.focus();
  });
  onCleanup(() => {
    dialog.removeEventListener("cancel", cancel);
    if (dialog.open && typeof dialog.close === "function") dialog.close();
    if (previous?.isConnected) previous.focus();
  });
  return <dialog ref={dialog} class="c-dialog" aria-label={p.label}
    onKeyDown={e => { if (e.key === "Escape") { e.preventDefault(); e.stopPropagation(); p.onClose(); } }}>
    {p.children}
  </dialog>;
}
