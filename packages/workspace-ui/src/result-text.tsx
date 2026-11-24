import { createMemo, For, useContext, type JSX } from "solid-js";
import { Dynamic } from "solid-js/web";
import { WsContext } from "./ctx";
import { fileReference, ResultLink, ResultScope } from "./result-links";
import { parse, tokenize, type Block, type Token } from "./markdown";
import { CopyCode } from "./message-copy";

export type { Block } from "./markdown";

/** Render supplied Markdown as text nodes and known elements, never executable HTML. */
const unescape = (text: string) => text.replace(/\\([!"#$%&'()*+,\-./:;<=>?@[\]\\^_`{|}~])/g, "$1");
/** Plain snippets share prose unescaping without changing literal code. */
export const resultPreview = (text: string) => text.split(/(```[\s\S]*?```|`[^`]+`)/g).map(part => part.startsWith("`") ? part : decodeEntities(unescape(part))).join("");

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
  if (!url || /[\u0000-\u0020\u007f\\]/.test(url) || url.startsWith("//")) return;
  if (/^(https?:\/\/|mailto:)/i.test(url) || url.startsWith("#")) return url;
  if (/^[a-z][a-z\d+.-]*:/i.test(url) && !/^[\w-]+\.[\w.-]+:\d+(?::\d+)?$/.test(url)) return;
  return fileReference(url, true) ? url : undefined;
};
function markdownFile(href: string) {
  if (/^(https?:\/\/|mailto:|#)/i.test(href)) return;
  try { return fileReference(decodeURIComponent(href), true); }
  catch { return; }
}
/** An image reference names a picture; nothing is loaded from it (the window loads no remote content). */
function imageRef(alt: string, raw: string, links: Links): JSX.Element {
  const href = safeHref(raw), target = decodeEntities(unescape(raw)).trim();
  const place = href && /^https?:/i.test(href) ? (() => { try { const u = new URL(href); return [u.host, u.pathname.split("/").filter(Boolean).pop()].filter(Boolean).join(" · "); } catch { return href; } })() : target.split("/").filter(Boolean).pop() ?? target;
  const label = <>Image: {decodeEntities(unescape(alt)).trim() || place}{alt.trim() && place ? " (" + place + ")" : ""}</>;
  return href && /^https?:/i.test(href) && !links.label ? renderLink(links, { class: "ws-img-ref", href, inert: links.inert, children: label }) : <span class="ws-img-ref">{label}</span>;
}
type Links = { paths: boolean; inert?: boolean; label?: boolean; markLink?: () => void };
function renderLink(links: Links, props: Parameters<typeof ResultLink>[0]) {
  links.markLink?.();
  return <ResultLink {...props} />;
}
function prose(text: string, links: Links): JSX.Element {
  if (!links.paths) return text;
  // Don't pick a suffix out of an already-tokenized URI or a different path.
  const candidates = /(?<![\p{L}\d_./:@%\\-])(?:\/|(?:\.\.?\/)+)?[\p{L}\d_./-]+(?::\d+(?::\d+)?|#L\d+)?/gu;
  const out: JSX.Element[] = [];
  let at = 0;
  for (const match of text.matchAll(candidates)) {
    const token = match[0].replace(/\.+$/, ""), start = match.index!;
    const ref = fileReference(token);
    if (!ref) continue;
    out.push(text.slice(at, start), renderLink(links, { href: "#", reference: ref, children: token }));
    at = start + token.length;
  }
  out.push(text.slice(at));
  return out;
}
/** Inline Markdown from the shared tokens (`markdown.ts`), the same ones the writing area paints under the owner's text. */
function render(text: string, tokens: Token[], links: Links): JSX.Element[] {
  return tokens.map((k): JSX.Element => {
    const part = (a: number, b: number) => text.slice(a, b);
    switch (k.t) {
      case "text": return prose(decodeEntities(part(k.s, k.e)), links);
      case "br": return <br />;
      case "esc": return part(k.s + 1, k.e);
      case "code": {
        const span = part(k.a, k.b), ref = links.paths ? fileReference(span.trim()) : undefined;
        return <code>{ref ? renderLink(links, { href: "#", reference: ref, children: span }) : span}</code>;
      }
      case "strong": return <strong>{render(text, k.kids, links)}</strong>;
      case "em": return <em>{render(text, k.kids, links)}</em>;
      case "del": return <del>{render(text, k.kids, links)}</del>;
      case "image": return imageRef(part(k.a, k.b), part(k.u, k.v), links);
      case "link": {
        const href = safeHref(part(k.u, k.v));
        const label = render(text, k.kids, { ...links, paths: false, label: true });
        return href && !links.label ? renderLink(links, { href: /^\w[\w.-]*:\d/.test(href) ? "#" : href, reference: links.paths ? markdownFile(href) : undefined, inert: links.inert, children: label }) : label;
      }
    }
  });
}
const inline = (text: string, links: Links): JSX.Element => render(text, tokenize(text), links);
function useLinks(inert?: boolean): Links {
  return { paths: !!useContext(WsContext) && !!useContext(ResultScope) && !inert, inert };
}
/** Question controls use only phrasing markup and the same link handling as the timeline. */
export function createInlineResultText(text: () => string) {
  const links = useLinks();
  return createMemo(() => {
    let hasLinks = false;
    const value = text();
    // Fences are not inline Markdown. Keep their literal contents and never scan them for paths.
    const nodes = parse(value, true).some(b => b.kind === "code") ? value : inline(value, { ...links, markLink: () => { hasLinks = true; } });
    return { nodes, hasLinks };
  });
}
export function InlineResultText(props: { text: string }) {
  const content = createInlineResultText(() => props.text);
  return <>{content().nodes}</>;
}
function Blocks(props: { blocks: Block[]; links: Links }) {
  return <For each={props.blocks}>{b =>
    b.kind === "heading" ? <Dynamic component={("h" + b.level) as keyof JSX.IntrinsicElements}>{inline(b.text, props.links)}</Dynamic>
      // A code block is a panel: its language, if the fence names one, and its own Copy, which copies the lines as written.
      : b.kind === "code" ? <div class="ws-code" data-md="code">
        <div class="ws-code-h"><span class="ws-code-lang">{b.lang ?? ""}</span><CopyCode text={b.text} lang={b.lang} /></div>
        <pre><code>{b.text}</code></pre>
      </div>
      : b.kind === "hr" ? <hr />
      : b.kind === "quote" ? <blockquote><Blocks links={props.links} blocks={b.blocks} /></blockquote>
      : b.kind === "list" ? <Dynamic component={b.ordered ? "ol" : "ul"} start={b.ordered ? b.start : undefined}><For each={b.items}>{item => <li><Blocks links={props.links} blocks={item} /></li>}</For></Dynamic>
      : b.kind === "table" ? <div class="ws-result-table"><table><thead><tr><For each={b.header}>{(cell, i) => <th scope="col" style={{ "text-align": b.align[i()] }}>{inline(cell, props.links)}</th>}</For></tr></thead><tbody><For each={b.rows}>{row => <tr><For each={row}>{(cell, n) => <td style={{ "text-align": b.align[n()] }}>{inline(cell, props.links)}</td>}</For></tr>}</For></tbody></table></div>
      : <p>{inline(b.text, props.links)}</p>
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
export function ResultText(props: { text?: string; breaks?: boolean; inertLinks?: boolean }) {
  const links = useLinks(props.inertLinks);
  const text = createMemo(() => props.text ?? "");
  const blocks = createMemo<Block[]>(previous => reuseBlocks(parse(text(), props.breaks), previous), []);
  return <div class="ws-result-copy"><Blocks links={links} blocks={blocks()} /></div>;
}
