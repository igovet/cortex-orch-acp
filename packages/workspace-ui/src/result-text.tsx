import { createMemo, For, type JSX } from "solid-js";
import { Dynamic } from "solid-js/web";

/** Render supplied Markdown as text nodes and known elements, never executable HTML. */
const unescape = (text: string) => text.replace(/\\([!"#$%&'()*+,\-./:;<=>?@[\]\\^_`{|}~])/g, "$1");
/** Plain snippets share prose unescaping without changing literal code. */
export const resultPreview = (text: string) => text.split(/(```[\s\S]*?```|`[^`]+`)/g).map(part => part.startsWith("`") ? part : unescape(part)).join("");

type Block =
  | { kind: "heading"; level: number; text: string }
  | { kind: "text" | "code"; text: string }
  | { kind: "quote"; blocks: Block[] }
  | { kind: "list"; ordered: boolean; start: number; items: Block[][] }
  | { kind: "table"; header: string[]; rows: string[][] };

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
    if (line.includes("|") && separator.length === header.length && separator.every(cell => /^:?-{3,}:?$/.test(cell))) {
      i += 2; const rows: string[][] = [];
      while (i < lines.length && lines[i].trim() && lines[i].includes("|")) {
        const row = cells(lines[i++]); rows.push(header.map((_, n) => row[n] ?? ""));
      }
      out.push({ kind: "table", header, rows }); continue;
    }
    const previous = out.at(-1);
    if (previous?.kind === "text" && i > 0 && lines[i - 1].trim()) previous.text += (breaks ? "\n" : " ") + line;
    else out.push({ kind: "text", text: line });
    i++;
  }
  return out;
}
const safeHref = (raw: string) => {
  const url = unescape(raw).trim();
  // Relative documentation links are fine; unknown/executable schemes are text only.
  return !/[\u0000-\u0020\u007f\\]/.test(url) && (!/^[a-z][a-z\d+.-]*:/i.test(url) || /^(https?:|mailto:)/i.test(url)) ? url : undefined;
};
function inline(text: string): JSX.Element {
  const tokens = /\\[!"#$%&'()*+,\-./:;<=>?@[\]\\^_`{|}~]|(`+)([\s\S]*?[^`])\1(?!`)|\*\*(.+?)\*\*|\[([^\]]+)\]\(([^()\s]+)\)|\n/g;
  const out: JSX.Element[] = [];
  let at = 0, match: RegExpExecArray | null;
  while ((match = tokens.exec(text))) {
    out.push(text.slice(at, match.index));
    if (match[0] === "\n") out.push(<br />);
    else if (match[0].startsWith("\\")) out.push(match[0].slice(1));
    else if (match[1]) out.push(<code>{match[2]}</code>);
    else if (match[3]) out.push(<strong>{inline(match[3])}</strong>);
    else {
      const href = safeHref(match[5]);
      out.push(href ? <a href={href} rel="noreferrer">{inline(match[4])}</a> : inline(match[4]));
    }
    at = tokens.lastIndex;
  }
  out.push(text.slice(at));
  return out;
}
function Blocks(props: { blocks: Block[] }) {
  return <For each={props.blocks}>{b =>
    b.kind === "heading" ? <Dynamic component={("h" + b.level) as keyof JSX.IntrinsicElements}>{inline(b.text)}</Dynamic>
      : b.kind === "code" ? <pre><code>{b.text}</code></pre>
      : b.kind === "quote" ? <blockquote><Blocks blocks={b.blocks} /></blockquote>
      : b.kind === "list" ? <Dynamic component={b.ordered ? "ol" : "ul"} start={b.ordered ? b.start : undefined}><For each={b.items}>{item => <li><Blocks blocks={item} /></li>}</For></Dynamic>
      : b.kind === "table" ? <div class="ws-result-table"><table><thead><tr><For each={b.header}>{cell => <th scope="col">{inline(cell)}</th>}</For></tr></thead><tbody><For each={b.rows}>{row => <tr><For each={row}>{cell => <td>{inline(cell)}</td>}</For></tr>}</For></tbody></table></div>
      : <p>{inline(b.text)}</p>
  }</For>;
}
export function ResultText(props: { text?: string; breaks?: boolean }) {
  const text = createMemo(() => props.text ?? "");
  const blocks = createMemo(() => parse(text(), props.breaks));
  return <div class="ws-result-copy"><Blocks blocks={blocks()} /></div>;
}
