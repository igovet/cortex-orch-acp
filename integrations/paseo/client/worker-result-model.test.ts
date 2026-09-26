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

test("a failed check an exact later rerun closed shows as resolved history, not an open failure", () => {
  assert.deepEqual(workerResultItems([
    { name: "unit tests", outcome: "failed", closedBy: "gateway:cmd:2" },
    { name: "browser flow", outcome: "failed" },
  ]), [
    { text: "unit tests", status: "resolved" },
    { text: "browser flow", status: "failed" },
  ]);
});

test("a claimed pass the runtime could not verify carries a weak-evidence marker, never a rewritten outcome", () => {
  assert.deepEqual(workerResultItems([
    { name: "typecheck", outcome: "unverified", claimedOutcome: "passed", evidenceGap: "tool_call_ref_missing", blocker: "Runtime kept this passed claim unverified." },
    { name: "lint", outcome: "failed", claimedOutcome: "passed", evidenceGap: "nonzero_exit" },
    { name: "build", outcome: "passed" },
  ]), [
    { text: "typecheck", status: "open", blocker: "Runtime kept this passed claim unverified.", weakEvidence: true },
    { text: "lint", status: "failed", weakEvidence: true },
    { text: "build", status: "done" },
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
