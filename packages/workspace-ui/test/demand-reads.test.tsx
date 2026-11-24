import { render } from "solid-js/web";
import { createRoot } from "solid-js";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import type { Decision as WireDecision } from "@cortex/workspace-contract";
import { emptyConfig, LOCAL_MACHINE_ID, type NativeTaskDelta } from "@cortex/workspace-domain";
import { homeView } from "../src/selectors";
import { Workspace } from "../src";
import type { ResultTarget } from "../src/result-links";
import { createRealPort, GIT_VISIBLE_REFRESH_MS, MACHINE_POLL_MS } from "../src/port/real";
import { createWorkspaceStore } from "../src/store";
import { capabilitiesWire, scriptedTransport, wireDecision, wireTask } from "./support";

/**
 * Two registered projects on a native service, as the desktop sees them. Nothing is read on a blind timer: a project's facts
 * are read for the screen on view, when it opens and when its events say it changed; its Git only for a Git screen.
 */
function twoProjects() {
  const high: Record<string, number> = { p: 3, q: 3 }, version: Record<string, number> = { p: 1, q: 1 };
  const title: Record<string, string> = { p: "Fix login", q: "Write docs" }, phase: Record<string, string> = { p: "not_started", q: "not_started" };
  const task = (id: string) => wireTask({ taskId: `${id}-task`, title: title[id], titleAvailability: "available", version: String(version[id]), phase: phase[id] as "not_started", executionId: phase[id] === "not_started" ? null : `${id}-e` });
  const envelope = (id: string, data: unknown) => ({ protocol: { major: 2, mapping: 2 }, projectId: id, generation: "g", version: "2", highWatermark: String(high[id]), data });
  const repo = (id: string) => ({ projectId: id, remote: "", tree: [], sources: {}, commits: [], branches: [], fetchedAt: null, openBranch: "main", defaultBranch: "main",
    checkouts: [{ id: `${id}:main`, projectId: id, kind: "main", branch: "main", path: `/${id}`, taskId: null, label: null, base: "main", ahead: 0, behind: 0, behindOf: {}, draft: "", sources: {}, updateConflict: null, files: [] }] });
  const s = scriptedTransport({
    "service.capabilities": () => ({ methods: ["native.query", "native.board", "machine.call"], machine: true }),
    "projects.list": () => ["p", "q"].map(id => ({ projectId: id, displayName: id.toUpperCase(), canonicalRoot: `/${id}`, activation: {} })),
    "capabilities.get": () => capabilitiesWire(["capabilities.get", "project.snapshot", "tasks.create", "tasks.start"]),
    "native.board": message => { const id = (message as { projectId: string }).projectId; return envelope(id, { type: "board", value: { items: [task(id)], nextCursor: null, highWatermark: String(high[id]) } }); },
    "machine.call": raw => {
      const { op, projectId } = raw as { op: string; projectId?: string };
      if (op === "overview") return { ok: true, result: { name: "This machine" } };
      if (op === "project.config.read") return { ok: true, result: { value: {}, tokens: {} } };
      if (op === "config.read") return { ok: true, result: { config: emptyConfig(), connections: [], tokens: {} } };
      if (op === "repo.snapshot") return { ok: true, result: repo(projectId!) };
      if (op === "git") return { ok: true, result: {} };
      return { ok: true, result: op === "workers" ? [] : { hasGit: true, tasks: {}, sprints: [] } };
    },
  });
  /** One change of a project's task, as its event: a new title, or a new state. */
  const change = (id: string, patch: { title?: string; phase?: string }) => {
    if (patch.title) title[id] = patch.title;
    if (patch.phase) phase[id] = patch.phase;
    const base = version[id], from = high[id] + 1;
    version[id]++; high[id]++;
    const delta: NativeTaskDelta = { projectId: id, batch: { generation: "g", streamKey: { scope: "project", project_id: id }, fromSequence: String(from), throughSequence: String(high[id]),
      changes: [{ kind: "task", id: `${id}-task`, baseVersion: String(base), newVersion: String(version[id]), operation: { type: "upsert", data: { kind: "task_projection", data: task(id) } } }] } };
    s.send({ type: "native_delta", ...delta });
  };
  const reads = (op: string, id: string) => s.calls.filter(c => c.operation === "machine.call" && (c.message as { op: string; projectId?: string }).op === op && (c.message as { projectId?: string }).projectId === id).length;
  return { s, change, reads };
}

let dispose: (() => void) | undefined;
beforeEach(() => { localStorage.clear(); sessionStorage.clear(); });
afterEach(() => { dispose?.(); dispose = undefined; document.body.innerHTML = ""; vi.useRealTimers(); delete (document as { hidden?: boolean }).hidden; });

it("updates a project no screen shows from its events alone, and reads the facts of the one on view when its state moves", async () => {
  const { s, change, reads } = twoProjects();
  const store = createRoot(d => { dispose = d; return createWorkspaceStore(createRealPort({ transport: s.transport, actorId: "owner", registry: true, streamFactsMs: 60_000 })); });
  // The task page of p-task is on view.
  store.interest("p", "facts", "p-task");
  await vi.waitFor(() => expect(store.state().tasks.map(t => t.id).sort()).toEqual(["p-task", "q-task"]));
  await vi.waitFor(() => expect(reads("product.facts", "p")).toBeGreaterThan(0));
  await new Promise(resolve => setTimeout(resolve, 100));
  const pFacts = reads("product.facts", "p");
  // Another project's work: its board and sidebar follow the event, with no facts or Git read for it.
  change("q", { title: "Write the docs" });
  await vi.waitFor(() => expect(store.state().tasks.find(t => t.id === "q-task")?.title).toBe("Write the docs"));
  await new Promise(resolve => setTimeout(resolve, 100));
  expect(reads("product.facts", "q")).toBe(0);
  expect(reads("repo.snapshot", "q")).toBe(0);
  expect(reads("product.facts", "p")).toBe(pFacts);
  // The project on view: a state change reads its facts at once, still no Git without a Git screen.
  const pRepo = reads("repo.snapshot", "p");
  change("p", { phase: "execution" });
  await vi.waitFor(() => expect(reads("product.facts", "p")).toBe(pFacts + 1));
  // Worker rows are asked for the task on view only.
  const asked = s.calls.filter(c => (c.message as { op?: string; projectId?: string }).op === "product.facts" && (c.message as { projectId?: string }).projectId === "p").at(-1)!.message as { params: { taskIds: string[] } };
  expect(asked.params.taskIds).toEqual(["p-task"]);
  expect(reads("repo.snapshot", "p")).toBe(pRepo);
  expect(pRepo).toBeLessThanOrEqual(1);
});

it("reads Git once as a Git screen opens and right after the owner's own Git action", async () => {
  const { s, reads } = twoProjects();
  const store = createRoot(d => { dispose = d; return createWorkspaceStore(createRealPort({ transport: s.transport, actorId: "owner", registry: true })); });
  await vi.waitFor(() => expect(store.state().tasks).toHaveLength(2));
  await new Promise(resolve => setTimeout(resolve, 100));
  expect(reads("repo.snapshot", "p") + reads("repo.snapshot", "q")).toBe(0);
  const release = store.interest("p", "git");
  await vi.waitFor(() => expect(reads("repo.snapshot", "p")).toBe(1));
  await vi.waitFor(() => expect(store.state().repos.map(r => r.projectId)).toEqual(["p"]));
  expect((await store.dispatch({ type: "git_fetch", projectId: "p", now: store.clock() })).ok).toBe(true);
  await vi.waitFor(() => expect(reads("repo.snapshot", "p")).toBe(2));
  expect(reads("repo.snapshot", "q")).toBe(0);
  release();
});

it("on a project's board reads no project on a timer, its Git page reads on open and on Refresh, and a hidden window reads nothing", async () => {
  vi.useFakeTimers();
  const { s, reads } = twoProjects();
  history.replaceState(null, "", `/m/${LOCAL_MACHINE_ID}/p/p/board`);
  const root = document.createElement("div"); document.body.append(root);
  dispose = render(() => <Workspace port={createRealPort({ transport: s.transport, actorId: "owner", registry: true })} />, root);
  await vi.advanceTimersByTimeAsync(2000);
  await vi.waitFor(() => expect(root.textContent).toContain("Fix login"));
  const opened = { pFacts: reads("product.facts", "p"), pRepo: reads("repo.snapshot", "p") };
  // One read for the board as it opens (with the branches, read once); none for the project not on screen.
  expect(opened.pFacts).toBeGreaterThan(0);
  expect(opened.pRepo).toBeLessThanOrEqual(1);
  // Ten minutes on the board with nothing happening read nothing at all, though its title band shows the Git state.
  await vi.advanceTimersByTimeAsync(2 * MACHINE_POLL_MS);
  expect(reads("product.facts", "p")).toBe(opened.pFacts);
  expect(reads("repo.snapshot", "p")).toBe(opened.pRepo);
  expect(reads("product.facts", "q") + reads("repo.snapshot", "q")).toBe(0);
  // The Git page reads Git as it opens.
  history.pushState(null, "", `/m/${LOCAL_MACHINE_ID}/p/p/git`); window.dispatchEvent(new PopStateEvent("popstate"));
  await vi.advanceTimersByTimeAsync(100);
  expect(reads("repo.snapshot", "p")).toBe(opened.pRepo + 1);
  // Its manual refresh reads it now.
  const refresh = await vi.waitFor(() => { const b = root.querySelector<HTMLButtonElement>('[aria-label="Refresh Git status"]'); expect(b).not.toBeNull(); return b!; });
  refresh.click();
  await vi.advanceTimersByTimeAsync(100);
  expect(reads("repo.snapshot", "p")).toBe(opened.pRepo + 2);
  // A hidden window reads nothing, even with the Git page on view.
  Object.defineProperty(document, "hidden", { configurable: true, get: () => true });
  document.dispatchEvent(new Event("visibilitychange"));
  await vi.advanceTimersByTimeAsync(GIT_VISIBLE_REFRESH_MS * 4);
  expect(reads("repo.snapshot", "p")).toBe(opened.pRepo + 2);
  expect(reads("product.facts", "q") + reads("repo.snapshot", "q")).toBe(0);
}, 20_000);

it("keeps the title band's Git state current on a task page without a timer: on open, on its task's state change, and on the menu", async () => {
  vi.useFakeTimers();
  const { s, change, reads } = twoProjects();
  history.replaceState(null, "", `/m/${LOCAL_MACHINE_ID}/t/p-task`);
  const root = document.createElement("div"); document.body.append(root);
  dispose = render(() => <Workspace port={createRealPort({ transport: s.transport, actorId: "owner", registry: true })} />, root);
  await vi.advanceTimersByTimeAsync(2000);
  await vi.waitFor(() => expect(root.querySelector('.titlebar [aria-label="Git actions"]')).not.toBeNull());
  // Read once as the page opens.
  const opened = reads("repo.snapshot", "p");
  expect(opened).toBeGreaterThanOrEqual(1);
  // Ten quiet minutes: no timed Git read for a task page.
  await vi.advanceTimersByTimeAsync(2 * MACHINE_POLL_MS);
  expect(reads("repo.snapshot", "p")).toBe(opened);
  // The task's state moves: its facts are read with Git.
  change("p", { phase: "execution" });
  await vi.advanceTimersByTimeAsync(3000);
  const moved = reads("repo.snapshot", "p");
  expect(moved).toBe(opened + 1);
  // Opening the Git menu reads it again; the last state stays on view meanwhile.
  const menu = root.querySelector<HTMLElement>("#m-git")!;
  menu.dispatchEvent(Object.assign(new Event("toggle"), { newState: "open" }));
  await vi.advanceTimersByTimeAsync(100);
  expect(reads("repo.snapshot", "p")).toBe(moved + 1);
  expect(root.querySelector('.titlebar [aria-label="Git"]')).not.toBeNull();
  // Still no timer afterwards.
  await vi.advanceTimersByTimeAsync(GIT_VISIBLE_REFRESH_MS * 4);
  expect(reads("repo.snapshot", "p")).toBe(moved + 1);
}, 20_000);

it("reads a Git status line once as it opens and after the owner's own Git action, never on a timer", async () => {
  vi.useFakeTimers();
  const { s, reads } = twoProjects();
  const store = createRoot(d => { dispose = d; return createWorkspaceStore(createRealPort({ transport: s.transport, actorId: "owner", registry: true })); });
  await vi.waitFor(() => expect(store.state().tasks).toHaveLength(2));
  await vi.advanceTimersByTimeAsync(100);
  const release = store.interest("p", "status");
  await vi.waitFor(() => expect(reads("repo.snapshot", "p")).toBe(1));
  await vi.advanceTimersByTimeAsync(GIT_VISIBLE_REFRESH_MS * 4);
  expect(reads("repo.snapshot", "p")).toBe(1);
  expect((await store.dispatch({ type: "git_fetch", projectId: "p", now: store.clock() })).ok).toBe(true);
  await vi.waitFor(() => expect(reads("repo.snapshot", "p")).toBe(2));
  await vi.advanceTimersByTimeAsync(GIT_VISIBLE_REFRESH_MS * 4);
  expect(reads("repo.snapshot", "p")).toBe(2);
  expect(reads("repo.snapshot", "q")).toBe(0);
  release();
});

it("clears Needs you on the board, sidebar and Inbox within a second of a recorded answer, while the task keeps streaming", async () => {
  let version = 1, high = 3, answered = false;
  const task = () => wireTask({ version: String(version), executionId: "execution", phase: "execution", executionState: "running", runtimeState: "EXECUTING" });
  const envelope = (data: unknown) => ({ protocol: { major: 2, mapping: 2 }, projectId: "p", generation: "g", version: "2", highWatermark: String(high), data });
  const question = (status: "pending" | "answered") => wireDecision({ decisionId: "ask", questionId: "qa", executionId: "execution", prompt: "Which store?", options: [{ optionId: "mem", label: "Memory" }],
    status, createdAt: "2026-11-17T10:00:02.500000+00:00", ...(status === "answered" ? { answerText: "Memory" } : {}) } as Partial<WireDecision>);
  const s = scriptedTransport({
    "service.capabilities": () => ({ methods: ["native.query", "native.board"], machine: true }),
    "projects.list": () => [{ projectId: "p", displayName: "P", canonicalRoot: "/p", activation: {} }],
    "capabilities.get": () => capabilitiesWire(["capabilities.get", "project.snapshot", "tasks.create", "tasks.start", "decisions.list", "decisions.answer"]),
    "native.board": () => envelope({ type: "board", value: { items: [task()], nextCursor: null, highWatermark: String(high) } }),
    // A details read takes longer than the gap between the running task's events, so each one is superseded by the next.
    "decisions.list": () => new Promise(resolve => setTimeout(() => resolve({ items: [question(answered ? "answered" : "pending")], nextCursor: null, highWatermark: "1" }), 40)),
    "decisions.answer": message => { answered = true; return { requestId: (message as { requestId: string }).requestId, outcome: "recorded", target: "ask", entityVersion: "3", recordedAt: "2026-11-17T10:01:00Z" }; },
    "machine.call": raw => {
      const op = (raw as { op: string }).op;
      if (op === "config.read") return { ok: true, result: { config: emptyConfig(), connections: [], tokens: {} } };
      if (op === "project.config.read") return { ok: true, result: { value: {}, tokens: {} } };
      return { ok: true, result: op === "workers" ? [] : op === "repo.snapshot" ? null : { tasks: {}, sprints: [] } };
    },
  });
  const step = () => {
    const base = version, from = high + 1; version++; high++;
    s.send({ type: "native_delta", projectId: "p", batch: { generation: "g", streamKey: { scope: "project", project_id: "p" }, fromSequence: String(from), throughSequence: String(high),
      changes: [{ kind: "task", id: "task-1", baseVersion: String(base), newVersion: String(version), operation: { type: "upsert", data: { kind: "task_projection", data: task() } } }] } } as NativeTaskDelta & { type: "native_delta" });
  };
  const store = createRoot(d => { dispose = d; return createWorkspaceStore(createRealPort({ transport: s.transport, actorId: "owner", registry: true })); });
  const column = () => store.index().byId.get("task-1")?.column;
  const needs = () => homeView(store.index(), LOCAL_MACHINE_ID).attention.length;
  await vi.waitFor(() => expect(column()).toBe("needs"), { timeout: 3000 });
  expect(needs()).toBe(1);
  const streaming = setInterval(step, 15);
  try {
    await new Promise(resolve => setTimeout(resolve, 200));
    expect((await store.dispatch({ type: "answer_decision", taskId: "task-1", decisionId: "ask", answer: "Memory", optionId: "mem" })).ok).toBe(true);
    const sent = Date.now();
    await vi.waitFor(() => expect(column()).toBe("work"), { timeout: 1000 });
    expect(needs()).toBe(0);
    expect(Date.now() - sent).toBeLessThan(1000);
    // It stays so while the task keeps working: no later read brings the question back.
    await new Promise(resolve => setTimeout(resolve, 300));
    expect(column()).toBe("work");
  } finally { clearInterval(streaming); }
});

it("reads worker rows only for the task on view, not every started task's", async () => {
  // Ten started tasks, each with three workers holding their recent activity, as on a busy project.
  const ids = Array.from({ length: 10 }, (_, n) => `t${n}`);
  const worker = (taskId: string, n: number) => ({ id: `${taskId}-w${n}`, taskId, executionId: `${taskId}-e`, role: "backend-dev", status: "completed", lane: "execution", goal: "Patch ".repeat(40),
    now: null, waitsFor: null, scope: [], route: null, attempt: { n: 1, of: 1 }, elapsedMs: 1, runningSince: null, context: null, usage: null, result: { summary: "Done", checks: [] },
    activity: Array.from({ length: 20 }, (_, i) => ({ sequence: i, kind: "tool", text: "Read a file ".repeat(20) })), activityCursor: null });
  let replyBytes = 0;
  const s = scriptedTransport({
    "service.capabilities": () => ({ methods: ["native.query", "native.board"], machine: true }),
    "projects.list": () => [{ projectId: "p", displayName: "P", canonicalRoot: "/p", activation: {} }],
    "capabilities.get": () => capabilitiesWire(["capabilities.get", "project.snapshot"]),
    "native.board": () => ({ protocol: { major: 2, mapping: 2 }, projectId: "p", generation: "g", version: "2", highWatermark: "3",
      data: { type: "board", value: { items: ids.map(id => wireTask({ taskId: id, executionId: `${id}-e`, phase: "result", executionState: "result" })), nextCursor: null, highWatermark: "3" } } }),
    "machine.call": raw => {
      const { op, params } = raw as { op: string; params?: { taskIds?: string[] } };
      if (op === "config.read") return { ok: true, result: { config: emptyConfig(), connections: [], tokens: {} } };
      if (op === "project.config.read") return { ok: true, result: { value: {}, tokens: {} } };
      if (op === "workers") { const rows = (params?.taskIds ?? []).flatMap(id => [0, 1, 2].map(n => worker(id, n))); replyBytes = JSON.stringify(rows).length; return { ok: true, result: rows }; }
      return { ok: true, result: op === "repo.snapshot" ? null : { tasks: {}, sprints: [] } };
    },
  });
  const port = createRealPort({ transport: s.transport, actorId: "owner", registry: true });
  const store = createRoot(d => { dispose = d; return createWorkspaceStore(port); });
  await vi.waitFor(() => expect(store.state().tasks).toHaveLength(10));
  // An explicit read for a project no screen shows carries every started task's workers, as before.
  await store.refreshProjectFacts("p", false);
  const all = replyBytes;
  expect(store.state().workers).toHaveLength(30);
  // The task page of t3 is on view: the read carries t3's workers only, and the rows an earlier read confirmed for the
  // other tasks' current executions are kept, so the other tasks do not lose their workers.
  store.interest("p", "facts", "t3");
  await vi.waitFor(() => expect(replyBytes).toBeLessThan(all));
  await vi.waitFor(() => expect(store.state().workers.slice(0, 3).map(w => w.taskId)).toEqual(["t3", "t3", "t3"]));
  expect(store.state().workers).toHaveLength(30);
  const one = replyBytes;
  expect(one * 9).toBeLessThan(all);
  console.info(`worker rows in a facts read: every started task ${all} bytes, the task on view ${one} bytes`);
});

// The retry case waits for the real 5 s checkout timer before asserting the popup state.
// Keep its 6.5 s assertion deadline inside the overall test deadline.
it.each(["late checkout", "retry"])("keeps a relative Result media popup open through a %s", async scenario => {
  const worktree = "/worktrees/p-t";
  const summary = "[Subtitles](out/linked/../sub%20titles.srt)";
  const task = wireTask({ taskId: "t", executionId: "t-e", phase: "result", executionState: "completed", runtimeState: "COMPLETED" });
  const saved = { status: "review", checkout: { kind: "worktree", id: "t-checkout", branch: "main" } };
  const checkout = (id: string, kind: string, path: string, taskId: string | null) =>
    ({ id, kind, path, taskId, projectId: "p", branch: "main", files: [], sources: {}, label: null, base: "main", ahead: 0, behind: 0, behindOf: {}, draft: "", updateConflict: null });
  const repo = (ready: boolean) => ({ projectId: "p", remote: "", tree: [], sources: {}, commits: [], branches: [], fetchedAt: null, openBranch: "main", defaultBranch: "main",
    checkouts: [checkout("p:main", "main", "/p", null), ...(ready ? [checkout("t-checkout", "worktree", worktree, "t")] : [])] });
  let publishRepo!: (value: ReturnType<typeof repo>) => void;
  const lateRepo = new Promise<ReturnType<typeof repo>>(resolve => { publishRepo = resolve; });
  let repoReads = 0, allowLateRead = false;
  const s = scriptedTransport({
    "service.capabilities": () => ({ methods: ["native.query", "native.board", "native.open_editor"], machine: true }),
    "projects.list": () => [{ projectId: "p", displayName: "P", canonicalRoot: "/p", activation: {} }],
    "capabilities.get": () => capabilitiesWire(["capabilities.get", "project.snapshot", "history.page"]),
    "native.board": () => ({ protocol: { major: 2, mapping: 2 }, projectId: "p", generation: "g", version: "2", highWatermark: "3",
      data: { type: "board", value: { items: [task], nextCursor: null, highWatermark: "3" } } }),
    "native.query": () => ({ protocol: { major: 2, mapping: 2 }, projectId: "p", generation: "g", version: "2", highWatermark: "0",
      data: { type: "history", value: { items: [], nextCursor: null, highWatermark: "0" } } }),
    "machine.call": async raw => {
      switch ((raw as { op: string }).op) {
        case "config.read": return { ok: true, result: { config: emptyConfig(), connections: [], tokens: {} } };
        case "project.config.read": return { ok: true, result: { value: {}, tokens: {} } };
        case "repo.snapshot":
          repoReads++;
          return { ok: true, result: allowLateRead ? await lateRepo : repo(false) };
        case "workers": return { ok: true, result: [] };
        case "product.board": return { ok: true, result: { hasGit: true, sprints: [], tasks: { t: saved } } };
        case "product.read": return { ok: true, result: { hasGit: true, sprints: [], tasks: { t: {
          ...saved,
          result: { version: 1, digest: "d", summary, checks: [] }, workspaceDigest: "d",
        } } } };
        default: return { ok: true, result: {} };
      }
    },
  });
  const port = createRealPort({ transport: s.transport, actorId: "owner", registry: true, streamFactsMs: 60_000 });
  const query = vi.spyOn(port, "query");
  const open = vi.fn(async (_target: ResultTarget) => {});
  history.replaceState(null, "", `/m/${LOCAL_MACHINE_ID}/t/t`);
  dispose = render(() => <Workspace port={port} openResultTarget={open} />, document.body);
  const route = location.href;
  const link = await vi.waitFor(() => {
    const link = document.querySelector<HTMLAnchorElement>('.rcard a[href="out/linked/../sub%20titles.srt"]');
    expect(link).not.toBeNull();
    return link!;
  });
  const activate = () => {
    const click = new MouseEvent("click", { bubbles: true, cancelable: true });
    link.dispatchEvent(click);
    expect(click.defaultPrevented).toBe(true);
  };
  const pop = () => document.querySelector('[role="dialog"][aria-label="File path"]');
  const button = (label: string) => [...(pop()?.querySelectorAll("button") ?? [])].find(b => b.textContent === label);
  const readCount = repoReads, queryCount = query.mock.calls.length;
  allowLateRead = scenario === "late checkout";
  activate();
  const dialog = pop();
  expect(dialog?.textContent).toContain("Finding the checkout path");
  expect(dialog?.querySelector(".ws-path-value")?.textContent).toBe("out/linked/../sub titles.srt");
  expect(link.getAttribute("aria-busy")).toBe("true");
  expect(button("Open in default app")).toBeUndefined();
  expect(open).not.toHaveBeenCalled();
  await vi.waitFor(() => expect(repoReads).toBeGreaterThan(readCount));
  const factsIndex = query.mock.calls.findIndex(([op, args], i) => i >= queryCount && op === "project.facts" && "git" in args && args.git === true);
  expect(factsIndex).toBeGreaterThanOrEqual(queryCount);
  await query.mock.results[factsIndex].value;
  expect(pop()).toBe(dialog);
  expect(button("Open in default app")).toBeUndefined();

  if (scenario === "retry") {
    await vi.waitFor(() => expect(button("Retry")).toBeDefined(), { timeout: 6500 });
    expect(dialog?.textContent).toContain("Could not confirm the checkout path.");
    expect(link.hasAttribute("aria-busy")).toBe(false);
    const retryReads = repoReads;
    allowLateRead = true;
    button("Retry")!.focus();
    button("Retry")!.click();
    expect(dialog?.textContent).toContain("Finding the checkout path");
    expect(document.activeElement).toBe(button("Copy path"));
    await vi.waitFor(() => expect(repoReads).toBeGreaterThan(retryReads));
  }
  publishRepo(repo(true));
  await vi.waitFor(() => expect(button("Open in default app")).toBeDefined());
  expect(pop()).toBe(dialog);
  const path = `${worktree}/out/linked/../sub titles.srt`;
  expect(dialog?.querySelector(".ws-path-value")?.textContent).toBe(path);
  expect(link.hasAttribute("aria-busy")).toBe(false);
  expect(open).not.toHaveBeenCalled();
  expect(location.href).toBe(route);
  button("Open in default app")!.click();
  await vi.waitFor(() => expect(pop()).toBeNull());
  expect(open).toHaveBeenCalledExactlyOnceWith({ kind: "file", path });
  expect(location.href).toBe(route);
}, 10_000);
