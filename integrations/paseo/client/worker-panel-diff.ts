export type DiffBlock = { path: string; oldText: string | null; newText: string | null };
export type DiffSegment = { text: string; changed: boolean };
export type DiffRow = { kind: "context" | "add" | "delete"; text: string; segments?: DiffSegment[] };

export function rawText(value: unknown): string | null {
  if (value == null) return null;
  if (Array.isArray(value) && value.length === 0) return null;
  if (typeof value === "object" && !Array.isArray(value) && Object.keys(value as object).length === 0) return null;
  if (typeof value === "string") return value;
  try { return JSON.stringify(value, null, 2); } catch { return String(value); }
}

export function diffBlocks(value: unknown): DiffBlock[] {
  // ACP content is normally an array of diff blocks. Some adapters wrap the
  // same retained blocks in a `content` array; unwrap that transport envelope
  // only, and require both text fields to be present so no diff is inferred.
  if (Array.isArray(value)) return value.flatMap((item) => diffBlocks(item));
  if (typeof value !== "object" || value === null) return [];
  const block = value as Record<string, unknown>;
  if (block.type === "diff" && typeof block.path === "string"
      && (block.oldText === null || typeof block.oldText === "string")
      && (block.newText === null || typeof block.newText === "string")) {
    return [{ path: block.path, oldText: block.oldText, newText: block.newText }];
  }
  return Array.isArray(block.content) ? diffBlocks(block.content) : [];
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
  return withWordSegments(rows);
}

/** Paseo's buildLineDiff: a removed line directly followed by an added one
 * marks the words that changed between them. */
function withWordSegments(rows: DiffRow[]): DiffRow[] {
  for (let index = 0; index < rows.length - 1; index += 1) {
    const [removed, added] = [rows[index], rows[index + 1]];
    if (removed.kind !== "delete" || added.kind !== "add") continue;
    const [oldSegments, newSegments] = wordSegments(removed.text, added.text);
    removed.segments = oldSegments;
    added.segments = newSegments;
  }
  return rows;
}

/** Runs of word characters and runs of everything else, as Paseo splits them. */
function wordTokens(text: string): string[] {
  return text.match(/\w+|\W+/g) ?? [];
}

function wordSegments(oldText: string, newText: string): [DiffSegment[], DiffSegment[]] {
  const a = wordTokens(oldText), b = wordTokens(newText);
  const table = Array.from({ length: a.length + 1 }, () => new Uint32Array(b.length + 1));
  for (let i = a.length - 1; i >= 0; i -= 1) for (let j = b.length - 1; j >= 0; j -= 1) {
    table[i][j] = a[i] === b[j] ? table[i + 1][j + 1] + 1 : Math.max(table[i + 1][j], table[i][j + 1]);
  }
  const keptA = new Set<number>(), keptB = new Set<number>();
  let i = 0, j = 0;
  while (i < a.length && j < b.length) {
    if (a[i] === b[j]) { keptA.add(i++); keptB.add(j++); }
    else if (table[i + 1][j] >= table[i][j + 1]) i += 1;
    else j += 1;
  }
  const merge = (tokens: string[], kept: Set<number>) => tokens.reduce<DiffSegment[]>((segments, text, index) => {
    const changed = !kept.has(index), last = segments[segments.length - 1];
    if (last && last.changed === changed) last.text += text; else segments.push({ text, changed });
    return segments;
  }, []);
  return [merge(a, keptA), merge(b, keptB)];
}
