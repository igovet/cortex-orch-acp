import assert from "node:assert/strict";
import test from "node:test";
import { boundaryFallbackMessage } from "./plugin-error-boundary-message.ts";

test("names the crashed surface and includes an Error's message", () => {
  const message = boundaryFallbackMessage("Pipeline panel", new Error("Cannot read property 'prototype' of undefined"));
  assert.equal(message, "Pipeline panel could not be shown (Cannot read property 'prototype' of undefined). Reopen it to try again.");
});

test("falls back to the thrown value's own text for a non-Error throw", () => {
  assert.equal(boundaryFallbackMessage("Workers panel", "boom"), "Workers panel could not be shown (boom). Reopen it to try again.");
  assert.equal(boundaryFallbackMessage("Worker card", null), "Worker card could not be shown (unknown error). Reopen it to try again.");
});

test("falls back to 'unknown error' for an Error with no message", () => {
  assert.equal(boundaryFallbackMessage("Pipeline", new Error("")), "Pipeline could not be shown (unknown error). Reopen it to try again.");
});

test("uses 'unknown error' when an arbitrary thrown value cannot be formatted", () => {
  const nullPrototype = Object.create(null);
  const throwingToString = { toString() { throw new Error("cannot stringify"); } };
  const errorWithThrowingMessage = new Error("initial message");
  Object.defineProperty(errorWithThrowingMessage, "message", {
    get() { throw new Error("cannot read message"); },
  });

  for (const thrown of [nullPrototype, throwingToString, errorWithThrowingMessage]) {
    assert.equal(boundaryFallbackMessage("Pipeline", thrown), "Pipeline could not be shown (unknown error). Reopen it to try again.");
  }
});
