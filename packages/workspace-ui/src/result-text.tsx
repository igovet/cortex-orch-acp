import { createMemo, For, type JSX } from "solid-js";
import { Dynamic } from "solid-js/web";

/** Render supplied Markdown as text nodes and known elements, never executable HTML. */
const unescape = (text: string) => text.replace(/\\([!"#$%&'()*+,\-./:;<=>?@[\]\\^_`{|}~])/g, "$1");
/** Plain snippets share prose unescaping without changing literal code. */
export const resultPreview = (text: string) => text.split(/(```[\s\S]*?```|`[^`]+`)/g).map(part => part.startsWith("`") ? part : decodeEntities(unescape(part))).join("");

export type Block =
  | { kind: "heading"; level: number; text: string }
  | { kind: "text" | "code"; text: string }
  | { kind: "quote"; blocks: Block[] }
  | { kind: "list"; ordered: boolean; start: number; items: Block[][] }
  | { kind: "table"; header: string[]; align: ("left" | "center" | "right" | undefined)[]; rows: string[][] };

/** Pipes inside escaped text or code spans are not column separators. */
function cells(line: string) {
  const parts: string[] = [];
  let cell = "", fence = "";
  for (let i = 0; i < line.length; i++) {
    const c = line[i];
    if (c === "\\" && i + 1 < line.length) { cell += c + line[++i]; continue; }
    if (c === "`") {
      const run = line.slice(i).match(/^`+/)![0];
      if (!fence) fence = run;
      else if (fence === run) fence = "";
      cell += run; i += run.length - 1; continue;
    }
    if (c === "|" && !fence) { parts.push(cell.trim()); cell = ""; }
    else cell += c;
  }
  parts.push(cell.trim());
  if (parts[0] === "") parts.shift();
  if (parts.at(-1) === "") parts.pop();
  return parts;
}
/** GFM: a delimiter cell is one or more hyphens with an optional colon at either end; the colons set the column's alignment. */
const DELIMITER = /^:?-+:?$/;
const alignOf = (cell: string) => cell.startsWith(":") ? (cell.endsWith(":") ? "center" : "left") : cell.endsWith(":") ? "right" : undefined;
/** A table row is any line up to a blank line or the start of another block; a row may hold no pipe at all (GFM). */
const endsTable = (line: string) => !line.trim() || /^\s*(`{3,}|~{3,})/.test(line) || /^\s{0,3}#{1,6}\s/.test(line) || /^>\s?/.test(line) || (!line.includes("|") && !!listItem(line));
const listItem = (line: string) => line.match(/^(\s*)([-+*]|\d+[.)])\s+(.*)$/);
/** Owner and plan text means its line breaks; a report's wrapped lines are one paragraph. */
function parse(text: string, breaks = false): Block[] {
  const lines = text.replace(/\r\n?/g, "\n").split("\n"), out: Block[] = [];
  for (let i = 0; i < lines.length;) {
    const line = lines[i];
    if (!line.trim()) { i++; continue; }
    const fence = line.match(/^\s*(`{3,}|~{3,})/);
    if (fence) {
      const code: string[] = [], marker = fence[1];
      i++;
      while (i < lines.length && !(lines[i].trim().startsWith(marker) && new RegExp("^" + marker[0] + "+$").test(lines[i].trim()))) code.push(lines[i++]);
      i++; out.push({ kind: "code", text: code.join("\n") }); continue;
    }
    const heading = line.match(/^(#{1,6})\s+(.+?)(?:\s+#+)?$/);
    if (heading) { out.push({ kind: "heading", level: heading[1].length, text: heading[2] }); i++; continue; }
    if (/^>\s?/.test(line)) {
      const quote: string[] = [];
      while (i < lines.length && /^>\s?/.test(lines[i])) quote.push(lines[i++].replace(/^>\s?/, ""));
      out.push({ kind: "quote", blocks: parse(quote.join("\n"), breaks) }); continue;
    }
    const first = listItem(line);
    if (first) {
      const ordered = /^\d/.test(first[2]), indent = first[1].length, items: Block[][] = [];
      while (i < lines.length) {
        const item = listItem(lines[i]);
        if (!item || item[1].length !== indent || /^\d/.test(item[2]) !== ordered) break;
        const body = [item[3]]; i++;
        while (i < lines.length && lines[i].trim() && /^\s+/.test(lines[i]) && (listItem(lines[i])?.[1].length ?? lines[i].search(/\S/)) > indent) {
          body.push(lines[i++].slice(indent + 2));
        }
        items.push(parse(body.join("\n"), breaks));
      }
      out.push({ kind: "list", ordered, start: ordered ? parseInt(first[2]) : 1, items }); continue;
    }
    const header = cells(line), separator = i + 1 < lines.length ? cells(lines[i + 1]) : [];
    if (line.includes("|") && separator.length === header.length && separator.every(cell => DELIMITER.test(cell))) {
      i += 2; const rows: string[][] = [];
      // Rows are padded or cut to the header's width; an empty cell is a cell.
      while (i < lines.length && !endsTable(lines[i])) {
        const row = cells(lines[i++]); rows.push(header.map((_, n) => row[n] ?? ""));
      }
      out.push({ kind: "table", header, align: separator.map(alignOf), rows }); continue;
    }
    const previous = out.at(-1);
    if (previous?.kind === "text" && i > 0 && lines[i - 1].trim()) previous.text += (breaks ? "\n" : " ") + line;
    else out.push({ kind: "text", text: line });
    i++;
  }
  return out;
}
/** Named character references a model writes in Markdown; any other name stays as written, as CommonMark leaves unknown ones. */
const NAMED: Record<string, string> = { amp: "&", lt: "<", gt: ">", quot: "\"", apos: "'", nbsp: "\u00a0", ensp: "\u2002", emsp: "\u2003", thinsp: "\u2009",
  copy: "\u00a9", reg: "\u00ae", trade: "\u2122", hellip: "\u2026", mdash: "\u2014", ndash: "\u2013", laquo: "\u00ab", raquo: "\u00bb", ldquo: "\u201c", rdquo: "\u201d",
  lsquo: "\u2018", rsquo: "\u2019", bdquo: "\u201e", times: "\u00d7", middot: "\u00b7", bull: "\u2022", deg: "\u00b0", euro: "\u20ac", larr: "\u2190", rarr: "\u2192",
  uarr: "\u2191", darr: "\u2193", le: "\u2264", ge: "\u2265", ne: "\u2260", minus: "\u2212", plusmn: "\u00b1", shy: "\u00ad", zwj: "\u200d", zwnj: "\u200c" };
/**
 * Character references (`&amp;`, `&#32;`, `&#x20;`) as the characters they stand for, the way CommonMark reads them in text:
 * the result is text, never markup. A code point that is not a character reads as U+FFFD. Code keeps them as written.
 */
export const decodeEntities = (text: string) => text.replace(/&(?:#([0-9]{1,7})|#[xX]([0-9a-fA-F]{1,6})|([A-Za-z][A-Za-z0-9]{1,31}));/g, (whole, dec: string | undefined, hex: string | undefined, name: string | undefined) => {
  if (name !== undefined) return NAMED[name] ?? whole;
  const code = dec !== undefined ? parseInt(dec, 10) : parseInt(hex!, 16);
  return code === 0 || code > 0x10ffff || (code >= 0xd800 && code <= 0xdfff) ? "\ufffd" : String.fromCodePoint(code);
});
const safeHref = (raw: string) => {
  const url = decodeEntities(unescape(raw)).trim();
  // Relative documentation links are fine; unknown/executable schemes are text only.
  return !/[\u0000-\u0020\u007f\\]/.test(url) && (!/^[a-z][a-z\d+.-]*:/i.test(url) || /^(https?:|mailto:)/i.test(url)) ? url : undefined;
};
/** An image reference names a picture; nothing is loaded from it (the window loads no remote content). */
function imageRef(alt: string, raw: string): JSX.Element {
  const href = safeHref(raw), target = decodeEntities(unescape(raw)).trim();
  const place = href && /^https?:/i.test(href) ? (() => { try { const u = new URL(href); return `${u.host}${u.pathname.split("/").filter(Boolean).length ? " · " + u.pathname.split("/").filter(Boolean).pop() : ""}`; } catch { return href; } })() : target.split("/").filter(Boolean).pop() ?? target;
  const label = <>Image: {decodeEntities(unescape(alt)).trim() || place}{alt.trim() && place ? ` (${place})` : ""}</>;
  return href && /^https?:/i.test(href) ? <a class="ws-img-ref" href={href} rel="noreferrer">{label}</a> : <span class="ws-img-ref">{label}</span>;
}
function inline(text: string): JSX.Element {
  const tokens = /\\[!"#$%&'()*+,\-./:;<=>?@[\]\\^_`{|}~]|(`+)([\s\S]*?[^`])\1(?!`)|\*\*(.+?)\*\*|!\[([^\]]*)\]\(([^()\s]+)\)|\[([^\]]+)\]\(([^()\s]+)\)|\n/g;
  const out: JSX.Element[] = [];
  let at = 0, match: RegExpExecArray | null;
  while ((match = tokens.exec(text))) {
    out.push(decodeEntities(text.slice(at, match.index)));
    if (match[0] === "\n") out.push(<br />);
    else if (match[0].startsWith("\\")) out.push(match[0].slice(1));
    else if (match[1]) out.push(<code>{match[2]}</code>);
    else if (match[3]) out.push(<strong>{inline(match[3])}</strong>);
    else if (match[5] !== undefined) out.push(imageRef(match[4], match[5]));
    else {
      const href = safeHref(match[7]);
      out.push(href ? <a href={href} rel="noreferrer">{inline(match[6])}</a> : inline(match[6]));
    }
    at = tokens.lastIndex;
  }
  out.push(decodeEntities(text.slice(at)));
  return out;
}
function Blocks(props: { blocks: Block[] }) {
  return <For each={props.blocks}>{b =>
    b.kind === "heading" ? <Dynamic component={("h" + b.level) as keyof JSX.IntrinsicElements}>{inline(b.text)}</Dynamic>
      : b.kind === "code" ? <pre><code>{b.text}</code></pre>
      : b.kind === "quote" ? <blockquote><Blocks blocks={b.blocks} /></blockquote>
      : b.kind === "list" ? <Dynamic component={b.ordered ? "ol" : "ul"} start={b.ordered ? b.start : undefined}><For each={b.items}>{item => <li><Blocks blocks={item} /></li>}</For></Dynamic>
      : b.kind === "table" ? <div class="ws-result-table"><table><thead><tr><For each={b.header}>{(cell, i) => <th scope="col" style={{ "text-align": b.align[i()] }}>{inline(cell)}</th>}</For></tr></thead><tbody><For each={b.rows}>{row => <tr><For each={row}>{(cell, n) => <td style={{ "text-align": b.align[n()] }}>{inline(cell)}</td>}</For></tr>}</For></tbody></table></div>
      : <p>{inline(b.text)}</p>
  }</For>;
}
/**
 * A re-parse keeps the previous block object for every block whose content is unchanged, so the keyed list below keeps
 * that block's DOM: a refreshed document updates in place, and the reader's scroll position stays where it was.
 */
export function reuseBlocks(next: Block[], previous: Block[]): Block[] {
  const pool = new Map<string, Block[]>();
  for (const b of previous) { const key = JSON.stringify(b), list = pool.get(key); if (list) list.push(b); else pool.set(key, [b]); }
  return next.map(b => pool.get(JSON.stringify(b))?.shift() ?? b);
}
export function ResultText(props: { text?: string; breaks?: boolean }) {
  const text = createMemo(() => props.text ?? "");
  const blocks = createMemo<Block[]>(previous => reuseBlocks(parse(text(), props.breaks), previous), []);
  return <div class="ws-result-copy"><Blocks blocks={blocks()} /></div>;
}
