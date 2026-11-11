/**
 * How a tool call reads to a person. An engine names its calls `mcp.cortex.project_read` or `mcp__cortex__read_skill`;
 * the screen says "Read" or "Read skill", the same words the Paseo plugin uses. Bookkeeping that is not work, such as
 * context-window updates or thinking, is left out of the activity. Nothing is deleted: this is presentation only.
 */
import { presentToolSemantics } from "../../../integrations/paseo/shared/tool-presentation";
import { chatToolIcon, nativeRow, summarizeToolRun } from "../../../integrations/paseo/client/worker-panel-groups";
import { isHiddenOperation } from "../../../integrations/paseo/shared/timeline-visibility";
// Which calls appear and every name come directly from Paseo's shared modules, so both clients show the same rows.
const MCP = /^(?:mcp__(.+?)__|mcp\.([^.\s]+)\.)([A-Za-z0-9_-]+)\s*(.*)$/s;

/** The call's own name without its server, or null when the text is not a tool name. */
function operation(text: string): { name: string; rest: string } | null {
  const m = text.trim().match(MCP);
  return m ? { name: m[3].toLowerCase(), rest: m[4].trim() } : null;
}

/** One bare `get_code_snippet`-style word: an engine's identifier, never words for a person. */
const IDENTIFIER = /^[A-Za-z][A-Za-z0-9]*(?:_+[A-Za-z0-9]+)+$/;
const plainName = (text: string) => IDENTIFIER.test(text.trim()) ? presentToolSemantics({ name: text.trim() }).label || "Tool" : text;

/** A plain name for a tool call, or the text itself when it is already a sentence. */
export function toolLabel(text: string): string {
  const op = operation(text);
  if (!op) return plainName(text);
  const label = presentToolSemantics({ name: text.trim().slice(0, text.trim().length - op.rest.length).trim() }).label;
  return op.rest ? `${label} ${op.rest}` : label;
}
/** Whether this text is an engine's name for a call rather than words for a person. */
export const isToolName = (text: string) => operation(text) !== null;
/** Context-window figures and engine status updates: real, but not part of the work a person reads. */
export function isHiddenStep(text: string): boolean {
  const t = text.trim();
  if (/^context \d+\s*\/\s*\d+$/i.test(t)) return true;
  const name = (operation(t)?.name ?? t).toLowerCase();
  return isHiddenOperation(name);
}

export type DisplayStep = { title?: string; callId?: string; kind?: string; status?: string; path?: string; command?: string; query?: string; ref?: string; oldText?: string; newText?: string; output?: string; notice?: string };
export const STEP_PREFIX = "Workspace step: ";
/** Only the helper's selected presentation fields, never raw protocol JSON. */
export function displayStep(text?: string): DisplayStep | null {
  if (!text) return null;
  const body = text.startsWith(STEP_PREFIX) ? text.slice(STEP_PREFIX.length) : text;
  // Only an object is a step. Anything else would throw in JSON.parse, and a long feed asks this of every row on every
  // streamed update: thousands of thrown errors were most of an update's cost (test/scroll-perf.py, `-live`).
  if (body.trimStart()[0] !== "{") return null;
  try {
    const value = JSON.parse(body);
    if (!value || typeof value !== "object" || Array.isArray(value)) return null;
    const out: DisplayStep = {};
    for (const key of ["title","callId","kind","status","path","command","query","ref","oldText","newText","output","notice"] as const)
      if (typeof value[key] === "string") out[key] = value[key];
    return out;
  } catch { return null; }
}
/** Whether opening a step shows something its row does not: what was sent, what came back, or a diff. A path or a query alone is already on the row. */
export const stepHasContent = (d: DisplayStep) => !!(d.command || d.output || d.newText || d.oldText);

/** The kinds and icons of the Paseo plugin's rows (integrations/paseo/client/worker-panel-groups.ts): eye for reads, pencil for edits, a terminal for commands. */
export type StepKind = "read" | "edit" | "shell" | "search" | "other";
type PresentedEvent = Parameters<typeof nativeRow>[0];
export type StepView = { kind: StepKind; icon: string; label: string; target: string; detail: DisplayStep | null; event?: PresentedEvent };
const ICONS: Record<StepKind, string> = { read: "eye", edit: "pencil", shell: "terminal", search: "search", other: "wrench" };
const view = (kind: StepKind, label: string, target = "", detail: DisplayStep | null = null): StepView => ({ kind, icon: ICONS[kind], label, target, detail });

export function viewOfDetail(d: DisplayStep): StepView {
  // Adapt only the service's selected display fields, never raw protocol payloads.
  const event: PresentedEvent = { id: 0, type: "tool_call", text: "", createdAt: "",
    title: d.title ?? (d.command ? "Shell" : d.kind ?? "Tool activity"),
    tool_kind: d.kind, raw_input: { path: d.path, command: d.command, query: d.query, report_id: d.ref, skill: d.ref } };
  const row = nativeRow(event), icon = chatToolIcon(event);
  const kind: StepKind = icon === "Eye" ? "read" : icon === "Pencil" ? "edit" : icon === "SquareTerminal" ? "shell" : icon === "Search" ? "search" : "other";
  return { ...view(kind, row.label, row.summary ?? (kind === "search" ? d.query ?? "" : ""), d), event };
}
/** A step named in words: its first word still tells a read from an edit. */
function named(text: string, detail: DisplayStep | null = null): StepView {
  const label = plainName(text);
  const first = /^(Read|Edit|Search|Shell)\b\s*(.*)$/s.exec(label);
  if (!first || detail) return view("other", label, "", detail);
  const kind = first[1] === "Read" ? "read" : first[1] === "Edit" ? "edit" : first[1] === "Search" ? "search" : "shell";
  // "Read src/a.py" splits at the path; "Read failed" stays whole as a sentence.
  return /[./\\_-]/.test(first[2]) ? view(kind, first[1], first[2], detail) : view(kind, label, "", detail);
}
/** A Conversation step: a recorded detail or a name already in words. */
export function viewOfText(text: string): StepView {
  const detail = displayStep(text);
  return detail ? viewOfDetail(detail) : isToolName(text) ? viewOfDetail({title:text}) : named(toolLabel(text));
}
export const stepLabel = (s: DisplayStep) => viewOfDetail(s).label;

/** Paseo's overview line (summarizeToolRun), word for word: "Edited 28 files, ran 35 commands, read 53 files, and used 1 other tool". */
export function summarizeSteps(steps: readonly StepView[]): string {
  return summarizeToolRun(steps.map((s, id) => ({
    ...(s.event ?? { type: "tool_call", text: "", createdAt: "", title: s.label,
      tool_kind: s.kind === "shell" ? "execute" : s.kind,
      raw_input: s.kind === "shell" ? {command:s.target} : {path:s.target} }), id,
  })));
}
/** The line a fold shows: how long the turn worked, what it did, and anything that needs a look. */
export function workedText(took: string, steps: readonly StepView[], failed: number) {
  const summary = summarizeSteps(steps);
  const did = took && summary ? summary[0].toLowerCase() + summary.slice(1) : summary;
  return [took ? `Worked ${took}` : "", did || (took ? "" : "Worked"), failed ? `${failed} tool call${failed === 1 ? "" : "s"} need${failed === 1 ? "s" : ""} attention` : ""].filter(Boolean).join(", ");
}
export const tookText = (ms: number) => {
  if (!Number.isFinite(ms) || ms < 1000) return "";
  const seconds = Math.floor(ms / 1000), minutes = Math.floor(seconds / 60);
  return minutes ? `${minutes}m ${seconds % 60}s` : `${seconds}s`;
};

type Step = { sequence: number; text: string; kind: string; at: number | null; error?: boolean; args?: string; output?: string; code?: string; diff?: unknown; toolCallRef?: string; command?: { state: string } | null };
/** One recorded activity as a step row. */
export function viewOfActivity(a: Step): StepView {
  const detail = displayStep(a.args) ?? displayStep(a.text);
  if (detail) return viewOfDetail({...detail, title: isToolName(a.text) ? a.text : detail.title});
  const raw = isToolName(a.text) ? a.text : a.code && isToolName(a.code) ? a.code : null;
  if (raw) return viewOfDetail({title:raw});
  if (a.command) return view("shell", "Shell", a.text);
  if (a.kind === "edit") return view("edit", "Edit", a.code ?? a.text);
  const step = named(a.text);
  return { ...step, target: a.code && !IDENTIFIER.test(a.code.trim()) ? a.code : step.target };
}
// Recorded activity is never changed in place, so what was decided about a record holds for as long as the record lives.
const hidden = new WeakMap<object, boolean>();
export const isHiddenActivity = (a: Pick<Step, "text" | "code" | "args">) => {
  const known = hidden.get(a);
  if (known !== undefined) return known;
  const detail = displayStep(a.args) ?? displayStep(a.text);
  // Paseo also judges the call's kind: thinking and context figures arrive as untitled steps of those kinds.
  const out = [a.text, a.code, detail?.title, detail?.kind].some(t => !!t && isHiddenStep(t));
  hidden.set(a, out);
  return out;
};
export const isFoldedStep = (a: Pick<Step, "kind" | "text" | "code">) => ["tool", "tools", "edit", "command"].includes(a.kind) || (a.kind === "note" && (isToolName(a.text) || IDENTIFIER.test(a.text.trim()) || (!!a.code && isToolName(a.code))));
export const stepFailed = (a: Pick<Step, "error" | "args" | "command">) => a.error === true || a.command?.state === "failed" || displayStep(a.args)?.status === "failed";

export type FeedRun<A> = { items: A[]; views: StepView[]; text: string; loading: boolean };
export type FeedItem<A> = { key: string; item: A } | { key: string; run: FeedRun<A> };
// A call's row, made from its first and its latest update, is the same object while those two are, so a streamed update to
// another row leaves this one as it was.
const merged = new WeakMap<object, WeakMap<object, object>>();
function callRow<A extends Step>(first: A, latest: A): A {
  let byLatest = merged.get(first);
  if (!byLatest) merged.set(first, byLatest = new WeakMap());
  let row = byLatest.get(latest) as A | undefined;
  if (!row) byLatest.set(latest, row = { ...latest, sequence: first.sequence, at: first.at });
  return row;
}
/** One call is one row: its updates keep the first update's place and read as the newest, as in Paseo's groupWorkerEvents. */
export function oneRowPerCall<A extends Step>(list: readonly A[]): A[] {
  const places = new Map<string, number>(), firsts = new Map<string, A>(), out: A[] = [];
  for (const a of list) {
    const ref = a.toolCallRef?.replace(/^tool-error:/, "tool:");
    const place = ref === undefined ? undefined : places.get(ref);
    if (place === undefined) { if (ref !== undefined) { places.set(ref, out.length); firsts.set(ref, a); } out.push(a); }
    else out[place] = callRow(firsts.get(ref!)!, a);
  }
  return out;
}

/**
 * Runs of tool steps become one block, as in Paseo's chat; every note, message and divider stays between them.
 * `runIds` remembers which block each step belongs to, so a block keeps its identity when its first steps roll out of
 * the bounded live window or older steps load above it. Without it a growing block would be a new block on every update.
 * `previous` is the last result: an entry that would say the same again is that object, built no second time, so a streamed
 * update costs and redraws only the rows it changed. (Rebuilding every row's views on every update dropped frames in a long
 * feed; test/scroll-perf.py measures it with `-live`.)
 */
export function feedOf<A extends Step>(list: readonly A[], runIds?: Map<number, string>, previous?: ReadonlyMap<string, FeedItem<A>>): FeedItem<A>[] {
  const out: FeedItem<A>[] = [], used = new Set<string>();
  let run: A[] = [];
  const runKey = () => {
    let id = run.map(a => runIds?.get(a.sequence)).find(k => k !== undefined && !used.has(k)) ?? `run:${run[0].sequence}`;
    while (used.has(id)) id += "+";
    used.add(id);
    for (const a of run) runIds?.set(a.sequence, id);
    return id;
  };
  const flush = () => {
    if (!run.length) return;
    const key = runKey(), old = previous?.get(key);
    // The block's views, text and state follow from its steps alone.
    if (old && "run" in old && old.run.items.length === run.length && old.run.items.every((a, i) => a === run[i])) { out.push(old); run = []; return; }
    const views = run.map(viewOfActivity), first = run[0].at, last = run.at(-1)!.at;
    const took = first === null || last === null ? "" : tookText(last - first);
    out.push({ key, run: { items: run, views, text: workedText(took, views, run.filter(stepFailed).length),
      loading: run.some(a => a.command?.state === "running" || ["in_progress", "pending"].includes(displayStep(a.args)?.status ?? "")) } });
    run = [];
  };
  for (const a of list) {
    if (isHiddenActivity(a)) continue;
    if (isFoldedStep(a)) { run.push(a); continue; }
    flush();
    const key = `row:${a.sequence}`, old = previous?.get(key);
    out.push(old && "item" in old && old.item === a ? old : { key, item: a });
  }
  flush();
  return out;
}

