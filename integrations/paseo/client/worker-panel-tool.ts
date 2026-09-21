import type { WorkerEvent } from "./worker-panel-events.js";

export type ToolPresentation = {
  kind: "read" | "shell" | "edit" | "generic";
  label: string;
  path: string | null;
  command: string | null;
  output: string | null;
  oldText: string | null;
  newText: string | null;
};

function fields(value: unknown): Record<string, unknown> {
  return value && typeof value === "object" && !Array.isArray(value) ? value as Record<string, unknown> : {};
}
function stringField(value: Record<string, unknown>, ...names: string[]): string | null {
  for (const name of names) if (typeof value[name] === "string") return value[name] as string;
  return null;
}
function basename(path: string | null): string | null {
  return path?.split(/[\\/]/).filter(Boolean).pop() ?? null;
}
function contentText(content: unknown): string | null {
  if (!Array.isArray(content)) return null;
  for (const entry of content) {
    const block = fields(entry);
    const nested = fields(block.content);
    const text = stringField(nested, "text") ?? stringField(block, "text");
    if (text != null) return text.replace(/^```[^\n]*\n/, "").replace(/\n```\s*$/, "");
  }
  return null;
}

/** A visual projection of fields actually retained in the ACP event. */
export function presentTool(event: WorkerEvent): ToolPresentation {
  const input = fields(event.raw_input ?? event.input);
  const path = stringField(input, "file_path", "path");
  const command = stringField(input, "command");
  const oldText = stringField(input, "old_string", "oldText");
  const newText = stringField(input, "new_string", "newText");
  const outputValue = event.raw_output ?? event.output;
  const output = typeof outputValue === "string" ? outputValue : contentText(event.content);
  const kind = event.tool_kind?.toLowerCase();
  const title = event.title ?? "";
  const read = kind === "read" || (kind == null && path !== null && /^Read(?:\s|$)/i.test(title));
  const shell = kind === "execute" || kind === "terminal" || (kind == null && command !== null);
  const edit = kind === "edit" || (kind == null && path !== null && oldText !== null && newText !== null);
  if (read) return { kind: "read", label: /^Read(?:\s|$)/i.test(title) ? title : `Read ${basename(path) ?? "file"}`, path, command, output, oldText, newText };
  if (shell) return { kind: "shell", label: command ? `Shell  ${command}` : title || "Shell", path, command, output, oldText, newText };
  if (edit) return { kind: "edit", label: /^Edit(?:\s|$)/i.test(title) ? title : `Edit ${basename(path) ?? "file"}`, path, command, output, oldText, newText };
  const raw = title || event.tool_kind || event.type;
  const name = raw.startsWith("mcp__") ? raw.replace(/^mcp__[^_]+__/, "").replace(/^mcp__/, "").replace(/_/g, " ").trim() : raw;
  return { kind: "generic", label: name ? name.charAt(0).toUpperCase() + name.slice(1) : raw, path, command, output, oldText, newText };
}
