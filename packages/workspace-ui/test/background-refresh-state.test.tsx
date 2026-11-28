import { render } from "solid-js/web";
import { afterEach, beforeEach, expect, it } from "vitest";
import { emptySession, emptyWorkspace, LOCAL_MACHINE_ID, type Check, type FileDiff, type Worker, type WorkspaceState } from "@cortex/workspace-domain";
import { Workspace } from "../src";
import { buildIndex, stableIndex } from "../src/selectors";
import { draftTask, flush, projection, scriptedPort, withTask } from "./support";

// A background read with changed data updates each screen in place: what the owner opened, picked or typed stays.
let dispose: (() => void) | undefined;
beforeEach(() => { localStorage.clear(); sessionStorage.clear(); });
afterEach(() => { dispose?.(); dispose = undefined; document.body.innerHTML = ""; });
const settle = async () => { for (let i = 0; i < 8; i++) await flush(); };
const taskPath = `/m/${LOCAL_MACHINE_ID}/t/t`;
async function mount(state: WorkspaceState, path: string) {
  const s = scriptedPort({ capabilities: { everything: true, operations: [] } });
  history.replaceState(null, "", path);
  const root = document.createElement("div"); document.body.append(root);
  dispose = render(() => <Workspace port={s.port} />, root);
  await settle(); s.emit(projection("1", state)); await settle();
  return s;
}
const button = (text: string) => [...document.querySelectorAll<HTMLButtonElement>("button")].find(b => b.textContent?.trim() === text);
const type = (el: HTMLTextAreaElement | HTMLInputElement, value: string) => { el.value = value; el.dispatchEvent(new Event("input", { bubbles: true })); };

it("shows the selected project's migration only while its runtime reports it", async () => {
  const state = withTask(emptyWorkspace(), "Work");
  state.machines[0].connection = "connected";
  const p = await mount(state, `/m/${LOCAL_MACHINE_ID}/p/p/board`);
  const status = () => [...document.querySelectorAll("[role=status]")].find(node => node.textContent?.includes("Moving project history"));
  expect(status()).toBeUndefined();
  p.emit({ kind: "project_migration", projectId: "other", active: true });
  await settle();
  expect(status()).toBeUndefined();
  p.emit({ kind: "project_migration", projectId: "p", active: true });
  await settle();
  expect(status()?.textContent).toContain("Moving project history");
  p.emit({ kind: "project_migration", projectId: "p", active: false });
  await settle();
  expect(status()).toBeUndefined();
});

const check = (patch: Partial<Check> = {}): Check => ({ name: "Unit tests", detail: "", outcome: "failed", kind: "automated", command: ["npm", "test"], cwd: "/p", output: "1 failed", exitCode: 1, rerunnable: true, ...patch });
const file = (path: string, add: number): FileDiff => ({ path, status: "M", add, del: 0, hunks: [{ header: "@@ -1 +1 @@", lines: Array.from({ length: add }, (_, i) => ({ kind: "add" as const, old: null, new: i + 1, text: `line ${i}` })) }] } as FileDiff);
function inReview(checks: Check[], files: FileDiff[]) {
  const s = withTask(emptyWorkspace(), "Work"); s.machines[0].connection = "connected";
  const t = s.tasks[0]; t.status = "review"; t.workspaceDigest = "digest";
  t.result = { version: 1, digest: "digest", summary: "Built the page", checks };
  s.reviews = [{ taskId: "t", sha: "abc", files, scopes: [], comments: [] }];
  return s;
}

it("Changes: an opened check output, the picked layout and the evidence stay while a background read changes the checks and diffs", async () => {
  const p = await mount(inReview([check(), check({ name: "Lint", outcome: "passed", output: "clean", exitCode: 0 })], [file("a.ts", 2), file("b.ts", 1)]), `${taskPath}/changes`);
  const out = [...document.querySelectorAll<HTMLDetailsElement>("details.rv-cmd")];
  expect(out).toHaveLength(2);
  out[0].open = true;
  button("Split")!.click(); await settle();
  expect(button("Split")!.getAttribute("aria-current")).toBe("true");
  const evidence = document.querySelector<HTMLDetailsElement>("details.evid")!;
  const diffB = document.getElementById("diff-b.ts")!;
  // The same checks, one with new output and a new one beside them; one file's diff grows.
  p.emit(projection("2", inReview([check({ output: "2 failed", detail: "Two tests fail" }), check({ name: "Lint", outcome: "passed", output: "clean", exitCode: 0 }), check({ name: "Types", outcome: "passed", output: "ok", exitCode: 0 })], [file("a.ts", 2), file("b.ts", 3)])));
  await settle();
  const now = [...document.querySelectorAll<HTMLDetailsElement>("details.rv-cmd")];
  expect(now).toHaveLength(3);
  expect(now[0]).toBe(out[0]);
  expect(now[0].open).toBe(true);
  expect(now[0].textContent).toContain("2 failed");
  expect(document.querySelector("details.evid")).toBe(evidence);
  expect(button("Split")!.getAttribute("aria-current")).toBe("true");
  // The changed file's block is updated in place, not rebuilt.
  expect(document.getElementById("diff-b.ts")).toBe(diffB);
  expect(diffB.textContent).toContain("line 2");
});

const worker = (patch: Partial<Worker> = {}): Worker => ({ id: "w", taskId: "t", executionId: "e", role: "frontend-dev", status: "working", lane: "execution", goal: "Build the page", now: null, waitsFor: null, scope: [],
  route: null, attempt: null, elapsedMs: null, runningSince: null, context: null, usage: null, activity: [], result: null, ...patch } as Worker);
function running(extra: string[], now: string) {
  const s = withTask(emptyWorkspace(), "Work"); s.machines[0].connection = "connected";
  s.tasks[0].status = "working"; s.tasks[0].process = "live";
  s.workers = [worker({ now })];
  const session = emptySession("t");
  session.timeline = [
    { id: "f1", kind: "fold" as const, text: "Read 2 files", steps: ["Read a.ts", "Read b.ts"] },
    ...extra.map((text, i) => ({ id: `c${i}`, kind: "cortex" as const, text })),
    { id: "facts:workers", kind: "workers", title: "Workers", workerIds: ["w"] },
  ];
  s.sessions = [session];
  return s;
}

it("Conversation: an opened fold, the typed message and the team bar stay while a background read changes the feed and the worker", async () => {
  const p = await mount(running(["First."], "Reading"), taskPath);
  const fold = [...document.querySelectorAll<HTMLButtonElement>(".timeline button.fold")].find(b => b.textContent?.includes("Read 2 files"))!;
  fold.click(); await settle();
  expect(fold.getAttribute("aria-expanded")).toBe("true");
  const box = document.querySelector<HTMLTextAreaElement>('.dock textarea')!;
  type(box, "Please also check the footer");
  const team = document.querySelector(".ws-team")!;
  p.emit(projection("2", running(["First.", "Second."], "Editing"))); await settle();
  expect(document.querySelector(".timeline")!.textContent).toContain("Second.");
  expect([...document.querySelectorAll<HTMLButtonElement>(".timeline button.fold")].find(b => b.textContent?.includes("Read 2 files"))).toBe(fold);
  expect(fold.getAttribute("aria-expanded")).toBe("true");
  expect(document.querySelector<HTMLTextAreaElement>('.dock textarea')).toBe(box);
  expect(box.value).toBe("Please also check the footer");
  expect(document.querySelector(".ws-team")).toBe(team);
  expect(team.querySelector(".ws-team-now")?.textContent).toBe("Editing");
});

function archived(titles: string[]) {
  const s = withTask(emptyWorkspace(), titles[0]); s.machines[0].connection = "connected";
  s.tasks = titles.map((title, i) => ({ ...draftTask(), id: `t${i}`, title, status: "done", doneAt: 1000 + i, archive: { at: 1000 + i, previousStatus: "done" } }));
  return s;
}

it("Archive: its rows and a checked row stay while a background read renames another task", async () => {
  const p = await mount(archived(["Alpha", "Beta"]), `/m/${LOCAL_MACHINE_ID}/archive`);
  const rows = () => [...document.querySelectorAll<HTMLElement>(".gr.ar-cols")];
  if (rows().length === 0) return expect.unreachable("the archive shows no rows for this fixture");
  const before = rows();
  const beta = before.find(r => r.textContent?.includes("Beta"))!;
  const box = beta.querySelector<HTMLInputElement>("input[type=checkbox]")!;
  box.click(); await settle();
  expect(box.checked).toBe(true);
  p.emit(projection("2", archived(["Alpha renamed", "Beta"]))); await settle();
  expect(rows().find(r => r.textContent?.includes("Beta"))).toBe(beta);
  expect(box.checked).toBe(true);
  expect(rows().find(r => r.textContent?.includes("Alpha renamed"))).toBe(before.find(r => r.textContent?.includes("Alpha")));
});

it("the index keeps every unchanged task view as the same object and replaces only the changed one", () => {
  const a = withTask(emptyWorkspace(), "One");
  a.tasks.push({ ...a.tasks[0], id: "t2", title: "Two" });
  const first = buildIndex(a, 0);
  const b = structuredClone(a); b.tasks[1].title = "Two, renamed";
  const next = stableIndex(first, buildIndex(b, 0));
  expect(next.byId.get("t")).toBe(first.byId.get("t"));
  expect(next.byId.get("t2")).not.toBe(first.byId.get("t2"));
  expect(next.byId.get("t2")!.task.title).toBe("Two, renamed");
  expect(next.summaries.get("p")!.views[0]).toBe(first.byId.get("t"));
  const same = stableIndex(next, buildIndex(structuredClone(b), 0));
  expect(same.views).toBe(next.views);
  expect(same.summaries.get("p")).toBe(next.summaries.get("p"));
});
