import { emptySession, emptyWorkspace, type Activity, type Check, type FileDiff, type Task, type Worker, type WorkspaceState } from "@cortex/workspace-domain";
import { STEP_PREFIX } from "../src/tool-names";
import { draftTask } from "./support";

// The review screens for the rendered layout gate: long Russian content wherever a result, a check or a branch can be long.
const BRANCH = "feature/очень-длинная-ветка-для-редизайна-дашборда-которая-не-помещается-ни-в-одну-кнопку-и-ни-в-одну-строку";
const TITLE = "Редизайн дашборда: единый современный светлый интерфейс на всех экранах, от страницы входа до журнала выгрузок и настроек";
const SUMMARY = `Готово: у дашборда новый светлый современный интерфейс, единый на всех экранах.

### Что изменилось

- Вместо узкой панели значков теперь понятное боковое меню с подписями и спокойная шапка.
- Единый стиль: цвета, шрифты, отступы, скругления и мягкие тени. Он применён к кнопкам, формам, карточкам, таблицам, статусам и уведомлениям.
- Главное действие заметно выделено, у элементов есть состояния наведения, фокуса и недоступности.
- Интерфейс перестраивается под ширину от 1920 до 360 px. На узких экранах меню открывается отдельной панелью, а таблица прокручивается внутри своего блока.
- Если выгрузка CSV сорвётся при закрытом журнале, появится красное уведомление с причиной и предупреждением о возможном неполном файле.

Логика работы, тексты и адреса страниц сохранены. Сервер не менялся.

### Как посмотреть

Приложение сейчас не запущено, а работающую копию я не трогал. Запустите его с новой сборкой: \`docker compose up -d --build\`.

### Не проверено

- Как программы чтения с экрана ведут себя, когда открыто боковое меню на узком экране — причина: программы чтения с экрана здесь недоступны; проверено только поведение в Chromium с клавиатуры.
- Вид интерфейса в Firefox — причина: браузерные тесты проекта настроены только на Chromium.
- Тесты сервера и базы данных — причина: код сервера не менялся, поэтому они не запускались.
- Повторный запуск тестов при обновлении документации — причина: на этом этапе менялись только документы, поэтому тесты там не запускались; общий прогон выполнен отдельно.`;
const UNVERIFIED = "Runtime kept this passed claim unverified: the cited command cannot verify anything (a no-op, print, listing, read or search command such as pwd, ls, echo, cat or grep).";
const MANUAL: [string, string][] = [
  ["installed fonts ship Cyrillic files", UNVERIFIED],
  ["rendered visual check of the new design", "This is a read-only design assignment and the new interface is not built yet, so there is nothing to render. Screenshots at the six target widths belong to the implementation and its review."],
  ["current build facts behind the spec (dark theme, 1024 px min-width, 72 px bar, 28 px rows, fontsource imports)", UNVERIFIED],
  ["IBM Plex Sans and JetBrains Mono Cyrillic font files present for weights used", UNVERIFIED],
  ["Cyrillic 500 weight font files present", UNVERIFIED],
  ["relative link targets in edited and rewritten docs exist", UNVERIFIED],
  ["screen reader behaviour of the inert background", "No screen reader (NVDA, JAWS, VoiceOver) is available in this environment; only the DOM attribute and keyboard behaviour in Chromium were exercised."],
  ["Firefox rendering", "The project's Playwright config runs only the default Chromium project; no Firefox run is configured and the assignment names only that config."],
  ["relative links in edited docs resolve", UNVERIFIED],
  ["dashboard unit tests and e2e suite re-run", "Documentation-only assignment with no code change; the runtime runs final verification. Predecessor results (Vitest 660 passed, next build exit 0, Playwright 35 passed and 180 skipped) are cited in the report from the recorded report and the recorded report, not re-run by this worker."],
  ["exploratory export probe with DOM-object assertions — проверка выгрузки журнала с длинным русским названием, которое не помещается в строку ни на одной ширине окна, потому что в нём перечислены все шаги: открыть журнал, закрыть его во время выгрузки и сравнить файл", "Broker terminated the diagnostic after 30 seconds (exit -9, incomplete, no output); no assertions were established."],
];
const manual = (): Check[] => MANUAL.map(([name, detail]) => ({ name, detail, outcome: "not_run", kind: "manual", rerunnable: false }));
const passed = (n: number): Check[] => Array.from({ length: n }, (_, i) => ({ name: i === 0 ? "Vitest — все модульные тесты дашборда, включая новые проверки адаптивной вёрстки на ширинах 360, 768, 1024 и 1920" : `Automated check ${i + 1}`, detail: i === 0 ? "660 passed in 41 s" : "", outcome: "passed", source: "runtime", command: i < 2 ? ["pnpm", "test", "--", "--run"] : undefined, cwd: i < 2 ? "/home/owner/projects/business-sell-data/dashboard" : undefined, exitCode: i < 2 ? 0 : undefined, output: i < 2 ? "Test Files  92 passed (92)\n     Tests  660 passed (660)" : undefined }));
const technical = (): Check[] => [
  { name: "Paid browser task on the staging host", detail: "", outcome: "not_run", kind: "policy", technicalReason: "Paid live check not requested by the owner", rerunnable: false, source: "runtime" },
  { name: "Playwright visual comparison against the approved screenshots", detail: "The screenshots differ in font rendering only.", outcome: "unverified", technicalReason: "No GPU on this machine, so the browser renders text with a different antialiasing than the approved captures.", source: "worker" },
];
const failing = (): Check => ({ name: "Focused tests for the export journal — журнал выгрузок и уведомление об ошибке", detail: "Exit 1: 2 of 35 failed", outcome: "failed", source: "runtime", command: ["pnpm", "exec", "playwright", "test", "e2e/export.spec.ts"], cwd: "/home/owner/projects/business-sell-data/dashboard", exitCode: 1, rerunnable: true, output: "  2 failed\n    e2e/export.spec.ts:41:3 › shows the failure notice\n    e2e/export.spec.ts:77:3 › keeps the partial file warning\n  33 passed (48.2s)" });
const result = (checks: Check[]) => ({ version: 1, digest: "digest", summary: SUMMARY, checks, at: Date.UTC(2026, 10, 9, 11, 32) });
const lines = (n: number) => Array.from({ length: n }, (_, i) => ({ kind: (i % 3 === 0 ? "del" : "add") as "del" | "add", old: i % 3 === 0 ? i + 1 : null, new: i % 3 === 0 ? null : i + 1, text: `  .sidebar-item-${i} { padding: 8px 12px; color: var(--text-secondary); /* боковое меню с подписями */ }` }));
const FILES: FileDiff[] = ["apps/dashboard/src/components/layout/sidebar-navigation-with-labels-and-collapsed-mobile-panel.tsx", "apps/dashboard/src/styles/tokens.css", "apps/dashboard/src/pages/export-journal/export-journal-failure-notice-with-partial-file-warning.tsx", "docs/DESIGN.md", "apps/dashboard/src/components/table/responsive-table.tsx"]
  .map((path, i) => ({ path, status: i === 3 ? "M" : i % 2 ? "M" : "A", add: 40 + i, del: i * 3, hunks: [{ header: `@@ -${i + 1},12 +${i + 1},18 @@ export function Component${i}()`, staged: false, lines: lines(18) }] }));

export function reviewWorld(): WorkspaceState {
  const s = emptyWorkspace();
  s.machines[0].connection = "connected";
  const machineId = s.machines[0].id;
  s.projects = [
    { id: "nogit", machineId, name: "Business-sell-data", repository: "/home/owner/projects/business-sell-data", openBranch: "", defaultBranch: "", hasGit: false },
    { id: "p", machineId, name: "Дашборд продаж с очень длинным названием проекта", repository: "/home/owner/projects/dashboard", openBranch: "main", defaultBranch: "main", hasGit: true },
  ];
  const task = (id: string, projectId: string, patch: Partial<Task>): Task => ({ ...draftTask(), id, projectId, title: TITLE, description: TITLE, workspaceDigest: "digest", process: "stopped", ...patch });
  const worktree = { kind: "worktree" as const, branch: BRANCH, path: `/home/owner/.cortex/worktrees/dashboard/${"x".repeat(12)}` };
  s.tasks = [
    task("unk", "p", { status: "working", process: "unknown", observedPhase: "unknown", warnings: ["contradictory_evidence"] }),
    task("rv", "nogit", { status: "review", result: result([...manual(), ...passed(20), ...technical()]) }),
    task("rg", "p", { status: "review", checkout: worktree, result: result([...manual(), ...passed(19), failing(), ...technical()]) }),
    task("rm", "p", { status: "ready_to_merge", checkout: worktree, result: { ...result([...manual(), ...passed(22)]), automaticReady: true } }),
    task("rn", "nogit", { status: "ready_to_merge", result: { ...result([...manual(), ...passed(22)]), automaticReady: true } }),
  ];
  s.sessions = [{ ...emptySession("rg"), changes: { files: FILES.length, add: FILES.reduce((n, f) => n + f.add, 0), del: FILES.reduce((n, f) => n + f.del, 0) } }];
  s.repos = [{ projectId: "p", hasGit: true, remote: "", tree: FILES.map(f => f.path), sources: {}, commits: [], branches: [], fetchedAt: null,
    checkouts: [
      { id: "p:main", projectId: "p", kind: "main", branch: "main", path: "/home/owner/projects/dashboard", taskId: null, label: null, base: "main", ahead: 0, behind: 0, behindOf: {}, files: [], draft: "", sources: {}, updateConflict: null },
      { id: "p:rg", projectId: "p", kind: "worktree", branch: BRANCH, path: worktree.path, taskId: "rg", label: null, base: "main", ahead: 3, behind: 0, behindOf: {}, files: [], draft: "", sources: {}, updateConflict: null },
      { id: "p:rm", projectId: "p", kind: "worktree", branch: BRANCH, path: worktree.path, taskId: "rm", label: null, base: "main", ahead: 3, behind: 0, behindOf: {}, files: [], draft: "", sources: {}, updateConflict: null },
    ] }];
  s.reviews = [{ taskId: "rg", sha: "abc1234", files: FILES, scopes: [], comments: [] }, { taskId: "rm", sha: "abc1234", files: FILES, scopes: [], comments: [] }];
  // A working task whose steps recorded grep output far wider than any column, as the owner saw it clipped on the right.
  s.tasks.push(task("out", "nogit", { status: "working", process: "live" }));
  s.workers = [outputWorker()];
  s.sessions.push({ ...emptySession("out"), timeline: [{ id: "history:out:1", kind: "fold", text: "Worked 49s, ran 1 command", steps: [STEP_PREFIX + JSON.stringify({ callId: "c1", title: "Bash", kind: "execute", command: GREP, status: "completed", output: GREP_OUTPUT })] }] });
  return s;
}

const GREP = "bash -c 'grep -rn \"plan: {\\|plan: plan\\|version: [0-9]\" packages/workspace-ui/test/*.ts* | head -40'";
const GREP_OUTPUT = Array.from({ length: 24 }, (_, i) => `packages/workspace-ui/test/lifecycle-next-action-and-owner-review-flow-${i}.test.tsx:${i * 7 + 29}:    s.emit(projection("${i + 1}", world({ plan: { version: ${i + 1}, items: [{ key: "a", title: "Write the brief", dependsOn: [] }] }, result: { version: 1, digest: "d" } })));`).join("\n");
function outputWorker(): Worker {
  const line = GREP_OUTPUT.split("\n")[0];
  const activity: Activity[] = [
    { sequence: 1, kind: "command", text: "grep", at: null, args: JSON.stringify({ kind: "execute", title: "Bash", command: GREP, output: GREP_OUTPUT, status: "completed" }), command: { state: "native", exit: 0, took: null, tail: GREP_OUTPUT, reason: null } },
    { sequence: 2, kind: "command", text: "grep again", at: null, command: { state: "passed", exit: 0, took: "1s", tail: GREP_OUTPUT.split("\n").slice(0, 6).join("\n"), reason: null } },
    { sequence: 3, kind: "edit", text: "Edited sprint-model.ts", at: null, diff: { add: 1, del: 1, lines: ["@@ -61 +61 @@", "-" + line, "+" + line.replace("version", "revision")] } },
  ];
  return { id: "w-out", taskId: "out", executionId: "e-out", role: "frontend-dev", status: "working", lane: "execution", goal: "Implement the pure, framework-free model for the large sprint dependency graph", now: null, waitsFor: null, scope: [], route: { model: "Opus 5.5", effort: "High" }, attempt: { n: 1, of: 1 }, elapsedMs: 337000, runningSince: null, context: null, usage: null, result: null, activity };
}
