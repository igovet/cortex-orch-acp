/**
 * The one reading of Markdown that every surface shares. `parse` turns text into blocks for the conversation, results and
 * reports; `paintMarkdown` reads the very same text with the very same scanner and inline tokens, but keeps every character
 * where it was written, so the writing area can show the formatting under the text the owner types without changing it.
 * Nothing here builds HTML: callers render text nodes and known elements only.
 */

export type Align = "left" | "center" | "right" | undefined;
export type Block =
  | { kind: "heading"; level: number; text: string }
  | { kind: "text"; text: string }
  | { kind: "code"; text: string; lang?: string }
  | { kind: "hr" }
  | { kind: "quote"; blocks: Block[] }
  | { kind: "list"; ordered: boolean; start: number; items: Block[][] }
  | { kind: "table"; header: string[]; align: Align[]; rows: string[][] };

/** One source line, or what is left of it once a quote or list marker was taken off: `col` is where `text` starts in line `n`. */
type Src = { text: string; n: number; col: number };

/** What the scanner says about the source as it reads it; the conversation passes none and the writing area records it. */
interface Painter {
  line(n: number, cls: string): void;
  mark(n: number, from: number, to: number, cls: string): void;
  box(n: number, kind: "code" | "quote", id: number): void;
  id(): number;
}

type Cell = { text: string; start: number };
/** Pipes inside escaped text or code spans are not column separators. Each cell keeps where its trimmed text starts. */
function cells(line: string): Cell[] {
  const parts: Cell[] = [];
  let from = 0, fence = "";
  const cut = (to: number) => { const raw = line.slice(from, to), text = raw.trim(); parts.push({ text, start: from + (raw.length - raw.trimStart().length) }); };
  for (let i = 0; i < line.length; i++) {
    const c = line[i];
    if (c === "\\" && i + 1 < line.length) { i++; continue; }
    if (c === "`") {
      const run = line.slice(i).match(/^`+/)![0];
      if (!fence) fence = run;
      else if (fence === run) fence = "";
      i += run.length - 1; continue;
    }
    if (c === "|" && !fence) { cut(i); from = i + 1; }
  }
  cut(line.length);
  if (parts[0]?.text === "") parts.shift();
  if (parts.at(-1)?.text === "") parts.pop();
  return parts;
}
/** GFM: a delimiter cell is one or more hyphens with an optional colon at either end; the colons set the column's alignment. */
const DELIMITER = /^:?-+:?$/;
const alignOf = (cell: string): Align => cell.startsWith(":") ? (cell.endsWith(":") ? "center" : "left") : cell.endsWith(":") ? "right" : undefined;
const listItem = (line: string) => line.match(/^(\s*)([-+*]|\d+[.)])\s+(.*)$/);
const FENCE = /^\s*(`{3,}|~{3,})/;
/** A thematic break: three or more of one of `-`, `*` or `_`, spaces allowed between them. */
const RULE = /^ {0,3}([-*_])(?:[ \t]*\1){2,}[ \t]*$/;
/** A table row is any line up to a blank line or the start of another block; a row may hold no pipe at all (GFM). */
const endsTable = (line: string) => !line.trim() || FENCE.test(line) || /^\s{0,3}#{1,6}\s/.test(line) || /^>\s?/.test(line) || (!line.includes("|") && !!listItem(line));

function scan(lines: Src[], breaks: boolean, p?: Painter): Block[] {
  const out: Block[] = [], paras: Src[][] = [];
  for (let i = 0; i < lines.length;) {
    const src = lines[i], line = src.text;
    if (!line.trim()) { i++; continue; }
    const fence = line.match(FENCE);
    if (fence) {
      const code: string[] = [], marker = fence[1], closing = new RegExp("^" + marker[0] + "+$"), id = p?.id() ?? 0;
      const info = line.slice(fence[0].length).trim(), lang = info.split(/\s+/)[0];
      const edge = (s: Src) => { p?.line(s.n, "md-code md-fence"); p?.box(s.n, "code", id); p?.mark(s.n, s.col, s.col + s.text.length, "md-mark"); };
      edge(src);
      if (lang) { const at = src.col + line.indexOf(lang, fence[0].length); p?.mark(src.n, at, at + lang.length, "md-lang"); }
      i++;
      while (i < lines.length && !(lines[i].text.trim().startsWith(marker) && closing.test(lines[i].text.trim()))) {
        p?.line(lines[i].n, "md-code md-code-body"); p?.box(lines[i].n, "code", id);
        code.push(lines[i++].text);
      }
      if (i < lines.length) edge(lines[i]);
      i++; out.push(lang ? { kind: "code", text: code.join("\n"), lang } : { kind: "code", text: code.join("\n") }); continue;
    }
    const heading = line.match(/^(#{1,6})\s+(.+?)(?:\s+#+)?$/);
    if (heading) {
      const level = heading[1].length, rest = line.slice(level);
      const start = Math.min(level + rest.length - rest.trimStart().length, line.length - heading[2].length), end = start + heading[2].length;
      p?.line(src.n, `md-h md-h${level}`); p?.mark(src.n, src.col, src.col + start, "md-mark");
      if (end < line.length) p?.mark(src.n, src.col + end, src.col + line.length, "md-mark");
      if (p) paintInline([{ text: heading[2], n: src.n, col: src.col + start }], p);
      out.push({ kind: "heading", level, text: heading[2] }); i++; continue;
    }
    if (/^>\s?/.test(line)) {
      const quote: Src[] = [], id = p?.id() ?? 0;
      while (i < lines.length && /^>\s?/.test(lines[i].text)) {
        const q = lines[i++], prefix = q.text.match(/^>\s?/)![0].length;
        p?.line(q.n, "md-quote"); p?.box(q.n, "quote", id); p?.mark(q.n, q.col, q.col + prefix, "md-mark");
        quote.push({ text: q.text.slice(prefix), n: q.n, col: q.col + prefix });
      }
      out.push({ kind: "quote", blocks: scan(quote, breaks, p) }); continue;
    }
    if (RULE.test(line)) {
      p?.line(src.n, "md-hr"); p?.mark(src.n, src.col, src.col + line.length, "md-mark");
      out.push({ kind: "hr" }); i++; continue;
    }
    const first = listItem(line);
    if (first) {
      const ordered = /^\d/.test(first[2]), indent = first[1].length, items: Block[][] = [];
      while (i < lines.length) {
        const s = lines[i], item = listItem(s.text);
        if (!item || item[1].length !== indent || /^\d/.test(item[2]) !== ordered) break;
        p?.line(s.n, "md-li"); p?.mark(s.n, s.col + indent, s.col + indent + item[2].length, "md-mark md-bullet");
        const body: Src[] = [{ text: item[3], n: s.n, col: s.col + s.text.length - item[3].length }]; i++;
        while (i < lines.length && lines[i].text.trim() && /^\s+/.test(lines[i].text) && (listItem(lines[i].text)?.[1].length ?? lines[i].text.search(/\S/)) > indent) {
          const c = lines[i++];
          body.push({ text: c.text.slice(indent + 2), n: c.n, col: c.col + Math.min(indent + 2, c.text.length) });
        }
        items.push(scan(body, breaks, p));
      }
      out.push({ kind: "list", ordered, start: ordered ? parseInt(first[2]) : 1, items }); continue;
    }
    const header = cells(line), separator = i + 1 < lines.length ? cells(lines[i + 1].text) : [];
    if (line.includes("|") && separator.length === header.length && separator.every(cell => DELIMITER.test(cell.text))) {
      if (p) {
        paintRow(src, header, header.length, p, "md-table md-th");
        const d = lines[i + 1];
        p.line(d.n, "md-table md-delim"); p.mark(d.n, d.col, d.col + d.text.length, "md-mark");
      }
      i += 2; const rows: string[][] = [];
      // Rows are padded or cut to the header's width; an empty cell is a cell.
      while (i < lines.length && !endsTable(lines[i].text)) {
        const s = lines[i++], row = cells(s.text);
        if (p) paintRow(s, row, header.length, p, "md-table");
        rows.push(header.map((_, n) => row[n]?.text ?? ""));
      }
      out.push({ kind: "table", header: header.map(c => c.text), align: separator.map(c => alignOf(c.text)), rows }); continue;
    }
    const previous = out.at(-1);
    if (previous?.kind === "text" && i > 0 && lines[i - 1].text.trim()) { previous.text += (breaks ? "\n" : " ") + line; paras.at(-1)!.push(src); }
    else { out.push({ kind: "text", text: line }); paras.push([src]); }
    i++;
  }
  if (p) for (const para of paras) paintInline(para, p);
  return out;
}

/** Owner and plan text means its line breaks; a report's wrapped lines are one paragraph. */
export function parse(text: string, breaks = false): Block[] {
  return scan(text.split(/\r\n?|\n/).map((line, n) => ({ text: line, n, col: 0 })), breaks);
}

/** Inline tokens with their places in the text they were read from. `a`..`b` is the content; `u`..`v` a link's address. */
export type Token =
  | { t: "text" | "esc" | "br"; s: number; e: number }
  | { t: "code"; s: number; e: number; a: number; b: number }
  | { t: "strong" | "em" | "del"; s: number; e: number; a: number; b: number; kids: Token[] }
  | { t: "image"; s: number; e: number; a: number; b: number; u: number; v: number }
  | { t: "link"; s: number; e: number; a: number; b: number; u: number; v: number; kids: Token[] };

const INLINE = /\\[!"#$%&'()*+,\-./:;<=>?@[\]\\^_`{|}~]|(`+)([\s\S]*?[^`])\1(?!`)|\*\*(.+?)\*\*|~~(?!\s)(.+?)(?<!\s)~~|(?<![*\\])\*(?![\s*])(.+?)(?<![\s\\*])\*(?!\*)|(?<![\p{L}\p{N}_\\])_(?![\s_])(.+?)(?<![\s\\_])_(?![\p{L}\p{N}_])|!\[([^\]]*)\]\(([^()\s]+)\)|\[([^\]]+)\]\(([^()\s]+)\)|\n/gu;
const PUNCT = /[\p{P}\p{S}]/u;
/** CommonMark's flanking rule for one `*` or `_`: a mark beside punctuation opens or closes only where the other side is open too. */
const flanks = (text: string, open: number, close: number) => {
  const outside = (c: string | undefined) => c === undefined || /\s/.test(c) || PUNCT.test(c);
  return (!PUNCT.test(text[open + 1]) || outside(text[open - 1])) && (!PUNCT.test(text[close - 1]) || outside(text[close + 1]));
};

/** Reads `text` from `from` to `to` into inline tokens; places are in `text`, so a nested read keeps them. */
export function tokenize(text: string, from = 0, to = text.length): Token[] {
  const part = text.slice(from, to), re = new RegExp(INLINE.source, INLINE.flags), out: Token[] = [];
  let at = 0, m: RegExpExecArray | null;
  const plain = (s: number, e: number) => { if (e > s) out.push({ t: "text", s: from + s, e: from + e }); };
  while ((m = re.exec(part))) {
    const s = m.index, e = s + m[0].length, inner = (open: number, close: number) => [from + s + open, from + e - close] as const;
    if ((m[5] !== undefined || m[6] !== undefined) && !flanks(part, s, e - 1)) { re.lastIndex = s + 1; continue; }
    plain(at, s);
    if (m[0] === "\n") out.push({ t: "br", s: from + s, e: from + e });
    else if (m[0].startsWith("\\")) out.push({ t: "esc", s: from + s, e: from + e });
    else if (m[1]) { const [a, b] = inner(m[1].length, m[1].length); out.push({ t: "code", s: from + s, e: from + e, a, b }); }
    else if (m[3] !== undefined || m[4] !== undefined) {
      const [a, b] = inner(2, 2);
      out.push({ t: m[3] !== undefined ? "strong" : "del", s: from + s, e: from + e, a, b, kids: tokenize(text, a, b) });
    } else if (m[5] !== undefined || m[6] !== undefined) {
      const [a, b] = inner(1, 1);
      out.push({ t: "em", s: from + s, e: from + e, a, b, kids: tokenize(text, a, b) });
    } else if (m[8] !== undefined) {
      const a = from + s + 2, b = a + m[7].length, u = b + 2;
      out.push({ t: "image", s: from + s, e: from + e, a, b, u, v: u + m[8].length });
    } else {
      const a = from + s + 1, b = a + m[9].length, u = b + 2;
      out.push({ t: "link", s: from + s, e: from + e, a, b, u, v: u + m[10].length, kids: tokenize(text, a, b) });
    }
    at = re.lastIndex;
  }
  plain(at, part.length);
  return out;
}

/** The marks inline tokens leave on the source: markers dimmed, their content styled as it reads in the conversation. */
function paintTokens(tokens: Token[], mark: (from: number, to: number, cls: string) => void) {
  for (const k of tokens) {
    if (k.t === "esc") mark(k.s, k.s + 1, "md-mark");
    else if (k.t === "code") { mark(k.s, k.e, "md-code-span"); mark(k.s, k.a, "md-mark"); mark(k.b, k.e, "md-mark"); }
    else if (k.t === "strong" || k.t === "em" || k.t === "del") {
      mark(k.s, k.e, `md-${k.t}`); mark(k.s, k.a, "md-mark"); mark(k.b, k.e, "md-mark"); paintTokens(k.kids, mark);
    } else if (k.t === "image") { mark(k.s, k.e, "md-mark"); mark(k.a, k.b, "md-img"); }
    else if (k.t === "link") {
      mark(k.s, k.a, "md-mark"); mark(k.a, k.b, "md-link"); mark(k.b, k.e, "md-mark"); mark(k.u, k.v, "md-url"); paintTokens(k.kids, mark);
    }
  }
}
/** Tokens are read over the joined content exactly as the conversation reads it, then mapped back to each source line. */
function paintInline(segs: Src[], p: Painter) {
  const starts: number[] = [];
  let length = 0;
  for (const s of segs) { starts.push(length); length += s.text.length + 1; }
  const content = segs.map(s => s.text).join("\n");
  paintTokens(tokenize(content), (from, to, cls) => {
    segs.forEach((s, i) => {
      const a = Math.max(from, starts[i]), b = Math.min(to, starts[i] + s.text.length);
      if (b > a) p.mark(s.n, s.col + a - starts[i], s.col + b - starts[i], cls);
    });
  });
}
function paintRow(src: Src, row: Cell[], width: number, p: Painter, cls: string) {
  p.line(src.n, cls);
  let at = 0;
  row.forEach((cell, n) => {
    // Separators are what lies between cells; a cell past the header's width is not shown in the conversation.
    p.mark(src.n, src.col + at, src.col + cell.start, "md-mark");
    if (n < width) paintInline([{ text: cell.text, n: src.n, col: src.col + cell.start }], p);
    else p.mark(src.n, src.col + cell.start, src.col + cell.start + cell.text.length, "md-mark");
    at = cell.start + cell.text.length;
  });
  p.mark(src.n, src.col + at, src.col + src.text.length, "md-mark");
}

/** A run of one source line that reads the same way; `cls` is empty for plain text. */
export type PaintRun = { text: string; cls: string };
/** One source line of the writing area: its block's classes, the box it sits in (a code block or a quote), and its runs. */
export type PaintLine = { cls: string; box: string; runs: PaintRun[] };

/**
 * The writing area's reading of its own text, line by line. The lines' runs join back to exactly the source text with its
 * line breaks, so the layer drawn under the field holds the same characters in the same places as the field itself.
 */
export function paintMarkdown(text: string, breaks = true): PaintLine[] {
  const lines = text.split(/\r\n?|\n/);
  const lineCls = lines.map(() => [] as string[]), boxes = lines.map(() => ""), marks = lines.map(line => new Array<string>(line.length).fill(""));
  let ids = 0;
  scan(lines.map((line, n) => ({ text: line, n, col: 0 })), breaks, {
    line: (n, cls) => { lineCls[n].push(cls); },
    mark: (n, from, to, cls) => { const row = marks[n]; for (let c = Math.max(0, from); c < Math.min(to, row.length); c++) row[c] = row[c] ? `${row[c]} ${cls}` : cls; },
    box: (n, kind, id) => { if (!boxes[n]) boxes[n] = `${kind}:${id}`; },
    id: () => ++ids,
  });
  return lines.map((line, n) => {
    const runs: PaintRun[] = [], row = marks[n];
    for (let c = 0; c < line.length;) {
      let e = c + 1;
      while (e < line.length && row[e] === row[c]) e++;
      runs.push({ text: line.slice(c, e), cls: [...new Set(row[c].split(" ").filter(Boolean))].join(" ") });
      c = e;
    }
    return { cls: [...new Set(lineCls[n].join(" ").split(" ").filter(Boolean))].join(" "), box: boxes[n], runs };
  });
}

/** The block kinds a reading holds, in document order, nested blocks included: what the writing area and the conversation must agree on. */
export function blockKinds(blocks: Block[]): string[] {
  return blocks.flatMap(b => b.kind === "quote" ? ["quote", ...blockKinds(b.blocks)]
    : b.kind === "list" ? ["list", ...b.items.flatMap(blockKinds)]
    : b.kind === "heading" ? [`h${b.level}`] : [b.kind]);
}
