import { gatewayPresentation } from "../shared/gateway-presentation.ts";
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
function nativeInput(value: unknown): Record<string, unknown> {
  const envelope = fields(value);
  // codex-acp forwards custom tools as { name, input }, while Claude sends
  // the command fields directly. Only unwrap the documented custom-tool
  // envelope so arbitrary tool arguments keep their original meaning.
  return typeof envelope.name === "string" && Object.keys(fields(envelope.input)).length > 0
    ? fields(envelope.input)
    : envelope;
}
function outputText(value: unknown): string | null {
  if (typeof value === "string") return value;
  const output = fields(value);
  const direct = stringField(output, "content", "output", "text", "formatted_output", "stdout");
  if (direct !== null) return direct;
  return stringField(fields(output.output), "content", "output", "text", "formatted_output", "stdout");
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
  const gateway = gatewayPresentation(event.title ?? "", event.raw_input ?? event.input, event.raw_output ?? event.output);
  const rawInput = gateway?.input ?? event.raw_input ?? event.input;
  const input = nativeInput(rawInput);
  const path = stringField(input, "file_path", "filePath", "path", "absolutePath");
  const command = stringField(input, "command", "cmd");
  const oldText = stringField(input, "old_string", "oldString", "oldText");
  const newText = stringField(input, "new_string", "newString", "newText", "content");
  const outputValue = gateway?.output ?? event.raw_output ?? event.output;
  const output = outputText(outputValue) ?? contentText(event.content);
  const kind = gateway?.kind === "shell" ? "execute" : gateway?.kind ?? event.tool_kind?.toLowerCase();
  const title = event.title ?? "";
  const read = kind === "read" || (kind == null && path !== null && /^Read(?:\s|$)/i.test(title));
  // As in the chat, a row is a shell only with a command: external MCP tools
  // also arrive as `execute`, and they are ordinary tools.
  const shell = command !== null && (kind === "execute" || kind === "terminal" || kind == null);
  const edit = kind === "edit" || (kind == null && path !== null && oldText !== null && newText !== null);
  if (read) return { kind: "read", label: /^Read(?:\s|$)/i.test(title) ? title : `Read ${basename(path) ?? "file"}`, path, command, output, oldText, newText };
  if (shell) return { kind: "shell", label: command ? `Shell  ${command}` : title || "Shell", path, command, output, oldText, newText };
  if (edit) return { kind: "edit", label: /^Edit(?:\s|$)/i.test(title) ? title : `Edit ${basename(path) ?? "file"}`, path, command, output, oldText, newText };
  const raw = title || event.tool_kind || event.type;
  const name = raw.startsWith("mcp__") ? raw.replace(/^mcp__[^_]+__/, "").replace(/^mcp__/, "").replace(/_/g, " ").trim() : raw;
  return { kind: "generic", label: name ? name.charAt(0).toUpperCase() + name.slice(1) : raw, path, command, output, oldText, newText };
}
