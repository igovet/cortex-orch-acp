import assert from "node:assert/strict";
import { QUESTION_ANSWER_CONFIG_ID, typedAnswer, wrapCortexConnection } from "./provider.ts";
import { CORTEX_QUESTION_INPUT_MARKER } from "../shared/question-marker.ts";

const actions = [{ id: "cortex-question-1", label: "1. Approve" }, { id: "cortex-question-2", label: "2. Finish" }];
assert.equal(typedAnswer({ Cortex: "1. Approve" }, actions), undefined);
assert.equal(typedAnswer({ Cortex: "  " }, actions), undefined);
assert.equal(typedAnswer({ Cortex: "Neither, do C" }, actions), "Neither, do C");

// A fake stock bridge: records what it is sent, answers configure requests.
const sent: any[] = [];
const bridgeListeners = new Set<(event: any) => void>();
const emit = (event: any) => bridgeListeners.forEach((listener) => listener(event));
let failConfigure = false;
const connection: any = {
  capabilities: [],
  async send(input: any) {
    sent.push(input);
    if (input.type === "session.configure") {
      queueMicrotask(() => emit(failConfigure
        ? { type: "request.failed", requestId: input.requestId, error: { message: "Unknown question." } }
        : { type: "request.completed", requestId: input.requestId }));
    }
  },
  onEvent(listener: (event: any) => void) {
    bridgeListeners.add(listener);
    return () => bridgeListeners.delete(listener);
  },
  async close() {},
};
const wrapped = wrapCortexConnection(connection, []);
const seen: any[] = [];
wrapped.onEvent((event) => seen.push(event));

function card(permissionId: string, questionId: string) {
  emit({
    type: "session.permission", sessionId: "s",
    request: {
      id: permissionId, name: "Approve?", kind: "tool", title: "Approve?",
      input: { [CORTEX_QUESTION_INPUT_MARKER]: true, questionId, question: "Approve?" },
      actions: [{ id: "cortex-question-1", label: "Approve", behavior: "allow" },
        { id: "cortex-question-2", label: "Finish", behavior: "allow" }],
    },
  });
}
const answer = (permissionId: string, text: string) => wrapped.send({
  type: "session.permission", sessionId: "s", permissionId,
  response: { behavior: "allow", updatedInput: { answers: { Cortex: text } } },
} as any);

// The card offers Paseo's "Other..." field.
card("permission:q1", "pq_1");
const question = seen.find((event) => event.type === "session.permission");
assert.equal(question.request.kind, "question");
assert.equal(question.request.input.questions[0].allowOther, true);

// A typed answer reaches Cortex first, shows in the chat, then retires the card.
await answer("permission:q1", "Neither, do C");
assert.deepEqual(sent.map((input) => input.type), ["session.configure", "session.permission"]);
assert.deepEqual(sent[0].changes.settings, {
  [QUESTION_ANSWER_CONFIG_ID]: JSON.stringify({ questionId: "pq_1", answer: "Neither, do C" }),
});
assert.deepEqual(sent[1].response, { behavior: "deny" });
assert.ok(seen.some((event) => event.type === "timeline.item" && event.item.type === "user_message"
  && event.item.text === "Neither, do C"));
assert.ok(!seen.some((event) => event.type?.startsWith("request.")));

// An option is still answered as that option.
sent.length = 0;
card("permission:q2", "pq_2");
await answer("permission:q2", "2. Finish");
assert.deepEqual(sent.map((input) => [input.type, input.response?.selectedActionId]), [["session.permission", "cortex-question-2"]]);

// A refused answer leaves the card open for another try.
sent.length = 0;
failConfigure = true;
card("permission:q3", "pq_3");
await assert.rejects(answer("permission:q3", "Other text"), /Unknown question/);
assert.deepEqual(sent.map((input) => input.type), ["session.configure"]);
failConfigure = false;
await answer("permission:q3", "Other text");
assert.deepEqual(sent.map((input) => input.type), ["session.configure", "session.configure", "session.permission"]);
assert.deepEqual(sent[2].response, { behavior: "deny" });
console.log("ok");
