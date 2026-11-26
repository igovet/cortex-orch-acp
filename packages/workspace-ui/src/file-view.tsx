import { LoadingStrip } from "./skeleton";
import { createMemo, createResource, createSignal, For, onCleanup, Show, type JSX } from "solid-js";
import type { FileRead } from "@cortex/workspace-domain";
import { Code } from "./diff";
import { I } from "./kit";
import { ResultText } from "./result-text";

/**
 * How one file is shown in Code. Text kinds have a formatted preview beside their source; pictures, video and sound
 * are shown from their bytes; a PDF or another binary file gets a placeholder, never a broken screen.
 */
export type ViewKind = "markdown" | "html" | "json" | "csv" | "tsv" | "svg" | "image" | "video" | "audio" | "pdf" | "binary" | "text";
export type FileViewMode = "preview" | "source";
export type ViewPlan = { kind: ViewKind; modes: FileViewMode[]; initial: FileViewMode };

const KIND: Record<string, ViewKind> = {
  md: "markdown", markdown: "markdown", mdown: "markdown", mkd: "markdown",
  html: "html", htm: "html", xhtml: "html",
  json: "json", jsonc: "json", geojson: "json", webmanifest: "json",
  csv: "csv", tsv: "tsv", tab: "tsv",
  svg: "svg",
  // The same list the machine sends bytes for (`media_type` in files.rs).
  png: "image", jpg: "image", jpeg: "image", gif: "image", webp: "image", bmp: "image", ico: "image", avif: "image",
  mp4: "video", m4v: "video", webm: "video", ogv: "video", mov: "video",
  mp3: "audio", wav: "audio", ogg: "audio", oga: "audio", opus: "audio", flac: "audio", m4a: "audio", aac: "audio",
  pdf: "pdf",
};
const BINARY = new Set(["zip", "gz", "tgz", "bz2", "xz", "zst", "7z", "rar", "tar", "jar", "war", "exe", "dll", "so", "dylib", "o", "a", "class", "wasm", "bin",
  "woff", "woff2", "ttf", "otf", "eot", "psd", "tif", "tiff", "heic", "sqlite", "sqlite3", "db", "doc", "docx", "xls", "xlsx", "ppt", "pptx", "odt", "ods", "odp", "deb", "rpm", "dmg", "iso", "pyc"]);

export function viewKind(path: string): ViewKind {
  const name = path.split("/").pop() ?? "";
  const dot = name.lastIndexOf(".");
  if (dot <= 0) return "text";
  const ext = name.slice(dot + 1).toLowerCase();
  return KIND[ext] ?? (BINARY.has(ext) ? "binary" : "text");
}
/** Markdown, tables and SVG open formatted; HTML and JSON are code first, one switch away from their preview. */
export function viewPlan(path: string): ViewPlan {
  const kind = viewKind(path);
  switch (kind) {
    case "markdown": case "csv": case "tsv": case "svg": return { kind, modes: ["preview", "source"], initial: "preview" };
    case "html": case "json": return { kind, modes: ["source", "preview"], initial: "source" };
    case "image": case "video": case "audio": case "pdf": case "binary": return { kind, modes: ["preview"], initial: "preview" };
    default: return { kind, modes: ["source"], initial: "source" };
  }
}
/** Whether the file's bytes, not its text, are what is shown. */
export const isMedia = (kind: ViewKind) => kind === "image" || kind === "video" || kind === "audio";

/** The one-action switch between a file's preview and its source. */
export function ViewSwitch(props: { view: FileViewMode; onView: (view: FileViewMode) => void }) {
  return <div class="seg fv-switch" role="group" aria-label="Show file as">
    <button aria-pressed={props.view === "preview" ? "true" : "false"} onClick={() => props.onView("preview")}>Preview</button>
    <button aria-pressed={props.view === "source" ? "true" : "false"} onClick={() => props.onView("source")}>Source</button>
  </div>;
}

// ---- Delimited tables -------------------------------------------------------------------------------------------
const MAX_ROWS = 1000;
/** RFC 4180 quoting; a stray quote inside a field is kept as text. */
export function parseDelimited(text: string, sep: string): string[][] {
  const rows: string[][] = [];
  let row: string[] = [], cell = "", quoted = false, i = 0;
  const src = text.replace(/^﻿/, "");
  while (i < src.length) {
    const c = src[i];
    if (quoted) {
      if (c === '"' && src[i + 1] === '"') { cell += '"'; i += 2; continue; }
      if (c === '"') { quoted = false; i++; continue; }
      cell += c; i++; continue;
    }
    if (c === '"' && cell === "") { quoted = true; i++; continue; }
    if (c === sep) { row.push(cell); cell = ""; i++; continue; }
    if (c === "\n" || c === "\r") {
      row.push(cell); rows.push(row); row = []; cell = "";
      i += c === "\r" && src[i + 1] === "\n" ? 2 : 1; continue;
    }
    cell += c; i++;
  }
  if (cell !== "" || row.length) { row.push(cell); rows.push(row); }
  return rows;
}
function Table(props: { text: string; sep: string }) {
  const rows = createMemo(() => parseDelimited(props.text, props.sep));
  const width = () => Math.max(0, ...rows().slice(0, MAX_ROWS + 1).map(r => r.length));
  const body = () => rows().slice(1, MAX_ROWS + 1);
  return <Show when={rows().length} fallback={<Placeholder title="This table is empty." />}>
    <Show when={rows().length - 1 > MAX_ROWS}><div class="cap faint ws-pad">Showing the first {MAX_ROWS} of {rows().length - 1} rows. Source shows them all.</div></Show>
    <div class="fv-table"><table>
      <thead><tr><For each={Array.from({ length: width() }, (_, n) => rows()[0][n] ?? "")}>{cell => <th scope="col">{cell}</th>}</For></tr></thead>
      <tbody><For each={body()}>{row => <tr><For each={Array.from({ length: width() }, (_, n) => row[n] ?? "")}>{cell => <td>{cell}</td>}</For></tr>}</For></tbody>
    </table></div>
  </Show>;
}

// ---- JSON -------------------------------------------------------------------------------------------------------
function Json(props: { path: string; text: string }) {
  const parsed = createMemo(() => { try { return { text: JSON.stringify(JSON.parse(props.text), null, 2) }; } catch (e) { return { error: e instanceof Error ? e.message : "It does not parse." }; } });
  return <Show when={parsed().text !== undefined} fallback={<Placeholder title="This JSON does not parse, so it has no formatted view." detail={parsed().error} />}>
    <div class="code fv-json"><For each={parsed().text!.split("\n")}>{line => <div class="cl"><span class="c"><Code path={props.path} text={line} /></span></div>}</For></div>
  </Show>;
}

// ---- HTML -------------------------------------------------------------------------------------------------------
/**
 * The page is parsed into an inert document (nothing runs or loads there) and rebuilt element by element from a short
 * list of structural tags. No script, style, form, frame, link or event attribute is ever copied, and an image is kept
 * only from inline bytes, so the preview can neither run code nor reach the network or the app.
 */
const HTML_KEEP = new Set(["p", "br", "hr", "h1", "h2", "h3", "h4", "h5", "h6", "ul", "ol", "li", "dl", "dt", "dd", "blockquote", "pre", "code", "em", "strong", "b", "i", "u", "s",
  "del", "ins", "sub", "sup", "small", "mark", "span", "div", "section", "article", "header", "footer", "main", "nav", "aside", "table", "thead", "tbody", "tfoot", "tr", "th", "td",
  "caption", "figure", "figcaption", "a", "img", "abbr", "kbd", "q", "cite", "details", "summary", "time", "address"]);
const HTML_DROP = new Set(["script", "style", "template", "iframe", "frame", "frameset", "object", "embed", "applet", "link", "meta", "base", "form", "input", "button", "textarea", "select",
  "option", "noscript", "svg", "math", "video", "audio", "source", "track", "canvas", "head", "title", "portal", "dialog"]);
const MAX_NODES = 20000;
export function safeHtml(text: string): { node: DocumentFragment; partial: boolean } {
  const inert = new DOMParser().parseFromString(text, "text/html");
  const out = document.createDocumentFragment();
  let budget = MAX_NODES, partial = false;
  const copy = (from: Node, to: Node) => {
    for (const child of Array.from(from.childNodes)) {
      if (budget-- <= 0) { partial = true; return; }
      if (child.nodeType === Node.TEXT_NODE) { to.appendChild(document.createTextNode(child.textContent ?? "")); continue; }
      if (child.nodeType !== Node.ELEMENT_NODE) continue;
      const el = child as Element, tag = el.tagName.toLowerCase();
      if (HTML_DROP.has(tag)) continue;
      if (!HTML_KEEP.has(tag)) { copy(el, to); continue; }
      if (tag === "img") {
        const src = el.getAttribute("src") ?? "", alt = el.getAttribute("alt") ?? "";
        if (/^data:image\/(png|jpe?g|gif|webp|bmp|avif);base64,[a-z0-9+/=\s]+$/i.test(src)) {
          const img = document.createElement("img"); img.src = src; img.alt = alt; to.appendChild(img);
        } else to.appendChild(Object.assign(document.createElement("span"), { className: "fv-alt", textContent: alt ? `[image: ${alt}]` : "[image]" }));
        continue;
      }
      const made = document.createElement(tag);
      // A link keeps its words only: its target is neither followed nor shown as a hover title.
      if (tag === "a") made.className = "fv-link";
      if (tag === "td" || tag === "th") for (const a of ["colspan", "rowspan"]) { const v = el.getAttribute(a); if (v && /^\d{1,3}$/.test(v)) made.setAttribute(a, v); }
      if (tag === "ol") { const v = el.getAttribute("start"); if (v && /^-?\d{1,6}$/.test(v)) made.setAttribute("start", v); }
      if (tag === "details" && el.hasAttribute("open")) made.setAttribute("open", "");
      to.appendChild(made);
      copy(el, made);
    }
  };
  copy(inert.body, out);
  return { node: out, partial };
}
function Html(props: { text: string }) {
  const built = createMemo(() => safeHtml(props.text));
  return <>
    <div class="cap faint ws-pad">Preview without scripts, styles, forms or remote content.{built().partial ? " The page is long, so only its start is shown." : ""}</div>
    <div class="fv-doc fv-html">{built().node}</div>
  </>;
}

// ---- Media ------------------------------------------------------------------------------------------------------
function bytesUrl(media: NonNullable<FileRead["media"]>): string {
  // The type is the machine's, chosen from the name; only media types reach here.
  if (typeof URL.createObjectURL !== "function") return `data:${media.mime};base64,${media.data}`;
  const raw = atob(media.data), bytes = new Uint8Array(raw.length);
  for (let i = 0; i < raw.length; i++) bytes[i] = raw.charCodeAt(i);
  return URL.createObjectURL(new Blob([bytes], { type: media.mime }));
}
function Media(props: { path: string; kind: "image" | "video" | "audio"; read: () => Promise<FileRead>; fallback: (title: string, detail?: string) => JSX.Element }) {
  const [read] = createResource(() => props.path, () => props.read().catch((e: unknown) => ({ text: null, reason: e instanceof Error ? e.message : "This file could not be read." }) as FileRead));
  const [broken, setBroken] = createSignal(false);
  const url = createMemo(() => {
    const media = read()?.media;
    if (!media) return null;
    const made = bytesUrl(media);
    if (made.startsWith("blob:")) onCleanup(() => URL.revokeObjectURL(made));
    return made;
  });
  const name = () => props.path.split("/").pop() ?? props.path;
  const what = () => props.kind === "image" ? "picture" : props.kind === "video" ? "video" : "sound";
  return <Show when={!read.loading} fallback={<LoadingStrip label="Opening…" />}>
    <Show when={url() && !broken()} fallback={props.fallback(broken() ? `This ${what()} cannot be played or shown here.` : read()?.reason ?? `This ${what()} cannot be shown here.`, broken() ? "Its format may not be supported by this window." : undefined)}>
      <div class="fv-media" data-kind={props.kind}>
        {props.kind === "image" ? <img src={url()!} alt={name()} onError={() => setBroken(true)} />
          : props.kind === "video" ? <video src={url()!} controls preload="metadata" aria-label={name()} onError={() => setBroken(true)} />
          : <audio src={url()!} controls preload="metadata" aria-label={name()} onError={() => setBroken(true)} />}
      </div>
    </Show>
  </Show>;
}

// ---- The preview ------------------------------------------------------------------------------------------------
function Placeholder(props: { title: string; detail?: string; children?: JSX.Element }) {
  return <div class="note-panel fv-none" role="status"><I n="file" s={14} class="faint" /><span class="w500">{props.title}</span><Show when={props.detail}><span>{props.detail}</span></Show>
    <Show when={props.children}><span class="acts fv-acts">{props.children}</span></Show></div>;
}
/**
 * The formatted view of one file. `text` is what the editor holds (or the last read); `readMedia` brings the bytes of a
 * picture, video or sound. `onSource` offers the raw text where there may be some, and `external` opens it elsewhere.
 */
export function FilePreview(props: { path: string; plan: ViewPlan; text: string | null; reason?: string | null; readMedia: () => Promise<FileRead>; onSource?: () => void; external?: JSX.Element }) {
  // A link in a previewed file is for reading: following it would navigate the app window itself.
  const stay = (e: MouseEvent) => { if ((e.target as Element | null)?.closest?.("a")) e.preventDefault(); };
  const fallback = (title: string, detail?: string) => <Placeholder title={title} detail={detail}>
    <Show when={props.onSource}><button class="btn btn-sm" onClick={() => props.onSource!()}>Show as text</button></Show>{props.external}
  </Placeholder>;
  const kind = () => props.plan.kind;
  const needsText = () => !isMedia(kind()) && kind() !== "pdf" && kind() !== "binary";
  return <div class="ebody fv-body" data-view-kind={kind()} ref={el => el.addEventListener("click", stay)}>
    <Show when={!needsText() || props.text !== null} fallback={fallback(props.reason ?? "This file has no text to preview.")}>
      {kind() === "markdown" ? <div class="fv-doc"><ResultText inertLinks text={props.text!} /></div>
        : kind() === "csv" ? <Table text={props.text!} sep="," />
        : kind() === "tsv" ? <Table text={props.text!} sep={"\t"} />
        : kind() === "json" ? <Json path={props.path} text={props.text!} />
        : kind() === "html" ? <Html text={props.text!} />
        : kind() === "svg" ? <div class="fv-media" data-kind="svg"><img src={`data:image/svg+xml;charset=utf-8,${encodeURIComponent(props.text!)}`} alt={props.path.split("/").pop()} /></div>
        : isMedia(kind()) ? <Media path={props.path} kind={kind() as "image" | "video" | "audio"} read={props.readMedia} fallback={fallback} />
        : kind() === "pdf" ? fallback("PDF files cannot be shown inside this window.", "Open it in another app to read it.")
        : fallback("This file type has no view here.", "It may be binary. Show as text tries to read it anyway.")}
    </Show>
  </div>;
}
