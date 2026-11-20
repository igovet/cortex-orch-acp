import { render } from "solid-js/web";
import { afterEach, expect, it, vi } from "vitest";
import { emptySession, emptyWorkspace, LOCAL_MACHINE_ID, type Decision, type Task, type TimelineEntry, type Worker, type WorkspaceState } from "@cortex/workspace-domain";
import { Workspace } from "../src";
import { conversationOf } from "../src/port/history";
import { editDiff, STEP_PREFIX, viewOfText } from "../src/tool-names";
import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { flush, projection, scriptedPort, withTask } from "./support";

// Defects seen in a paid live run of the dev build in a real window: edit rows without a diff, two messages run together,
// a one-character "Read ." link, a refused plan with no reason, a stopped team called finished, a button over a worker's
// checks, and an answer line that moved the page.
let dispose: (() => void) | undefined;
afterEach(() => { dispose?.(); dispose = undefined; document.body.innerHTML = ""; localStorage.clear(); vi.unstubAllGlobals(); });
const settle = async () => { for (let i = 0; i < 8; i++) await flush(); };
const step = (d: Record<string, string>) => STEP_PREFIX + JSON.stringify(d);
const row = (i: number, kind: string, text: string, toolStatus: string | null = null) => ({ historyId: `h${i}`, revision: "1", kind, text, toolStatus, recordedAt: `2026-11-17T02:12:${String(i).padStart(2, "0")}Z` });

function world(entries: TimelineEntry[], patch: Partial<Task> = {}, workers: Worker[] = [], decisions: Decision[] = []): WorkspaceState {
  const state = withTask(emptyWorkspace(), "Live defects");
  Object.assign(state.tasks[0], { status: "working", process: "live", executionId: "e" }, patch);
  state.machines[0].connection = "connected";
  const session = emptySession("t");
  session.timeline = entries; session.decisions = decisions;
  state.sessions = [session]; state.workers = workers;
  state.repos = [{ projectId: "p", remote: "", tree: ["README.md"], sources: {}, commits: [], branches: [], fetchedAt: null, checkouts: [{ id: "main", projectId: "p", kind: "main", path: "/fixture", branch: "main", files: [], sources: {}, taskId: null, label: null, base: "main", ahead: 0, behind: 0, behindOf: {}, draft: "", updateConflict: null }] }];
  return state;
}
async function mount(state: WorkspaceState, path = `/m/${LOCAL_MACHINE_ID}/t/t`) {
  const p = scriptedPort({ capabilities: { everything: true, operations: [] } });
  p.port.readFile = async () => ({ text: "# Readme\n", revision: "fixture" });
  history.replaceState(null, "", path);
  const root = document.createElement("div"); document.body.append(root);
  dispose = render(() => <Workspace port={p.port} />, root);
  await settle(); p.emit(projection("1", state)); await settle();
  return p;
}
const openFold = async () => { document.querySelector<HTMLButtonElement>(".timeline button.fold")!.click(); await settle(); };
const stepRow = (label: string, scope = ".timeline") => [...document.querySelectorAll<HTMLElement>(`${scope} .step-row`)].find(r => r.querySelector(".step-l")?.textContent === label)!;

// claude-agent-acp names a write's call first and sends its input in later untitled updates; the history merges them by call id.
const claudeWrite = [
  row(1, "tool", step({ title: "worker-technical-writer: mcp__cortex__project_write", callId: "xa_w:w1", kind: "other", status: "pending" }), "pending"),
  row(2, "tool", step({ callId: "xa_w:w1", kind: "other", path: "ARCHITECTURE.md", newText: "# Architecture\n\n- src/cli.js: entry point\n" })),
  row(3, "tool", step({ callId: "xa_w:w1", kind: "other", status: "completed" }), "completed"),
];

it("opens a Conversation edit row on its diff, with its counts on the row", async () => {
  const [fold] = conversationOf(claudeWrite, { description: "", answers: [] });
  if (fold.entry.kind !== "fold") throw new Error("not a fold");
  expect(fold.entry.text).toBe("Edited 1 file");
  await mount(world([fold.entry]));
  await openFold();
  const edit = stepRow("Edit");
  expect(edit.tagName).toBe("BUTTON");
  expect(edit.querySelector(".step-t")?.textContent).toBe("ARCHITECTURE.md");
  expect(edit.querySelector(".step-r")?.textContent).toBe("+3 −0");
  edit.click(); await settle();
  const box = document.querySelector('.timeline [role="region"][aria-label="Edit"]')!;
  // The diff is open with the row: no second "View edit" to find.
  expect(document.querySelector(".timeline details")).toBeNull();
  expect([...box.querySelectorAll(".a")].map(l => l.textContent)).toEqual(["+# Architecture", "+", "+- src/cli.js: entry point"]);
});

it("keeps an edit with no recorded text a plain row that opens its file, never an empty diff", async () => {
  const bare = step({ title: "worker-technical-writer: mcp__cortex__project_edit", callId: "xa_w:e1", kind: "other", status: "completed", path: "README.md" });
  await mount(world([{ id: "history:f:1", kind: "fold", text: "Edited 1 file", steps: [bare] }]));
  await openFold();
  const edit = stepRow("Edit");
  expect(edit.tagName).toBe("DIV");
  expect(edit.querySelector("button.step-t")?.textContent).toBe("README.md");
  expect(document.querySelector('.timeline [aria-label="Edit"]')).toBeNull();
});

it("shows an edit as its changed lines with a little context", () => {
  expect(editDiff({ oldText: "a\nb\nc\nd\ne", newText: "a\nb\nC\nd\ne" })).toEqual({ add: 1, del: 1, lines: [" a", " b", "-c", "+C", " d", " e"] });
  expect(editDiff({ oldText: "same", newText: "same" })).toBeNull();
  expect(editDiff({})).toBeNull();
});

const editActivity = (lines: string[]): Worker["activity"][number] => ({ sequence: 7, kind: "edit", text: "Edited", code: "ARCHITECTURE.md", at: null, diff: { add: 21, del: 0, lines } });
const writer = (activity: Worker["activity"], patch: Partial<Worker> = {}): Worker => ({ id: "w", taskId: "t", executionId: "e", role: "technical-writer", status: "completed", lane: "execution", goal: "Write ARCHITECTURE.md",
  now: null, waitsFor: null, scope: [], route: null, attempt: null, elapsedMs: null, runningSince: null, context: null, usage: null, activity,
  result: { summary: "Created ARCHITECTURE.md.", checks: [{ name: "node test/counter.test.js", outcome: "passed", detail: "", source: "worker" }] }, ...patch } as Worker);

it("never draws an empty Diff box on a worker's page: no lines means counts on a plain row", async () => {
  await mount(world([], { status: "review" }, [writer([editActivity([])])]), `/m/${LOCAL_MACHINE_ID}/t/t/workers/w`);
  document.querySelector<HTMLButtonElement>(".livepane button.fold")!.click(); await settle();
  const edit = stepRow("Edit", ".livepane");
  expect(edit.tagName).toBe("DIV");
  expect(edit.querySelector(".step-r")?.textContent).toBe("+21 −0");
  expect(edit.querySelector("button.step-t")?.textContent).toBe("ARCHITECTURE.md");
  expect(document.querySelector('.livepane [aria-label="Diff"]')).toBeNull();
});

it("opens a worker's recorded edit on its changed lines", async () => {
  const lines = ["@@ -0,0 +1,2 @@", "+# Architecture", "+"];
  await mount(world([], { status: "review" }, [writer([editActivity(lines)])]), `/m/${LOCAL_MACHINE_ID}/t/t/workers/w`);
  document.querySelector<HTMLButtonElement>(".livepane button.fold")!.click(); await settle();
  const edit = stepRow("Edit", ".livepane");
  expect(edit.tagName).toBe("BUTTON");
  edit.click(); await settle();
  const box = document.querySelector('.livepane [aria-label="Diff"]')!;
  expect([...box.children].map(l => [l.className, l.textContent])).toEqual([["h", "@@ -0,0 +1,2 @@"], ["a", "+# Architecture"], ["a", "+"]]);
});

it("puts Ask about this worker on the finished worker's result card, not in a floating dock over its checks", async () => {
  await mount(world([], { status: "review" }, [writer([editActivity([])])]), `/m/${LOCAL_MACHINE_ID}/t/t/workers/w`);
  const ask = [...document.querySelectorAll<HTMLButtonElement>(".livepane button")].find(b => b.textContent === "Ask about this worker")!;
  expect(ask.closest(".ws-worker-result")).not.toBeNull();
  expect(document.querySelector(".livepane .d-dock")).toBeNull();
  // The card's checks come before the button, inside the same scrolled feed.
  const card = ask.closest(".ws-worker-result")!;
  expect(card.textContent!.indexOf("node test/counter.test.js")).toBeLessThan(card.textContent!.indexOf("Ask about this worker"));
  ask.click(); await settle();
  expect(location.pathname).toBe(`/m/${LOCAL_MACHINE_ID}/t/t`);
});

it("keeps a coordinator line and the next message apart, while one streamed message stays one", async () => {
  // Display history marks a chunk that begins a different message with a paragraph break (cortex-core history projection).
  const rows = [row(1, "assistant", "Индексирую `ARCH"), row(2, "assistant", "ITECTURE.md`."), row(3, "assistant", "\n\nI"), row(4, "assistant", "'ll start by checking the index.")];
  const [entry] = conversationOf(rows, { description: "", answers: [] });
  if (entry.entry.kind !== "cortex") throw new Error("not a message");
  expect(entry.entry.text).toBe("Индексирую `ARCHITECTURE.md`.\n\nI'll start by checking the index.");
  await mount(world([entry.entry]));
  const paragraphs = [...document.querySelectorAll(".timeline .ai p")].map(p => p.textContent);
  expect(paragraphs).toEqual(["Индексирую ARCHITECTURE.md.", "I'll start by checking the index."]);
});

it("names a read of the project root in words and opens Code on it", async () => {
  const root = step({ title: "orchestrator: mcp__cortex__project_read", callId: "xa_c:r", kind: "other", status: "completed", path: "." });
  expect(viewOfText(root).target).toBe("project root");
  await mount(world([{ id: "history:f:1", kind: "fold", text: "Read 1 file", steps: [root] }]));
  await openFold();
  const link = stepRow("Read").querySelector<HTMLButtonElement>("button.step-t")!;
  expect(link.textContent).toBe("project root");
  expect(link.getAttribute("aria-label")).toBe("Open project root in Code");
  link.click(); await settle();
  expect(location.pathname).toBe(`/m/${LOCAL_MACHINE_ID}/t/t/code`);
  expect(document.querySelector(".step-sub [role=alert]")).toBeNull();
});

it("says why the runtime refused a change, in words, on the failed row", async () => {
  const reason = "The file was not changed (scope_denied). Ask for access to the path and try again.";
  const rows = [
    row(1, "tool", step({ title: "orchestrator: mcp__cortex__project_edit", callId: "xa_c:p1", kind: "other", status: "pending", path: "src/a.ts" }), "pending"),
    row(2, "tool", step({ callId: "xa_c:p1", kind: "other", status: "failed", failure: reason }), "failed"),
  ];
  const [fold] = conversationOf(rows, { description: "", answers: [] });
  if (fold.entry.kind !== "fold") throw new Error("not a fold");
  expect(fold.entry.text).toBe("Edited 1 file, 1 tool call needs attention");
  await mount(world([fold.entry]));
  await openFold();
  const plan = stepRow("Edit");
  expect(plan.tagName).toBe("BUTTON");
  expect(plan.querySelector(".i")?.getAttribute("class")).toContain("st-bad");
  plan.click(); await settle();
  const note = document.querySelector(".timeline .ws-step-failure")!;
  expect(note.textContent).toBe(reason);
  expect(document.querySelector(".timeline .step-sub")!.textContent).not.toMatch(/[{}"]|recorded":/);
});

it("counts a stopped worker as cancelled on the team line, as the header does", async () => {
  const cancelled = writer([], { id: "w", role: "backend-dev", status: "cancelled", result: null });
  await mount(world([{ id: "workers", kind: "workers", title: "Workers", workerIds: ["w"] }], { status: "needs_you" }, [cancelled]));
  // The team line stands in the bar pinned above the composer, not in the feed.
  const team = document.querySelector<HTMLButtonElement>(".task-panel .dock .ws-team-all")!;
  expect(team.textContent).toBe("Workers: 0 finished, 1 cancelled");
  expect(document.querySelector(".timeline .ws-team")).toBeNull();
  expect(document.querySelector(".taskbar")!.textContent).toContain("0 finished, 1 cancelled");
});

const question: Decision = { id: "d1", kind: "question", from: "Coordinator", workerId: null, at: null, text: "How to continue after the stop?", options: [{ label: "Finish with the checks", optionId: "o1" }, { label: "Close as is", optionId: "o2" }], answer: null, withdrawn: false };

it("keeps the answer line in place when an option is chosen, so the page does not move", async () => {
  await mount(world([{ id: "e-d1", kind: "decision", decisionId: "d1" }], {}, [], [question]));
  const line = () => [...document.querySelectorAll(".timeline .dc-answer > .mc-effect")];
  expect(line().map(l => l.textContent)).toEqual(["Answer: not chosen yet"]);
  const before = line()[0];
  document.querySelector<HTMLButtonElement>(".timeline .opt")!.click(); await settle();
  expect(line().map(l => l.textContent)).toEqual(["Answer: Finish with the checks"]);
  expect(line()[0]).toBe(before);
  // One line high whatever the option says (workspace.css).
  const src = ["src", join("packages", "workspace-ui", "src")].find(dir => existsSync(join(dir, "workspace.css")))!;
  const css = readFileSync(join(src, "workspace.css"), "utf8");
  expect(css).toMatch(/\.dc-answer > \.mc-effect \{[^}]*white-space: nowrap/);
});

it("brings the owner's new message into view above the composer even after scrolling up", async () => {
  const observers: (() => void)[] = [];
  vi.stubGlobal("ResizeObserver", class { constructor(cb: () => void) { observers.push(cb); } observe() {} disconnect() {} });
  const entries: TimelineEntry[] = [{ id: "c1", kind: "cortex", text: "The README now has a Usage section." }];
  const state = world(entries, { status: "review", result: { version: 1, digest: "d", summary: "Done.", checks: [] } as unknown as Task["result"] });
  const p = await mount(state);
  const box = document.querySelector<HTMLElement>(".timeline")!;
  let top = 0;
  Object.defineProperties(box, { clientHeight: { get: () => 400 }, scrollHeight: { get: () => 2000 }, scrollTop: { get: () => top, set: (v: number) => { top = v; } } });
  observers.forEach(cb => cb()); await new Promise(r => setTimeout(r, 40)); await settle();
  expect(top).toBe(1600);
  // The owner scrolls up to read the result, then sends a request for changes from its dialog.
  box.dispatchEvent(new WheelEvent("wheel", { deltaY: -100 })); top = 300; box.dispatchEvent(new Event("scroll")); await settle();
  expect(document.querySelector(".timeline-pill, .ws-pill")).not.toBeNull();
  const next = structuredClone(state);
  Object.assign(next.tasks[0], { status: "working", process: "live", result: null });
  next.sessions[0].timeline = [...entries, { id: "s1", kind: "said", text: "Add one line before the example." }];
  p.emit(projection("2", next)); await settle();
  expect(top).toBe(1600);
  expect(document.querySelector(".ws-pill")).toBeNull();
});
