import { existsSync, readdirSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { HELP } from "../src/orch-help";
import { workspaceCopy } from "../src/copy";

/** The rules a source file can answer on its own. What needs layout is in sweep.measure.mjs. */
const files = (dir: string): string[] => readdirSync(dir).flatMap(name => { const path = join(dir, name); return statSync(path).isDirectory() ? files(path) : /\.tsx?$/.test(name) ? [path] : []; });
// This package's sources, from the package or the repository root, where a bare "src" is another tree.
const SRC = ["src", join("packages", "workspace-ui", "src")].find(dir => existsSync(join(dir, "workspace.css")) && existsSync(join(dir, "composer.tsx")))!;
const source = files(SRC).map(path => ({ path, text: readFileSync(path, "utf8").replace(/\/\*[\s\S]*?\*\//g, "").replace(/^\s*\/\/.*$/gm, "").replace(/\s\/\/ .*$/gm, "") }));
const hits = (rule: RegExp) => source.flatMap(f => f.text.split("\n").filter(line => rule.test(line)).map(line => `${f.path}: ${line.trim().slice(0, 120)}`));

describe("interface rules", () => {
  it("working indicators move only while pending, by transform and opacity, and pause when nobody sees them", () => {
    const css = readFileSync(join(SRC, "workspace.css"), "utf8").replace(/\/\*[\s\S]*?\*\//g, "");
    // Every repeating animation sits on an indicator that exists only while something is pending.
    const pending = /^(\.spin|\.ws-loading \.spin|\.spin\.ws-wait|\.shimmer|\[data-busy\]::after|\.btn\[aria-busy="true"\]::after)$/;
    const looping = [...css.matchAll(/([^{}]+)\{([^{}]*)\}/g)].filter(m => /animation[^;]*infinite/.test(m[2]));
    expect(looping.length).toBeGreaterThan(0);
    for (const m of looping) for (const selector of m[1].split(",").map(s => s.trim())) expect(selector).toMatch(pending);
    // What they run changes transform or opacity only: no layout, paint, shadow or filter per frame.
    const used = new Set(looping.flatMap(m => [...m[2].matchAll(/animation(?:-name)?:\s*([^;]+)/g)].flatMap(a => a[1].split(/,(?![^(]*\))/).map(part => part.trim().split(/\s+/)[0]))));
    const frames = new Map([...readFileSync(join(SRC, "mockup-styles.css"), "utf8").concat(css).matchAll(/@keyframes ([\w-]+) \{((?:[^{}]*\{[^}]*\})*)\s*\}/g)].map(k => [k[1], k[2]]));
    for (const name of used) {
      expect(frames.has(name), name).toBe(true);
      for (const p of frames.get(name)!.matchAll(/([\w-]+)\s*:/g)) expect(p[1], name).toMatch(/^(transform|opacity)$/);
    }
    // A hidden window and a parked tab pause them, pseudo-element bars included.
    expect(css).toMatch(/html\[data-hidden\] \*, html\[data-hidden\] \*::after \{[^}]*animation-play-state: paused/);
    expect(css).toMatch(/\.ws-pane\[data-parked\] \*, \.ws-pane\[data-parked\] \*::after \{[^}]*animation-play-state: paused/);
    // Reduced motion still shows life, as a slow pulse.
    expect(css).toMatch(/prefers-reduced-motion: reduce\)\s*\{[^}]*\[data-busy\]::after[^{]*\{[^}]*animation: ws-pulse/);
    expect(css).toMatch(/prefers-reduced-motion: reduce\)\s*\{\s*\.spin[^{]*\{[^}]*animation: ws-pulse/);
  });

  it("keeps default hints and labels in English, with only the approved Russian overrides", () => {
    const copyPath = join(SRC, "copy.ts");
    const approved = new Set([
      `${copyPath}: sprintProgressTitle: "Спринт: {title}",`,
      `${copyPath}: sprintProgressDone: "Готово {done} из {total}",`,
      `${copyPath}: sprintProgressPartial: "Готово ≥{done} из {total}",`,
      `${copyPath}: sprintProgressUnavailable: "Нет данных",`,
      `${copyPath}: sprintProgressWorking: "В работе: {count}",`,
      `${copyPath}: sprintProgressWaiting: "Ожидают вас: {count}",`,
      `${copyPath}: sprintProgressNotStarted: "Ещё не начаты: {count}",`,
      `${copyPath}: sprintProgressOutside: "Из готовых — вне приложения: {count}",`,
      `${copyPath}: sprintProgressUnverifiedDone: "Из готовых — закрыты без проверки: {count}",`,
      `${copyPath}: sprintProgressClosed: "Закрыты, не проверены: {count}",`,
      `${copyPath}: sprintProgressArchived: "В архиве: {count}",`,
      `${copyPath}: sprintProgressUnknown: "Статус неизвестен: {count}",`,
      `${copyPath}: questionRecommended: "Рекомендую",`,
      `${copyPath}: discardTextDraft: "Удалить черновик сообщения",`,
      `${copyPath}: discardAnswerDraft: "Удалить черновик ответа",`,
      `${copyPath}: textDraftStorageUnavailable: "Не удалось обновить черновик на этом устройстве.",`,
      `${copyPath}: question: "Вопрос",`,
      `${copyPath}: count: n => \`Вопросы (\${n})\`,`,
      `${copyPath}: position: (n, total) => \`Вопрос \${n} из \${total}\`,`,
      `${copyPath}: askedBy: from => \`Спрашивает \${from}\`,`,
      `${copyPath}: coordinator: "координатор",`,
      `${copyPath}: askedAutomatically: "Вопрос задан автоматически",`,
      `${copyPath}: answer: "Ответ:",`,
      `${copyPath}: notChosen: "пока не выбран",`,
      `${copyPath}: choices: n => n === undefined ? "Ответы" : \`Ответы на вопрос \${n}\`,`,
      `${copyPath}: answerLabel: n => n === undefined ? "Ваш ответ" : \`Ваш ответ на вопрос \${n}\`,`,
      `${copyPath}: write: files => files ? "Напишите ответ, упомяните файлы через @…" : "Напишите ответ…",`,
      `${copyPath}: own: files => files ? "Или напишите свой ответ, упомяните файлы через @…" : "Или напишите свой ответ…",`,
      `${copyPath}: add: files => files ? "Дополните ответ, упомяните файлы через @…" : "Дополните ответ…",`,
      `${copyPath}: send: "Отправить ответ",`,
      `${copyPath}: sendAll: "Отправить ответы",`,
      `${copyPath}: sending: many => many ? "Отправляем ответы…" : "Отправляем ответ…",`,
      `${copyPath}: attachments: "Вложения ещё загружаются",`,
      `${copyPath}: incomplete: (n, total) => \`Сначала ответьте на все вопросы: \${n} из \${total}\`,`,
      `${copyPath}: choose: "Выберите или напишите ответ",`,
      `${copyPath}: progress: (n, total) => \`Дано ответов: \${n} из \${total}\`,`,
      `${copyPath}: failure: "Не удалось отправить ответы.",`,
      `${copyPath}: retained: "Ваши ответы сохранены. Отправьте их ещё раз, когда будете готовы.",`,
      `${copyPath}: retry: "Повторить",`,
      `${copyPath}: sent: many => many ? "Ответы отправлены. Работа продолжается." : "Ответ отправлен. Работа продолжается.",`,
      `${copyPath}: waiting: duration => \`Ждём \${duration}\`,`,
      `${copyPath}: paused: "Вопрос сохранён. Чтобы ответить, возобновите задачу.",`,
      `${copyPath}: resume: "Сначала возобновите задачу",`,
      `${copyPath}: blocked: "Сейчас нельзя ответить на этот вопрос",`,
      `${copyPath}: reviewGoalNotMet: "Этап одобрен, цель не достигнута",`,
      `${copyPath}: verificationWillNotRun: "Не будет запущена: нужны доработки",`,
      `${copyPath}: deleteWorktreeFolder: "Удалить worktree папку",`,
      `${copyPath}: closeDeleteWorktreeFolder: "Закрыть и удалить worktree папку",`,
      `${copyPath}: taskWorkspaceHeading: "Где будет работать задача",`,
      `${copyPath}: taskWorkspaceFolder: "Рабочая папка",`,
      `${copyPath}: taskWorkspaceProjectFolder: "Папка проекта",`,
      `${copyPath}: taskWorkspaceWorktree: "Отдельный worktree",`,
      `${copyPath}: taskWorkspaceOpenBranch: "Открытая ветка по последним данным",`,
      `${copyPath}: taskWorkspaceTaskBranch: "Ветка задачи",`,
      `${copyPath}: taskWorkspaceSprintCheckout: "Рабочая копия спринта",`,
      `${copyPath}: taskWorkspaceCreatedAtStart: "Создаётся при запуске задачи",`,
      `${copyPath}: taskWorkspaceMergeInto: "Влить в",`,
      `${copyPath}: taskWorkspaceTargetHint: "Целевая ветка для завершения задачи.",`,
      `${copyPath}: taskWorkspaceGateMerge: "Задача остановится на этапе «Готова к слиянию» и будет ждать вашего подтверждения.",`,
      `${copyPath}: taskWorkspaceGateFinish: "Задача остановится на этапе «Готова к завершению» и будет ждать вашего подтверждения.",`,
      `${copyPath}: taskWorkspaceTargetUnknown: "Целевая ветка не подтверждена.",`,
      `${copyPath}: taskWorkspaceGitDefault: "Ветка Git по умолчанию",`,
      `${copyPath}: taskWorkspaceOtherBranch: "Другая ветка…",`,
      `${copyPath}: taskWorkspaceBranchName: "Название ветки",`,
      `${copyPath}: taskWorkspaceFollowsSprint: "Рабочая копия определяется спринтом.",`,
      `${copyPath}: taskWorkspaceLocalChanges: "Последние данные об изменениях",`,
      `${copyPath}: taskWorkspaceNoGit: "Сведения об изменениях Git для этой папки недоступны.",`,
      `${copyPath}: taskWorkspaceNoListedFiles: "В последнем снимке Git для этой папки нет изменений.",`,
      `${copyPath}: taskWorkspaceFilesUnknown: "Данные о состоянии рабочей копии ещё не получены.",`,
      `${copyPath}: taskWorkspaceMoreFiles: "Ещё путей в последнем снимке: {count}.",`,
      `${copyPath}: taskWorkspaceSharedEffect: "Задача работает с текущими файлами папки проекта, включая незакоммиченные изменения.",`,
      `${copyPath}: taskWorkspaceInheritedEffect: "Задача работает в копии спринта. Здесь показаны изменения папки проекта.",`,
      `${copyPath}: taskWorkspaceSeparateEffect: "Новый worktree создаётся из коммита HEAD проекта. Изменения выше остаются в папке проекта.`,
      `${copyPath}: taskWorkspaceOtherTasks: "Другие задачи проекта",`,
      `${copyPath}: taskWorkspaceTasksUnknown: "Состояние других задач пока не подтверждено.",`,
      `${copyPath}: taskWorkspaceNoConfirmedTasks: "В текущих данных проекта нет активных задач или задач с неизвестным состоянием.",`,
      `${copyPath}: taskWorkspaceMoreTasks: "Ещё задач в текущем представлении: {count}.",`,
      `${copyPath}: taskWorkspaceUnknown: "Неизвестно",`,
      `${copyPath}: taskWorkspaceWaiting: "Требует внимания",`,
      `${copyPath}: taskWorkspaceActive: "Активна",`,
      `${copyPath}: taskWorkspaceFolderUnknown: "Рабочая копия не подтверждена",`,
      `${copyPath}: taskWorkspaceMachineDetails: "Сведения о машине",`,
      `${copyPath}: worktreeNeedsGit: "Для отдельного worktree нужен репозиторий Git.",`,
      `${copyPath}: noTaskWorktrees: "Служба не поддерживает отдельный worktree для задачи.",`,
      `${copyPath}: worktreeNeedsManualOrHybrid: "Для отдельного worktree нужен ручной или гибридный режим.",`,
      `${copyPath}: sideChat: "Побочный чат",`,
      `${copyPath}: sideChatNote: "Не влияет на задачу",`,
      `${copyPath}: sprintCorrectsTask: "Исправляет задачу",`,
      `${copyPath}: sprintAffectedStarted: "Затронутые задачи, которые уже запущены",`,
      `${copyPath}: requestSprintRevision: "Запросить пересмотр спринта",`,
      `${copyPath}: requestSprintRevisionHelp: "Откройте выполняющуюся задачу, которую касается изменение, и опишите его в чате. Координатор`,
      `${copyPath}: copyTaskId: "Скопировать ID задачи",`,
      `${copyPath}: copySprintId: "Скопировать ID спринта",`,
      `${copyPath}: copyProjectId: "Скопировать ID проекта",`,
      `${copyPath}: objectIdCopied: "ID скопирован",`,
      `${copyPath}: objectIdManual: "Буфер обмена недоступен. Скопируйте выделенный ID.",`,
      `${copyPath}: sprintBlockers: "Что мешает спринту?",`,
      `${copyPath}: sprintBlockersClose: "Закрыть",`,
      `${copyPath}: sprintBlockersOffline: "Машина не в сети. Текущие препятствия не подтверждены.",`,
      `${copyPath}: sprintBlockersIncomplete: "Состав спринта, состояния задач или их зависимости известны не полностью. Точных чисел пока н`,
      `${copyPath}: sprintBlockersCycle: "В спринте есть цикл зависимостей. Определить первое препятствие невозможно.",`,
      `${copyPath}: sprintBlockersBlocked: "Ждут зависимостей",`,
      `${copyPath}: sprintBlockersAffected: "Зависимых черновиков",`,
      `${copyPath}: sprintBlockersClear: "Зависимости выполнены",`,
      `${copyPath}: sprintBlockersQuiet: "Ни один из подтверждённых черновиков не ждёт зависимости.",`,
      `${copyPath}: sprintBlockersQueuePaused: "Очередь спринта на паузе. Паузы отдельных задач и их зависимости учитываются отдельно.",`,
      `${copyPath}: sprintBlockersImmediately: "Осталась только эта зависимость",`,
      `${copyPath}: sprintBlockersLater: "Есть и другие условия",`,
      `${copyPath}: sprintBlockersChain: "Пример цепочки зависимостей",`,
      `${copyPath}: sprintBlockersOverlap: "У черновика может быть несколько препятствий. Он будет показан у каждого из них.",`,
      `${copyPath}: sprintBlockersQueueHold: "Приостановка очереди",`,
      `${copyPath}: sprintBlockersWaitingOn: "Ещё ждёт",`,
      `${copyPath}: sprintBlockersTaskPaused: "Задача на паузе",`,
      `${copyPath}: sprintBlockersTaskWorking: "В работе",`,
      `${copyPath}: sprintBlockersTaskDraft: "Черновик · зависимости выполнены",`,
      `${copyPath}: sprintBlockersTaskDecision: "Нужно решение",`,
      `${copyPath}: sprintBlockersTaskReview: "Ждёт проверки",`,
      `${copyPath}: sprintBlockersTaskIntegration: "Ждёт интеграции",`,
      `${copyPath}: sprintBlockersTaskStopped: "Запуск завершился, зависимость не выполнена",`,
      `${copyPath}: sprintBlockersTaskStopping: "Останавливается",`,
      `${copyPath}: sprintBlockersTaskUnsatisfied: "Условие ещё не выполнено",`,
      `${copyPath}: sprintBlockersPauseNote: "Переход к задаче не снимает паузу. Одного возобновления недостаточно для выполнения зависимост`,
      `${copyPath}: sprintRevisionFrom: "Изменения спринта от задачи {task}",`,
      `${copyPath}: sprintRevisionChangedBy: "изменено задачей {task}",`,
      `${copyPath}: sprintRevisionAddedBy: "добавлено задачей {task}",`,
      `${copyPath}: sprintRevisionHistory: "История изменений спринта",`,
      `${copyPath}: phoneNotify: "Уведомления на телефоне",`,
      `${copyPath}: phoneNotifyNote: "Вопросы, результаты и сбои с ваших компьютеров. Пока они включены, тихое уведомление показывает, что п`,
      `${copyPath}: phoneNotifyBlocked: "Android запрещает уведомления этого приложения. Разрешите их в настройках телефона, в разделе «Прил`,
      `${copyPath}: phoneNotifyBattery: "Во сне телефон может приостанавливать связь. Чтобы она сохранялась, разрешите приложению работать б`,
      `${copyPath}: phoneNotifyBatteryButton: "Настройки батареи",`,
      `${copyPath}: phoneNotifyOngoingTitle: "На связи с компьютерами",`,
      `${copyPath}: phoneNotifyOngoingText: "Следим за вопросами, результатами и сбоями на ваших компьютерах.",`,
      `${copyPath}: phoneNotifyChannelOngoing: "Связь",`,
      `${copyPath}: phoneNotifyChannelOngoingNote: "Тихое уведомление, пока приложение держит связь с вашими компьютерами.",`,
      `${copyPath}: phoneNotifyChannelAnswer: "Нужен ваш ответ",`,
      `${copyPath}: phoneNotifyChannelAnswerNote: "Задача задала вопрос, остановилась для вашего решения или ей нужен доступ.",`,
      `${copyPath}: phoneNotifyChannelDone: "Задача завершена",`,
      `${copyPath}: phoneNotifyChannelDoneNote: "Задача закончила работу, и её результат ждёт вашей проверки.",`,
      `${copyPath}: phoneNotifyChannelFailure: "Сбой задачи",`,
      `${copyPath}: phoneNotifyChannelFailureNote: "Сбой задачи, пауза спринта или задача без прогресса.",`,
      `${copyPath}: phoneNotifyQuestion: "Нужен ваш ответ",`,
      `${copyPath}: phoneNotifyStop: "Задача остановлена: нужно ваше решение",`,
      `${copyPath}: phoneNotifyAccess: "Задаче нужен доступ",`,
      `${copyPath}: phoneNotifyPlanning: "Вопрос по планированию спринта",`,
      `${copyPath}: phoneNotifyResult: "Задача завершена",`,
      `${copyPath}: phoneNotifyFailed: "Сбой задачи",`,
      `${copyPath}: phoneNotifyPaused: "Спринт на паузе",`,
      `${copyPath}: phoneNotifyStalled: "Нет прогресса",`,
      `${copyPath}: coordinatorModels: "Модели координатора",`,
      `${copyPath}: coordinatorModelsLead: "Модели, которые может использовать координатор: каждая в отмеченных категориях и с effort из их `,
      `${copyPath}: coordinatorModelsListed: "Модели, доступные координатору",`,
      `${copyPath}: coordinatorModelsAdd: "Добавить модель",`,
      `${copyPath}: coordinatorModelsSprint: "Как планирование спринта выбирает модель",`,
      `${copyPath}: coordinatorModelsNone: "Списка моделей координатора ещё нет",`,
      `${copyPath}: coordinatorModelsLegacy: "В этом профиле нет списка моделей координатора, поэтому координатор может использовать любую м`,
      `${copyPath}: coordinatorModelsStart: "Создать список",`,
      `${copyPath}: mcpProjectLead: "Серверы проекта {project}. Серверы машины действуют и здесь: включите или выключите любой только для эт`,
      `${copyPath}: mcpInherited: "С машины",`,
      `${copyPath}: mcpInheritedNote: "Менять их можно в MCP-серверах машины. Переключатель здесь меняет только этот проект.",`,
      `${copyPath}: mcpInheritedNone: "На машине нет MCP-серверов.",`,
      `${copyPath}: mcpForProject: "В этом проекте",`,
      `${copyPath}: mcpReplaced: "Заменён сервером этого проекта с тем же именем",`,
      `${copyPath}: mcpMachineOn: "Включён, как на машине",`,
      `${copyPath}: mcpMachineOff: "Выключен, как на машине",`,
      `${copyPath}: mcpHereOn: "Включён только для этого проекта",`,
      `${copyPath}: mcpHereOff: "Выключен только для этого проекта",`,
      `${copyPath}: mcpEditOnMachine: "Изменить на машине",`,
      `${copyPath}: mcpOwn: "Только в этом проекте",`,
      `${copyPath}: mcpOwnNone: "У проекта нет своих серверов. Добавьте сервер, чтобы дать дополнительные инструменты только этому проекту."`,
      `${copyPath}: mcpReplacesMachine: "Здесь заменяет сервер машины с тем же именем",`,
    ]);
    expect(hits(/[\u0400-\u04ff]/).filter(line => !approved.has(line))).toEqual([]);
    expect(JSON.stringify(HELP)).not.toMatch(/[\u0400-\u04ff]/);
    expect(workspaceCopy("questionRecommended", "en-US")).toBe("Recommended");
    expect(workspaceCopy("questionRecommended", "ru-RU")).toBe("Рекомендую");
    expect(workspaceCopy("reviewGoalNotMet", "ru-RU")).toBe("Этап одобрен, цель не достигнута");
    expect(workspaceCopy("verificationWillNotRun", "en-US")).toBe("Will not run: changes needed");
    expect(workspaceCopy("coordinatorModels", "en-US")).toBe("Coordinator models");
    expect(workspaceCopy("coordinatorModels", "ru-RU")).toBe("Модели координатора");
  });
  it("does not name the program inside the program", () => {
    expect(hits(/\bCortex\b/)).toEqual([]);
  });
  it("has no second control for renaming what a click on its title edits", () => {
    expect(hits(/label="Rename|>Rename<|Title generated|Title set/)).toEqual([]);
  });
  it("never spells out a route or connection id in words", () => {
    expect(hits(/\b(?:xr|cn)_/).filter(line => !line.includes(".test("))).toEqual([]);
  });
  it("has no native hover titles, the owner reads no popups", () => {
    // The tag a title= sits on, walking back past balanced braces so props inside handlers do not count.
    const tagOf = (text: string, at: number) => { let depth = 0; for (let i = at; i >= 0; i--) { const c = text[i]; if (c === "}") depth++; else if (c === "{") depth--; else if (c === "<" && depth === 0 && /[A-Za-z]/.test(text[i + 1] ?? "")) return /^[\w.]+/.exec(text.slice(i + 1))![0]; } return ""; };
    const native = source.flatMap(f => [...f.text.matchAll(/\stitle=/g)].flatMap(m => { const tag = tagOf(f.text, m.index!); return tag === "A" || /^[a-z]/.test(tag) ? [`${f.path}: <${tag} title=`] : []; }));
    expect(native).toEqual([]);
    expect(hits(/["']?title["']?\s*:\s*reason|\.title\s*=/)).toEqual([]);
  });
  it("gives every disabled control its reason in visible words, never only a state", () => {
    const opened = (text: string) => { const out: string[] = []; for (const m of text.matchAll(/<(?:[a-z][\w-]*|A)\b/g)) { let depth = 0, i = m.index!; for (; i < text.length; i++) { const c = text[i]; if (c === "{") depth++; else if (c === "}") depth--; else if (c === ">" && depth === 0 && text[i - 1] !== "=") break; } out.push(text.slice(m.index!, i)); } return out; };
    const unexplained = source.flatMap(f => opened(f.text).filter(tag => /aria-disabled=/.test(tag) && !/data-why=|ws\.needs\(/.test(tag)).map(tag => `${f.path}: ${tag.slice(0, 80)}`));
    expect(unexplained).toEqual([]);
  });
  it("never draws a disabled reason inside a control", () => {
    const sheets = readdirSync(SRC).filter(name => name.endsWith(".css")).map(name => ({ name, css: readFileSync(join(SRC, name), "utf8").replace(/\/\*[\s\S]*?\*\//g, "") }));
    expect(sheets.map(s => s.name)).toEqual(expect.arrayContaining(["layout.css", "mockup-styles.css", "workspace.css"]));
    const rules = sheets.flatMap(s => [...s.css.matchAll(/([^{}]+)\{([^{}]*)\}/g)].map(m => ({ sheet: s.name, selector: m[1].trim(), body: m[2] })));
    // data-why is a declaration for ReasonSlot and ReasonTip to read. No sheet may style by it, or print it with content: attr().
    expect(rules.filter(r => r.selector.includes("data-why")).map(r => `${r.sheet}: ${r.selector}`)).toEqual([]);
    // The one printed attribute is a settings table's column name over a cell on a phone (data-label), never a reason.
    expect(sheets.flatMap(s => s.css.match(/content:\s*attr\((?!data-label\))[^)]*\)/g) ?? [])).toEqual([]);
    // Nor may a rule grow the control itself, however it gets there.
    expect(rules.filter(r => /\.btn\b|\.ib\b/.test(r.selector) && /height:\s*auto|flex-wrap:\s*wrap|flex-flow:[^;]*\bwrap|white-space:\s*normal/.test(r.body)).map(r => `${r.sheet}: ${r.selector}`)).toEqual([]);
  });
  it("keeps the layout primitives in one layer that holds every class they use", () => {
    const css = readFileSync(join(SRC, "layout.css"), "utf8");
    expect(css.replace(/\/\*[\s\S]*?\*\//g, "").trim()).toMatch(/^@layer layout \{[\s\S]*\}$/);
    const tsx = source.find(f => f.path.endsWith("layout.tsx"))!.text;
    const used = [...new Set([...tsx.matchAll(/\bl-[a-z-]+/g)].map(m => m[0]))].filter(name => !name.endsWith("-"));
    expect(used.length).toBeGreaterThan(15);
    expect(used.filter(name => !css.includes(`.${name}`))).toEqual([]);
  });
  it("counts through Count, so an empty list never reads zero", () => {
    expect(hits(/<span class="n">\{[^}]*\.length\}<\/span>/)).toEqual([]);
  });
});
