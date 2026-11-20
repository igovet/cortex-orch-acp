import { describe, expect, it } from "vitest";
import { share } from "../src/stable";

describe("share activity sequences", () => {
  const step = (sequence: number, text = String(sequence)) => ({ sequence, text, kind: "tool", at: null });
  it("preserves identities across prepend, removal and reorder while preserving next data", () => {
    const prev = [step(2), step(3), step(4)];
    const next = [step(1), step(4), step(2), step(3)];
    const out = share(prev, next);
    expect(out).toEqual(next);
    expect(out[1]).toBe(prev[2]);
    expect(out[2]).toBe(prev[0]);
    expect(out[3]).toBe(prev[1]);
    const removed = share(out, [step(4), step(3)]);
    expect(removed).toEqual([step(4), step(3)]);
    expect(removed[0]).toBe(prev[2]);
    expect(removed[1]).toBe(prev[1]);
  });
  it("replaces changed steps and returns the previous list when unchanged", () => {
    const prev = [step(1), step(2)];
    expect(share(prev, [step(1), step(2)])).toBe(prev);
    const next = [step(1, "changed"), step(2)];
    const out = share(prev, next);
    expect(out).toEqual(next);
    expect(out[0]).not.toBe(prev[0]);
    expect(out[1]).toBe(prev[1]);
  });
  it("does not match ambiguous sequences and prioritizes existing ids", () => {
    const prev = [step(1, "a"), step(1, "b")];
    const next = [step(1, "b"), step(1, "a")];
    const out = share(prev, next);
    expect(out).toEqual(next);
    expect(out[0]).not.toBe(prev[1]);
    const ids = [{ id: "a", sequence: 1 }, { id: "b", sequence: 2 }];
    const reordered = share(ids, [{ id: "b", sequence: 2 }, { id: "a", sequence: 1 }]);
    expect(reordered[0]).toBe(ids[1]);
    expect(reordered[1]).toBe(ids[0]);
  });
});
