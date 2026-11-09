import { describe, expect, it } from "vitest";
import { freshness } from "../src/freshness";
import { duration } from "../src/session-model";

it.each([[7799.999999999, "7s"], [61799.9, "1m 1s"], [600000, "10m 0s"], [3661999, "1h 1m 1s"], [-1, "0s"], [NaN, ""], [null, ""]])("formats %s milliseconds without decimals", (ms, text) => {
  expect(duration(ms)).toBe(text);
});

describe("confirmed data age", () => {
  it("omits missing stamps, never a dash or a made-up time", () => {
    expect(freshness(null, 10000)).toBe("");
  });
  it("uses the last successful read, not the disconnect time", () => {
    expect(freshness(1000, 121000)).toMatch(/Last confirmed.* 2m old/);
  });
  it("reads as words, never as a machine timestamp", () => {
    expect(freshness(1000, 7300000)).not.toMatch(/T\d{2}:|Z\b/);
    expect(freshness(0, 2 * 86400000)).toMatch(/ 2d old/);
  });
});
