/**
 * How a tool call reads to a person. An engine names its calls `mcp.cortex.project_read` or `mcp__cortex__read_skill`;
 * the screen says "Read" or "Read skill", the same words the Paseo plugin uses. Bookkeeping that is not work, such as
 * context-window updates or thinking, is left out of the activity. Nothing is deleted: this is presentation only.
 */
import { presentToolSemantics } from "../../../integrations/paseo/shared/tool-presentation";
import { chatToolIcon, nativeRow, summarizeToolRun } from "../../../integrations/paseo/client/worker-panel-groups";
import { isHiddenOperation } from "../../../integrations/paseo/shared/timeline-visibility";
/** Engine updates that are not steps of the work. */
// Additional omissions of the Workspace service's selected feed (workspace_api/history.py).
// Paseo's engine noise policy and all tool names come directly from its shared modules.
const HIDDEN = new Set(["result_submit", "create_draft", "draft_write", "write_report", "read_skill", "tools_discover", "read_capsule", "task_get_context", "toolsearch"]);
const MCP = /^(?:mcp__(.+?)__|mcp\.([^.\s]+)\.)([A-Za-z0-9_-]+)\s*(.*)$/s;

/** The call's own name without its server, or null when the text is not a tool name. */
function operation(text: string): { name: string; rest: string } | null {
  const m = text.trim().match(MCP);
  return m ? { name: m[3].toLowerCase(), rest: m[4].trim() } : null;
}

/** A plain name for a tool call, or the text itself when it is already a sentence. */
export function toolLabel(text: string): string {
  const op = operation(text);
  if (!op) return text;
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
  return isHiddenOperation(name) || HIDDEN.has(name.replace(/ /g, "_"));
}

export type DisplayStep = { title?: string; callId?: string; kind?: string; status?: string; path?: string; command?: string; query?: string; oldText?: string; newText?: string; output?: string; notice?: string };
export const STEP_PREFIX = "Workspace step: ";
/** Only the helper's selected presentation fields, never raw protocol JSON. */
export function displayStep(text?: string): DisplayStep | null {
  if (!text) return null;
  try {
    const value = JSON.parse(text.startsWith(STEP_PREFIX) ? text.slice(STEP_PREFIX.length) : text);
    if (!value || typeof value !== "object" || Array.isArray(value)) return null;
    const out: DisplayStep = {};
    for (const key of ["title","callId","kind","status","path","command","query","oldText","newText","output","notice"] as const)
      if (typeof value[key] === "string") out[key] = value[key];
    return out;
  } catch { return null; }
}

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
    tool_kind: d.kind, raw_input: { path: d.path, command: d.command, query: d.query } };
  const row = nativeRow(event), icon = chatToolIcon(event);
  const kind: StepKind = icon === "Eye" ? "read" : icon === "Pencil" ? "edit" : icon === "SquareTerminal" ? "shell" : icon === "Search" ? "search" : "other";
  return { ...view(kind, row.label, row.summary ?? (kind === "search" ? d.query ?? "" : ""), d), event };
}
/** A step named in words: its first word still tells a read from an edit. */
function named(label: string, detail: DisplayStep | null = null): StepView {
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

type Step = { sequence: number; text: string; kind: string; at: number | null; error?: boolean; args?: string; output?: string; code?: string; diff?: unknown; command?: { state: string } | null };
/** One recorded activity as a step row. */
export function viewOfActivity(a: Step): StepView {
  const detail = displayStep(a.args) ?? displayStep(a.text);
  if (detail) return viewOfDetail({...detail, title: isToolName(a.text) ? a.text : detail.title});
  if (a.command) return view("shell", "Shell", a.text);
  if (a.kind === "edit") return view("edit", "Edit", a.code ?? a.text);
  const raw = isToolName(a.text) ? a.text : a.code && isToolName(a.code) ? a.code : null;
  if (raw) return viewOfDetail({title:raw});
  return { ...named(a.text), target: a.code ?? named(a.text).target };
}
export const isHiddenActivity = (a: Pick<Step, "text" | "code" | "args">) =>
  [a.text, a.code, displayStep(a.args)?.title, displayStep(a.text)?.title].some(t => !!t && isHiddenStep(t));
export const isFoldedStep = (a: Pick<Step, "kind" | "text" | "code">) => ["tool", "tools", "edit", "command"].includes(a.kind) || (a.kind === "note" && (isToolName(a.text) || (!!a.code && isToolName(a.code))));
export const stepFailed = (a: Pick<Step, "error" | "args" | "command">) => a.error === true || a.command?.state === "failed" || displayStep(a.args)?.status === "failed";

export type FeedRun<A> = { items: A[]; views: StepView[]; text: string; loading: boolean };
export type FeedItem<A> = { key: string; item: A } | { key: string; run: FeedRun<A> };
/** Runs of tool steps become one block, as in Paseo's chat; every note, message and divider stays between them. */
export function feedOf<A extends Step>(list: readonly A[]): FeedItem<A>[] {
  const out: FeedItem<A>[] = [];
  let run: A[] = [];
  const flush = () => {
    if (!run.length) return;
    const views = run.map(viewOfActivity), first = run[0].at, last = run.at(-1)!.at;
    const took = first === null || last === null ? "" : tookText(last - first);
    out.push({ key: `run:${run[0].sequence}`, run: { items: run, views, text: workedText(took, views, run.filter(stepFailed).length),
      loading: run.some(a => a.command?.state === "running" || ["in_progress", "pending"].includes(displayStep(a.args)?.status ?? "")) } });
    run = [];
  };
  for (const a of list) {
    if (isHiddenActivity(a)) continue;
    if (isFoldedStep(a)) { run.push(a); continue; }
    flush();
    out.push({ key: `row:${a.sequence}`, item: a });
  }
  flush();
  return out;
}

