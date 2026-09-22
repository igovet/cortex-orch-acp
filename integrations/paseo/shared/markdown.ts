/** A deliberately minimal markdown parser: headings, fenced code blocks,
 * bullet/numbered lists, paragraphs, and inline bold/italic/code. No tables,
 * links, or nested lists -- worker prose does not use them, and the plugin
 * SDK has no markdown renderer to build on (see worker-panel.tsx). */

export type MarkdownInline = { text: string; bold?: boolean; italic?: boolean; code?: boolean };

export type MarkdownBlock =
  | { kind: "heading"; level: number; spans: MarkdownInline[] }
  | { kind: "code"; text: string }
  | { kind: "list"; ordered: boolean; items: MarkdownInline[][] }
  | { kind: "paragraph"; spans: MarkdownInline[] };

export function parseInline(text: string): MarkdownInline[] {
  const spans: MarkdownInline[] = [];
  const re = /`([^`]+)`|\*\*([^*]+)\*\*|\*([^*]+)\*/g;
  let last = 0;
  let match: RegExpExecArray | null;
  while ((match = re.exec(text))) {
    if (match.index > last) spans.push({ text: text.slice(last, match.index) });
    if (match[1] !== undefined) spans.push({ text: match[1], code: true });
    else if (match[2] !== undefined) spans.push({ text: match[2], bold: true });
    else spans.push({ text: match[3], italic: true });
    last = re.lastIndex;
  }
  if (last < text.length) spans.push({ text: text.slice(last) });
  return spans.length ? spans : [{ text }];
}

export function parseMarkdown(source: string): MarkdownBlock[] {
  const lines = source.replace(/\r\n/g, "\n").split("\n");
  const blocks: MarkdownBlock[] = [];
  let paragraph: string[] = [];
  let listItems: string[] = [];
  let listOrdered = false;
  const flushParagraph = () => {
    if (paragraph.length) blocks.push({ kind: "paragraph", spans: parseInline(paragraph.join(" ")) });
    paragraph = [];
  };
  const flushList = () => {
    if (listItems.length) blocks.push({ kind: "list", ordered: listOrdered, items: listItems.map(parseInline) });
    listItems = [];
  };
  let i = 0;
  while (i < lines.length) {
    const line = lines[i];
    if (/^\s*```/.test(line)) {
      flushParagraph();
      flushList();
      const codeLines: string[] = [];
      i += 1;
      while (i < lines.length && !/^\s*```/.test(lines[i])) { codeLines.push(lines[i]); i += 1; }
      i += 1;
      blocks.push({ kind: "code", text: codeLines.join("\n") });
      continue;
    }
    const heading = line.match(/^(#{1,6})\s+(.*)$/);
    if (heading) {
      flushParagraph();
      flushList();
      blocks.push({ kind: "heading", level: heading[1].length, spans: parseInline(heading[2]) });
      i += 1;
      continue;
    }
    const listItem = line.match(/^\s*(?:[-*]|\d+\.)\s+(.*)$/);
    if (listItem) {
      flushParagraph();
      listOrdered = /^\s*\d+\./.test(line);
      listItems.push(listItem[1]);
      i += 1;
      continue;
    }
    if (line.trim() === "") {
      flushParagraph();
      flushList();
      i += 1;
      continue;
    }
    flushList();
    paragraph.push(line.trim());
    i += 1;
  }
  flushParagraph();
  flushList();
  return blocks;
}
