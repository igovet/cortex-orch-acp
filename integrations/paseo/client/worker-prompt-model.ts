import type { WorkerHistory } from "../shared/worker-history.ts";

type Attempt = WorkerHistory["assignments"][number]["attempts"][number];
export type AttemptPrompt = NonNullable<Attempt["prompt"]>;

/** One readable part of the assignment text an attempt was sent. `markdown`
 * keeps the original text as written; `list` and `code` render literal
 * values (paths, report references, JSON) without reformatting them. */
export type PromptSection =
  | { kind: "markdown"; heading: string; text: string }
  | { kind: "list"; heading: string; items: string[] }
  | { kind: "code"; heading: string; text: string };

export type PromptView = {
  /** One plain sentence on whether and when this exact attempt received it. */
  status: string;
  tone: "normal" | "warning";
  /** `null`: no content may be shown (unavailable, mismatched or absent). */
  sections: PromptSection[] | null;
  truncatedNote: string | null;
  /** Short collapsed preview, from the objective when there is one. */
  preview: string | null;
};

const STATUS: Record<AttemptPrompt["state"], { text: string; tone: PromptView["tone"] }> = {
  sent: { text: "Sent to this attempt", tone: "normal" },
  queued: { text: "Queued: not yet recorded as sent to this attempt", tone: "warning" },
  unrecorded: { text: "Sending was not recorded for this attempt (older history, or it ended before its first prompt). Showing the assignment the runtime dispatched", tone: "warning" },
  mismatch: { text: "The retained assignment does not match what was recorded as sent to this attempt, so it is not shown", tone: "warning" },
  unavailable: { text: "The assignment prompt is not retained for this attempt", tone: "warning" },
};

function record(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function literal(value: unknown): string {
  return typeof value === "string" ? value : JSON.stringify(value, null, 2);
}

function reportItem(value: unknown): string {
  if (!record(value)) return literal(value);
  const parts = [value.node, value.report_ref].filter((part): part is string => typeof part === "string" && part.length > 0);
  return parts.length === Object.keys(value).length && parts.length > 0 ? parts.join(" · ") : JSON.stringify(value);
}

/** A goal or evidence field the runtime shortened to fit the brief: its
 * delivered preview plus the shortening the worker itself was told about. */
function shortened(heading: string, value: Record<string, unknown>, kind: "markdown" | "code"): PromptSection[] {
  const shown = typeof value.shown_characters === "number" ? value.shown_characters : null;
  const total = typeof value.original_characters === "number" ? value.original_characters : null;
  const note = shown !== null && total !== null ? ` (shortened in delivery: ${shown} of ${total} characters)` : " (shortened in delivery)";
  const preview = typeof value.preview === "string" ? value.preview : "";
  return [{ kind, heading: `${heading}${note}`, text: preview }];
}

const KNOWN_KEYS = new Set(["objective", "scope", "dependency_reports", "original_goal", "review_evidence"]);

/** Readable sections of a runtime brief (the JSON object the scheduler
 * encodes), or the whole text as Markdown when it is not that shape. */
export function promptSections(brief: string): PromptSection[] {
  let parsed: unknown;
  try { parsed = JSON.parse(brief); } catch { parsed = undefined; }
  if (!record(parsed) || typeof parsed.objective !== "string") return [{ kind: "markdown", heading: "Assignment", text: brief }];
  const sections: PromptSection[] = [{ kind: "markdown", heading: "Objective", text: parsed.objective }];
  if (Array.isArray(parsed.scope)) sections.push({ kind: "list", heading: "Scope", items: parsed.scope.map(literal) });
  else if (parsed.scope !== undefined) sections.push({ kind: "code", heading: "Scope", text: literal(parsed.scope) });
  if (Array.isArray(parsed.dependency_reports) && parsed.dependency_reports.length > 0) {
    sections.push({ kind: "list", heading: "Dependency reports", items: parsed.dependency_reports.map(reportItem) });
  }
  const goal = parsed.original_goal;
  if (typeof goal === "string" && goal.trim()) sections.push({ kind: "markdown", heading: "Original goal", text: goal });
  else if (record(goal) && goal.truncated === true) sections.push(...shortened("Original goal", goal, "markdown"));
  const evidence = parsed.review_evidence;
  if (record(evidence) && evidence.truncated === true) sections.push(...shortened("Review evidence", evidence, "code"));
  else if (evidence !== undefined && evidence !== null && !(Array.isArray(evidence) && evidence.length === 0)) {
    sections.push({ kind: "code", heading: "Review evidence", text: literal(evidence) });
  }
  const other = Object.fromEntries(Object.entries(parsed).filter(([key]) => !KNOWN_KEYS.has(key)));
  if (Object.keys(other).length > 0) sections.push({ kind: "code", heading: "Other fields", text: JSON.stringify(other, null, 2) });
  return sections;
}

function firstLine(text: string): string | null {
  const line = text.split("\n").map((value) => value.trim()).find(Boolean);
  return line ? (line.length > 160 ? `${line.slice(0, 159)}…` : line) : null;
}

/** The selected attempt's prompt section, or `null` when the read carried no
 * prompt data at all (a catalogue read, or a server without this field). */
export function promptView(prompt: AttemptPrompt | undefined): PromptView | null {
  if (!prompt) return null;
  const status = STATUS[prompt.state];
  const withContent = (prompt.state === "sent" || prompt.state === "queued" || prompt.state === "unrecorded") && typeof prompt.brief === "string";
  const sections = withContent ? [
    ...(prompt.title ? [{ kind: "markdown" as const, heading: "Title", text: prompt.title }] : []),
    ...promptSections(prompt.brief!),
    ...(prompt.acceptance ? [{ kind: "markdown" as const, heading: "Acceptance", text: prompt.acceptance }] : []),
  ] : null;
  const objective = sections?.find((section) => section.kind === "markdown" && section.heading !== "Title");
  return {
    status: prompt.state === "sent" && prompt.sentAt ? `${status.text} · ${prompt.sentAt}` : status.text,
    tone: status.tone,
    sections,
    truncatedNote: withContent && prompt.truncated ? "Prompt clipped for display; the worker received the complete text." : null,
    preview: objective && objective.kind === "markdown" ? firstLine(objective.text) : null,
  };
}
