import { createSignal, onCleanup, Show, useContext, type Accessor, type JSX } from "solid-js";
import type { AttachmentInput, CommandReceipt, WorkspaceCommand } from "@cortex/workspace-domain";
import { AttachChips, createAttachments, createDictation, MessageComposer } from "./composer";
import { useWs } from "./ctx";
import { createUnsent, ScreenActive } from "./freshness";
import { I, Keyed } from "./kit";
import { ResultText } from "./result-text";
import { clock } from "./selectors";

export type Ask = { id: string; text: string; from: string; at: number | null; unavailable?: string; options: { label: string; sub?: string; optionId?: string }[]; answer: { text: string } | null };

type Shared = { many: boolean; needs: WorkspaceCommand["type"]; blocked: (ask: Ask) => string | undefined; note: (ask: Ask) => JSX.Element;
  /** The model and effort row of the standard composer, made once per question so every answer box has its own. */
  route?: (ask: Ask) => JSX.Element; onAnswer: (ask: Ask, text: string, optionId: string | null, attachments?: AttachmentInput[]) => Promise<CommandReceipt> | undefined };

/**
 * One question: choosing an option only prepares the answer, and the plane sends it. Writing your own answer clears the
 * chosen option, so what the plane sends is always what the card shows.
 */
function Panel(props: Shared & { ask: Accessor<Ask>; active: boolean }) {
  const ws = useWs();
  const ask = () => props.ask();
  const off = () => props.blocked(ask());
  const keyOf = (o: Ask["options"][number]) => o.optionId ?? o.label;
  const [chosen, setChosen] = createSignal<string | null>(null);
  const [text, setText] = createSignal("");
  const files = createAttachments(), mic = createDictation();
  createUnsent(() => !!text().trim() || files.files().length > 0);
  const option = () => ask().options.find(o => keyOf(o) === chosen()) ?? null;
  const typed = () => { const a = ask().answer; return a && !ask().options.some(o => o.label === a.text) ? a.text : null; };
  const write = (value: string) => { setText(value); if (value.trim()) setChosen(null); };
  const sendWhy = () => files.busy() ? "Attachments are still loading" : option() || text().trim() ? undefined : "Choose or write an answer first";
  const send = (typedText = text()) => {
    const o = option();
    const sent = o ? props.onAnswer(ask(), o.label, o.optionId ?? null) : typedText.trim() && !files.busy() ? props.onAnswer(ask(), typedText, null, files.files()) : undefined;
    void sent?.then(r => { if (r.ok) { setChosen(null); setText(""); files.clear(); } });
  };
  const held = () => ask().answer ? `Ready: ${ask().answer!.text}. It goes with the other answers.` : null;
  return <div class="dc-panel" id={`decision-${ask().id}`} role={props.many ? "tabpanel" : undefined} aria-hidden={props.active ? undefined : "true"} inert={props.active ? undefined : true}>
    <div class="dc-q"><ResultText breaks text={ask().text} /></div>
    <Show when={ask().unavailable}><div class="cap st-warn ws-dc-note">{ask().unavailable}</div></Show>
    {props.note(ask())}
    <div class="dc-opts" role="radiogroup" aria-label="Answers">
      <Keyed each={ask().options} by={keyOf}>{(o, i) => <button type="button" class="opt" role="radio" aria-checked={chosen() === keyOf(o()) || (!chosen() && !text().trim() && ask().answer?.text === o().label) ? "true" : "false"} aria-disabled={off() ? "true" : undefined} data-why={off()} {...ws.needs(props.needs)}
        onClick={() => { if (!off() && !ws.gate(props.needs)) setChosen(chosen() === keyOf(o()) ? null : keyOf(o())); }}><span class="n">{i() + 1}</span><span class="t">{o().label}</span><Show when={o().sub}><span class="sub">{o().sub}</span></Show></button>}</Keyed>
    </div>
    <Show when={typed() && !text().trim()}>{t => <div class="cap muted ws-dc-note"><ResultText breaks text={`Your answer: ${t()}`} /></div>}</Show>
    <Show when={props.many && held()}>{h => <div class="cap muted ws-dc-note">{h()}</div>}</Show>
    <MessageComposer class="dc-answer" label="Your answer" placeholder={ask().options.length ? "Or write your own answer…" : "Write your answer…"} text={text()} onText={write} files={files} mic={mic}
      route={props.route?.(ask())}
      effect={option() ? <>Answer: <strong>{option()!.label}</strong></> : undefined} off={off()}
      send={{ label: "Send answer", needs: props.needs, why: sendWhy(), go: () => send(), insertAndSend: value => send(value) }} />
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
  const shown = useContext(ScreenActive);
  const onKey = (e: KeyboardEvent) => {
    const el = document.activeElement, typing = el instanceof HTMLTextAreaElement || el instanceof HTMLInputElement;
    if (!shown() || typing || e.ctrlKey || e.metaKey || e.altKey || document.querySelector("dialog[open]") || !/^[1-8]$/.test(e.key)) return;
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
      <Keyed each={props.asks} by={a => a.id}>{a => <Panel ask={a} active={active().id === a().id} many={many()} needs={props.needs} blocked={props.blocked} note={props.note} route={props.route} onAnswer={answer} />}</Keyed>
    </div>
    <Show when={many()}><div class="cap muted ws-dc-note">The answers go together, after the last one.</div></Show>
  </div>;
}

/** A question that was settled: what was asked, then what the owner said. */
export const Answered = (props: { question: string; answer: string; at: number | null | undefined; icon?: string; files?: AttachmentInput[] }) => <div class="qa">
  <I n={props.icon ?? "question"} s={14} />
  <div class="qa-b"><div class="qa-q"><ResultText breaks text={props.question || "You answered"} /></div><div class="qa-a"><ResultText breaks text={props.answer} /><AttachChips files={props.files} open /></div></div>
  <Show when={props.at != null}><span class="qa-t">{clock(props.at!)}</span></Show>
</div>;
