import { createSignal, onCleanup, Show, untrack, useContext, type Accessor, type JSX } from "solid-js";
import type { AttachmentInput, CommandReceipt, WorkspaceCommand } from "@cortex/workspace-domain";
import { AttachChips, createAttachments, createDictation, MessageComposer } from "./composer";
import { useWs } from "./ctx";
import { createUnsent, ScreenActive } from "./freshness";
import { I, Keyed } from "./kit";
import { Action } from "./layout";
import { ResultText } from "./result-text";
import { clock } from "./selectors";

/** `optionsOnly`: its options act by themselves (the runtime's stop question, a skill proposal), so an option takes no added words. */
export type Ask = { id: string; text: string; from: string; at: number | null; unavailable?: string; topic?: string; optionsOnly?: true; options: { label: string; sub?: string; optionId?: string; recommended?: true }[]; answer: { text: string } | null };
/** The owner's answer to one question, as the form will send it. */
export type AskAnswer = { ask: Ask; text: string; optionId: string | null; attachments?: AttachmentInput[] };

type Shared = { needs: WorkspaceCommand["type"]; blocked: (ask: Ask) => string | undefined; note: (ask: Ask) => JSX.Element; /** Whose files an answer may tag with `@`. An answer has no `/` commands or skills. */ projectId?: string };
/** What a question's field gives the form: its answer once one is chosen or written, and whether files are still being read. */
type Field = { answer: () => AskAnswer | null; busy: () => boolean; clear: () => void };

const keyOf = (o: Ask["options"][number]) => o.optionId ?? o.label;

/**
 * One question of the form: choosing an option only prepares its answer. The owner may choose an option, add their own
 * words and attach files; all of it is the one answer: the option's label, then a blank line and the words, with the files.
 * A question whose options act by themselves keeps them apart: writing clears the option and choosing one clears the words,
 * so what the form sends is always what the card shows. A single question keeps its own paper plane; questions asked
 * together are sent by the form's one button.
 */
function Question(props: Shared & { ask: Accessor<Ask>; n: number; of: number; hidden?: boolean; register: (id: string, field: Field | null) => void; plane?: { why: string | undefined; go: () => void } }) {
  const ws = useWs();
  const ask = () => props.ask();
  const off = () => props.blocked(ask());
  // An answer the app already holds for this question (a batch answered before this form) is where the form starts.
  const start = untrack(ask).answer?.text ?? null, startOption = untrack(ask).options.find(o => start === o.label || !!start?.startsWith(`${o.label}\n\n`));
  const [chosen, setChosen] = createSignal<string | null>(startOption ? keyOf(startOption) : null);
  const [text, setText] = createSignal(start === null ? "" : startOption ? start.slice(startOption.label.length).trim() : start);
  const files = createAttachments(), mic = createDictation();
  createUnsent(() => !!text().trim() || files.files().length > 0);
  const option = () => ask().options.find(o => keyOf(o) === chosen()) ?? null;
  const write = (value: string) => { setText(value); if (value.trim() && ask().optionsOnly) setChosen(null); };
  const choose = (key: string | null) => { setChosen(key); if (key !== null && ask().optionsOnly) setText(""); };
  const answer = (): AskAnswer | null => {
    const o = option(), words = text().trim();
    if (o) return { ask: ask(), text: words ? `${o.label}\n\n${words}` : o.label, optionId: o.optionId ?? null, attachments: files.files() };
    return words ? { ask: ask(), text: text(), optionId: null, attachments: files.files() } : null;
  };
  const id = untrack(ask).id;
  props.register(id, { answer, busy: files.busy, clear: () => { setChosen(null); setText(""); files.clear(); } });
  onCleanup(() => props.register(id, null));
  // A question with options always keeps its answer line, so choosing one fills the line instead of adding it and the page
  // does not move under the pointer.
  const answerLine = () => option() ? <>Answer: <strong>{option()!.label}</strong></> : ask().options.length ? <>Answer: <span class="faint">not chosen yet</span></> : undefined;
  const many = () => props.of > 1;
  // Built once for this question: a refreshed record of it must not rebuild the note's links and lose their focus. The note
  // reads the question's current facts by its id.
  const note = untrack(() => props.note(ask()));
  return <div class="dc-panel" hidden={props.hidden} id={`decision-${ask().id}`} role={many() ? "group" : undefined} aria-label={many() ? `Question ${props.n} of ${props.of}` : undefined}>
    {/* A batch member may carry a short topic label naming its subject. */}
    <Show when={many()}><div class="dc-n cap muted">Question {props.n} of {props.of}<Show when={ask().topic}> · <span class="ws-dc-topic">{ask().topic}</span></Show></div></Show>
    {/* A worker's question is answered here, in the task's Conversation, and says which worker asked it. */}
    <Show when={ask().from}><div class="dc-n cap muted ws-dc-from">Asked by {ask().from}</div></Show>
    <div class="dc-q"><ResultText breaks text={ask().text} /></div>
    <Show when={ask().unavailable}><div class="cap st-warn ws-dc-note">{ask().unavailable}</div></Show>
    {note}
    {/* Answers that say what they do (the runtime's stop question) are full-width rows, the label over its consequence. */}
    <div class={ask().options.some(o => o.sub || o.recommended) ? "dc-opts dc-opts-rows" : "dc-opts"} role="radiogroup" aria-label={many() ? `Answers to question ${props.n}` : "Answers"}>
      <Keyed each={ask().options} by={keyOf}>{(o, i) => <button type="button" class="opt" role="radio" aria-checked={chosen() === keyOf(o()) ? "true" : "false"} aria-disabled={off() ? "true" : undefined} data-why={off()} {...ws.needs(props.needs)}
        onClick={() => { if (!off() && !ws.gate(props.needs)) choose(chosen() === keyOf(o()) ? null : keyOf(o())); }}><span class="n">{i() + 1}</span><span class="t">{o().label}<Show when={o().recommended}> <span class="cap st-ok ws-dc-rec">Recommended</span></Show></span><Show when={o().sub}><span class="sub">{o().sub}</span></Show></button>}</Keyed>
    </div>
    {/* The answer box writes the answer only: the task's model and effort were chosen for its session and an answer cannot change them. */}
    <MessageComposer class="dc-answer" label={many() ? `Your answer to question ${props.n}` : "Your answer"} placeholder={option() && !ask().optionsOnly ? (props.projectId ? "Add to your answer, tag @files…" : "Add to your answer…") : ask().options.length ? (props.projectId ? "Or write your own answer, tag @files…" : "Or write your own answer…") : props.projectId ? "Write your answer, tag @files…" : "Write your answer…"} text={text()} onText={write} files={files} mic={mic}
      suggest={props.projectId ? { projectId: props.projectId } : undefined}
      effect={answerLine()} off={off()}
      send={props.plane ? { label: "Send answer", needs: props.needs, why: props.plane.why, go: props.plane.go } : undefined} />
  </div>;
}

/**
 * The open questions of one run, as one form. Every question asked together is on view at once with its own answer, and
 * one button sends them all as one answer. Once sent, the form reads as answered until the machine's record catches up;
 * a refused send keeps every answer as it was, with the reason.
 */
export function QuestionForm(props: Shared & { asks: Ask[]; waiting: (ask: Ask) => string; onSend: (answers: AskAnswer[]) => Promise<CommandReceipt> | undefined }) {
  const ws = useWs();
  const [fields, setFields] = createSignal<Record<string, Field>>({});
  const register = (id: string, field: Field | null) => setFields(all => { const next = { ...all }; if (field) next[id] = field; else delete next[id]; return next; });
  /** The answers the machine recorded from this form, by question. */
  const [sent, setSent] = createSignal<Record<string, { text: string; files?: AttachmentInput[] }>>({});
  /** The answers on their way, shown as answered from the press. Their fields stay behind them, so a refusal loses nothing. */
  const [inFlight, setInFlight] = createSignal<Record<string, { text: string; files?: AttachmentInput[] }>>({});
  const [sending, setSending] = createSignal(false);
  const [failure, setFailure] = createSignal<string | null>(null);
  const open = () => props.asks.filter(a => sent()[a.id] === undefined && inFlight()[a.id] === undefined);
  const many = () => props.asks.length > 1;
  const answers = () => open().map(a => fields()[a.id]?.answer() ?? null);
  const ready = () => answers().filter(Boolean).length;
  const why = () => {
    const blocked = open().map(props.blocked).find(Boolean);
    if (blocked) return blocked;
    if (sending()) return "Sending the answers";
    if (open().some(a => fields()[a.id]?.busy())) return "Attachments are still loading";
    if (ready() < open().length) return many() ? `Answer every question first: ${ready()} of ${open().length} answered` : "Choose or write an answer first";
    return undefined;
  };
  const send = () => {
    if (why() || ws.gate(props.needs)) return;
    const list = answers() as AskAnswer[];
    const work = props.onSend(list);
    if (!work) return;
    const given = Object.fromEntries(list.map(a => [a.ask.id, { text: a.text, files: a.attachments }]));
    setFailure(null); setSending(true); setInFlight(given);
    void work.then(r => {
      if (r.ok) { setSent(prior => ({ ...prior, ...given })); for (const a of list) fields()[a.ask.id]?.clear(); }
      else setFailure(r.message || "The answers were not sent.");
    }, () => setFailure("The answers were not sent.")).finally(() => { setInFlight({}); setSending(false); });
  };
  const first = () => open()[0] ?? props.asks[0];
  const shown = useContext(ScreenActive);
  // Number keys choose an option of a single question; with several questions a number would not say which one it answers.
  const onKey = (e: KeyboardEvent) => {
    const el = document.activeElement, typing = el instanceof HTMLTextAreaElement || el instanceof HTMLInputElement;
    if (many() || !shown() || typing || e.ctrlKey || e.metaKey || e.altKey || document.querySelector("dialog[open]") || !/^[1-8]$/.test(e.key)) return;
    document.querySelector<HTMLElement>(`[id="decision-${first().id}"] .opt:nth-child(${e.key})`)?.click();
  };
  document.addEventListener("keydown", onKey);
  onCleanup(() => document.removeEventListener("keydown", onKey));
  return <div class={open().length === 0 && !sending() ? "dc dc-done" : "dc"} role={many() ? "group" : undefined} aria-label={many() ? `${props.asks.length} questions` : undefined}>
    <div class="dc-h"><I n="question" s={14} />{many() ? `${props.asks.length} questions` : "Question"}<span class="r">{open().length ? props.waiting(first()) : ""}</span></div>
    <Keyed each={props.asks} by={a => a.id}>{(a, i) => <>
      <Show when={sent()[a().id] ?? inFlight()[a().id]}>{given => <Answered question={a().text} answer={given().text} files={given().files} at={null} />}</Show>
      <Show when={sent()[a().id] === undefined}>
        <Question ask={a} n={i() + 1} of={props.asks.length} hidden={inFlight()[a().id] !== undefined} needs={props.needs} blocked={props.blocked} note={props.note} projectId={props.projectId} register={register}
          plane={many() ? undefined : { get why() { return why(); }, go: send }} />
      </Show>
    </>}</Keyed>
    <Show when={failure()}>{f => <div class="cap st-warn ws-dc-note" role="alert">{f()} Your answers are kept; send them again when ready. <button type="button" class="lnk" onClick={send}>Retry</button></div>}</Show>
    <Show when={sending()}><div class="cap muted ws-dc-note" role="status"><I n="loader" s={12} class="spin ws-wait" />{many() ? "Sending the answers…" : "Sending the answer…"}</div></Show>
    <Show when={open().length === 0 && !sending()}><div class="cap muted ws-dc-note" role="status">{many() ? "Answers sent; work continues with them." : "Answer sent; work continues with it."}</div></Show>
    <Show when={many() && open().length > 0}>
      <div class="dc-send">
        <span class="cap muted">{ready()} of {open().length} answered</span>
        <Action kind="primary" why={ws.gate(props.needs) ?? why()} onClick={send}><I n="send" />Send answers</Action>
      </div>
    </Show>
  </div>;
}

/** A question that was settled: what was asked, then what the owner said. */
export const Answered = (props: { question: string; answer: string; at: number | null | undefined; icon?: string; files?: AttachmentInput[] }) => <div class="qa">
  <I n={props.icon ?? "question"} s={14} />
  <div class="qa-b"><div class="qa-q"><ResultText breaks text={props.question || "You answered"} /></div><div class="qa-a"><ResultText breaks text={props.answer} /><AttachChips files={props.files} open /></div></div>
  <Show when={props.at != null}><span class="qa-t">{clock(props.at!)}</span></Show>
</div>;
