import assert from "node:assert/strict";
import { diffBlocks, diffLines, rawText } from "./worker-panel-diff.ts";

const kinds = (oldText: string | null, newText: string | null) => diffLines(oldText, newText).map((row) => `${row.kind}:${row.text}`);

assert.deepEqual(kinds("one\ntwo\nthree", "one\nTWO\nthree"), ["context:one", "delete:two", "add:TWO", "context:three"]);
assert.deepEqual(kinds(null, "new file\n"), ["add:new file"]);
assert.deepEqual(kinds("deleted\n", null), ["delete:deleted"]);
assert.deepEqual(kinds("a\r\nb\r\n", "a\nb\n"), ["context:a", "context:b"]);
assert.deepEqual(kinds("same\n", "same"), ["context:same"]);
const large = "x\n".repeat(451);
assert.equal(diffLines(large, large.replaceAll("x", "y")).every((row) => row.kind !== "context"), true);
const blocks = diffBlocks([
  { type: "diff", path: "same.txt", oldText: null, newText: "a" },
  { type: "diff", path: "same.txt", oldText: "a", newText: null },
]);
assert.equal(blocks.length, 2);
assert.equal(blocks[0].path, blocks[1].path);
const wrapped = diffBlocks({ content: [{ type: "diff", path: "wrapped.txt", oldText: "before", newText: "after" }] });
assert.deepEqual(wrapped, [{ path: "wrapped.txt", oldText: "before", newText: "after" }]);
assert.equal(rawText({}), null);
assert.equal(rawText([]), null);

console.log("worker-panel diff tests passed");
