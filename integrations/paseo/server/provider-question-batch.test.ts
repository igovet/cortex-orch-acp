import assert from "node:assert/strict";
import { ProviderEventSchema } from "@getpaseo/plugin/server/provider";
import { wrapCortexConnection, QUESTION_ANSWER_CONFIG_ID } from "./provider.ts";
import { batchAnswers, batchForm, fallbackTopic, parseQuestionBatch } from "../shared/question-batch.ts";

const raw = { __cortex_question: true, batchId: "qb_test", questionId: "pq_first", questions: [
  { questionId: "pq_first", question: "Which layout?", options: ["Compact", "Spacious"] },
  { questionId: "pq_second", question: "Which theme?", options: ["Light", "Dark"] },
] };
assert.throws(() => parseQuestionBatch({ ...raw, questionId: "pq_second" }));
assert.throws(() => parseQuestionBatch({ ...raw, questions: [raw.questions[0], raw.questions[0]] }));
assert.throws(() => parseQuestionBatch({ ...raw, questions: [raw.questions[0], { ...raw.questions[1], options: ["Light", " light "] }] }));
assert.throws(() => batchAnswers(raw, { "Which layout": "Compact", foreign: "Dark" }));
// An optional per-question topic becomes the batch UI's tab label verbatim;
// a missing one falls back to the question's own bounded, punctuation-
// stripped first words -- never the raw question id.
assert.throws(() => parseQuestionBatch({ ...raw, questions: [{ ...raw.questions[0], topic: "  " }, raw.questions[1]] }));
assert.throws(() => parseQuestionBatch({ ...raw, questions: [{ ...raw.questions[0], topic: "x".repeat(25) }, raw.questions[1]] }));
const withTopic = parseQuestionBatch({ ...raw, questions: [{ ...raw.questions[0], topic: "Layout" }, raw.questions[1]] });
assert.equal(withTopic.questions[0].topic, "Layout");
assert.equal(withTopic.questions[1].topic, undefined);
assert.deepEqual(batchForm(withTopic).questions.map((q) => q.header), ["Layout", "Which theme"]);
assert.equal(fallbackTopic("Which theme?"), "Which theme");
assert.equal(fallbackTopic("x"), "x");
assert.deepEqual(batchAnswers(withTopic, { Layout: "Compact", "Which theme": "Dark" }),
  [{ questionId: "pq_first", answer: "Compact" }, { questionId: "pq_second", answer: "Dark" }]);
assert.deepEqual(batchAnswers(withTopic, { pq_first: "Compact", pq_second: "Dark" }),
  [{ questionId: "pq_first", answer: "Compact" }, { questionId: "pq_second", answer: "Dark" }]);
assert.throws(() => batchAnswers(withTopic, { Layout: "Compact", pq_second: "Dark" }), /every/);
// Header collisions (including a label that already has a suffix) remain
// distinct, bounded, and bind each answer to its original member identity.
for (const topics of [
  ["Layout", "Layout", "Layout 2", "Layout"],
  ["x".repeat(24), "x".repeat(24), "x".repeat(24), "x".repeat(22) + " 2"],
]) {
  const colliding = parseQuestionBatch({ batchId: "qb_collision", questionId: "pq_0", questions:
    topics.map((topic, index) => ({ ...raw.questions[index % 2], questionId: `pq_${index}`, topic })) });
  const headers = batchForm(colliding).questions.map((q) => q.header);
  assert.equal(new Set(headers).size, topics.length);
  assert.ok(headers.every((header) => header.length <= 24));
  assert.equal(headers[1], topics[0].slice(0, 22) + " 2");
  const answers = Object.fromEntries(headers.map((header, index) => [header, `Answer ${index}`]));
  assert.deepEqual(batchAnswers(colliding, answers),
    topics.map((_, index) => ({ questionId: `pq_${index}`, answer: `Answer ${index}` })));
  delete answers[headers[1]];
  assert.throws(() => batchAnswers(colliding, answers), /every/);
}
const fallbackCollision = { questions: [raw.questions[0], { ...raw.questions[1], question: raw.questions[0].question }] };
assert.deepEqual(batchForm(fallbackCollision).questions.map((q) => q.header), ["Which layout", "Which layout 2"]);
assert.deepEqual(batchAnswers(fallbackCollision, { "Which layout": "Compact", "Which layout 2": "Dark" }),
  [{ questionId: "pq_first", answer: "Compact" }, { questionId: "pq_second", answer: "Dark" }]);

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
// No explicit topic was sent: the tab header falls back to the question's
// own first words, bounded and punctuation-stripped -- never the raw id.
assert.deepEqual(seen[0].request.input.questions.map((q: any) => [q.header, q.allowOther, q.multiSelect]),
  [["Which layout", true, false], ["Which theme", true, false]]);
assert.deepEqual(seen[0].request.input.questions[0].options, [{ label: "Compact" }, { label: "Spacious" }]);
assert.throws(() => response({ "Which layout": "Compact" }), /every/);
assert.throws(() => response({ "Which layout": "Compact", "Which theme": " " }), /every/);
assert.throws(() => response({ "Which layout": "Compact", "Which theme": "Dark" }, "permission:batch", "foreign"), /another session/);
assert.equal(sent.length, 0);
// A new transport permission for the same pending batch is an alias, not a second card.
card("permission:alias");
assert.equal(seen.length, 1);
const answers = { "Which theme": "2. custom numeric text", "Which layout": "Compact" };
reject = true;
await assert.rejects(response(answers), /Rejected/);
const firstPayload = sent[0].changes.settings[QUESTION_ANSWER_CONFIG_ID];
assert.equal(seen.length, 1);
reject = false; delay = true;
const pending = response(answers);
const duplicate = response(answers);
assert.throws(() => response({ "Which layout": "Spacious", "Which theme": "Dark" }), /in flight/);
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
assert.throws(() => response({ "Which layout": "Spacious", "Which theme": "Dark" }), /different/);
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
