import { render } from "solid-js/web";
import { afterEach, expect, it } from "vitest";
import { emptySession, emptyWorkspace, LOCAL_MACHINE_ID, type Activity, type Worker } from "@cortex/workspace-domain";
import { KNOWN_CORTEX_TOOL_NAMES } from "../../../integrations/paseo/shared/tool-presentation";
import { isHiddenWorkerEvent } from "../../../integrations/paseo/shared/timeline-visibility";
import { Workspace } from "../src";
import { feedOf, isHiddenActivity, oneRowPerCall, STEP_PREFIX, toolLabel, viewOfActivity, viewOfText } from "../src/tool-names";
import { flush, projection, scriptedPort, withTask } from "./support";

let dispose: (() => void) | undefined;
afterEach(() => { dispose?.(); document.body.innerHTML = ""; localStorage.clear(); });
const settle = async () => { for (let i = 0; i < 8; i++) await flush(); };
async function mount(path: string, state: ReturnType<typeof emptyWorkspace>) {
  state.machines[0].connection = "connected";
  const p = scriptedPort({ capabilities: { everything: true, operations: [] } });
  history.replaceState(null, "", path);
  const root = document.createElement("div"); document.body.append(root);
  dispose = render(() => <Workspace port={p.port} />, root);
  await settle(); p.emit(projection("1", state)); await settle();
  return p;
}
const live = (activity: Activity[], patch: Partial<Worker> = {}): Worker => ({ id: "w", taskId: "t", executionId: "e", role: "frontend-dev", status: "working", lane: "execution", goal: "Build", now: null, waitsFor: null, scope: [], route: null, attempt: null, elapsedMs: null, runningSince: null, context: null, usage: null, result: null, activity, ...patch });
const command = (sequence: number, status = "in_progress"): Activity => ({ sequence, kind: "command", text: `step ${sequence}`, at: null, args: JSON.stringify({ kind: "execute", title: "Bash", command: `step ${sequence}`, output: "ok", status }), command: { state: "native", exit: null, took: null, tail: "ok", reason: null } });
/** A reader scrolled away from the bottom, in a test DOM that has no layout. */
function readingAt(box: HTMLElement, position: number) {
  let top = position;
  Object.defineProperties(box, { clientHeight: { configurable: true, get: () => 400 }, scrollHeight: { configurable: true, get: () => 2000 }, scrollTop: { configurable: true, get: () => top, set: (v: number) => { top = v; } } });
  box.dispatchEvent(new Event("scroll"));
  return () => top;
}
const working = (title: string) => { const s = withTask(emptyWorkspace(), title); s.tasks[0].status = "working"; s.tasks[0].process = "live"; return s; };

it("keeps a worker's block and step open, and the reader in place, while the block grows, rolls out of the window and finishes", async () => {
  const state = working("Live");
  state.workers = [live([1, 2, 3, 4, 5].map(n => command(n)))];
  const p = await mount(`/m/${LOCAL_MACHINE_ID}/t/t/workers/w`, structuredClone(state));
  const top = readingAt(document.querySelector<HTMLElement>(".stream")!, 300);
  const fold = document.querySelector<HTMLButtonElement>("button.fold")!;
  fold.click(); await settle();
  const step = [...document.querySelectorAll<HTMLButtonElement>("button.step-row")].at(-1)!;
  step.click(); await settle();
  expect(step.getAttribute("aria-expanded")).toBe("true");
  for (let n = 6; n <= 9; n++) {
    state.workers[0].activity = [...state.workers[0].activity.slice(1), command(n)];
    state.workers[0].activity[3] = command(5, "completed");
    p.emit(projection(String(n), structuredClone(state))); await settle();
    expect(document.querySelector("button.fold")).toBe(fold);
    expect(fold.getAttribute("aria-expanded")).toBe("true");
    expect(step.isConnected).toBe(true);
    expect(step.getAttribute("aria-expanded")).toBe("true");
    expect(document.querySelector(".ws-step-detail .args")?.textContent).toBe("ok");
    expect(top()).toBe(300);
  }
  state.workers[0].status = "completed";
  p.emit(projection("10", structuredClone(state))); await settle();
  expect(fold.isConnected).toBe(true);
  expect(fold.getAttribute("aria-expanded")).toBe("true");
  expect(step.getAttribute("aria-expanded")).toBe("true");
  expect(top()).toBe(300);
});

it("keeps a Conversation fold and step open while the step's recorded status changes", async () => {
  const state = working("Main");
  const session = emptySession("t");
  const step = (status: string, output?: string) => STEP_PREFIX + JSON.stringify({ callId: "c1", title: "Bash", kind: "execute", command: "npm test", status, output });
  session.timeline = [{ id: "history:a:1", kind: "fold", text: "Ran 1 command", steps: [step("in_progress")] }];
  state.sessions = [session];
  const p = await mount(`/m/${LOCAL_MACHINE_ID}/t/t`, structuredClone(state));
  const top = readingAt(document.querySelector<HTMLElement>(".timeline")!, 300);
  const fold = document.querySelector<HTMLButtonElement>("button.fold")!;
  fold.click(); await settle();
  const row = document.querySelector<HTMLButtonElement>("button.step-row")!;
  row.click(); await settle();
  session.timeline = [{ id: "history:a:1", kind: "fold", text: "Worked 3s, ran 1 command", steps: [step("completed", "ok"), STEP_PREFIX + JSON.stringify({ callId: "c2", title: "Read", kind: "read", path: "a.py" })] }];
  p.emit(projection("2", structuredClone(state))); await settle();
  expect(document.querySelector("button.fold")).toBe(fold);
  expect(fold.getAttribute("aria-expanded")).toBe("true");
  expect(document.querySelector("button.step-row")).toBe(row);
  expect(row.getAttribute("aria-expanded")).toBe("true");
  expect(document.querySelector(".ws-step-detail .args")?.textContent).toBe("ok");
  expect(top()).toBe(300);
});

it("gives a block the same identity when its first steps roll away or older steps load above it", () => {
  const step = (sequence: number) => ({ sequence, kind: "tool", text: "Read", at: null });
  const ids = new Map<number, string>();
  const key = (list: number[]) => feedOf(list.map(step), ids).map(x => x.key);
  const first = key([3, 4, 5]);
  expect(key([4, 5, 6])).toEqual(first);
  expect(key([1, 2, 3, 4, 5, 6, 7])).toEqual(first);
});

it("merges one call's updates into one row at the first update's place", () => {
  const update = (sequence: number, status: string) => ({ sequence, kind: "tool", text: "Read", at: sequence, toolCallRef: "c1", args: JSON.stringify({ status }) });
  const merged = oneRowPerCall<{ sequence: number; kind: string; text: string; at: number; toolCallRef?: string; args?: string }>([update(1, "pending"), { sequence: 2, kind: "note", text: "Between", at: 2 }, update(3, "completed")]);
  expect(merged.map(a => a.sequence)).toEqual([1, 2]);
  expect(JSON.parse(merged[0].args!).status).toBe("completed");
  expect(oneRowPerCall([{ sequence: 1, kind: "tool", text: "Read", at: 1 }, { sequence: 2, kind: "tool", text: "Read", at: 2 }])).toHaveLength(2);
});

it("opens only rows that have something to show, as plain lines otherwise", async () => {
  const state = working("Rows");
  state.workers = [live([
    { sequence: 1, kind: "tool", text: "Read", at: null, args: JSON.stringify({ kind: "read", title: "Read", path: "src/a.py" }) },
    { sequence: 2, kind: "tool", text: "mcp__codebase-memory-mcp__get_code_snippet", at: null, args: JSON.stringify({ kind: "other", title: "mcp__codebase-memory-mcp__get_code_snippet" }) },
    { sequence: 3, kind: "tool", text: "custom_unknown_tool", at: null, error: true },
    { sequence: 4, kind: "tool", text: "Tool activity", at: null, args: "{}" },
    command(5),
    { sequence: 6, kind: "edit", text: "Edit", at: null, args: JSON.stringify({ kind: "edit", title: "Edit", path: "b.py", oldText: "a", newText: "b" }) },
  ])];
  await mount(`/m/${LOCAL_MACHINE_ID}/t/t/workers/w`, structuredClone(state));
  document.querySelector<HTMLButtonElement>("button.fold")!.click(); await settle();
  const rows = [...document.querySelectorAll<HTMLElement>(".stream .step-row")];
  expect(rows.map(r => r.querySelector(".step-l")?.textContent)).toEqual(["Read", "Get Code Snippet", "Custom Unknown Tool", "Tool activity", "Shell", "Edit"]);
  expect(rows.map(r => r.tagName)).toEqual(["DIV", "DIV", "DIV", "DIV", "BUTTON", "BUTTON"]);
  for (const plain of rows.slice(0, 4)) { expect(plain.hasAttribute("aria-expanded")).toBe(false); expect(plain.getAttribute("tabindex")).toBeNull(); }
  expect(document.querySelector(".stream")!.textContent).not.toMatch(/mcp[._]|[a-z]+_[a-z]+_[a-z]+/);
});

it("never turns a machine name into a label", () => {
  for (const name of ["mcp__x__y_z", "mcp.server.some_tool", "get_code_snippet", "mcp__codebase_memory_mcp__get_code_snippet"]) {
    const views = [viewOfText(name), viewOfText(`${STEP_PREFIX}${JSON.stringify({ title: name, status: "failed" })}`), viewOfActivity({ sequence: 1, kind: "note", text: name, at: null })];
    for (const v of views) expect(`${v.label} ${v.target}`).not.toMatch(/mcp[._]|_/);
    expect(toolLabel(name)).not.toMatch(/mcp[._]|_/);
  }
});

it("shows and hides exactly what Paseo does", () => {
  const noise = ["agent_thought_chunk", "thought", "thinking", "reasoning", "wait", "wait_agent", "task_notification", "usage_update", "current_mode_update", "config_option_update", "available_commands_update", "session_info_update", "turn_started", "turn_complete"];
  for (const title of [...KNOWN_CORTEX_TOOL_NAMES, ...noise, "tools_discover", "toolsearch"].flatMap(n => [n, `mcp__cortex__${n}`]))
    expect(isHiddenActivity({ text: title })).toBe(isHiddenWorkerEvent({ title }));
  // Paseo judges the kind too: an untitled thought or context figure is not a step.
  for (const tool_kind of ["agent_thought_chunk", "usage_update"]) {
    expect(isHiddenWorkerEvent({ tool_kind })).toBe(true);
    expect(isHiddenActivity({ text: "Considering the plan", args: JSON.stringify({ title: "Tool activity", kind: tool_kind }) })).toBe(true);
  }
  expect(isHiddenActivity({ text: "Read", args: JSON.stringify({ title: "Read", kind: "read" }) })).toBe(false);
});
