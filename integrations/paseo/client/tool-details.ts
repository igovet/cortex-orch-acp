/** Extract human-facing text; never stringify an opaque protocol object (see toolDetailText for that). */
export function readableToolText(value: unknown, depth = 0): string | null {
  if (depth > 8 || value == null) return null;
  if (typeof value === "string") {
    if (!value.trim()) return null;
    if (/^\s*[{[]/.test(value)) {
      try { return readableToolText(JSON.parse(value), depth + 1); } catch { return null; }
    }
    return value;
  }
  if (Array.isArray(value)) {
    const parts = value.map(v => readableToolText(v, depth + 1)).filter((v): v is string => v !== null);
    return parts.length ? parts.join("\n") : null;
  }
  if (typeof value !== "object") return null;
  const data = value as Record<string, unknown>;
  const streams = [data.stdout, data.stderr].filter((v): v is string => typeof v === "string" && v.length > 0);
  if (streams.length) return streams.join("\n");
  // `data` is a common MCP result envelope (e.g. GitKraken's {"data":{"output":…}}).
  for (const key of ["text", "message", "content", "output", "result", "structuredContent", "data"]) {
    const text = readableToolText(data[key], depth + 1);
    if (text) return text;
  }
  return null;
}

function meaningful(value: unknown): boolean {
  if (value == null || value === "") return false;
  if (Array.isArray(value)) return value.length > 0;
  return typeof value !== "object" || Object.keys(value as object).length > 0;
}

/** An MCP call envelope's own payload: `{result:{structuredContent}}` or the
 * text of `{result:{content:[{text}]}}` (parsed when it is JSON). */
function unwrapMcp(value: unknown): unknown {
  if (typeof value === "string") { try { return unwrapMcp(JSON.parse(value)); } catch { return value; } }
  if (!value || typeof value !== "object" || Array.isArray(value)) return value;
  const result = (value as Record<string, unknown>).result;
  if (!result || typeof result !== "object") return value;
  const { structuredContent, content } = result as Record<string, unknown>;
  if (meaningful(structuredContent)) return structuredContent;
  const texts = Array.isArray(content) ? content.map((item) => (item as Record<string, unknown>)?.text).filter((t) => typeof t === "string") : [];
  if (texts.length === 1) { try { return JSON.parse(texts[0] as string); } catch { return texts[0]; } }
  return texts.length ? texts.join("\n") : value;
}

/** A tool's detail section text, as the chat's Input/Output sections show it:
 * readable text when there is some, otherwise the value as formatted JSON.
 * Null only when there is nothing at all. */
export function toolDetailText(value: unknown): string | null {
  const readable = readableToolText(value);
  if (readable) return readable;
  const payload = unwrapMcp(value);
  if (!meaningful(payload)) return null;
  if (typeof payload === "string") return payload;
  try { return JSON.stringify(payload, null, 2); } catch { return String(payload); }
}

/** The arguments a tool was called with, without the gateway envelope. */
export function toolArguments(rawInput: unknown): unknown {
  const envelope = rawInput && typeof rawInput === "object" && !Array.isArray(rawInput) ? rawInput as Record<string, unknown> : null;
  return envelope && typeof envelope.server === "string" && typeof envelope.tool === "string" ? envelope.arguments : rawInput;
}
