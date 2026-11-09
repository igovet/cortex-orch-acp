import type { TimelineEntry } from "@cortex/workspace-domain";
import { displayStep, STEP_PREFIX, isHiddenStep, isToolName, tookText, toolLabel, viewOfText, workedText } from "../tool-names";

/** What the runtime's display feed holds for one task, in order. */
export type FeedRow = { historyId: string; revision: string; kind: string; text?: string | null; toolStatus?: string | null; recordedAt?: string | null; availability?: string };
export type Dated = { at: string; entry: TimelineEntry };

const STATUS = /\s·\s(pending|in_progress|completed|failed|cancelled)$/;
/** `call:coordinator:es_1:tc_2` or `verification:1:212:0`: a machine's name for a step, which means nothing to a person. */
const isIdentifier = (text: string) => /^[A-Za-z_]+:\S+$/.test(text);
const NOISE = "Display update:";
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

type Steps = { first: FeedRow; last: FeedRow; names: string[]; failed: number };
function fold(run: Steps): Dated | null {
  if (!run.names.length) return null;
  const counted: { name: string; n: number }[] = [];
  for (const name of run.names) { const tail = counted.at(-1); if (!displayStep(name) && tail?.name === name) tail.n++; else counted.push({ name, n: 1 }); }
  const took = span(run.first.recordedAt ?? "", run.last.recordedAt ?? "");
  const text = workedText(took, run.names.map(viewOfText), run.failed);
  return { at: run.first.recordedAt ?? "", entry: { id: `history:${run.first.historyId}:${run.first.revision}`, kind: "fold", text, steps: counted.map(c => c.n > 1 ? `${c.name} ×${c.n}` : c.name) } };
}

/**
 * The feed as a person reads it. It arrives as streamed chunks and raw tool events, so one sentence is many rows and one tool
 * call is several. Text joins back into the message it was, tool events fold into one line per turn, and a machine identifier
 * or a display update is never shown. Nothing here invents an event: every entry comes from the rows it replaces.
 */
export function conversationOf(rows: FeedRow[], known: { description: string; answers: string[] }): Dated[] {
  // Split ACP updates have one call id. Merge fields without losing the initial input.
  const calls = new Map<string, FeedRow>(), ordered: FeedRow[] = [];
  for (const row of rows) {
    const detail = row.kind === "tool" ? displayStep(row.text ?? "") : null;
    if (detail?.callId) {
      const prior = calls.get(detail.callId);
      if (prior) {
        const merged = { ...displayStep(prior.text ?? ""), ...detail };
        Object.assign(prior, { text: STEP_PREFIX + JSON.stringify(merged), toolStatus: row.toolStatus });
      } else { const copy = {...row}; calls.set(detail.callId, copy); ordered.push(copy); }
    } else ordered.push(row);
  }
  const out: Dated[] = [], answers = [...known.answers];
  let text: { row: FeedRow; parts: string[] } | null = null, steps: Steps | null = null;
  const endText = () => {
    if (text) { const body = text.parts.join("").trim(); if (body) out.push({ at: text.row.recordedAt ?? "", entry: { id: `history:${text.row.historyId}:${text.row.revision}`, kind: "cortex", text: body } }); }
    text = null;
  };
  const endSteps = () => { if (steps) { const entry = fold(steps); if (entry) out.push(entry); } steps = null; };
  for (const row of ordered) {
    const body = row.text ?? "";
    if (row.kind === "user" && row.text != null) {
      endText(); endSteps();
      const said = body.trim(), answered = answers.indexOf(said);
      // The task's own description heads the page, and an answer reads with its question.
      if (answered >= 0) answers.splice(answered, 1);
      else if (said && said !== known.description.trim()) out.push({ at: row.recordedAt ?? "", entry: { id: `history:${row.historyId}:${row.revision}`, kind: "said", text: said } });
    } else if (row.kind === "tool" || row.kind === "system") {
      const detail = displayStep(body);
      if (row.text == null || body.startsWith(NOISE) || WORKER_LAUNCH.test(body) || isHiddenStep(detail?.title ?? body.replace(STATUS, ""))) continue;
      endText();
      steps ??= { first: row, last: row, names: [], failed: 0 };
      steps.last = row;
      const title = body.replace(STATUS, "");
      if (row.toolStatus === "failed") steps.failed++;
      if (detail) steps.names.push(body);
      else if (body.startsWith("Plan\n")) steps.names.push("Updated the plan");
      else if (!isIdentifier(title) && row.toolStatus !== "completed") steps.names.push(stepName(title));
    } else if (row.text != null) {
      if (!body.trim() && !text) continue;
      endSteps();
      (text ??= { row, parts: [] }).parts.push(body);
    } else {
      endText(); endSteps();
      out.push({ at: row.recordedAt ?? "", entry: { id: `history:${row.historyId}:${row.revision}`, kind: "line", icon: "help", text: "Part of this conversation could not be read", at: null, rule: false } });
    }
  }
  endText(); endSteps();
  return out;
}
