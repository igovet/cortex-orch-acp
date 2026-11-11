import { render } from "solid-js/web";
import { emptyWorkspace, LOCAL_MACHINE_ID, type PlanItem, type TaskOverlay } from "@cortex/workspace-domain";
import type { Task as WireTask } from "@cortex/workspace-contract";
import logo from "../../../apps/cortex-workspace-desktop/public/cortex-256.png";
import { Workspace } from "../src";
import { createRealPort } from "../src/port/real";
import type { TerminalPort } from "../src/terminal-client";
import { capabilitiesWire, projection, scriptedPort, scriptedTransport, snapshot, wireTask } from "./support";
import { Primitives } from "./layout-fit-primitives";
import { reviewWorld } from "./layout-fit-review";
import { conversationWorld } from "./conversation-world";
import { pagesWorld } from "./pages-world";
import "../src/fonts.css";
import "../src/mockup-styles.css";
import "../src/workspace.css";
import "../src/layout.css";

// Disposable, opt-in fake data over the real port: long names everywhere a row could break. No service, engines or real stores.
// `scroll` makes every list long for the scroll benchmark (scroll-perf.py): 400 more files in the tree, a 4,000-line file,
// 400 commits, 40 changed files of 300 lines, the 60-task sprint, a long conversation and worker feed, and with `terminal` a
// shell that printed 900 lines. The layout gate never sets it.
const scroll = new URLSearchParams(location.search).has("scroll");
const BRANCH = "feature/an-extremely-long-branch-name-that-keeps-going-well-past-any-sensible-width";
const BASE = "origin/release/2026-11-a-very-long-upstream-branch-name-for-the-summary-line";
const DIR = "apps/api/scripts/a-deeply-nested-directory/with-another-long-directory-name";
const NAMES = ["canonical-display-name-dev-repair-with-a-very-long-suffix.mjs", "canonical-exact-label-dev-repair-and-one-more-long-suffix.mjs", "canonical-pricing-label-decisions-from-the-review-round.json", "audit-company-websites-across-every-known-market-segment.mjs", "check-model-attribution-browser-with-an-unreasonable-name.mjs"];
const MORE = scroll ? [...Array.from({ length: 400 }, (_, i) => `src/generated/area-${i % 20}/module-${i}.ts`), "docs/long-notes.txt"] : [];
const tree = [".prettierrc", "README.md", ...NAMES.map(n => `${DIR}/${n}`), ...Array.from({ length: 24 }, (_, i) => `${DIR}/generated-module-${i}-${"x".repeat(60)}.ts`), ...MORE];
const changed = [...NAMES.map(n => `${DIR}/${n}`), ...MORE.slice(0, scroll ? 35 : 0)];
const hunk = { header: "@@ -1 +1,2 @@", staged: false, lines: [{ kind: "ctx", old: 1, new: 1, text: "line" }, { kind: "add", old: null, new: 2, text: "added" }] };
const longHunk = (file: number) => ({ header: `@@ -1,300 +1,300 @@ export function module${file}()`, staged: false, lines: Array.from({ length: 300 }, (_, j) => ({ kind: j % 7 === 3 ? "del" : j % 5 === 1 ? "add" : "ctx", old: j % 5 === 1 ? null : j + 1, new: j % 7 === 3 ? null : j + 1, text: `  const value${j} = compute(input.items[${j}], options); // line ${j} of file ${file}` })) });
const LONG_FILE = Array.from({ length: 4000 }, (_, i) => i % 40 === 0 ? `export function block${i}(input: Input, options: Options): Result {` : i % 40 === 39 ? "}" : `  const value${i} = compute(input.items[${i}], { ...options, index: ${i}, label: "line ${i}" });`).join("\n");
const graph = { paths: [], dot: { cls: "d1", x: 12 } };
const repoFacts = {
  projectId: "p", remote: "git@example.invalid:owner/a-repository-with-a-long-name.git", tree, sources: Object.fromEntries(tree.map(path => [path, "the first line of this file\nthe second line of this file\n"])), fetchedAt: 1000, openBranch: BRANCH, defaultBranch: "main",
  branches: [{ name: `${BRANCH}-spare`, where: "No checkout, an old branch left over from an earlier piece of work on this project", task: "A task title that is also very long and has no reason to fit in its column at all", ahead: 12, behind: 340 }],
  commits: Array.from({ length: scroll ? 400 : 12 }, (_, i) => ({ sha: `abc${i}def`, message: `Normalize the moderation wrappers and everything around them, round ${i}, with a message nobody can read in one line`, author: "A person with a remarkably long name", when: `${i + 12} hours ago`, branch: BRANCH, taskId: null, tags: i % 3 ? [] : ["release-candidate-with-a-long-tag"], files: 3, add: 10, del: 2, pushed: true, graph })),
  checkouts: [
    { id: "p:main", projectId: "p", kind: "main", branch: BRANCH, path: "/home/owner/projects/a-repository-with-a-long-name", taskId: null, label: null, base: BASE, upstream: BASE, ahead: 0, behind: 0, behindOf: {}, draft: "", sources: {}, updateConflict: null,
      files: changed.map((path, i) => ({ path, status: i % 2 ? "M" : "U", add: 10, del: 2, hunks: [scroll ? longHunk(i) : hunk], stage: i < 2 ? "staged" : i < 4 ? "unstaged" : "untracked" })) },
    { id: "p:wt", projectId: "p", kind: "worktree", branch: `${BRANCH}-worktree`, path: "/home/owner/.cortex/worktrees/a-worktree-with-a-long-path-of-its-own", taskId: null, label: null, base: BASE, ahead: 3, behind: 1, behindOf: {}, draft: "", sources: {}, updateConflict: null, files: [] },
  ],
};
const OWNER = "1. Make the dashboard simpler\n2. Load everything, **without** a limit\n\nFirst line of the note\nSecond line of the note\n\n- one\n- two";
const items: PlanItem[] = [
  { key: "a", title: "Write the brief", description: "# Goal\n\nFirst line\nSecond line\n\n1. Inspect\n2. Change", acceptance: "- Links work\n- `pnpm test` passes", dependsOn: [] },
  { key: "b", title: "Build the page", description: "Build it.", dependsOn: ["a"] },
];
const sprint = { id: "sp", projectId: "p", description: OWNER, title: "A sprint", runMode: "manual", targetBranch: null, planning: "ready", planningTaskId: "task-1", materializedVersion: null, plan: { version: 1, items }, conversation: [{ kind: "note", text: "Planner line one\nPlanner line two\n\n- **first** point" }, { kind: "message", text: "Owner line one\nOwner line two" }], taskIds: [], queueState: "idle", pauseReason: null };
// A paused queue whose next task the project has not loaded: its board's run control is disabled, with a reason of a real length.
const pausedSprint = { ...sprint, id: "paused", title: "Paused sprint with a title long enough to fill its scope button and then some more", runMode: "automatic", planning: "created", planningTaskId: null, materializedVersion: 1, taskIds: ["task-2", "not-loaded"], queueState: "paused", pauseReason: "A task stopped" };
// The queue in every state the board can show, with long Russian titles and a target branch that cannot fit.
const SPRINT_LONG = "Спринт: не могу вставлять картинки в момент создания задач и во всех формах сообщений, а также всё остальное, что с этим связано и ещё немного";
const title = (name: string) => `${name}: автоматическое изменение высоты полей ввода по мере ввода текста во всех формах создания задач и спринтов`;
const boardTask = (taskId: string, name: string, phase: WireTask["phase"], text = title(name)) => wireTask({ taskId, description: text, title: text, titleAvailability: "generated", executionId: phase === "not_started" ? null : `e-${taskId}`, phase, runtimeState: null, executionState: "not_started" });
const CLAMPED = title("Очень длинная задача").repeat(5);
const boardTasks = [
  boardTask("r-now", "Сейчас идёт", "execution"), boardTask("r-next", "Следующая", "not_started"), boardTask("r-dep", "Ждёт зависимость", "not_started"),
  boardTask("p-stop", "Вложения в формах", "paused"), boardTask("p-next", "Следующая", "not_started"),
  boardTask("n-ask", "Нужен ответ", "waiting"), boardTask("n-next", "Следующая", "not_started"), boardTask("n-dep", "Ждёт зависимость", "not_started"),
  boardTask("i-a", "Первая", "not_started"), boardTask("i-b", "Вторая", "not_started"),
  boardTask("m-a", "Первая", "not_started"), boardTask("m-b", "Вторая", "not_started"),
  boardTask("z-now", "Единственная", "execution"),
  boardTask("d-a", "Готовая", "not_started"),
  boardTask("c-stop", "", "paused", CLAMPED),
];
const boardOverlays: Record<string, TaskOverlay> = { "r-dep": { dependencies: ["r-now"] }, "n-dep": { dependencies: ["n-ask"] }, "d-a": { status: "done" } };
const boardSprint = (id: string, taskIds: string[], patch: Record<string, unknown> = {}) => ({ ...pausedSprint, id, title: SPRINT_LONG, targetBranch: BRANCH, taskIds, queueState: "idle", pauseReason: null, ...patch });
const boardSprints = [
  boardSprint("b-run", ["r-now", "r-next", "r-dep"], { queueState: "running" }),
  boardSprint("b-pause", ["p-stop", "p-next"], { queueState: "paused", pauseReason: "A task stopped" }),
  boardSprint("b-need", ["n-ask", "n-next", "n-dep"]),
  boardSprint("b-idle", ["i-a", "i-b"]),
  boardSprint("b-manual", ["m-a", "m-b"], { runMode: "manual" }),
  boardSprint("b-none", ["z-now"], { runMode: "manual" }),
  boardSprint("b-done", ["d-a"], { queueState: "complete" }),
  boardSprint("b-clamp", ["c-stop"], { queueState: "paused", pauseReason: "A task stopped" }),
];
const config = structuredClone(emptyWorkspace().machines[0].config);
const query = new URLSearchParams(location.search);
// `graph` opens sprint plans in the Graph view and adds two created sprints with large plans of a typical shape: `big` (60
// tasks) for the layout gate and the screenshots, `huge` (200 tasks) for the frame-time diagnostic. Some tasks need the owner
// or are running, so the graph shows states.
const VERBS = ["Add", "Write", "Wire", "Test", "Document", "Migrate", "Index", "Stream"];
const NOUNS = ["audit events", "billing export", "login history", "API keys", "retention job", "admin search", "CSV writer", "upload worker", "rate limits", "event schema"];
const bigTitle = (i: number) => `${VERBS[i % VERBS.length]} the ${NOUNS[(i * 7) % NOUNS.length]}${i % 5 === 0 ? " for every workspace and region" : ""}`;
const bigDeps = (i: number) => i < 3 ? [] : [...new Set([i - 1 - (i * 7) % 3, i - 3 - (i * 5) % 7].filter(d => d >= 0))];
function bigSprint(id: string, n: number) {
  const ids = Array.from({ length: n }, (_, i) => `${id}-${i + 1}`);
  const items: PlanItem[] = ids.map((key, i) => ({ key, title: bigTitle(i), description: `${bigTitle(i)}, with its checks.`, dependsOn: bigDeps(i).map(d => ids[d]) }));
  const tasks = ids.map((taskId, i) => boardTask(taskId, "", i % 11 === 3 ? "waiting" : i % 13 === 5 ? "execution" : "not_started", bigTitle(i)));
  const overlays: Record<string, TaskOverlay> = Object.fromEntries(ids.map((taskId, i) => [taskId, { dependencies: bigDeps(i).map(d => ids[d]) }]));
  return { tasks, overlays, sprint: { ...sprint, id, title: `Audit platform rollout, ${n} tasks`, runMode: "manual", planning: "created", planningTaskId: null, materializedVersion: 1, plan: { version: 1, items }, taskIds: ids } };
}
const graphs = !query.has("graph") && !scroll ? [] : query.get("graph") === "200" ? [bigSprint("huge", 200)] : [bigSprint("big", 60)];
// The service hands tasks over in pages of 50, as the contract's limit says.
const PAGE = 50;
if (query.has("graph")) localStorage.setItem("cw.plan.view", JSON.stringify("graph")); else localStorage.removeItem("cw.plan.view");
// `queued` holds a long Russian message with a pasted image in the running task's queue, the row the owner saw collide.
const QUEUED = "Проверь, пожалуйста, что вложения в очереди больше не наезжают на кнопки и что длинный текст сообщения обрезается в одну строку на любой ширине окна";
const queued = query.has("queued") ? [{ id: "q-long", taskId: query.get("queued") || "r-now", workerId: null, text: QUEUED, mode: "queue", delivery: "queued", at: 1, deliveredAt: null, failure: null, attachments: [{ name: "clipboard.png", mimeType: "image/png", data: btoa("fixture png") }] }] : [];
if (query.has("dark")) { localStorage.setItem("cw.theme", "dark"); document.documentElement.dataset.theme = "dark"; } else localStorage.removeItem("cw.theme");
// `sidebar` is the sidebar's width as the owner left it after dragging its edge.
if (query.has("sidebar")) localStorage.setItem("cw.ws.sidebarWidth", String(Number(query.get("sidebar")))); else localStorage.removeItem("cw.ws.sidebarWidth");
// `terminal` opens the project shell, which has already printed 900 lines (60 KiB, within the 64 KiB the service keeps).
if (query.has("terminal")) localStorage.setItem("cw.ws.terminalOpen", "true"); else localStorage.removeItem("cw.ws.terminalOpen");
function printedShell(): TerminalPort {
  const text = Array.from({ length: 900 }, (_, i) => `\x1b[32m PASS \x1b[0m packages/workspace-ui/test/case-${i}.test.tsx \x1b[2m(${(i * 37) % 300} ms)\x1b[0m\r\n`).join("");
  const bytes = [...new TextEncoder().encode(text)].slice(-60000);
  let at = 0;
  const page = () => { const data = bytes.slice(at, at + 16000); at += data.length; return { sessionId: "shell", data, next: at, truncated: false, exited: false }; };
  return { open: async () => { at = 0; return page(); }, read: async () => page(), write: async (_p, _s, data) => ({ written: data.length }), resize: async () => ({}), close: async () => ({}) };
}
const terminal = query.has("terminal") ? printedShell() : undefined;
// Startup, held by the gate: `pending` answers nothing until released. `failed` is a real failed start: the service answers, but
// the workspace it holds is refused until released. A service that cannot be reached at all opens the offline workspace instead.
const startup = query.get("startup");
let release = () => {};
const held = startup === "pending" ? new Promise<void>(resolve => { release = resolve; }) : null;
let refusing = startup === "failed";
(window as unknown as { release: () => void }).release = () => { refusing = false; release(); };
const startupGate = async () => { await held; };
const refusedRead = () => { if (refusing) throw new Error("Служба рабочего пространства не ответила вовремя: сокет занят другим процессом, и его владелец не отвечает на запросы уже несколько минут подряд. Проверьте, что служба запущена, и повторите попытку."); };
const chrome = { maximized: () => false, minimize() {}, toggleMaximize() {}, close() {}, startDragging() {}, guardClose() {}, closeNow() {} };
const allTasks = () => [wireTask({ executionId: "e", phase: "not_started", runtimeState: null, executionState: "not_started" }), wireTask({ taskId: "task-2", description: OWNER, title: "A task", titleAvailability: "generated", executionId: "e2", phase: "not_started", runtimeState: null, executionState: "not_started" }), ...boardTasks, ...graphs.flatMap(x => x.tasks)];
// With `scroll`, 30 more projects of six live tasks each, all open in the sidebar.
const EXTRA = scroll ? Array.from({ length: 30 }, (_, i) => `x${i}`) : [];
const extraTasks = (projectId: string) => Array.from({ length: 6 }, (_, i) => boardTask(`${projectId}-${i}`, "", i % 3 === 0 ? "execution" : i % 3 === 1 ? "waiting" : "paused", `${bigTitle(i + Number(projectId.slice(1)))} in ${projectId}`));
const tasksOf = (projectId?: string) => !projectId || projectId === "p" ? allTasks() : extraTasks(projectId);
if (scroll) localStorage.setItem("cw.ws.expanded", JSON.stringify(Object.fromEntries(["p", ...EXTRA].map(id => [id, true]))));
const s = scriptedTransport({
  "service.capabilities": async () => { await startupGate(); return { methods: ["machine.call", "native.open_editor"], machine: true }; },
  "projects.list": () => [{ projectId: "p", displayName: "A project with a rather long display name", canonicalRoot: "/home/owner/projects/a-repository-with-a-long-name", activation: {} },
    ...EXTRA.map((projectId, i) => ({ projectId, displayName: `Service ${i} with a rather long display name`, canonicalRoot: `/home/owner/projects/service-${i}`, activation: {} }))],
  "capabilities.get": () => capabilitiesWire(["capabilities.get", "project.snapshot", "tasks.create", "tasks.start", "decisions.list", "decisions.answer", "results.get", "executions.get", "history.page"]),
  "project.snapshot": message => { const id = (message as { projectId?: string }).projectId, all = tasksOf(id), s = snapshot("4", all.slice(0, PAGE), id ?? "p"); return all.length > PAGE ? { ...s, tasks: { ...s.tasks, nextCursor: String(PAGE) } } : s; },
  "tasks.list": message => { const at = Number((message as { cursor: string }).cursor), all = tasksOf((message as { projectId?: string }).projectId); return { items: all.slice(at, at + PAGE), nextCursor: at + PAGE < all.length ? String(at + PAGE) : null, highWatermark: "4" }; },
  "decisions.list": () => ({ items: [], nextCursor: null, highWatermark: "4" }),
  "history.page": () => ({ items: [], nextCursor: null, highWatermark: "0" }),
  "machine.call": message => {
    const { op, params } = message as { op: string; params: { file: string } };
    if (op === "repo.snapshot") return { ok: true, result: repoFacts };
    if (op === "product.read") { refusedRead(); return { ok: true, result: structuredClone({ tasks: Object.assign({ "task-1": { planning: true }, ...boardOverlays }, ...graphs.map(x => x.overlays)), sprints: [sprint, pausedSprint, ...boardSprints, ...graphs.map(x => x.sprint)], messages: queued }) }; }
    if (op === "workers") return { ok: true, result: [] };
    if (op === "overview") return { ok: true, result: { name: "Desk", retentionDays: 30, autoArchiveDays: 7, worktreeRoot: "~/t", version: "1", diskMb: 2, storage: [], cleanup: null, diagnostics: null } };
    if (op === "config.read") return { ok: true, result: { config, connections: [], tokens: {}, usage: [] } };
    if (op === "project.config.read") return { ok: true, result: { value: { revisions: {} }, tokens: {} } };
    if (op === "repo.file") return { ok: true, result: { file: params.file, text: scroll ? LONG_FILE : "{\n  \"singleQuote\": true\n}\n", revision: "r1", readOnly: null, writer: null } };
    return { ok: true, result: {} };
  },
});
if (query.get("world") === "review") {
  // Results, Changes, Ready to merge and their dialogs, from one projection: what the real port would hand the screens.
  const port = scriptedPort({ capabilities: { everything: true, operations: [] } });
  history.replaceState(null, "", (query.get("route") ?? "/m/<M>/t/rg/changes").replaceAll("<M>", LOCAL_MACHINE_ID));
  render(() => <Workspace port={port.port} />, document.getElementById("root")!);
  setTimeout(() => port.emit(projection("1", reviewWorld())), 0);
} else if (query.get("world") === "conversation" || query.get("world") === "pages") {
  // The task screen as a conversation: a question waiting, work running with a queued message, and a result to review.
  // `pages` adds a draft, an accepted and a closed task, a sprint being planned and the model choices, for every other page.
  const port = scriptedPort({ capabilities: { everything: true, operations: [] } });
  // The coordinator's record as its transcript keeps it: each reply in the pieces it streamed in, between folded tool steps.
  const pieces = (from: number, text: string) => text.split(/(?<= )/).map((t, i) => ({ sequence: from + i, kind: "note" as const, text: t, at: null }));
  const step = (sequence: number) => ({ sequence, kind: "tool" as const, text: "mcp__cortex__project_read", at: null });
  const workerActivity = port.port.readWorkerActivity?.bind(port.port);
  port.port.readWorkerActivity = async (taskId, workerId, cursor) => !workerId.startsWith("coordinator:") && workerActivity ? workerActivity(taskId, workerId, cursor) : ({ activity: [
    ...pieces(1, "Начинаю делать поля ввода растущими вместе с текстом."), step(40),
    ...pieces(41, "Изменения готовы, сейчас идёт финальная проверка."), step(80),
    ...pieces(81, "Готово: поля ввода в формах создания и сообщений теперь растут вместе с текстом. Поле становится выше, когда вы печатаете или вставляете длинный текст, и снова уменьшается, когда текст удаляют."),
  ], activityCursor: null, messages: [] });
  history.replaceState(null, "", (query.get("route") ?? "/m/<M>/t/c-ask").replaceAll("<M>", LOCAL_MACHINE_ID));
  render(() => <Workspace port={port.port} chrome={chrome} />, document.getElementById("root")!);
  const sprintState = query.get("sprint");
  const world = query.get("world") === "pages" ? pagesWorld(sprintState === "running" || sprintState === "closed" ? sprintState : "planning") : scroll ? longConversation(conversationWorld()) : conversationWorld();
  setTimeout(() => port.emit(projection("1", world)), 0);
  // `scroll` only: one streamed update, as a working task sends them, for the scroll gate to send while it scrolls.
  let version = 1;
  // What each update cost: making the fake data (the fixture's own work) and the screens taking it in (the app's).
  const cost = { updates: 0, data: 0, app: 0, appMax: 0 };
  if (scroll) (window as unknown as { liveCost: typeof cost }).liveCost = cost;
  if (scroll) (window as unknown as { liveUpdate: () => void }).liveUpdate = () => {
    const t0 = performance.now(), next = structuredClone(world), timeline = next.sessions.find(x => x.taskId === "c-work")!.timeline;
    const at = timeline.map(e => e.kind).lastIndexOf("cortex"), last = timeline[at];
    if (last?.kind === "cortex") timeline[at] = { ...last, text: `${last.text} More.` };
    const worker = next.workers.find(w => w.id === "w-be2")!;
    worker.activity = [...worker.activity, { sequence: worker.activity.length + 1, kind: "note", text: `Streamed note ${version}.`, at: null }];
    Object.assign(world, next);
    const t1 = performance.now();
    port.emit(projection(String(++version), next));
    const t2 = performance.now();
    cost.updates++; cost.data += t1 - t0; cost.app += t2 - t1; cost.appMax = Math.max(cost.appMax, t2 - t1);
  };
} else if (query.get("view") === "primitives") {
  // The fixture's #root is the window's two-row grid; the harness takes the whole of it.
  const root = document.getElementById("root")!;
  const pane = query.get("pane");
  render(() => <div style={{ "grid-row": "1 / -1", "min-width": "0", "min-height": "0" }}><Primitives stress={query.has("stress")} dialog={query.has("dialog")} pane={pane ? Number(pane) : null} /></div>, root);
} else {
  // The screens' routes are named in layout-fit.py, one per entry; this fills in the two values only the data here knows.
  const route = query.get("route") ?? "/m/<M>/p/p/git";
  history.replaceState(null, "", route.replaceAll("<M>", LOCAL_MACHINE_ID).replaceAll("<FILE>", encodeURIComponent(changed[0])));
  // `chrome` puts the desktop window's own controls in the title band, as the real window has them.
  render(() => <Workspace port={createRealPort({ transport: s.transport, actorId: "owner", registry: true })} terminal={terminal} chrome={startup || query.has("chrome") ? chrome : undefined} logo={startup ? <img src={logo} alt="" width="96" height="96" /> : undefined} />, document.getElementById("root")!);
}
// The working task after a long day: 300 turns of replies with Markdown, folded tool steps and owner messages, and its
// Backend Dev's feed of 1,500 notes and steps.
function longConversation(s: ReturnType<typeof conversationWorld>) {
  const reply = (i: number) => `### Step ${i}: what changed\n\nThe refresh handler now takes the row lock before it reads the token, so a second refresh waits. Checked **${i % 9 + 1} files** and the migration.\n\n- Kept the old error message\n- Added \`version\` to the token row\n\n\`\`\`ts\nawait db.lock("tokens", id); // turn ${i}\n\`\`\``;
  const session = s.sessions.find(x => x.taskId === "c-work")!;
  session.timeline = [...session.timeline, ...Array.from({ length: 300 }, (_, i) => [
    { id: `long-${i}-a`, kind: "cortex" as const, text: reply(i) },
    { id: `long-${i}-f`, kind: "fold" as const, text: `Edited ${i % 4 + 1} files, ran ${i % 3 + 1} commands`, steps: Array.from({ length: 4 }, () => "mcp__cortex__project_read") },
    ...(i % 5 === 0 ? [{ id: `long-${i}-s`, kind: "said" as const, text: `Please also keep the mobile error text, round ${i}.` }] : []),
  ]).flat()];
  const worker = s.workers.find(w => w.id === "w-be2")!;
  worker.activity = Array.from({ length: 1500 }, (_, i) => i % 3 === 2 ? { sequence: i + 1, kind: "tool" as const, text: "mcp__cortex__project_read", at: null } : { sequence: i + 1, kind: "note" as const, text: reply(i), at: null });
  return s;
}
(window as unknown as { fixture: string | null }).fixture = query.get("load");
