import type { TimelineEntry } from "@cortex/workspace-domain";
import { displayStep, STEP_PREFIX, isHiddenStep, isPlumbingStep, isToolName, tookText, toolLabel, viewOfText, workedText } from "../tool-names";

/** What the runtime's display feed holds for one task, in order. */
export type FeedRow = { historyId: string; revision: string; kind: string; text?: string | null; toolStatus?: string | null; recordedAt?: string | null; availability?: string };
export type Dated = { at: string; entry: TimelineEntry };

export function shownFold(fold: Extract<TimelineEntry, { kind: "fold" }>, showAllTools: boolean) {
  return (showAllTools && fold.all) || fold;
}

const STATUS = /\s·\s(pending|in_progress|completed|failed|cancelled)$/;
/** `call:coordinator:es_1:tc_2` or `verification:1:212:0`: a machine's name for a step, which means nothing to a person. */
const isIdentifier = (text: string) => /^[A-Za-z_]+:\S+$/.test(text);
const NOISE = "Display update:";
/** The runtime's divider row for a completed context compaction (`COMPACTION_DIVIDER` in the core history projection). */
const COMPACTED = "Context compacted";
/** The runtime's recovery capsule starts with these words in every engine prompt. A message that carries them is the
 *  model repeating its own input (once even inventing a compaction notice after it), never something to read as a reply. */
const CAPSULE = "retained-state recovery capsule";
/** The engine's own context frames reach the model wrapped in this tag; a reply that writes it is inventing prompt text. */
const REMINDER = "<system-reminder>";
/** What may stand before an echo at a message's start: a few stray characters (a cut word) and, before the capsule header,
 *  the one word that names the product. A sentence before it is prose. */
const STRAY = /^\S{0,8}$/, NAMED = /^\S{0,8}(?:\s+\S{1,10})?$/;
const count = (body: string, part: string) => body.split(part).length - 1;
/**
 * Whether a message is the model repeating prompt text rather than a reply: it opens with the capsule header or a reminder
 * block, carries two or more reminder blocks, or carries the capsule header followed by its JSON data. A reply that only
 * mentions the tag or the capsule once, inside its own prose, is a reply and stays visible.
 */
const echoed = (body: string) => {
  const text = body.trimStart(), capsule = text.indexOf(CAPSULE), reminder = text.indexOf(REMINDER);
  if ((capsule >= 0 && NAMED.test(text.slice(0, capsule).trim())) || (reminder >= 0 && STRAY.test(text.slice(0, reminder).trim()))) return true;
  if (count(text, REMINDER) >= 2) return true;
  return capsule >= 0 && /^[^{]{0,200}\{\s*"/.test(text.slice(capsule + CAPSULE.length));
};
export const CAPSULE_ECHO = "The model repeated its internal working context instead of replying. It is hidden here.";
/** A worker launch is already shown in the Workers card under the turn; repeating it as a step shows every worker twice. */
const WORKER_LAUNCH = /^Worker:\s/;

/** `mcp__cortex__project_read` reads as "Read". A step that is already a sentence stays one. */
function stepName(text: string) {
  if (isToolName(text)) return toolLabel(text);
  const name = text.replace(/^mcp__[^_]+(?:_[^_]+)*?__/, "");
  if (/\s/.test(name)) return name;
  const words = name.replace(/[_-]+/g, " ").trim();
  return words ? words[0].toUpperCase() + words.slice(1) : name;
}
const span = (from: string, to: string) => tookText(Date.parse(to) - Date.parse(from));

/** One reading of a turn's steps: what it counted, and between which rows. */
type Reading = { first: FeedRow; last: FeedRow; names: string[]; failed: number };
/** The simplified reading leaves out the runtime's own steps; `all` is the reading with them, kept so the reader can switch to it. */
type Steps = Reading & { all: Reading };
function foldOf(run: Reading): { text: string; steps: string[] } {
  const counted: { name: string; n: number }[] = [];
  for (const name of run.names) { const tail = counted.at(-1); if (!displayStep(name) && tail?.name === name) tail.n++; else counted.push({ name, n: 1 }); }
  const took = span(run.first.recordedAt ?? "", run.last.recordedAt ?? "");
  const text = workedText(took, run.names.map(viewOfText), run.failed);
  return { text, steps: counted.map(c => c.n > 1 ? `${c.name} ×${c.n}` : c.name) };
}
function fold(run: Steps): Dated | null {
  if (!run.all.names.length) return null;
  const simple = run.names.length ? foldOf(run) : { text: "", steps: [] as string[] };
  const all = foldOf(run.all);
  // A turn of only the runtime's own steps is an empty fold: the page draws it only when every tool call is asked for.
  return { at: run.all.first.recordedAt ?? "", entry: { id: `history:${run.all.first.historyId}:${run.all.first.revision}`, kind: "fold", ...simple,
    ...(all.text !== simple.text || all.steps.length !== simple.steps.length ? { all } : {}) } };
}

/**
 * The feed as a person reads it. It arrives as streamed chunks and raw tool events, so one sentence is many rows and one tool
 * call is several. Text joins back into the message it was, tool events fold into one line per turn, and a machine identifier
 * or a display update is never shown. Nothing here invents an event: every entry comes from the rows it replaces.
 */
export function conversationOf(rows: FeedRow[], known: { description: string; answers: string[]; partial?: boolean }): Dated[] {
  // Split ACP updates have one call id. Merge fields without losing the initial input.
  const calls = new Map<string, FeedRow>(), ordered: FeedRow[] = [];
  for (const row of rows) {
    const detail = row.kind === "tool" ? displayStep(row.text ?? "") : null;
    if (detail?.callId) {
      const prior = calls.get(detail.callId);
      if (prior) {
        const merged = { ...displayStep(prior.text ?? ""), ...detail };
        // Receipt-backed updates already contain the complete allowlisted step.
        // Keep their reference while coalescing the broker's start and finish.
        const text = detail.callId.startsWith("cortex-broker:") ? row.text : STEP_PREFIX + JSON.stringify(merged);
        Object.assign(prior, { text, toolStatus: row.toolStatus });
      } else { const copy = {...row}; calls.set(detail.callId, copy); ordered.push(copy); }
    } else ordered.push(row);
  }
  const out: Dated[] = [], answers = [...known.answers];
  let text: { row: FeedRow; parts: string[] } | null = null, steps: Steps | null = null;
  // With only the latest part of the conversation read (`partial`), its first message or turn of steps may have begun among
  // the rows not read: its text would read cut at its start and its fold would count only part of that work. It is left out
  // until the rows before it are read. An owner's message or an unreadable row ends that first part.
  let leading = known.partial === true;
  const endText = () => {
    if (text && leading) leading = false;
    else if (text) {
      const body = text.parts.join("").trim(), id = `history:${text.row.historyId}:${text.row.revision}`, at = text.row.recordedAt ?? "";
      if (echoed(body)) out.push({ at, entry: { id, kind: "line", icon: "archive", text: CAPSULE_ECHO, at: null, rule: false } });
      else if (body) out.push({ at, entry: { id, kind: "cortex", text: body } });
    }
    text = null;
  };
  const endSteps = () => { if (steps && leading) leading = false; else if (steps) { const entry = fold(steps); if (entry) out.push(entry); } steps = null; };
  for (const row of ordered) {
    const body = row.text ?? "";
    if (row.kind === "user" && row.text != null) {
      endText(); endSteps(); leading = false;
      const said = body.trim(), answered = answers.indexOf(said);
      // The task's own description heads the page, and an answer reads with its question.
      if (answered >= 0) answers.splice(answered, 1);
      else if (said && said !== known.description.trim()) out.push({ at: row.recordedAt ?? "", entry: { id: `history:${row.historyId}:${row.revision}`, kind: "said", text: said } });
    } else if (row.kind === "system" && body === COMPACTED) {
      endText(); endSteps(); leading = false;
      out.push({ at: row.recordedAt ?? "", entry: { id: `history:${row.historyId}:${row.revision}`, kind: "divider", text: COMPACTED } });
    } else if (row.kind === "tool" || row.kind === "system") {
      const detail = displayStep(body);
      if (row.text == null || body.startsWith(NOISE) || WORKER_LAUNCH.test(body) || isHiddenStep(detail?.title ?? body.replace(STATUS, ""))) continue;
      endText();
      const title = body.replace(STATUS, "");
      steps ??= { first: row, last: row, names: [], failed: 0, all: { first: row, last: row, names: [], failed: 0 } };
      const readings = isPlumbingStep(detail?.title ?? title) ? [steps.all] : [steps, steps.all];
      for (const reading of readings) {
        if (!reading.names.length) reading.first = row;
        reading.last = row;
        if (row.toolStatus === "failed") reading.failed++;
        if (detail) reading.names.push(body);
        else if (body.startsWith("Plan\n")) reading.names.push("Updated the plan");
        else if (!isIdentifier(title) && row.toolStatus !== "completed") reading.names.push(stepName(title));
      }
    } else if (row.text != null) {
      if (!body.trim() && !text) continue;
      endSteps();
      (text ??= { row, parts: [] }).parts.push(body);
    } else {
      endText(); endSteps(); leading = false;
      out.push({ at: row.recordedAt ?? "", entry: { id: `history:${row.historyId}:${row.revision}`, kind: "line", icon: "help", text: "Part of this conversation could not be read", at: null, rule: false } });
    }
  }
  endText(); endSteps();
  return out;
}
