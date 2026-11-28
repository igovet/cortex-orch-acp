import { createSignal, createUniqueId, onCleanup, Show, untrack, useContext, type Accessor, type JSX } from "solid-js";
import type { AttachmentInput, CommandReceipt, Decision, WorkspaceCommand } from "@cortex/workspace-domain";
import { AttachChips, createAttachments, createDictation, MessageComposer } from "./composer";
import { questionCopy, workspaceCopy } from "./copy";
import { useWs } from "./ctx";
import { createUnsent, ScreenActive } from "./freshness";
import { I, Keyed } from "./kit";
import { Action } from "./layout";
import { createLocalInput, questionDraftScope } from "./local-drafts";
import { createInlineResultText, InlineResultText, ResultText } from "./result-text";
import { clock } from "./selectors";

/** `optionsOnly`: its options act by themselves (the runtime's stop question, a skill proposal), so an option takes no added words. */
export type Ask = { id: string; draftBinding?: Decision["draftBinding"]; text: string; from: string; language?: "ru" | "en"; at: number | null; unavailable?: string; topic?: string; optionsOnly?: true; options: { label: string; sub?: string; optionId?: string; recommended?: true }[]; answer: { text: string } | null };
/** The owner's answer to one question, as the form will send it. */
export type AskAnswer = { ask: Ask; text: string; optionId: string | null; attachments?: AttachmentInput[] };

type Shared = { needs: WorkspaceCommand["type"]; blocked: (ask: Ask) => string | undefined; note: (ask: Ask) => JSX.Element; /** Whose files an answer may tag with `@`. An answer has no `/` commands or skills. */ projectId?: string; draftScope?: readonly (string | null)[] };
/** What a question's field gives the form: its answer once one is chosen or written, and whether files are still being read. */
type Field = { answer: () => AskAnswer | null; busy: () => boolean; clearOnConfirm: () => () => void };

const keyOf = (o: Ask["options"][number]) => o.optionId ?? o.label;
const answerKey = (ask: Ask) => JSON.stringify([ask.id, ask.draftBinding?.executionId, ask.draftBinding?.ownerEpoch,
  ask.draftBinding?.sessionId, ask.draftBinding?.requirementRevision]);

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
  const copy = () => questionCopy(ask().language);
  const off = () => props.blocked(ask());
  // An answer the app already holds for this question (a batch answered before this form) is where the form starts.
  const start = untrack(ask).answer?.text ?? null, startOption = untrack(ask).options.find(o => start === o.label || !!start?.startsWith(`${o.label}\n\n`));
  const [chosen, setChosen] = createSignal<string | null>(startOption ? keyOf(startOption) : null);
  const scope = () => props.draftScope && questionDraftScope(props.draftScope, ask());
  const local = createLocalInput(ws.localDrafts, scope);
  const [ephemeral, setEphemeral] = createSignal(start === null ? "" : startOption ? start.slice(startOption.label.length).trim() : start);
  const text = () => local.hasDraft() ? local.text() : ephemeral();
  const setText = (value: string) => {
    if (local.enabled()) { local.set(value); setEphemeral(""); }
    else setEphemeral(value);
  };
  const files = createAttachments(scope), mic = createDictation();
  createUnsent(() => !!text().trim() || files.files().length > 0);
  const option = () => ask().options.find(o => keyOf(o) === chosen()) ?? null;
  const write = (value: string) => { setText(value); if (value.trim() && ask().optionsOnly) setChosen(null); };
  const choose = (key: string | null) => { setChosen(key); if (key !== null && ask().optionsOnly) setText(""); };
  const answer = (): AskAnswer | null => {
    const o = option(), words = text().trim();
    if (o) return { ask: ask(), text: words ? `${o.label}\n\n${words}` : o.label, optionId: o.optionId ?? null, attachments: files.files() };
    return words ? { ask: ask(), text: text(), optionId: null, attachments: files.files() } : null;
  };
  const id = answerKey(untrack(ask));
  props.register(id, { answer, busy: files.busy, clearOnConfirm: () => {
    const draft = local.snapshot(), clearSubmitted = files.clearSubmitted();
    return () => { draft?.confirm(); clearSubmitted(); };
  } });
  onCleanup(() => props.register(id, null));
  // A question with options always keeps its answer line, so choosing one fills the line instead of adding it and the page
  // does not move under the pointer.
  const answerLine = () => option() ? <>{copy().answer} <span class="w500"><InlineResultText text={option()!.label} /></span></> : ask().options.length ? <>{copy().answer} <span class="faint">{copy().notChosen}</span></> : undefined;
  const many = () => props.of > 1;
  // Built once for this question: a refreshed record of it must not rebuild the note's links and lose their focus. The note
  // reads the question's current facts by its id.
  const note = untrack(() => props.note(ask()));
  return <div class="dc-panel" hidden={props.hidden} id={`decision-${ask().id}`} role={many() ? "group" : undefined} lang={ask().language ?? "en"} aria-label={many() ? copy().position(props.n, props.of) : undefined}>
    {/* A batch member may carry a short topic label naming its subject. */}
    <Show when={many()}><div class="dc-n cap muted">{copy().position(props.n, props.of)}<Show when={ask().topic}> · <span class="ws-dc-topic">{ask().topic}</span></Show></div></Show>
    {/* Only the fixed question-source tokens are translated; worker roles stay as received. */}
    <Show when={ask().from} keyed>{from =>
      <div class="dc-n cap muted ws-dc-from">{from === "runtime" ? copy().askedAutomatically : copy().askedBy(from === "coordinator" ? copy().coordinator : from)}</div>
    }</Show>
    <div class="dc-q"><ResultText breaks text={ask().text} /></div>
    <Show when={ask().unavailable}><div class="cap st-warn ws-dc-note">{ask().unavailable}</div></Show>
    {note}
    {/* Answers that say what they do (the runtime's stop question) are full-width rows, the label over its consequence. */}
    <div class={ask().options.some(o => o.sub || o.recommended) ? "dc-opts dc-opts-rows" : "dc-opts"} role="radiogroup" aria-label={copy().choices(many() ? props.n : undefined)}>
      <Keyed each={ask().options} by={keyOf}>{(o, i) => {
        const id = createUniqueId();
        const checked = () => chosen() === keyOf(o());
        const label = createInlineResultText(() => o().label);
        const detail = createInlineResultText(() => o().sub ?? "");
        const select = () => { if (!off() && !ws.gate(props.needs)) choose(checked() ? null : keyOf(o())); };
        const contents = () => <>
          <span class="n" aria-hidden="true">{i() + 1}</span>
          <span class="t" id={id + "-label"}>{label().nodes}<Show when={o().recommended}> <span class="cap st-ok ws-dc-rec">{workspaceCopy("questionRecommended", ask().language ?? "en")}</span></Show></span>
          <Show when={o().sub}><span class="sub" id={id + "-detail"}>{detail().nodes}</span></Show>
        </>;
        // Only options with links need separate hit targets. The link is never a descendant of the radio.
        return <Show when={label().hasLinks || detail().hasLinks} fallback={
          <button type="button" class="opt" role="radio" aria-checked={checked()} aria-disabled={off() ? "true" : undefined} data-why={off()} {...ws.needs(props.needs)} onClick={select}>{contents()}</button>
        }>
          <label class="opt ws-ask-option" classList={{ "ws-ask-selected": checked(), "ws-ask-disabled": !!off() }} for={id}>
            <button type="button" class="ws-ask-radio" id={id} role="radio" aria-labelledby={id + "-label"} aria-describedby={o().sub ? id + "-detail" : undefined}
              aria-checked={checked()} aria-disabled={off() ? "true" : undefined} data-why={off()} {...ws.needs(props.needs)} onClick={select} />
            {contents()}
          </label>
        </Show>;
      }}</Keyed>
    </div>
    {/* The answer box writes the answer only: the task's model and effort were chosen for its session and an answer cannot change them. */}
    <MessageComposer class="dc-answer" label={copy().answerLabel(many() ? props.n : undefined)} placeholder={option() && !ask().optionsOnly ? copy().add(!!props.projectId) : ask().options.length ? copy().own(!!props.projectId) : copy().write(!!props.projectId)} text={text()} onText={write} files={files} mic={mic}
      suggest={props.projectId ? { projectId: props.projectId } : undefined}
      effect={answerLine()} off={off()}
      draft={{ shown: local.hasDraft(), warning: local.warning() ? workspaceCopy("textDraftStorageUnavailable", ask().language ?? "en") : undefined,
        discard: () => setText(""), label: workspaceCopy("discardAnswerDraft", ask().language ?? "en") }}
      send={props.plane ? { label: copy().send, needs: props.needs, why: props.plane.why, go: props.plane.go } : undefined} />
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
  const open = () => props.asks.filter(a => sent()[answerKey(a)] === undefined && inFlight()[answerKey(a)] === undefined);
  const many = () => props.asks.length > 1;
  const language = () => props.asks[0]?.language ?? "en";
  const copy = () => questionCopy(language());
  const heading = () => many() ? copy().count(props.asks.length) : copy().question;
  const answers = () => open().map(a => fields()[answerKey(a)]?.answer() ?? null);
  const ready = () => answers().filter(Boolean).length;
  const why = () => {
    const blocked = open().map(props.blocked).find(Boolean);
    if (blocked) return blocked;
    if (sending()) return copy().sending(many());
    if (open().some(a => fields()[answerKey(a)]?.busy())) return copy().attachments;
    if (ready() < open().length) return many() ? copy().incomplete(ready(), open().length) : copy().choose;
    return undefined;
  };
  const send = () => {
    if (why() || ws.gate(props.needs)) return;
    const list = answers() as AskAnswer[];
    const confirmations = list.map(a => fields()[answerKey(a.ask)]?.clearOnConfirm());
    const work = props.onSend(list);
    if (!work) return;
    const given = Object.fromEntries(list.map(a => [answerKey(a.ask), { text: a.text, files: a.attachments }]));
    setFailure(null); setSending(true); setInFlight(given);
    void work.then(r => {
      if (r.ok) { for (const confirmed of confirmations) confirmed?.(); setSent(prior => ({ ...prior, ...given })); }
      else setFailure(r.message || copy().failure);
    }, () => setFailure(copy().failure)).finally(() => { setInFlight({}); setSending(false); });
  };
  const first = () => open()[0] ?? props.asks[0];
  const shown = useContext(ScreenActive);
  let form!: HTMLDivElement;
  // Number keys choose an option of a single visible question. Focus identifies a target when several forms are present.
  const onKey = (e: KeyboardEvent) => {
    const el = document.activeElement, typing = el instanceof HTMLTextAreaElement || el instanceof HTMLInputElement;
    if (many() || !shown() || typing || e.ctrlKey || e.metaKey || e.altKey || document.querySelector("dialog[open]") || !/^[1-8]$/.test(e.key)) return;
    if (!form || form.closest("[hidden]")) return;
    const nearby = form.closest(".conversation-actions");
    if (nearby && !form.contains(el) && nearby.querySelectorAll("[data-question-form]").length > 1) return;
    form.querySelector<HTMLElement>(`[id="decision-${first().id}"] .opt:nth-child(${e.key})`)?.click();
  };
  document.addEventListener("keydown", onKey);
  onCleanup(() => document.removeEventListener("keydown", onKey));
  return <div ref={form} data-question-form="" class={open().length === 0 && !sending() ? "dc dc-done" : "dc"} role={many() ? "group" : undefined} lang={language()} aria-label={many() ? heading() : undefined}>
    <div class="dc-h"><I n="question" s={14} />{heading()}<span class="r">{open().length ? props.waiting(first()) : ""}</span></div>
    <Keyed each={props.asks} by={answerKey}>{(a, i) => <>
      <Show when={sent()[answerKey(a())] ?? inFlight()[answerKey(a())]}>{given => <Answered question={a().text} answer={given().text} files={given().files} at={null} />}</Show>
      <Show when={sent()[answerKey(a())] === undefined}>
        <Question ask={a} n={i() + 1} of={props.asks.length} hidden={inFlight()[answerKey(a())] !== undefined} needs={props.needs} blocked={props.blocked} note={props.note} projectId={props.projectId} draftScope={props.draftScope} register={register}
          plane={many() ? undefined : { get why() { return why(); }, go: send }} />
      </Show>
    </>}</Keyed>
    <Show when={failure()}>{f => <div class="cap st-warn ws-dc-note" role="alert">{f()} {copy().retained} <button type="button" class="lnk" onClick={send}>{copy().retry}</button></div>}</Show>
    <Show when={sending()}><div class="cap muted ws-dc-note" role="status"><I n="loader" s={12} class="spin ws-wait" />{copy().sending(many())}</div></Show>
    <Show when={open().length === 0 && !sending()}><div class="cap muted ws-dc-note" role="status">{copy().sent(many())}</div></Show>
    <Show when={many() && open().length > 0}>
      <div class="dc-send">
        <span class="cap muted">{copy().progress(ready(), open().length)}</span>
        <Action kind="primary" why={ws.gate(props.needs) ?? why()} onClick={send}><I n="send" />{copy().sendAll}</Action>
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
