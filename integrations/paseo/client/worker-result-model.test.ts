import assert from "node:assert/strict";
import test from "node:test";
import { workerResultItems } from "./worker-result-model.ts";

test("worker results keep passing, failing, and unverified checks distinct", () => {
  assert.deepEqual(workerResultItems([
    { name: "typecheck", outcome: "passed" },
    { name: "browser flow", outcome: "failed" },
    { name: "manual observation", outcome: "not_run" },
  ]), [
    { text: "typecheck", status: "done" },
    { text: "browser flow", status: "failed" },
    { text: "manual observation", status: "open" },
  ]);
});

test("worker results retain blocker reasons only for unresolved checks", () => {
  assert.deepEqual(workerResultItems([
    { name: "browser flow", outcome: "not_run", blocker: "No browser session was available." },
    { name: "history", outcome: "unverified", blocker: "History output was truncated." },
    { name: "typecheck", outcome: "passed", blocker: "ignored" },
    { name: "old result", outcome: "not_run" },
  ]), [
    { text: "browser flow", status: "open", blocker: "No browser session was available." },
    { text: "history", status: "open", blocker: "History output was truncated." },
    { text: "typecheck", status: "done" },
    { text: "old result", status: "open" },
  ]);
});
