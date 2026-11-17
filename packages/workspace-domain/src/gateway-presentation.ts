const fields = (value: unknown): Record<string, unknown> =>
  value && typeof value === "object" && !Array.isArray(value) ? value as Record<string, unknown> : {};

/** Legacy OpenCode Code Mode snapshot, using the same naming rules as
 * engine/opencode_tool_calls.py. Never evaluate JS or recover absent ACP content. */
export function codeModePresentation(title: string, rawInput: unknown, rawOutput: unknown, kind?: unknown) {
  const input = fields(rawInput);
  if (title.trim() !== "execute" || (kind != null && kind !== "other")
    || "command" in input || "cmd" in input || "argv" in input) return null;
  const metadata = fields(fields(rawOutput).metadata);
  const calls = Array.isArray(metadata.toolCalls)
    ? metadata.toolCalls.map(fields).filter(call => typeof call.tool === "string" && call.tool.trim())
    : [];
  // The ACP bridge drops kind. A title-only pending snapshot is ambiguous
  // with a native shell; wait for code or inner calls rather than relabel it.
  if (kind !== "other" && typeof input.code !== "string" && calls.length === 0) return null;
  const operation = (name: string): [string | null, string] => {
    const dot = name.indexOf(".");
    return dot > 0 && dot < name.length - 1 ? [name.slice(0, dot), name.slice(dot + 1)] : [null, name];
  };
  const primary = calls.find(call => operation((call.tool as string).trim())[0]) ?? calls[0];
  const code = typeof input.code === "string" ? input.code : undefined;
  const match = code?.match(/\btools\s*\.\s*([A-Za-z_$][\w$]*)\s*\.\s*([A-Za-z_$][\w$]*)\s*\(/);
  const resolved = primary ? operation((primary.tool as string).trim())
    : match ? [match[1], match[2]] : null;
  const name = resolved ? resolved[0] ? `mcp.${resolved[0]}.${resolved[1]}` : resolved[1] : "Code Mode";
  const envelope = resolved ? {
    ...(resolved[0] ? { server: resolved[0] } : {}),
    tool: resolved[1],
    ...(primary ? { arguments: fields(primary.input) } : {}),
    ...(code === undefined ? {} : { code }),
  } : input;
  return {
    name, input: envelope, output: rawOutput,
    failed: metadata.error === true || calls.some(call => call.status === "error"),
  };
}

/** Presentation only: retain the original ACP payload alongside this projection. */
export function gatewayPresentation(title: string, rawInput: unknown, rawOutput: unknown) {
  const codeMode = codeModePresentation(title, rawInput, rawOutput);
  title = codeMode?.name ?? title;
  rawInput = codeMode?.input ?? rawInput;
  const envelope = fields(rawInput);
  const name = envelope.server === "cortex" && typeof envelope.tool === "string"
    ? envelope.tool : title.match(/^(?:mcp\.cortex\.|mcp__cortex__)([a-z_]+)$/)?.[1];
  const kinds: Record<string, "read" | "edit" | "shell"> = {
    project_read: "read", project_edit: "edit", project_write: "edit", draft_write: "edit", command_run: "shell",
    outside_command: "shell",
  };
  if (!name || !kinds[name]) return null;
  const input = envelope.server === "cortex" ? fields(envelope.arguments) : envelope;
  const result = fields(fields(rawOutput).result);
  let output: unknown = rawOutput;
  if (typeof output === "string") { try { output = JSON.parse(output); } catch { /* Plain output remains literal. */ } }
  if (result.structuredContent) output = result.structuredContent;
  else if (Array.isArray(result.content)) {
    const text = fields(result.content[0]).text;
    if (typeof text === "string") { try { output = JSON.parse(text); } catch { output = text; } }
  }
  // Only a retained successful edit result carries a committed diff. Do not
  // fabricate a diff from proposed replacements on a pending/failed call.
  const edit = fields(output);
  const projected: Record<string, unknown> = name === "project_edit" && typeof edit.diff === "string"
    ? { ...input, unifiedDiff: edit.diff,
        ...(typeof edit.oldText === "string" && typeof edit.newText === "string"
          ? { oldText: edit.oldText, newText: edit.newText } : {}) }
    : input;
  if (name === "project_edit" && typeof edit.diff === "string") output = { ...edit, content: edit.diff };
  const argv = input.argv;
  const command = Array.isArray(argv) && argv.every(item => typeof item === "string")
    ? argv.map(item => /^[a-zA-Z0-9_./:=+-]+$/.test(item) ? item : JSON.stringify(item)).join(" ") : undefined;
  const projectedInput: Record<string, unknown> = { ...projected, ...(command === undefined ? {} : { command }) };
  return { name, kind: kinds[name], input: projectedInput, output };
}
