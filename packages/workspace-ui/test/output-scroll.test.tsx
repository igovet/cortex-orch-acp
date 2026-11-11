import { existsSync, readFileSync } from "node:fs";
import { render } from "solid-js/web";
import { afterEach, expect, it } from "vitest";
import { emptySession, emptyWorkspace, LOCAL_MACHINE_ID, type Activity, type Worker } from "@cortex/workspace-domain";
import { Workspace } from "../src";
import { STEP_PREFIX } from "../src/tool-names";
import { flush, projection, scriptedPort, withTask } from "./support";

// A step's recorded output keeps its long lines and scrolls sideways inside its own box, which keys can reach and move.
const SRC = existsSync("src/workspace.css") ? "src/" : "packages/workspace-ui/src/";
const LONG = Array.from({ length: 6 }, (_, i) => `packages/workspace-ui/test/a-file-${i}.test.tsx:${i + 10}:    state.tasks.push({ ...draftTask(), id: "t-${i}", title: "${"a long title ".repeat(12)}" })`).join("\n");
let dispose: (() => void) | undefined;
afterEach(() => { dispose?.(); document.body.innerHTML = ""; document.head.innerHTML = ""; localStorage.clear(); });
const settle = async () => { for (let i = 0; i < 8; i++) await flush(); };
async function mount(path: string, state: ReturnType<typeof emptyWorkspace>) {
  const s = document.createElement("style");
  s.textContent = readFileSync(SRC + "mockup-styles.css", "utf8") + readFileSync(SRC + "workspace.css", "utf8");
  document.head.append(s);
  state.machines[0].connection = "connected";
  const p = scriptedPort({ capabilities: { everything: true, operations: [] } });
  history.replaceState(null, "", path);
  const root = document.createElement("div"); document.body.append(root);
  dispose = render(() => <Workspace port={p.port} />, root);
  await settle(); p.emit(projection("1", state)); await settle();
}
const working = () => { const s = withTask(emptyWorkspace(), "Output"); s.tasks[0].status = "working"; s.tasks[0].process = "live"; return s; };
async function openAll() {
  for (const b of document.querySelectorAll<HTMLButtonElement>("button.fold[aria-expanded=false]")) b.click();
  await settle();
  for (const b of document.querySelectorAll<HTMLButtonElement>("button.step-row[aria-expanded=false]")) b.click();
  await settle();
}
function expectScrollable(box: HTMLElement, label: string) {
  expect(box.tabIndex).toBe(0);
  expect(box.getAttribute("role")).toBe("region");
  expect(box.getAttribute("aria-label")).toBe(label);
  const style = getComputedStyle(box);
  expect({ x: style.overflowX, space: style.whiteSpace }).toEqual({ x: "auto", space: "pre" });
}

it("scrolls a worker step's long output, tail and diff sideways inside a focusable box", async () => {
  const state = working();
  const w: Worker = { id: "w", taskId: "t", executionId: "e", role: "frontend-dev", status: "working", lane: "execution", goal: "Build", now: null, waitsFor: null, scope: [], route: null, attempt: null, elapsedMs: null, runningSince: null, context: null, usage: null, result: null, activity: [] };
  const activity: Activity[] = [
    { sequence: 1, kind: "command", text: "grep", at: null, args: JSON.stringify({ kind: "execute", title: "Bash", command: "grep -rn plan packages/", output: LONG, status: "completed" }), command: { state: "native", exit: 0, took: null, tail: LONG, reason: null } },
    { sequence: 2, kind: "command", text: "grep again", at: null, command: { state: "passed", exit: 0, took: "1s", tail: LONG, reason: null } },
    { sequence: 3, kind: "edit", text: "Edited a.ts", at: null, diff: { add: 1, del: 1, lines: ["@@ -1 +1 @@", "-" + LONG.split("\n")[0], "+" + LONG.split("\n")[1]] } },
  ];
  state.workers = [{ ...w, activity }];
  await mount(`/m/${LOCAL_MACHINE_ID}/t/t/workers/w`, structuredClone(state));
  await openAll();
  const boxes = [...document.querySelectorAll<HTMLElement>(".ws-out")];
  expect(boxes.map(b => b.getAttribute("aria-label")).sort()).toEqual(["Command output", "Command output", "Diff"]);
  for (const box of boxes) expectScrollable(box, box.getAttribute("aria-label")!);
  expect(document.querySelector(".ws-step-detail .ws-out")?.textContent).toBe(LONG);
  expect(document.querySelector(".tail.ws-out")?.textContent).toBe(LONG);
});

it("scrolls a Conversation step's long output sideways inside a focusable box", async () => {
  const state = working();
  const session = emptySession("t");
  session.timeline = [{ id: "history:a:1", kind: "fold", text: "Ran 1 command", steps: [STEP_PREFIX + JSON.stringify({ callId: "c1", title: "Bash", kind: "execute", command: "grep -rn plan packages/", status: "completed", output: LONG })] }];
  state.sessions = [session];
  await mount(`/m/${LOCAL_MACHINE_ID}/t/t`, structuredClone(state));
  await openAll();
  const box = document.querySelector<HTMLElement>(".ws-step-detail .ws-out")!;
  expect(box.textContent).toBe(LONG);
  expectScrollable(box, "Command output");
  box.focus();
  expect(document.activeElement).toBe(box);
});
