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

/** Format the exact retained value, without selecting fields or unwrapping MCP. */
export function toolDetailText(value: unknown): string | null {
  if (value == null) return null;
  if (typeof value === "string") return value;
  try { return JSON.stringify(value, null, 2) ?? null; } catch { return null; }
}

/** Bound both width-independent text size and the number of initial lines. */
export function toolDetailPreview(text: string, expanded: boolean) {
  let end = Math.min(1000, text.length);
  let newline = -1;
  for (let line = 0; line < 24; line++) {
    newline = text.indexOf("\n", newline + 1);
    if (newline < 0 || newline >= end) break;
    if (line === 23) end = newline;
  }
  // Don't split a supplementary character at the preview boundary.
  if (end < text.length && /[\uD800-\uDBFF]/.test(text.charAt(end - 1))) end--;
  return { text: expanded ? text : text.slice(0, end), canExpand: end < text.length };
}
