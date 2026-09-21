/** Presentation only: retain the original ACP payload alongside this projection. */
export function gatewayPresentation(title: string, rawInput: unknown, rawOutput: unknown) {
  const fields = (value: unknown): Record<string, unknown> =>
    value && typeof value === "object" && !Array.isArray(value) ? value as Record<string, unknown> : {};
  const envelope = fields(rawInput);
  const name = envelope.server === "cortex" && typeof envelope.tool === "string"
    ? envelope.tool : title.match(/^(?:mcp\.cortex\.|mcp__cortex__)([a-z_]+)$/)?.[1];
  const kinds: Record<string, "read" | "edit" | "shell"> = {
    project_read: "read", project_write: "edit", draft_write: "edit", command_run: "shell",
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
  const argv = input.argv;
  const command = Array.isArray(argv) && argv.every(item => typeof item === "string")
    ? argv.map(item => /^[a-zA-Z0-9_./:=+-]+$/.test(item) ? item : JSON.stringify(item)).join(" ") : undefined;
  return { name, kind: kinds[name], input: { ...input, ...(command === undefined ? {} : { command }) }, output };
}
