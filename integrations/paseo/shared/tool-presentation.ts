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
  /** Project actions retain their expandable native detail; Cortex rows do not. */
  expandable: boolean;
  /** True for Cortex/system operations, which render as compact rows. */
  systemOperation: boolean;
  /** Internal coordinator delegation stays out of the user-facing timeline. */
  visible: boolean;
};

export type ToolPresentationInput = {
  name?: unknown;
  title?: unknown;
  toolKind?: unknown;
  input?: unknown;
  rawInput?: unknown;
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
  // Coordinator graph and evidence operations.
  delegate: { kind: "generic", icon: "Wrench", action: "Delegate", visible: false },
  list_events: { kind: "read", icon: "FileText", action: "List events" },
  clarify: { kind: "generic", icon: "Wrench", action: "Clarify" },
  cancel: { kind: "generic", icon: "Wrench", action: "Cancel" },
  decide: { kind: "governance", icon: "Scale", action: "Decide", visible: false },
  integrate: { kind: "edit", icon: "FilePenLine", action: "Integrate" },
  update_plan: { kind: "edit", icon: "FilePenLine", action: "Update plan" },
  read_plan: { kind: "read", icon: "FileText", action: "Read plan" },
  prepare_candidate: { kind: "generic", icon: "Wrench", action: "Prepare candidate" },
  request_check: { kind: "shell", icon: "Terminal", action: "Request check" },
  read_check: { kind: "read", icon: "FileText", action: "Read check" },
  assess_check: { kind: "governance", icon: "Scale", action: "Assess check" },
  // Shared source, capsule, skill, and diagnostic operations.
  list_sources: { kind: "read", icon: "FileText", action: "List sources" },
  read_source: { kind: "read", icon: "FileText", action: "Read source" },
  read_skill: { kind: "skill", icon: "BookOpen", action: "Read skill" },
  read_capsule: { kind: "read", icon: "FileText", action: "Read capsule", visible: false },
  read_large_output: { kind: "read", icon: "FileText", action: "Read output" },
  read_diagnostics: { kind: "read", icon: "FileText", action: "Read diagnostics" },
  ask_question: { kind: "governance", icon: "Scale", action: "Ask question" },
  ask_user: { kind: "governance", icon: "Scale", action: "Ask user" },
  publish_result: { kind: "edit", icon: "FilePenLine", action: "Publish result" },
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

function isCortexMcpName(value: unknown): boolean {
  return typeof value === "string" && /(?:^|mcp(?:__|\.))cortex(?:__|\.)/i.test(value.trim());
}

function accessesInternalCortex(value: unknown, keyHint = ""): boolean {
  if (typeof value === "string") {
    const key = keyHint.toLowerCase();
    const pathLike = /path|file|cwd|command|cmd|argv|query|pattern|location/.test(key);
    return pathLike && /(?:^|[\\/\s"'`])\.cortex(?:$|[\\/\s"'`])/i.test(value);
  }
  if (Array.isArray(value)) return value.some((item) => accessesInternalCortex(item, keyHint));
  if (!value || typeof value !== "object") return false;
  return Object.entries(value as Record<string, unknown>).some(([key, child]) => accessesInternalCortex(child, key));
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

function humanize(value: string): string {
  return value
    .replace(/^mcp(?:__|\.)[^_.]+(?:__|\.)/i, "")
    .replace(/^mcp__?/i, "")
    .replace(/[._-]+/g, " ")
    .trim()
    .replace(/\b\w/g, (char) => char.toUpperCase()) || "Tool";
}

function operationName(input: ToolPresentationInput): string {
  const raw = String(input.name ?? input.title ?? input.toolKind ?? "").trim();
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
  const args = input.rawInput ?? input.input;
  const structuredArgs = input.rawInput !== undefined && input.input !== input.rawInput ? input.input : undefined;
  const path = stringValue(args, "file_path", "filePath", "path", "absolutePath")
    ?? nestedString(args, new Set(["file_path", "filePath", "path", "absolutePath"]))
    ?? stringValue(structuredArgs, "file_path", "filePath", "path", "absolutePath")
    ?? nestedString(structuredArgs, new Set(["file_path", "filePath", "path", "absolutePath"]));
  const command = stringValue(args, "command", "cmd", "argv");
  const author = stringValue(args, "author", "report_author", "worker_name", "worker")
    ?? stringValue(input.metadata, "author", "report_author", "worker_name", "worker");
  const descriptive = stringValue(args, "skill", "summary", "node_key", "statement");
  const value = stringValue(args, "report_id", "report_ref", "draft_id", "source_ref", "check_ref", "candidate_ref", "assignment_ref");
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
  const name = operationName(input);
  const title = String(input.title ?? input.name ?? "").trim();
  const nativeKind = String(input.toolKind ?? "").trim().toLowerCase();
  const cortexMcpIdentity = isCortexMcpName(input.name) || record(input.input).server === "cortex" || record(input.rawInput).server === "cortex";
  const internalCortexPath = accessesInternalCortex(input.input) || accessesInternalCortex(input.rawInput);
  const native = title.match(/^(read|view|cat)\b/i) || nativeKind === "read" ? NATIVE_READ
    : title.match(/^(edit|patch|write|replace|modify)\b/i) ? NATIVE_EDIT
      : nativeKind === "edit" ? NATIVE_EDIT
        : title.match(/^(shell|bash|terminal|execute|run(?:\s+command)?)\b/i) || nativeKind === "execute" || nativeKind === "terminal" ? NATIVE_SHELL
          : title.match(/^skill\b/i) || nativeKind === "skill" ? NATIVE_SKILL
            : title.match(/^(governance|decide)\b/i) || nativeKind === "governance" ? NATIVE_GOVERNANCE
            : undefined;
  const mapped = ACTIONS[name] ?? native;
  const cortexOperation = ACTIONS[name] !== undefined && (isCortexMcpName(input.name) || record(input.input).server === "cortex" || record(input.rawInput).server === "cortex");
  if (!mapped) {
    const action = humanize(name || title || "tool");
    return { kind: "generic", icon: "Wrench", action, target: null, label: action, expandable: false, systemOperation: false, visible: !cortexMcpIdentity && !internalCortexPath };
  }
  let target = targetFor(name, input, mapped.action);
  if (!target && native) {
    const suffix = title.replace(/^(read|view|cat|edit|patch|write|replace|modify|shell|bash|terminal|execute|run(?:\s+command)?|skill|governance|decide)\b\s*(?:[—:-]\s*)?/i, "").trim();
    if (suffix && suffix.toLowerCase() !== mapped.action.toLowerCase()) target = compactTarget(suffix);
  }
  // The coordinator's internal context is sometimes projected by the host as
  // a native-looking `Read — capsule` card instead of `read_capsule`.
  const internalCapsuleRead = mapped.kind === "read" && target?.toLowerCase() === "capsule"
    && name !== "read_source" && name !== "read_report";
  const unclassifiedNativeRead = native !== undefined && native.kind === "read" && target === null;
  const nativeKindForPresentation = native?.kind;
  return {
    ...mapped,
    target,
    label: target ? `${mapped.action} — ${target}` : mapped.action,
    expandable: !cortexOperation && native !== undefined && nativeKindForPresentation !== "read" && nativeKindForPresentation !== "skill",
    systemOperation: cortexOperation || native === undefined || nativeKindForPresentation === "read" || nativeKindForPresentation === "skill",
    visible: (mapped as { visible?: boolean }).visible !== false && !cortexMcpIdentity && !cortexOperation && !internalCapsuleRead && !internalCortexPath && !unclassifiedNativeRead,
  };
}

// Short aliases make the shared boundary easy to consume from either panel.
export const toolPresentation = presentToolSemantics;
export const semanticToolLabel = presentToolSemantics;

// Keep this catalogue tied to the two advertised contract tables.  ``ask_user``
// remains an accepted alias for history from an older host, but is not a
// currently advertised Cortex MCP operation.
export const KNOWN_CORTEX_TOOL_NAMES = Object.freeze([
  "create_task", "set_governance", "create_draft", "read_draft", "write_report",
  "list_reports", "read_report", "delegate", "list_events", "clarify", "cancel",
  "decide", "integrate", "update_plan", "read_plan", "prepare_candidate",
  "request_check", "read_check", "assess_check", "list_sources", "read_source",
  "read_skill", "read_capsule", "read_large_output", "read_diagnostics",
  "ask_question", "publish_result",
] as const);
