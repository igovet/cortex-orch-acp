import { expect, it } from "vitest";
import { conversationOf, type FeedRow } from "../src/port/history";
import { feedOf } from "../src/tool-names";

const at = (n: number) => `2026-11-15T10:00:${String(n).padStart(2, "0")}.000000+00:00`;
const say = (n: number, text: string) => ({ historyId: `m${n}`, revision: "1", kind: "assistant", text, recordedAt: at(n) }) as FeedRow;

// The service leaves a worker's narration out of the task history (history.rs), so the Conversation reads only what remains.
it("reads the coordinator's messages as before when the service has left the workers' out", () => {
  const out = conversationOf([say(1, "Запускаю воркера."), say(2, "\n\nГотово.")], { description: "", answers: [] }).map(d => d.entry);
  expect(out).toHaveLength(1);
  expect((out[0] as { text: string }).text).toBe("Запускаю воркера.\n\nГотово.");
});

it("shows a worker's narration on its own page as note rows between its tool steps, in order", () => {
  const rows = [
    { sequence: 10, kind: "tool" as const, text: "Read a.ts", at: 1000, args: "" },
    { sequence: 11, kind: "note" as const, text: "Running the focused test suites and the mobile typecheck.", at: 2000 },
    { sequence: 12, kind: "tool" as const, text: "Read b.ts", at: 3000, args: "" },
  ];
  const feed = feedOf(rows as never);
  expect(feed).toHaveLength(3);
  expect(feed.findIndex(f => "item" in f && f.item.kind === "note")).toBe(1);
});
