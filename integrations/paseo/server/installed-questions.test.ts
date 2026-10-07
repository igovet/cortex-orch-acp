/** No-paid installed QuestionFormCard structure/state check; not desktop qualification. */
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { runInNewContext } from "node:vm";
import { batchForm, parseQuestionBatch, batchAnswers } from "../shared/question-batch.ts";
const path = process.argv[2];
assert.ok(path, "Supply the currently installed Paseo Metro bundle path");
const bundle = readFileSync(path, "utf8");
function load(marker: string, dependencies: any[]) {
  const at = bundle.indexOf(marker), start = bundle.lastIndexOf("__d(", at), end = bundle.indexOf("\n__d(", at);
  assert.ok(at >= 0 && start >= 0 && end > at);
  let factory: any, ids: number[] = [];
  runInNewContext(bundle.slice(start, end), { __d(fn: any, _id: number, imports: number[]) { factory = fn; ids = imports; } }, { timeout: 1000 });
  assert.equal(ids.length, dependencies.length);
  const exports: any = {};
  factory({}, (id: number) => dependencies[ids.indexOf(id)], undefined, undefined, {}, exports, ids);
  return exports;
}
const helper = load("e.parseQuestionFormQuestions=function", []);
const batch = parseQuestionBatch({ batchId: "qb_test", questionId: "pq_a", questions: [
  { questionId: "pq_a", question: "Layout?", topic: "Interface", options: ["Compact", "Spacious"] },
  { questionId: "pq_b", question: "Theme?", topic: "Interface", options: ["Light", "Dark"] },
] });
const input = batchForm(batch);
assert.deepEqual(input.questions.map((q) => q.header), ["Interface", "Interface 2"]);
const parsed = helper.parseQuestionFormQuestions(input);
assert.equal(parsed.length, 2);
assert.equal(helper.areQuestionsAnswered(parsed, { 0: new Set([0]) }, {}), false);
assert.equal(helper.areQuestionsAnswered(parsed, { 0: new Set([0]) }, { 1: "custom" }), true);
assert.deepEqual(batchAnswers(batch, helper.buildQuestionFormAnswers(parsed, { 0: new Set([0]) }, { 1: "2. keep numeric text" })),
  [{ questionId: "pq_a", answer: "Compact" }, { questionId: "pq_b", answer: "2. keep numeric text" }]);
// Execute the actual installed component with JSX/React Native stand-ins.
// This inspects its element tree, not pixel rendering, focus or a real host session.
let states: any[] = [], cursor = 0;
const react = { useState(initial: any) {
  const index = cursor++;
  if (!(index in states)) states[index] = initial;
  return [states[index], (update: any) => { states[index] = typeof update === "function" ? update(states[index]) : update; }];
}, useRef: () => ({ current: null }), useEffect: () => {}, useCallback: (fn: any) => fn };
const jsx = (type: any, props: any) => ({ type, props });
const theme = new Proxy({}, { get: () => "token" });
const unistyles = { useUnistyles: () => ({ theme: { colors: theme } }), StyleSheet: { create: () => ({}) } };
let compact = false, responding = false;
const component = load("e.QuestionFormCard=function", [
  { Pressable: "Pressable" }, { Text: "Text" }, { View: "View" },
  { c: (size: number) => Array(size).fill(Symbol.for("react.memo_cache_sentinel")) }, {}, react,
  {}, {}, {}, unistyles, { useIsCompactFormFactor: () => compact }, { Check: "Check", X: "X" },
  { useTranslation: () => ({ t: (key: string) => key }) }, { isWeb: true }, {}, helper, { jsx, jsxs: jsx },
]);
const replies: any[] = [];
function render() {
  cursor = 0;
  return component.QuestionFormCard({ permission: { request: { input } }, onRespond: (answer: any) => replies.push(answer), isResponding: responding });
}
function nodes(tree: any): any[] {
  if (!tree || typeof tree !== "object") return [];
  if (Array.isArray(tree)) return tree.flatMap(nodes);
  return tree.props ? [tree, ...nodes(tree.props.children)] : [];
}
let tree = nodes(render());
assert.ok(tree.some((node) => node.props.testID === "question-form-current-question" && node.props.children === "Layout?"));
assert.ok(tree.some((node) => node.props.accessibilityRole === "radiogroup" && node.props.accessibilityLabel === "Layout?"));
assert.deepEqual(tree.filter((node) => node.props.option).map((node) => node.props.option.label), ["Compact", "Spacious"]);
assert.ok(tree.some((node) => node.props.onChange && node.props.accessibilityLabel === "Layout?"));
assert.equal(tree.find((node) => node.props.testID === "question-form-primary-action").props.disabled, true);
assert.ok(tree.some((node) => node.props.questions?.length === 2)); // one paginated form, not two cards
// Select first answer; the real component advances to the second question.
tree.find((node) => node.props.option).props.onToggle(0, 0, false);
tree = nodes(render());
assert.ok(tree.some((node) => node.props.testID === "question-form-current-question" && node.props.children === "Theme?"));
assert.equal(tree.find((node) => node.props.testID === "question-form-primary-action").props.disabled, true);
tree.find((node) => node.props.onChange && node.props.accessibilityLabel === "Theme?").props.onChange(1, "Custom theme");
tree = nodes(render());
const submit = tree.find((node) => node.props.testID === "question-form-primary-action");
assert.equal(submit.props.disabled, false);
submit.props.onPress();
assert.equal(replies.length, 1);
assert.deepEqual(batchAnswers(batch, replies[0].updatedInput.answers), [{ questionId: "pq_a", answer: "Compact" }, { questionId: "pq_b", answer: "Custom theme" }]);
responding = true;
assert.equal(nodes(render()).find((node) => node.props.testID === "question-form-primary-action").props.disabled, true);
responding = false; compact = true;
const compactTree = nodes(render());
assert.ok(compactTree.some((node) => node.props.questions?.length === 2));
assert.equal(compactTree.find((node) => node.props.testID === "question-form-primary-action").props.disabled, false);
compactTree.find((node) => node.props.testID === "question-form-dismiss").props.onPress();
assert.equal(replies.at(-1).behavior, "deny");
console.log("installed aggregate form structure and all-member submission ok");
