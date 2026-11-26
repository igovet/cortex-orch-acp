import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";

describe("task inspector note", () => {
  it("never hardcodes the next step", () => {
    const src = readFileSync("src/task-chrome.tsx", "utf8");
    expect(src).not.toContain("Next step is Review.");
    expect(src).toContain("chip().next");
  });
});
