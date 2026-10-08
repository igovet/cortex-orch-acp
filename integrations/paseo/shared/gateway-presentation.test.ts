import assert from "node:assert/strict";
import test from "node:test";
import { gatewayPresentation } from "./gateway-presentation.ts";

test("committed project_edit projects a real bounded diff for chat and worker views", () => {
  const row = gatewayPresentation("mcp.cortex.project_edit", { path: "app.py", edits: [{ old_string: "a", new_string: "b" }] },
    { result: { structuredContent: { diff: "-a\n+b\n", oldText: "a", newText: "b", sha256: "new" } } });
  assert.equal(row?.kind, "edit");
  assert.equal(row?.input.unifiedDiff, "-a\n+b\n");
  assert.equal(row?.input.oldText, "a");
  assert.equal(row?.input.newText, "b");
  assert.equal((row?.output as { content: string }).content, "-a\n+b\n");
});

test("pending edit does not claim a committed diff; line read metadata stays intact", () => {
  const pending = gatewayPresentation("mcp.cortex.project_edit", { path: "app.py", edits: [{ old_string: "a", new_string: "b" }] }, undefined);
  assert.equal(pending?.input.unifiedDiff, undefined);
  assert.equal(pending?.input.oldText, undefined);
  const read = gatewayPresentation("mcp.cortex.project_read", { path: "app.py", offset: 21, limit: 10 },
    { result: { structuredContent: { start_line: 21, end_line: 30, partial: true } } });
  assert.equal(read?.kind, "read");
  assert.equal(read?.input.offset, 21);
  assert.equal((read?.output as { partial: boolean }).partial, true);
});
