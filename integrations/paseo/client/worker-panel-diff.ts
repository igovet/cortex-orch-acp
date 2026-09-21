export type DiffBlock = { path: string; oldText: string | null; newText: string | null };
export type DiffRow = { kind: "context" | "add" | "delete"; text: string };

export function rawText(value: unknown): string | null {
  if (value == null) return null;
  if (typeof value === "string") return value;
  try { return JSON.stringify(value, null, 2); } catch { return String(value); }
}

export function diffBlocks(value: unknown): DiffBlock[] {
  if (!Array.isArray(value)) return [];
  return value.flatMap((item) => {
    if (typeof item !== "object" || item === null) return [];
    const block = item as Record<string, unknown>;
    if (block.type !== "diff" || typeof block.path !== "string" || (block.oldText !== null && typeof block.oldText !== "string") || (block.newText !== null && typeof block.newText !== "string")) return [];
    return [{ path: block.path, oldText: block.oldText, newText: block.newText }];
  });
}

export function diffLines(oldText: string | null, newText: string | null): DiffRow[] {
  const split = (text: string | null) => {
    if (text == null || text.length === 0) return [];
    const lines = text.replace(/\r\n?/g, "\n").split("\n");
    return lines[lines.length - 1] === "" ? lines.slice(0, -1) : lines;
  };
  const oldLines = split(oldText);
  const newLines = split(newText);
  if (oldLines.length === 0) return newLines.map((text) => ({ kind: "add", text }));
  if (newLines.length === 0) return oldLines.map((text) => ({ kind: "delete", text }));
  if (oldLines.length * newLines.length > 200_000) {
    return [...oldLines.map((text) => ({ kind: "delete" as const, text })), ...newLines.map((text) => ({ kind: "add" as const, text }))];
  }
  const table = Array.from({ length: oldLines.length + 1 }, () => new Uint32Array(newLines.length + 1));
  for (let i = oldLines.length - 1; i >= 0; i -= 1) for (let j = newLines.length - 1; j >= 0; j -= 1) {
    table[i][j] = oldLines[i] === newLines[j] ? table[i + 1][j + 1] + 1 : Math.max(table[i + 1][j], table[i][j + 1]);
  }
  const rows: DiffRow[] = [];
  let i = 0; let j = 0;
  while (i < oldLines.length && j < newLines.length) {
    if (oldLines[i] === newLines[j]) { rows.push({ kind: "context", text: oldLines[i] }); i += 1; j += 1; }
    else if (table[i + 1][j] >= table[i][j + 1]) { rows.push({ kind: "delete", text: oldLines[i] }); i += 1; }
    else { rows.push({ kind: "add", text: newLines[j] }); j += 1; }
  }
  while (i < oldLines.length) rows.push({ kind: "delete", text: oldLines[i++] });
  while (j < newLines.length) rows.push({ kind: "add", text: newLines[j++] });
  return rows;
}
