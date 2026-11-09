import { createSignal, Match, onCleanup, onMount, Show, Switch, type JSX } from "solid-js";
import { receiptId, useDesk, type ConfirmOpts } from "./ctx";
import { Glyph, type GlyphName } from "./glyphs";
import { CHECK_STYLE, EXEC_STYLE, WAIT_GLYPH, stamp, type Check, type TaskView, type Tone } from "./model";

let uid = 0;
const FOCUSABLE = 'button:not([disabled]), [href], input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';

export function Badge(props: { tone: Tone; glyph?: GlyphName; children: JSX.Element }) {
  return <span class={`a-badge a-tone-${props.tone}`}>
    <Show when={props.glyph && props.glyph !== "none"}><Glyph name={props.glyph!} /></Show>
    <span>{props.children}</span>
  </span>;
}

export const ExecBadge = (props: { v: TaskView }) =>
  <Badge tone={EXEC_STYLE[props.v.exec].tone} glyph={EXEC_STYLE[props.v.exec].glyph}>
    {props.v.exec}{props.v.created && props.v.exec === "Not started" ? " (mock)" : ""}
  </Badge>;

export const WaitBadge = (props: { v: TaskView }) => <Switch>
  <Match when={props.v.wait}>{w => <Badge tone="warn" glyph={WAIT_GLYPH[w()]}>{w()}</Badge>}</Match>
  <Match when={props.v.answered}><Badge tone="neutral" glyph="check">Answered (mock)</Badge></Match>
  <Match when={props.v.stale}><Badge tone="warn" glyph="history">Result stale</Badge></Match>
</Switch>;

export const CheckBadge = (props: { outcome: Check["outcome"] }) =>
  <Badge tone={CHECK_STYLE[props.outcome].tone} glyph={CHECK_STYLE[props.outcome].glyph}>{CHECK_STYLE[props.outcome].label}</Badge>;

export const MockTag = () => <span class="a-mock-tag" aria-hidden="true"> · Mock</span>;

/** Every mutating control: visible "Mock" tag, "(mock)" in the accessible name, visible disabled reason. */
export function MockButton(props: {
  label: string; onClick?: () => void; reason?: string | null; tone?: "primary" | "danger"; id?: string; submit?: boolean;
}) {
  const rid = `a-reason-${++uid}`;
  return <span class="a-mock-action">
    <button type={props.submit ? "submit" : "button"} id={props.id} class={props.tone ? `a-btn-${props.tone}` : undefined}
      disabled={!!props.reason} aria-describedby={props.reason ? rid : undefined}
      aria-label={`${props.label} (mock)`} onClick={() => props.onClick?.()}>
      {props.label}<MockTag />
    </button>
    <Show when={props.reason}><span id={rid} class="a-reason">{props.reason}</span></Show>
  </span>;
}

export function Receipt(props: { k: string }) {
  const desk = useDesk();
  return <Show when={desk.receipt(props.k)}>{msg =>
    <p class="a-receipt" id={receiptId(props.k)} tabindex="-1" role="status"><Glyph name="flask" /><span>{msg()}</span></p>
  }</Show>;
}

export function Dialog(props: { title: string; onClose: () => void; children: JSX.Element; class?: string; initial?: string }) {
  let box!: HTMLDivElement;
  const invoker = document.activeElement as HTMLElement | null;
  const titleId = `a-dlg-${++uid}`;
  onMount(() => {
    const target = (props.initial ? box.querySelector<HTMLElement>(props.initial) : null) ?? box.querySelector<HTMLElement>(FOCUSABLE);
    (target ?? box).focus();
  });
  onCleanup(() => { if (invoker?.isConnected) invoker.focus(); });
  const onKey = (e: KeyboardEvent) => {
    if (e.key === "Escape") { e.preventDefault(); e.stopPropagation(); props.onClose(); return; }
    if (e.key !== "Tab") return;
    const items = [...box.querySelectorAll<HTMLElement>(FOCUSABLE)];
    if (!items.length) return;
    const first = items[0], last = items[items.length - 1];
    if (e.shiftKey && document.activeElement === first) { e.preventDefault(); last.focus(); }
    else if (!e.shiftKey && document.activeElement === last) { e.preventDefault(); first.focus(); }
  };
  return <div class="a-overlay" onMouseDown={e => { if (e.target === e.currentTarget) props.onClose(); }}>
    <div class={`a-dialog ${props.class ?? ""}`} role="dialog" aria-modal="true" aria-labelledby={titleId} ref={box} tabindex="-1" onKeyDown={onKey}>
      <h2 id={titleId} class="a-dlg-title">{props.title}</h2>
      {props.children}
    </div>
  </div>;
}

export function ConfirmDialog(props: { opts: ConfirmOpts; onClose: () => void }) {
  return <Dialog title={props.opts.title} onClose={props.onClose} initial=".a-safe">
    <dl class="a-facts">
      <dt>Effect</dt><dd>{props.opts.effect}</dd>
      <dt>Scope</dt><dd>{props.opts.scope}</dd>
    </dl>
    <div class="a-actions">
      <button type="button" class="a-safe" onClick={() => props.onClose()}>Cancel</button>
      <MockButton label={props.opts.confirmLabel} tone="danger" onClick={() => { props.onClose(); props.opts.onConfirm(); }} />
    </div>
  </Dialog>;
}

/** Connection banner shown at the top of each primary area. */
export function ConnBanner(props: { area: string }) {
  const desk = useDesk();
  const machine = () => desk.store.state().fixture.machines[0];
  return <Switch>
    <Match when={desk.conn() === "disconnected"}>
      <div class="a-banner a-banner-danger" role="group" aria-label="Connection state">
        <Glyph name="plugoff" />
        <p><strong>Machine disconnected.</strong> Last confirmed {stamp(machine().lastConfirmedAt)}. Current states are unknown. Actions are unavailable.</p>
        <span class="a-actions">
          <button type="button" aria-label="Reconnect (mock)" onClick={() => desk.setReceipt(`reconnect-${props.area}`,
            "Mock outcome: no reconnect was attempted and nothing was sent. The scenario stays disconnected; choose another scenario in the prototype chrome.", true)}>Reconnect<MockTag /></button>
          <button type="button" onClick={() => desk.setPane({ kind: "connection" })}>Connection settings</button>
        </span>
        <Receipt k={`reconnect-${props.area}`} />
      </div>
    </Match>
    <Match when={desk.conn() === "offline"}>
      <div class="a-banner a-banner-neutral" role="group" aria-label="Connection state">
        <Glyph name="clock" />
        <p><strong>As of {stamp(machine().lastConfirmedAt)} (cached).</strong> Showing the last confirmed state. Read-only: actions are unavailable.</p>
      </div>
    </Match>
  </Switch>;
}

export function LongText(props: { text: string; id: string }) {
  const [open, setOpen] = createSignal(false);
  const lines = () => Math.ceil(props.text.length / 72);
  const long = () => lines() > 12;
  return <div class="a-longtext">
    <p class="a-prose" classList={{ "a-clamp": long() && !open() }} id={props.id}>{props.text}</p>
    <Show when={long()}>
      <button type="button" aria-expanded={open()} aria-controls={props.id} onClick={() => setOpen(!open())}>
        {open() ? "Show less" : `Show full message (about ${lines()} lines)`}
      </button>
    </Show>
  </div>;
}
