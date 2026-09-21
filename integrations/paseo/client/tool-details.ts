/** Extract human-facing text, never stringify an opaque protocol object. */
export function readableToolText(value: unknown, depth = 0): string | null {
  if (depth > 5 || value == null) return null;
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
  for (const key of ["text", "message", "content", "output", "result", "structuredContent"]) {
    const text = readableToolText(data[key], depth + 1);
    if (text) return text;
  }
  return null;
}
