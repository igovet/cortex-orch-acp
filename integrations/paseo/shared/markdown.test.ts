import assert from "node:assert/strict";
import { parseInline, parseMarkdown } from "./markdown.ts";

assert.deepEqual(parseInline("plain text"), [{ text: "plain text" }]);
assert.deepEqual(parseInline("use `vi.mock` here"), [
  { text: "use " }, { text: "vi.mock", code: true }, { text: " here" },
]);
assert.deepEqual(parseInline("**bold** and *italic*"), [
  { text: "bold", bold: true }, { text: " and " }, { text: "italic", italic: true },
]);

const blocks = parseMarkdown("## Summary\n\nSome *text* with `code`.\n\n- one\n- two\n\n```\nraw block\n```");
assert.deepEqual(blocks[0], { kind: "heading", level: 2, spans: [{ text: "Summary" }] });
assert.equal(blocks[1].kind, "paragraph");
assert.deepEqual(blocks[2], { kind: "list", ordered: false, items: [[{ text: "one" }], [{ text: "two" }]] });
assert.deepEqual(blocks[3], { kind: "code", text: "raw block" });

const ordered = parseMarkdown("1. first\n2. second");
assert.equal(ordered[0].kind, "list");
if (ordered[0].kind === "list") assert.equal(ordered[0].ordered, true);

console.log("markdown parsing tests passed");
