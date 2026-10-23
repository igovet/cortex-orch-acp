/** No-paid installed QuestionFormCard structure/state check; not desktop qualification. */
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { runInNewContext } from "node:vm";
import { batchForm, parseQuestionBatch, batchAnswers } from "../shared/question-batch.ts";
const path = process.argv[2];
assert.ok(path, "Supply the currently installed Paseo Metro bundle path");
const bundle = readFileSync(path, "utf8");
// Discover module identities from the installed registration table, not a
// version-specific dependency position. Only selected factories execute; the
// application and its transitive imports never run.
const modules = new Map<number, string>();
const starts = [...bundle.matchAll(/(?:^|\n)__d\(/g)].map((match) => match.index + (match[0].startsWith("\n") ? 1 : 0));
for (let index = 0; index < starts.length; index++) {
  const source = bundle.slice(starts[index], starts[index + 1] ?? bundle.length).trim();
  const registration = source.match(/\},(\d+),\[[\d,]*\]\);?$/);
  if (registration) {
    const id = Number(registration[1]);
    assert.ok(!modules.has(id), "Duplicate installed Metro module identity");
    modules.set(id, source);
  }
}
function moduleAt(marker: string): number {
  const matches = [...modules].filter(([, source]) => source.includes(marker));
  assert.equal(matches.length, 1, `Expected one installed module for ${marker}`);
  return matches[0][0];
}
function registered(id: number) {
  const source = modules.get(id);
  assert.ok(source, `Missing complete installed Metro module ${id}`);
  let factory: any, ids: number[] = [];
  runInNewContext(source, { __d(fn: any, actualId: number, imports: number[]) {
    assert.equal(actualId, id);
    factory = fn; ids = imports;
  } }, { timeout: 1000 });
  return { factory, ids };
}
function exportNames(id: number, visited = new Set<number>()): Set<string> {
  assert.ok(!visited.has(id), "Cyclic installed export forwarding");
  visited.add(id);
  const { factory, ids } = registered(id);
  const source = factory.toString();
  const args = source.slice(source.indexOf("(") + 1, source.indexOf(")")).split(",");
  const [requireArg, moduleArg, exportArg, depsArg] = [args[1], args[4], args[5], args[6]];
  // Metro's React/compiler/JSX entry modules forward their sole dependency.
  if (ids.length === 1 && source.includes(`${moduleArg}.exports=${requireArg}(${depsArg}[0])`)) {
    return exportNames(ids[0], visited);
  }
  const escaped = exportArg.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  return new Set([
    ...[...source.matchAll(new RegExp(`\\b${escaped}\\.([\\w$]+)\\s*=`, "g"))].map((match) => match[1]),
    ...[...source.matchAll(new RegExp(`Object\\.defineProperty\\(${escaped},["']([^"']+)["']`, "g"))].map((match) => match[1]),
  ]);
}
type StandIn = { names: string[]; value: any; exact?: boolean };
function load(marker: string, standIns: StandIn[]) {
  const { factory, ids } = registered(moduleAt(marker));
  const dependencies = new Map<number, any>();
  for (const standIn of standIns) {
    const matches = ids.filter((id) => {
      const names = exportNames(id);
      names.delete("__esModule");
      return standIn.names.every((name) => names.has(name)) && (!standIn.exact || names.size === standIn.names.length);
    });
    assert.equal(matches.length, 1, `Installed dependency shape changed: ${standIn.names.join(", ")}`);
    assert.ok(!dependencies.has(matches[0]), "Ambiguous installed dependency stand-in");
    dependencies.set(matches[0], standIn.value);
  }
  const exports: any = {};
  factory({}, (id: number) => {
    assert.ok(ids.includes(id), "Unexpected installed import");
    // Side-effect-only imports need no stand-in. Any newly used export fails
    // loudly instead of silently returning an empty object or guessed value.
    return dependencies.get(id) ?? new Proxy({}, { get(_target, name) {
      throw new Error(`Unmapped installed dependency ${id} export ${String(name)}`);
    } });
  }, undefined, undefined, {}, exports, ids);
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
// A missing/changed export must fail closed, not produce a passing empty form.
assert.throws(() => load("e.QuestionFormCard=function", [
  { names: ["__cortex_missing_export__"], value: {} },
]), /Installed dependency shape changed/);
const component = load("e.QuestionFormCard=function", [
  { names: ["Pressable"], exact: true, value: { Pressable: "Pressable" } },
  { names: ["Text"], exact: true, value: { Text: "Text" } },
  { names: ["View"], exact: true, value: { View: "View" } },
  { names: ["c"], value: { c: (size: number) => Array(size).fill(Symbol.for("react.memo_cache_sentinel")) } },
  { names: ["useState", "useRef"], value: react },
  { names: ["useUnistyles", "StyleSheet"], value: unistyles },
  { names: ["useIsCompactFormFactor"], value: { useIsCompactFormFactor: () => compact } },
  { names: ["Check", "X"], value: { Check: "Check", X: "X" } },
  { names: ["useTranslation"], value: { useTranslation: () => ({ t: (key: string) => key }) } },
  { names: ["isWeb", "isNative"], value: { isWeb: true } },
  { names: ["EditingTextInput"], value: { EditingTextInput: "EditingTextInput" } },
  { names: ["parseQuestionFormQuestions"], value: helper },
  { names: ["jsx", "jsxs"], value: { jsx, jsxs: jsx } },
  { names: ["Alert"], value: { Alert: "Alert" } },
  { names: ["LoadingSpinner"], value: { LoadingSpinner: "LoadingSpinner" } },
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
