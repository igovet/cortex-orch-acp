import { expect, it, vi } from "vitest";
import { createTerminalPort, inputChunks } from "../src/terminal-client";
const page = { sessionId: "s", data: [65], next: 1, truncated: false, exited: false };
it("negotiates the entire closed surface and never sends launch configuration", async () => {
  const request = vi.fn(async (operation: string) => operation === "service.capabilities"
    ? { methods: ["open", "read", "write", "resize", "close"].map(op => `terminal.${op}`) }
    : operation === "terminal.write" ? { written: 2 } : page);
  const client = createTerminalPort({ request });
  expect(await client.open("p", 80, 24)).toEqual(page);
  await client.read("p", "s", 1);
  await client.write("p", "s", "é");
  await client.resize("p", "s", 90, 30);
  await client.close("p", "s");
  expect(request.mock.calls).toEqual([
    ["service.capabilities", {}], ["terminal.open", { projectId: "p", cols: 80, rows: 24 }],
    ["terminal.read", { projectId: "p", sessionId: "s", after: 1 }],
    ["terminal.write", { projectId: "p", sessionId: "s", data: "é" }],
    ["terminal.resize", { projectId: "p", sessionId: "s", cols: 90, rows: 30 }],
    ["terminal.close", { projectId: "p", sessionId: "s" }],
  ]);
});
it("refuses missing capabilities and malformed output; a short or lost write is never replayed", async () => {
  const unsupported = vi.fn(async () => ({ methods: ["terminal.open"] }));
  await expect(createTerminalPort({ request: unsupported }).open("p", 80, 24)).rejects.toThrow("does not offer");
  expect(unsupported).toHaveBeenCalledTimes(1);
  for (const reply of [{ ...page, next: -1 }, { ...page, data: [256] }, { ...page, data: Array(16385).fill(0) }, {}]) {
    await expect(createTerminalPort({ request: async () => reply }).read("p", "s", 0)).rejects.toThrow("Invalid terminal");
  }
  const short = vi.fn(async () => ({ written: 1 }));
  await expect(createTerminalPort({ request: short }).write("p", "s", "two")).rejects.toThrow("not resent");
  expect(short).toHaveBeenCalledTimes(1);
  const lost = vi.fn(async () => { throw new Error("lost"); });
  await expect(createTerminalPort({ request: lost }).write("p", "s", "two")).rejects.toThrow("lost");
  expect(lost).toHaveBeenCalledTimes(1);
});
it("splits pasted input by byte limit without splitting Unicode", () => {
  const text = "🦀é".repeat(2000), parts = inputChunks(text);
  expect(parts.join("")).toBe(text);
  expect(parts.every(p => new TextEncoder().encode(p).length <= 4096)).toBe(true);
});
