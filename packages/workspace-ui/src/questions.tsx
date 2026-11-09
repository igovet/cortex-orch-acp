import { createSignal, onCleanup, Show, type Accessor, type JSX } from "solid-js";
import type { AttachmentInput, CommandReceipt, WorkspaceCommand } from "@cortex/workspace-domain";
import { OtherAnswer } from "./composer";
import { useWs } from "./ctx";
import { I, Keyed } from "./kit";
import { ResultText } from "./result-text";
import { clock } from "./selectors";

export type Ask = { id: string; text: string; from: string; at: number | null; unavailable?: string; options: { label: string; sub?: string; optionId?: string }[]; answer: { text: string } | null };

type Shared = { many: boolean; needs: WorkspaceCommand["type"]; blocked: (ask: Ask) => string | undefined; note: (ask: Ask) => JSX.Element; onAnswer: (ask: Ask, text: string, optionId: string | null, attachments?: AttachmentInput[]) => Promise<CommandReceipt> | undefined };

function Panel(props: Shared & { ask: Accessor<Ask>; active: boolean }) {
  const ws = useWs();
  const [other, setOther] = createSignal(false);
  const ask = () => props.ask();
  const off = () => props.blocked(ask());
  const typed = () => { const a = ask().answer; return a && !ask().options.some(o => o.label === a.text) ? a.text : null; };
  return <div class="dc-panel" id={`decision-${ask().id}`} role={props.many ? "tabpanel" : undefined} aria-hidden={props.active ? undefined : "true"}>
    <div class="dc-q"><ResultText breaks text={ask().text} /></div>
    <Show when={ask().unavailable}><div class="cap st-warn ws-dc-note">{ask().unavailable}</div></Show>
    {props.note(ask())}
    <div class="dc-opts">
      <Keyed each={ask().options} by={o => o.optionId ?? o.label}>{(o, i) => <button class="opt" aria-pressed={ask().answer?.text === o().label ? "true" : undefined} aria-disabled={off() ? "true" : undefined} data-why={off()} {...ws.needs(props.needs)} onClick={() => { if (!off()) props.onAnswer(ask(), o().label, o().optionId ?? null); }}><span class="n">{i() + 1}</span><span class="t">{o().label}</span><Show when={o().sub}><span class="sub">{o().sub}</span></Show></button>}</Keyed>
      <button class="opt" aria-pressed={typed() ? "true" : undefined} aria-disabled={off() ? "true" : undefined} data-why={off()} onClick={() => { if (!off()) setOther(true); }}><span class="n">…</span><span class="t">Other answer</span></button>
    </div>
    <Show when={typed()}>{t => <div class="cap muted ws-dc-note"><ResultText breaks text={`Your answer: ${t()}`} /></div>}</Show>
    <Show when={other()}><OtherAnswer onSend={(text, attachments) => { void props.onAnswer(ask(), text, null, attachments)?.then(r => { if (r.ok) setOther(false); }); }} onCancel={() => setOther(false)} /></Show>
  </div>;
}

/**
 * The open questions of one run, as one card. Several questions are tabs, so the owner sees how many there are and can read
 * each one. Every panel shares one grid cell, so the card keeps the height of its tallest question and nothing moves.
 */
export function QuestionTabs(props: Omit<Shared, "many"> & { asks: Ask[]; waiting: (ask: Ask) => string }) {
  const [picked, setPicked] = createSignal<string | null>(null);
  const first = () => props.asks.find(a => !a.answer) ?? props.asks[0];
  const active = () => props.asks.find(a => a.id === picked()) ?? first();
  const many = () => props.asks.length > 1;
  const answered = () => props.asks.filter(a => a.answer).length;
  const answer: Shared["onAnswer"] = (ask, text, optionId, attachments) => { const sent = props.onAnswer(ask, text, optionId, attachments); void sent?.then(r => { if (r.ok) setPicked(null); }); return sent; };
  const onKey = (e: KeyboardEvent) => {
    const el = document.activeElement, typing = el instanceof HTMLTextAreaElement || el instanceof HTMLInputElement;
    if (typing || e.ctrlKey || e.metaKey || e.altKey || document.querySelector("dialog[open]") || !/^[1-8]$/.test(e.key)) return;
    document.querySelector<HTMLElement>(`[id="decision-${active().id}"] .opt:nth-child(${e.key})`)?.click();
  };
  document.addEventListener("keydown", onKey);
  onCleanup(() => document.removeEventListener("keydown", onKey));
  const step = (e: KeyboardEvent) => {
    const by = e.key === "ArrowRight" ? 1 : e.key === "ArrowLeft" ? -1 : 0;
    if (!by) return;
    e.preventDefault();
    const list = props.asks, next = list[(list.indexOf(active()) + by + list.length) % list.length];
    setPicked(next.id);
    (e.currentTarget as HTMLElement).querySelector<HTMLElement>(`[data-ask="${next.id}"]`)?.focus();
  };
  return <div class="dc">
    <div class="dc-h"><I n="question" s={14} />{many() ? `${props.asks.length} questions` : "Question"}<span class="r">{props.waiting(active())}</span></div>
    <Show when={many()}><div class="dc-tabs" role="tablist" aria-label="Questions" onKeyDown={step}>
      <Keyed each={props.asks} by={a => a.id}>{(a, i) => <button class="dc-tab" role="tab" data-ask={a().id} aria-selected={active().id === a().id ? "true" : "false"} aria-label={`Question ${i() + 1} of ${props.asks.length}${a().answer ? ", answered" : ""}`} tabindex={active().id === a().id ? 0 : -1} onClick={() => setPicked(a().id)}>
        <span>{i() + 1}</span><I n="check" s={12} class={a().answer ? undefined : "ws-off"} />
      </button>}</Keyed>
      <span class="r">{answered()} of {props.asks.length} answered</span>
    </div></Show>
    <div class="dc-panels">
      <Keyed each={props.asks} by={a => a.id}>{a => <Panel ask={a} active={active().id === a().id} many={many()} needs={props.needs} blocked={props.blocked} note={props.note} onAnswer={answer} />}</Keyed>
    </div>
    <Show when={many()}><div class="cap muted ws-dc-note">The answers go together, after the last one.</div></Show>
  </div>;
}

/** A question that was settled: what was asked, then what the owner said. */
export const Answered = (props: { question: string; answer: string; at: number | null | undefined; icon?: string }) => <div class="qa">
  <I n={props.icon ?? "question"} s={14} />
  <div class="qa-b"><div class="qa-q"><ResultText breaks text={props.question || "You answered"} /></div><div class="qa-a"><ResultText breaks text={props.answer} /></div></div>
  <Show when={props.at != null}><span class="qa-t">{clock(props.at!)}</span></Show>
</div>;
