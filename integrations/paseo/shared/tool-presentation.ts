import { isHiddenOperation } from "./timeline-visibility.ts";
import { codeModePresentation, gatewayPresentation } from "./gateway-presentation.ts";
import { shellCommand } from "./shell-command.ts";
/** Shared semantic projection for collapsed tool rows.
 *
 * This is presentation data only.  The original call and its complete input
 * and output stay available in the expanded view; this projection deliberately
 * never serializes JSON or an engine's raw MCP name into a compact title.
 */

export type ToolPresentationKind =
  | "read"
  | "edit"
  | "shell"
  | "skill"
  | "governance"
  | "generic";

export type ToolPresentation = {
  kind: ToolPresentationKind;
  /** Stable icon token understood by the Paseo clients. */
  icon: "FileText" | "FilePenLine" | "Terminal" | "BookOpen" | "Scale" | "Wrench";
  /** Short human action, e.g. Read, Update plan, or Publish result. */
  action: string;
  /** Compact, safe target. Empty means that the action has no useful target. */
  target: string | null;
  /** Collapsed row title: action plus optional target. */
  label: string;
  /** Every observed call retains its expandable native detail. */
  expandable: boolean;
  /** Reserved host hint; Cortex uses the native expandable representation. */
  systemOperation: boolean;
  /** Whether the operation belongs in the user-facing timeline. */
  visible: boolean;
};

export type ToolPresentationInput = {
  name?: unknown;
  title?: unknown;
  toolKind?: unknown;
  input?: unknown;
  rawInput?: unknown;
  rawOutput?: unknown;
  metadata?: unknown;
};

const ACTIONS: Record<string, { kind: ToolPresentationKind; icon: ToolPresentation["icon"]; action: string; visible?: boolean }> = {
  // Core Cortex task/report operations.
  create_task: { kind: "generic", icon: "Wrench", action: "Create task" },
  set_governance: { kind: "governance", icon: "Scale", action: "Set governance" },
  create_draft: { kind: "edit", icon: "FilePenLine", action: "Create draft" },
  read_draft: { kind: "read", icon: "FileText", action: "Read draft" },
  write_report: { kind: "edit", icon: "FilePenLine", action: "Write report" },
  list_reports: { kind: "read", icon: "FileText", action: "List reports" },
  read_report: { kind: "read", icon: "FileText", action: "Read report" },
  project_read: { kind: "read", icon: "FileText", action: "Read" },
  project_write: { kind: "edit", icon: "FilePenLine", action: "Edit" },
  project_delete: { kind: "edit", icon: "FilePenLine", action: "Delete" },
  project_mkdir: { kind: "edit", icon: "FilePenLine", action: "Create directory" },
  project_git: { kind: "read", icon: "FileText", action: "Read project Git" },
  draft_write: { kind: "edit", icon: "FilePenLine", action: "Write draft" },
  command_run: { kind: "shell", icon: "Terminal", action: "Shell" },
  // Runtime proposal operations.
  answer_submit: { kind: "edit", icon: "FilePenLine", action: "Submit answer" },
  replan_request: { kind: "governance", icon: "Scale", action: "Request revised plan" },
  scope_request: { kind: "governance", icon: "Scale", action: "Request access" },
  escalation_request: { kind: "governance", icon: "Scale", action: "Request consultation" },
  task_get_context: { kind: "read", icon: "FileText", action: "Read task context" },
  plan_submit: { kind: "edit", icon: "FilePenLine", action: "Submit plan" },
  synthesis_submit: { kind: "edit", icon: "FilePenLine", action: "Submit synthesis" },
  user_question_request: { kind: "governance", icon: "Scale", action: "Ask user" },
  result_submit: { kind: "edit", icon: "FilePenLine", action: "Submit result" },
  // Shared source, capsule, skill, and diagnostic operations.
  list_sources: { kind: "read", icon: "FileText", action: "List sources" },
  read_source: { kind: "read", icon: "FileText", action: "Read source" },
  read_skill: { kind: "skill", icon: "BookOpen", action: "Read skill" },
  read_capsule: { kind: "read", icon: "FileText", action: "Read capsule", visible: true },
  read_large_output: { kind: "read", icon: "FileText", action: "Read output" },
  read_diagnostics: { kind: "read", icon: "FileText", action: "Read diagnostics" },
  ask_question: { kind: "governance", icon: "Scale", action: "Ask question" },
};

const NATIVE_READ = { kind: "read" as const, icon: "FileText" as const, action: "Read" };
const NATIVE_EDIT = { kind: "edit" as const, icon: "FilePenLine" as const, action: "Edit" };
const NATIVE_SHELL = { kind: "shell" as const, icon: "Terminal" as const, action: "Shell" };
const NATIVE_SKILL = { kind: "skill" as const, icon: "BookOpen" as const, action: "Skill" };
const NATIVE_GOVERNANCE = { kind: "governance" as const, icon: "Scale" as const, action: "Governance" };

function record(value: unknown): Record<string, unknown> {
  return value && typeof value === "object" && !Array.isArray(value)
    ? value as Record<string, unknown>
    : {};
}

function stringValue(value: unknown, ...keys: string[]): string | null {
  const source = record(value);
  for (const key of keys) {
    if (typeof source[key] === "string" && source[key].trim()) return source[key].trim();
  }
  return null;
}

function nestedString(value: unknown, keys: Set<string>): string | null {
  if (Array.isArray(value)) {
    for (const item of value) {
      const found = nestedString(item, keys);
      if (found) return found;
    }
    return null;
  }
  if (!value || typeof value !== "object") return null;
  for (const [key, child] of Object.entries(value as Record<string, unknown>)) {
    if (keys.has(key) && typeof child === "string" && child.trim()) return child.trim();
    const found = nestedString(child, keys);
    if (found) return found;
  }
  return null;
}

function compactTarget(value: string | null): string | null {
  if (!value) return null;
  const cleaned = value.replace(/\s+/g, " ").trim();
  // A collapsed row must never become a disguised payload/JSON dump.
  if (/^[{[]/.test(cleaned) || cleaned.length === 0) return null;
  return cleaned.length > 160 ? `${cleaned.slice(0, 157)}…` : cleaned;
}

function isPlaceholderTarget(value: string): boolean {
  return /^(?:preparing\s+file|editing\s+files?|file|tool\s+call)(?:\s*(?:…|\.\.\.))?$/i.test(value.trim());
}

function humanize(value: string): string {
  return value
    .replace(/^mcp(?:__|\.)[^_.]+(?:__|\.)/i, "")
    .replace(/^mcp__?/i, "")
    .replace(/[._-]+/g, " ")
    .trim()
    .replace(/\b\w/g, (char) => char.toUpperCase()) || "Tool";
}

function operationName(input: ToolPresentationInput): string {
  const envelope = record(input.rawInput ?? input.input);
  const raw = String(envelope.server === "cortex" && typeof envelope.tool === "string"
    ? `mcp.cortex.${envelope.tool}` : input.name ?? input.title ?? input.toolKind ?? "").trim();
  // Claude/Codex may put an MCP namespace in either dotted or underscored form.
  const mcp = raw.match(/mcp(?:__|\.)(?:[^_.]+)(?:__|\.)([^\s(]+)/i);
  if (mcp?.[1]) return mcp[1].replace(/[^A-Za-z0-9_-].*$/, "").toLowerCase();
  return raw
    .replace(/^mcp__[^_]+__/i, "")
    .replace(/^mcp\.[^.]+\./i, "")
    .split(/[\s(]/, 1)[0]
    .replace(/[^A-Za-z0-9_-]/g, "")
    .toLowerCase();
}

function targetFor(name: string, input: ToolPresentationInput, action: string): string | null {
  const gateway = gatewayPresentation(String(input.name ?? input.title ?? ""), input.rawInput ?? input.input, undefined);
  const envelope = record(input.rawInput ?? input.input);
  const args = gateway?.input ?? (envelope.server === "cortex" ? envelope.arguments : input.rawInput ?? input.input);
  const structuredArgs = input.rawInput !== undefined && input.input !== input.rawInput ? input.input : undefined;
  const path = stringValue(args, "file_path", "filePath", "path", "absolutePath")
    ?? nestedString(args, new Set(["file_path", "filePath", "path", "absolutePath"]))
    ?? stringValue(structuredArgs, "file_path", "filePath", "path", "absolutePath")
    ?? nestedString(structuredArgs, new Set(["file_path", "filePath", "path", "absolutePath"]));
  const command = shellCommand(args);
  const author = stringValue(args, "author", "report_author", "worker_name", "worker")
    ?? stringValue(input.metadata, "author", "report_author", "worker_name", "worker");
  const descriptive = stringValue(args, "skill", "summary", "node_key", "statement");
  // result_submit names a published report as report: {report_ref}.
  const value = stringValue(args, "report_id", "report_ref", "draft_id", "source_ref", "check_ref", "candidate_ref", "assignment_ref")
    ?? stringValue(record(args).report, "report_ref");
  if ((name === "read_skill" || name === "skill") && descriptive) return compactTarget(descriptive);
  if (name === "read_report") return compactTarget(author ? `by ${author}` : value);
  if (name === "publish_result") return compactTarget(descriptive ?? value);
  if (name === "update_plan") return compactTarget(descriptive ?? value);
  if ((name === "read_draft" || name === "write_report") && value) return compactTarget(value);
  if (name === "read_source" && value) return compactTarget(value);
  if ((name === "read_check" || name === "assess_check") && value) return compactTarget(value);
  if (name === "read_plan" || name === "list_reports" || name === "list_events" || name === "list_sources" || name === "read_capsule" || name === "read_diagnostics") return null;
  if (path) return compactTarget(path);
  if (command) return compactTarget(command);
  if (value) return compactTarget(value);
  if (action === "Governance" || action === "Set governance") return compactTarget(stringValue(args, "mode"));
  return null;
}

/** Return one safe semantic label for native and Cortex tool calls. */
export function presentToolSemantics(input: ToolPresentationInput): ToolPresentation {
  const codeMode = codeModePresentation(String(input.name ?? input.title ?? ""),
    input.rawInput ?? input.input, input.rawOutput, input.toolKind);
  if (codeMode) input = { ...input, name: codeMode.name, title: codeMode.name, rawInput: codeMode.input };
  const name = operationName(input);
  if (isHiddenOperation(name) || isHiddenOperation(input.toolKind)) {
    return { kind: "generic", icon: "Wrench", action: "", target: null, label: "", expandable: false, systemOperation: true, visible: false };
  }
  const title = String(input.title ?? input.name ?? "").trim();
  const nativeKind = String(input.toolKind ?? "").trim().toLowerCase();
  const native = title.match(/^(read|view|cat)\b/i) || nativeKind === "read" ? NATIVE_READ
    : title.match(/^(edit|patch|write|replace|modify)\b/i) ? NATIVE_EDIT
      : nativeKind === "edit" ? NATIVE_EDIT
        : title.match(/^(shell|bash|terminal|execute|run(?:\s+command)?)\b/i) || nativeKind === "execute" || nativeKind === "terminal" ? NATIVE_SHELL
          : title.match(/^skill\b/i) || nativeKind === "skill" ? NATIVE_SKILL
            : title.match(/^(governance|decide)\b/i) || nativeKind === "governance" ? NATIVE_GOVERNANCE
            : undefined;
  // A shell call titled with its own command (Claude's Bash: "for f in …")
  // carries that command in its input even when no tool kind came with it.
  const shellInput = !ACTIONS[name] && !native && shellCommand(input.rawInput ?? input.input) ? NATIVE_SHELL : undefined;
  const mapped = ACTIONS[name] ?? native ?? shellInput;
  if (!mapped) {
    const action = humanize(name || title || "tool");
    // A title that is itself a command line (Claude's Grep: `grep -rn "x" src`)
    // says what the call does; its first word alone ("Grep") does not.
    const commandLine = !/^mcp(?:__|\.)/i.test(title) && /\s/.test(title) ? compactTarget(title) : null;
    return { kind: "generic", icon: "Wrench", action, target: commandLine, label: commandLine ?? action, expandable: true, systemOperation: false, visible: true };
  }
  let target = targetFor(name, input, mapped.action);
  if (!target && native && !/^mcp(?:__|\.)/i.test(title)) {
    const suffix = title.replace(/^(read|view|cat|edit|patch|write|replace|modify|shell|bash|terminal|execute|run(?:\s+command)?|skill|governance|decide)\b\s*(?:[—:-]\s*)?/i, "").trim();
    if (suffix && suffix.toLowerCase() !== mapped.action.toLowerCase() && !isPlaceholderTarget(suffix)) target = compactTarget(suffix);
  }
  return {
    ...mapped,
    target,
    // Keep-verbatim form: the installed Paseo client humanizes (strips `-`,
    // `_`, lowercases) any tool row name that contains none of `:` `.` `/`
    // and no `__`. A target without a dot or slash (a bare argv, a report
    // id) used to trip that rule through the old "Action — target" em-dash
    // separator (O1). The colon here guarantees the host's keep-verbatim
    // test (`/[:./]/`) matches, so every label renders exactly as sent.
    label: target ? `${mapped.action}: ${target}` : mapped.action,
    expandable: !(name === "project_read" || name === "read_skill" || (!ACTIONS[name] && native === NATIVE_READ)),
    systemOperation: false,
    visible: true,
  };
}

// Short aliases make the shared boundary easy to consume from either panel.
export const toolPresentation = presentToolSemantics;
export const semanticToolLabel = presentToolSemantics;

// The same operations drive both labels and the public presentation catalogue.
export const KNOWN_CORTEX_TOOL_NAMES = Object.freeze(Object.keys(ACTIONS));
