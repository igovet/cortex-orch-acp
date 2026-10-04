import assert from "node:assert/strict";
import { ProviderEventSchema } from "@getpaseo/plugin/server/provider";
import { wrapCortexConnection, QUESTION_ANSWER_CONFIG_ID } from "./provider.ts";
import { batchAnswers, parseQuestionBatch } from "../shared/question-batch.ts";

const raw = { __cortex_question: true, batchId: "qb_test", questionId: "pq_first", questions: [
  { questionId: "pq_first", question: "Which layout?", options: ["Compact", "Spacious"] },
  { questionId: "pq_second", question: "Which theme?", options: ["Light", "Dark"] },
] };
assert.throws(() => parseQuestionBatch({ ...raw, questionId: "pq_second" }));
assert.throws(() => parseQuestionBatch({ ...raw, questions: [raw.questions[0], raw.questions[0]] }));
assert.throws(() => parseQuestionBatch({ ...raw, questions: [raw.questions[0], { ...raw.questions[1], options: ["Light", " light "] }] }));
assert.throws(() => batchAnswers(raw, { pq_first: "Compact", foreign: "Dark" }));
const sent: any[] = [], seen: any[] = [];
const openPermissions = new Set<string>();
const listeners = new Set<(event: any) => void>();
const emit = (event: any) => listeners.forEach((listener) => listener(event));
let reject = false, retireFails = false, ack: (() => void) | undefined, delay = false;
const wrapped = wrapCortexConnection({
  capabilities: [],
  async send(input: any) {
    sent.push(input);
    if (input.type === "session.configure") {
      ack = () => emit(reject ? { type: "request.failed", requestId: input.requestId, error: { message: "Rejected" } }
        : { type: "request.completed", requestId: input.requestId });
      if (!delay) queueMicrotask(ack);
    } else {
      if (retireFails) throw new Error("Retire failed");
      assert.ok(openPermissions.delete(JSON.stringify([input.sessionId, input.permissionId])), "unknown or duplicate native permission retirement");
    }
  },
  onEvent(listener: (event: any) => void) { listeners.add(listener); return () => listeners.delete(listener); },
  async close() {},
} as any, []);
wrapped.onEvent((event) => { ProviderEventSchema.parse(event); seen.push(event); });
function card(id = "permission:batch", sessionId = "s", input = raw) {
  openPermissions.add(JSON.stringify([sessionId, id]));
  emit({ type: "session.permission", sessionId, request: { id, name: "Questions", kind: "tool", input,
    actions: raw.questions.flatMap((q) => q.options.map((label, i) => ({ id: `cortex-batch:${q.questionId}:${i + 1}`, label, behavior: "allow" }))),
  } });
}
const response = (answers: any, permissionId = "permission:batch", sessionId = "s") => wrapped.send({
  type: "session.permission", permissionId, sessionId, response: { behavior: "allow", updatedInput: { answers } },
} as any);
card(); card();
assert.equal(seen.length, 1);
assert.equal(seen[0].request.kind, "question");
assert.deepEqual(seen[0].request.input.questions.map((q: any) => [q.header, q.allowOther, q.multiSelect]),
  [["pq_first", true, false], ["pq_second", true, false]]);
assert.deepEqual(seen[0].request.input.questions[0].options, [{ label: "Compact" }, { label: "Spacious" }]);
assert.throws(() => response({ pq_first: "Compact" }), /every/);
assert.throws(() => response({ pq_first: "Compact", pq_second: " " }), /every/);
assert.throws(() => response({ pq_first: "Compact", pq_second: "Dark" }, "permission:batch", "foreign"), /another session/);
assert.equal(sent.length, 0);
// A new transport permission for the same pending batch is an alias, not a second card.
card("permission:alias");
assert.equal(seen.length, 1);
const answers = { pq_second: "2. custom numeric text", pq_first: "Compact" };
reject = true;
await assert.rejects(response(answers), /Rejected/);
const firstPayload = sent[0].changes.settings[QUESTION_ANSWER_CONFIG_ID];
assert.equal(seen.length, 1);
reject = false; delay = true;
const pending = response(answers);
const duplicate = response(answers);
assert.throws(() => response({ pq_first: "Spacious", pq_second: "Dark" }), /in flight/);
assert.equal(sent.length, 2);
assert.equal(sent[1].changes.settings[QUESTION_ANSWER_CONFIG_ID], firstPayload);
assert.equal(seen.length, 1); // no echo or retire before durable ACK
ack!(); await pending; await duplicate;
assert.equal(sent.length, 4);
assert.deepEqual(sent[2].response, { behavior: "deny" });
const payload = JSON.parse(firstPayload);
assert.equal(payload.batchId, "qb_test");
assert.ok(payload.requestKey);
assert.deepEqual(payload.answers, [{ questionId: "pq_first", answer: "Compact" }, { questionId: "pq_second", answer: "2. custom numeric text" }]);
assert.equal(seen.filter((event) => event.type === "timeline.item").length, 2);
await response(answers); // ACK retry never duplicates config or answer history
assert.equal(sent.filter((input) => input.type === "session.configure").length, 2);
assert.equal(seen.filter((event) => event.type === "timeline.item").length, 2);
assert.throws(() => response({ pq_first: "Spacious", pq_second: "Dark" }), /different/);
card("permission:recovery");
assert.equal(seen.filter((e) => e.type === "session.permission").length, 1);
assert.equal(sent.at(-1).permissionId, "permission:recovery");
// A session-distinct batch is independent; dismiss never records an answer.
card("permission:cancel", "other");
await wrapped.send({ type: "session.permission", sessionId: "other", permissionId: "permission:cancel", response: { behavior: "deny" } } as any);
assert.equal(sent.filter((input) => input.type === "session.configure").length, 2);
card("permission:cancel2", "other");
assert.equal(seen.filter((e) => e.type === "session.permission").length, 3);
delay = false; retireFails = true;
await assert.rejects(response(answers, "permission:cancel2", "other"), /Retire failed/);
retireFails = false;
await response(answers, "permission:cancel2", "other");
assert.equal(sent.filter((input) => input.type === "session.configure").length, 3);
assert.throws(() => card("permission:conflict", "other", { ...raw, questions: [raw.questions[0], { ...raw.questions[1], question: "Changed" }] }), /Conflicting/);
// A lost connection before ACK rejects the submission, never echoes an answer.
card("permission:close", "closing");
delay = true;
const closing = response(answers, "permission:close", "closing");
const rejectedOnClose = assert.rejects(closing, /closed before answer acknowledgment/);
await wrapped.close();
await rejectedOnClose;
console.log("batch transport and recovery ok");
