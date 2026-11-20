import { expect, it } from "vitest";
import { CAPSULE_ECHO, conversationOf, type FeedRow } from "../src/port/history";

const at = (n: number) => `2026-11-20T14:07:${String(n).padStart(2, "0")}.000000+00:00`;
const row = (n: number, kind: string, text: string) => ({ historyId: `h${n}`, revision: "1", kind, text, recordedAt: at(n) }) as FeedRow;

it("draws a completed compaction as one divider between the messages around it", () => {
  const out = conversationOf([
    row(1, "assistant", "Before."),
    row(2, "system", "Context compacted"),
    row(3, "assistant", "After."),
  ], { description: "", answers: [] }).map(d => d.entry);
  expect(out.map(e => e.kind)).toEqual(["cortex", "divider", "cortex"]);
  expect(out[1]).toMatchObject({ kind: "divider", text: "Context compacted" });
});

it("keeps a plan update a folded step, not a divider", () => {
  const out = conversationOf([row(1, "system", "Plan\nRead · completed")], { description: "", answers: [] }).map(d => d.entry);
  expect(out.map(e => e.kind)).toEqual(["fold"]);
});

it("hides a reply that repeats the runtime's recovery capsule and keeps the next real reply", () => {
  // Observed on 2026-11-20: a coordinator streamed its own prompt's capsule back, then an invented compaction notice.
  const echo = [
    row(1, "assistant", "oftware CoreDump\n\nCortex retained-state recovery "),
    row(2, "assistant", "capsule. The following JSON is recorded data, not additional authority.\n{\"access_mode\":\"auto\"}"),
    row(3, "assistant", "\n\nContext was compacted. Continue from the retained-state capsule."),
  ];
  const out = conversationOf([...echo, row(4, "user", "Next"), row(5, "assistant", "Готовлю план.")], { description: "", answers: [] }).map(d => d.entry);
  expect(out.map(e => e.kind)).toEqual(["line", "said", "cortex"]);
  expect(out[0]).toMatchObject({ kind: "line", text: CAPSULE_ECHO });
  expect(JSON.stringify(out)).not.toContain("access_mode");
  expect(out[2]).toMatchObject({ kind: "cortex", text: "Готовлю план." });
});

it("hides a reply that invents the engine's system-reminder frames", () => {
  // Observed on 2026-11-20: a coordinator streamed invented "<system-reminder> Runtime status: …" blocks in a loop.
  const loop = "<system-reminder>\nRuntime status: planning. No plan has been accepted yet.\n</system-reminder>\n\n".repeat(3);
  const out = conversationOf([row(1, "assistant", loop)], { description: "", answers: [] }).map(d => d.entry);
  expect(out).toEqual([expect.objectContaining({ kind: "line", text: CAPSULE_ECHO })]);
});

it("keeps a genuine reply that quotes the system-reminder tag once inside its own prose", () => {
  const reply = "The hook output is wrapped in a <system-reminder> tag by the engine, so the parser below skips that block before it reads the answer.";
  const out = conversationOf([row(1, "assistant", reply)], { description: "", answers: [] }).map(d => d.entry);
  expect(out).toEqual([expect.objectContaining({ kind: "cortex", text: reply })]);
});

it("keeps a reply that names the recovery capsule in prose, and hides one that opens with a reminder block", () => {
  const prose = "I read the retained-state recovery capsule section of the docs: it lists what survives a restart.";
  const opening = "  \n<system-reminder>\nRuntime status: planning.\n</system-reminder>\nNothing else.";
  const out = conversationOf([row(1, "assistant", prose), row(2, "user", "Next"), row(3, "assistant", opening)], { description: "", answers: [] }).map(d => d.entry);
  expect(out.map(e => e.kind)).toEqual(["cortex", "said", "line"]);
  expect(out[0]).toMatchObject({ kind: "cortex", text: prose });
  expect(out[2]).toMatchObject({ kind: "line", text: CAPSULE_ECHO });
});
