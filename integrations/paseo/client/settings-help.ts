/**
 * Bilingual help for the Cortex Settings screen: what each field does and a
 * recommendation. Shown under a single field, or as one block for a group of
 * similar fields (rows, per-model controls). Static text only: no model id is
 * named here, every model comes from the owner's settings and engine catalogs.
 */

export type HelpText = { en: string; ru: string };
export type HelpLanguage = "en" | "ru" | "both";

export const HELP_LANGUAGES: ReadonlyArray<{ value: HelpLanguage; label: string }> = [
  { value: "both", label: "EN + RU" },
  { value: "en", label: "EN" },
  { value: "ru", label: "RU" },
];

/** A block that explains several fields: one "• Field — text" line each. */
export function helpBlock(intro: HelpText | null, items: ReadonlyArray<[string, HelpText]>): HelpText {
  const lines = (lang: "en" | "ru") => [
    ...(intro ? [intro[lang]] : []),
    ...items.map(([label, text]) => `• ${label === "Recommended" && lang === "ru" ? "Рекомендация" : label} — ${text[lang]}`),
  ].join("\n");
  return { en: lines("en"), ru: lines("ru") };
}

const h = (en: string, ru: string): HelpText => ({ en, ru });

export const HELP = {
  // --- screen ---------------------------------------------------------------
  helpLanguage: h(
    "Language of these help texts. Field names stay in English.",
    "Язык подсказок. Названия полей остаются на английском.",
  ),
  project: h(
    "Which project the Project ceiling and Project routing file act on. Owner only edits just your user-wide settings. Recommended: Owner only, unless one project needs its own rules.",
    "Для какого проекта работают разделы Project ceiling и проектный файл маршрутизации. Owner only — правятся только ваши общие настройки. Рекомендация: Owner only, если одному проекту не нужны свои правила.",
  ),
  section: h(
    "Which group of settings to show. Profiles stay visible above every section.",
    "Какую группу настроек показать. Блок профилей виден над любым разделом.",
  ),

  // --- profiles -------------------------------------------------------------
  profilesIntro: h(
    "A profile is a saved snapshot of user defaults, user routing (including engine and model switches), MCP usage policy and connection session limits. Save a section first; then save the applied settings into a profile. Project settings, credentials and MCP launch commands are never stored. Activating a profile rewrites those settings and discards unsaved form edits.",
    "Профиль — сохранённый снимок настроек по умолчанию, пользовательской маршрутизации (включая переключатели движков и моделей), правил использования MCP и лимитов сессий подключений. Сначала сохраните раздел, затем — применённые настройки в профиль. Настройки проектов, учётные данные и команды запуска MCP в профиль не попадают. Активация профиля перезаписывает эти настройки и отбрасывает несохранённые правки форм.",
  ),
  currentProfile: helpBlock(
    h("Choose which saved profile is active.", "Выбор активного профиля."),
    [
      ["Current profile", h(
        "Switches all bound settings to that profile. No profile keeps the current settings without a link. Recommended: keep one profile active (for example default) and save changes into it, so you can always return to a known state.",
        "Переключает все связанные настройки на этот профиль. No profile — настройки остаются как есть, без привязки. Рекомендация: держите один профиль активным (например, default) и сохраняйте изменения в него — так всегда можно вернуться к известному состоянию.",
      )],
      ["Save to current profile", h(
        "Writes the currently applied settings into the active profile.",
        "Записывает текущие применённые настройки в активный профиль.",
      )],
      ["Revert to saved", h(
        "Throws away applied changes and restores the active profile's snapshot.",
        "Отменяет применённые изменения и восстанавливает снимок активного профиля.",
      )],
      ["Reload profile state", h(
        "Re-reads profile state from disk, for example after editing files by hand.",
        "Перечитывает состояние профилей с диска, например после ручной правки файлов.",
      )],
    ],
  ),
  newProfile: helpBlock(null, [
    ["New profile name", h(
      "Name of the new snapshot: lowercase letters, digits, - and _. Recommended: a short purpose name such as default or cheap-review.",
      "Имя нового снимка: строчные буквы, цифры, - и _. Рекомендация: короткое имя по назначению, например default или cheap-review.",
    )],
    ["New profile description", h(
      "Free note shown in the profile list; does not affect behavior.",
      "Свободная заметка в списке профилей, на поведение не влияет.",
    )],
    ["Save as new profile", h(
      "Captures every applied setting into the new profile and makes it active.",
      "Сохраняет все применённые настройки в новый профиль и делает его активным.",
    )],
  ]),
  manageProfiles: helpBlock(null, [
    ["Saved profile", h("Which saved profile to inspect or change.", "Какой профиль просмотреть или изменить.")],
    ["Rename profile to / Profile description", h(
      "Change only the name and note; the stored settings stay the same.",
      "Меняют только имя и заметку; сохранённые настройки не меняются.",
    )],
    ["Activate profile", h("Same as choosing it under Current profile.", "То же, что выбрать его в Current profile.")],
    ["Delete profile", h(
      "Removes only the snapshot; current settings stay as they are.",
      "Удаляет только снимок; текущие настройки остаются как есть.",
    )],
  ]),

  // --- user defaults --------------------------------------------------------
  defaultRoute: helpBlock(
    h(
      "The exact route preselected for new Cortex sessions (the coordinator), tied to one registered connection. Set all three or none.",
      "Точный маршрут, заранее выбранный для новых сессий Cortex (координатора), привязанный к одному подключению. Задавайте все три поля или ни одного.",
    ),
    [
      ["Route id", h("Observed route id (xr_…), listed per model under Engines & models.", "Идентификатор маршрута (xr_…), виден у каждой модели в Engines & models.")],
      ["Connection", h("Registered connection that serves the route.", "Подключение, через которое идёт маршрут.")],
      ["Model (exact id)", h("Exact model id of the route; OpenCode ids are provider/model.", "Точный id модели маршрута; у OpenCode — provider/model.")],
      ["Recommended", h(
        "leave empty and use Portable coordinator default instead: route ids change when connections are re-registered.",
        "оставьте пустым и используйте Portable coordinator default: id маршрутов меняются при перерегистрации подключений.",
      )],
    ],
  ),
  coordinator: helpBlock(
    h(
      "Which engine and model run the coordinator (the agent you talk to) in new sessions. Matched by exact engine/model against the current model list, so it survives re-registering a connection. Clear Default route before setting this.",
      "Какой движок и модель работают координатором (агентом, с которым вы общаетесь) в новых сессиях. Ищется по точной паре движок/модель в текущем списке моделей, поэтому переживает перерегистрацию подключения. Перед заданием очистите Default route.",
    ),
    [
      ["Coordinator engine", h("Engine id: codex-acp, claude-agent-acp or opencode.", "Id движка: codex-acp, claude-agent-acp или opencode.")],
      ["Coordinator model (exact id)", h("Exact model id switched on under Models.", "Точный id модели, включённой в Models.")],
      ["Coordinator parameter (name / value)", h(
        "Native options sent to the engine: thought_level (reasoning effort: low … max) and mode (agent). Only values the model advertises are accepted.",
        "Собственные параметры движка: thought_level (глубина рассуждений: low … max) и mode (agent). Принимаются только значения, которые модель поддерживает.",
      )],
      ["Recommended", h(
        "a strong general model with thought_level high and mode agent; the coordinator plans and checks work, so do not save on it.",
        "сильная универсальная модель, thought_level high и mode agent: координатор планирует и проверяет работу, экономить на нём не стоит.",
      )],
    ],
  ),
  language: h(
    "Language Cortex uses for its replies, plans and reports. Automatic follows the language of your message. Recommended: Automatic, or ru to always get Russian.",
    "Язык ответов, планов и отчётов Cortex. Automatic — по языку вашего сообщения. Рекомендация: Automatic или ru, чтобы всегда получать ответы на русском.",
  ),
  parameters: helpBlock(
    h(
      "Native engine parameters used together with Default route. Each row is a lowercase name and its value; only pairs the selected route advertises are accepted.",
      "Собственные параметры движка, применяемые вместе с Default route. Каждая строка — имя в нижнем регистре и значение; принимаются только пары, которые поддерживает маршрут.",
    ),
    [
      ["Parameter N name / value", h("For example thought_level = high.", "Например thought_level = high.")],
      ["Recommended", h(
        "leave empty and set coordinator parameters instead; remove every row to delete the setting.",
        "оставьте пустым и задавайте параметры координатора; удалите все строки, чтобы убрать настройку.",
      )],
    ],
  ),
  ownerCeiling: h(
    "The widest limits any project or task may use; a project ceiling can only narrow them. Recommended: off unless you need a hard lock — everyday control is easier with Models and Engines & models.",
    "Самые широкие ограничения для любых проектов и задач; ограничение проекта может их только сузить. Рекомендация: выключено, если не нужен жёсткий запрет — для повседневного управления удобнее Models и Engines & models.",
  ),

  // --- ceiling --------------------------------------------------------------
  allowedRoutes: h(
    "When on, Cortex may dispatch only to the listed exact route ids (xr_…, shown per model under Engines & models); anything else, including pins, is refused. An empty list allows nothing. Recommended: off — switch models on and off under Models instead.",
    "Если включено, Cortex может запускать работу только на перечисленных маршрутах (xr_…, видны у моделей в Engines & models); всё остальное, включая закрепления, отклоняется. Пустой список не разрешает ничего. Рекомендация: выключено — включайте и выключайте модели в Models.",
  ),
  allowedConnections: h(
    "When on, Cortex runs models only through the listed connection ids (cn_…): automatic routing skips other connections and a dispatch through any other connection is refused. An empty list allows nothing. Recommended: off; to stop using an engine, switch it off under Engines & models.",
    "Если включено, Cortex запускает модели только через перечисленные подключения (cn_…): автоматика пропускает остальные, а запуск через любое другое подключение отклоняется. Пустой список не разрешает ничего. Рекомендация: выключено; чтобы не использовать движок, выключите его в Engines & models.",
  ),
  allowedRecipients: h(
    "Which providers (connection provider ids, for example openai or anthropic) may receive task data: other providers are skipped by automatic routing and refused at dispatch. An empty list allows none. Recommended: off, unless a project's code must not reach a particular provider.",
    "Каким провайдерам (id провайдера подключения, например openai или anthropic) можно передавать данные задачи: остальных автоматика пропускает, а запуск через них отклоняется. Пустой список не разрешает никого. Рекомендация: выключено, если код проекта не должен уходить определённому провайдеру.",
  ),
  limits: h(
    "Maximum model runs (workers, retries, fallbacks) one task may dispatch; the next one is refused. Blank means no limit. Recommended: blank, or about 8–12 to stop runaway retry loops.",
    "Сколько всего запусков моделей (исполнители, повторы, fallback) может сделать одна задача; следующий отклоняется. Пусто — без лимита. Рекомендация: пусто или примерно 8–12, чтобы остановить бесконечные повторы.",
  ),
  fallback: helpBlock(
    h(
      "Fallback lets Cortex move a failed worker to another route on its own. It stays off unless every condition below allows it.",
      "Fallback позволяет Cortex самому перенести упавшего исполнителя на другой маршрут. Он выключен, пока все условия ниже его не разрешат.",
    ),
    [
      ["Restrict fallback target routes", h(
        "Exact route ids a fallback may move to; off means any eligible route.",
        "Точные маршруты, куда можно перенести работу; выключено — любой подходящий.",
      )],
      ["Restrict fallback failure classes", h(
        "Which failures trigger it: engine_failure, capacity_unavailable. authorization_loss, cancellation, provider_restriction and refusal never do.",
        "Какие сбои его запускают: engine_failure, capacity_unavailable. authorization_loss, cancellation, provider_restriction и refusal — никогда.",
      )],
      ["Fallback budget", h(
        "Maximum fallback moves per task; blank means no separate cap.",
        "Сколько переносов допускается на задачу; пусто — без отдельного лимита.",
      )],
      ["Fallback data-recipient consent", h(
        "Allows a fallback to send task data to a different provider (a per-recipient consent record is still required).",
        "Разрешает fallback передавать данные задачи другому провайдеру (дополнительно нужна запись согласия для получателя).",
      )],
      ["Recommended", h(
        "leave fallback off. If you want it: both failure classes, budget 1, target routes limited to models you trust, consent on only if another provider may see the code.",
        "не включать. Если нужен: оба класса сбоев, бюджет 1, только доверенные маршруты, согласие — только если код можно показывать другому провайдеру.",
      )],
    ],
  ),
  projectCeiling: h(
    "Narrows the owner ceiling for the selected project only; it is stored in that project and never in a profile. The fields mean the same as in the owner ceiling. Recommended: only for projects that must stay on specific routes or providers.",
    "Сужает ограничения владельца только для выбранного проекта; хранится в проекте и не попадает в профили. Поля значат то же, что в ограничении владельца. Рекомендация: только для проектов, которые должны работать строго на определённых маршрутах или провайдерах.",
  ),

  // --- routing --------------------------------------------------------------
  routingFile: h(
    "Which routing file to edit. Cortex uses only the first file that exists — project, then $CORTEX_ROUTING, then user — and never merges them. Recommended: User routing file; create a project file only when one project needs different models.",
    "Какой файл маршрутизации править. Cortex использует только первый существующий файл — проектный, затем $CORTEX_ROUTING, затем пользовательский — и не объединяет их. Рекомендация: User routing file; проектный создавайте, только если проекту нужны другие модели.",
  ),
  modelPolicy: helpBlock(
    h(
      "Per-engine, per-model rules for automatic worker routing. Models are matched by exact id; nothing is guessed from names. A model without automatic membership and a tier is never picked automatically (it stays usable by hand).",
      "Правила по движкам и моделям для автоматического выбора исполнителей. Модели сравниваются по точному id, по названию ничего не угадывается. Модель без автоматического членства и уровня автоматически не выбирается (вручную доступна).",
    ),
    [
      ["Policy engine / Policy model + Add model policy", h(
        "Add a rule block for an exact engine id and model id.",
        "Добавляет блок правил для точного id движка и модели.",
      )],
      ["Automatic members for <engine>", h(
        "Complete list of that engine's models automatic routing may use; empty means none. Recommended: list every model you want workers to use.",
        "Полный список моделей движка для автоматического выбора; пустой — ни одной. Рекомендация: перечислите все модели, которые должны брать исполнители.",
      )],
      ["Policy tiers", h(
        "Work level the model serves: light (simple, cheap tasks; effort low), standard (normal work; medium), deep (architecture, security, hard analysis; high), flagship (only on escalation or when you ask for that model; high). Several tiers are allowed. Recommended: cheap fast models light/standard, strong models deep, the most capable flagship.",
        "Уровень работы модели: light (простые дешёвые задачи; effort low), standard (обычная работа; medium), deep (архитектура, безопасность, сложный анализ; high), flagship (только при эскалации или по вашей просьбе; high). Можно несколько. Рекомендация: быстрые дешёвые — light/standard, сильные — deep, самая мощная — flagship.",
      )],
      ["effort_floor", h(
        "Lowest effort this model is ever run at. Recommended: set it for models that work badly at low effort.",
        "Минимальный effort для модели. Рекомендация: задавайте для моделей, которые плохо работают на низком effort.",
      )],
      ["max_effort", h(
        "Highest effort allowed; without it a flagship model stops at high. Recommended: unset, or xhigh/max only for a model you accept as expensive.",
        "Максимальный effort; без него flagship останавливается на high. Рекомендация: не задавать или xhigh/max только для модели, дороговизна которой допустима.",
      )],
      ["effort_ladder", h(
        "For a light model: 1–3 ascending efforts for light work, standard work, and a retry after falling short. Recommended: for example high, xhigh for a fast model that also does standard work.",
        "Для light-модели: 1–3 возрастающих effort для лёгкой работы, обычной работы и повтора после неудачи. Рекомендация: например high, xhigh для быстрой модели, которая берёт и обычную работу.",
      )],
      ["also_standard_for", h(
        "Work kinds where a light model also counts as a standard candidate (for example testing).",
        "Виды работ, где light-модель тоже считается кандидатом уровня standard (например testing).",
      )],
      ["preferred_for", h(
        "Work kinds this model is preferred for within its tier; pick them from the list. The model must be an automatic member. With preferred_when set, these kinds only narrow that condition (none: any kind).",
        "Виды работ, где модель предпочтительна внутри своего уровня; выбираются из списка. Модель должна быть в автоматическом списке. Если задано preferred_when, эти виды лишь сужают условие (ни одного — любой вид).",
      )],
      ["preferred_when", h(
        "Optional free-text condition, for example \"frontend changes without design work\". The runtime cannot judge text: the coordinator reads it, and for each task step that meets it names this model, which then ranks first as preferred. Steps that do not meet it treat the model as an ordinary candidate. Retries and fixes ignore the condition. Up to 500 characters.",
        "Необязательное условие свободным текстом, например «для фронтенда без дизайна». Runtime текст не понимает: его читает координатор и для каждого шага, который подходит под условие, указывает эту модель — тогда она ставится первой как предпочтительная. Для остальных шагов модель остаётся обычным кандидатом. Повторы и исправления условие не учитывают. До 500 символов.",
      )],
      ["request_names", h(
        "Extra lowercase names (Russian stems allowed) that count as you naming this model in a request such as \"do it with …\". Recommended: the short name you actually type.",
        "Дополнительные имена в нижнем регистре (можно русские основы), по которым Cortex понимает, что вы назвали эту модель в просьбе вроде «сделай через …». Рекомендация: короткое имя, которым вы реально пишете.",
      )],
    ],
  ),
  quotaWindows: helpBlock(
    h(
      "Ties the plan usage windows an engine reports (5-hour, weekly…) to exact models, so an exhausted window blocks only the models it covers. Without it every window counts for every model of that engine (safe but may block too much).",
      "Связывает окна лимитов тарифа, которые сообщает движок (5 часов, неделя…), с точными моделями, чтобы исчерпанное окно блокировало только свои модели. Без этого каждое окно считается общим для всех моделей движка (безопасно, но может блокировать лишнее).",
    ),
    [
      ["<engine> quota windows", h("Turns on explicit rules for that engine.", "Включает явные правила для движка.")],
      ["window N exact label", h("The window name exactly as the engine reports it.", "Имя окна точно как его сообщает движок.")],
      ["window N scope", h(
        "Shared / account: applies to all models. Exact models: only the listed ids; an empty list excludes the window from every model.",
        "Shared / account — для всех моделей. Exact models — только перечисленные id; пустой список исключает окно для всех моделей.",
      )],
      ["Recommended", h(
        "map a window only when the provider really has a separate limit for some models; otherwise leave it shared.",
        "задавайте окно, только если у провайдера действительно отдельный лимит для части моделей; иначе оставьте общим.",
      )],
    ],
  ),
  pins: helpBlock(
    h(
      "Pins force a worker role onto one exact route. A pin is strict: if the route is unavailable or not allowed, the work is refused rather than moved.",
      "Закрепление жёстко привязывает роль исполнителя к одному маршруту. Если маршрут недоступен или запрещён, работа отклоняется, а не переносится.",
    ),
    [
      ["Pin N role", h("Worker role (for example worker-security-auditor) or * for all roles.", "Роль исполнителя (например worker-security-auditor) или * для всех.")],
      ["Pin N engine / model", h("Exact engine and model id; OpenCode models are provider/model.", "Точные движок и id модели; у OpenCode — provider/model.")],
      ["Pin N effort", h("Fixed effort, or Engine default.", "Фиксированный effort или Engine default.")],
      ["Recommended", h(
        "no pins — automatic routing balances cost, quota and quality. Pin only a role that must always use one model.",
        "без закреплений — автоматика сама балансирует цену, лимиты и качество. Закрепляйте только роль, которой всегда нужна одна модель.",
      )],
    ],
  ),
  deny: h(
    "Models or engines Cortex must never use: hidden from the model selector, refused when chosen or pinned, never routed. Entry forms: exact model id, engine/model, or engine/* for a whole engine. Deny always wins. Recommended: let the Models and Engines & models switches write these entries.",
    "Модели и движки, которые Cortex не должен использовать: скрыты из выбора, отклоняются при ручном выборе или закреплении, не назначаются автоматически. Формы записи: точный id модели, engine/model или engine/* для всего движка. Запрет всегда приоритетнее. Рекомендация: пусть эти записи создают переключатели Models и Engines & models.",
  ),
  autoAllow: h(
    "Lets a model that is excluded from automatic routing by default (for example an OpenCode route, written as opencode/<provider>/<model>) be picked automatically on an engine without an Automatic members list. Does not override deny or the engine list. Recommended: empty — use Automatic members instead.",
    "Разрешает автоматически выбирать модель, по умолчанию исключённую из автоматики (например маршрут OpenCode в виде opencode/<provider>/<model>), на движке без списка Automatic members. Не отменяет запреты и список движков. Рекомендация: пусто — используйте Automatic members.",
  ),
  tiers: helpBlock(
    h(
      "Legacy one-tier-per-model placement for every engine. The per-engine Policy tiers above take precedence.",
      "Устаревшее размещение «одна модель — один уровень» для всех движков. Policy tiers выше имеют приоритет.",
    ),
    [
      ["Tier N model id / tier", h("Exact model id and its tier.", "Точный id модели и её уровень.")],
      ["Recommended", h("leave off and use Policy tiers.", "не включать, используйте Policy tiers.")],
    ],
  ),
  providers: h(
    "Engines automatic worker routing, failover and pins may use. Every engine stays selectable by hand; the order does not rank them. Recommended: off (all engines), or the engines whose plans you want workers to spend.",
    "Движки, которые могут использовать автоматический выбор исполнителей, fallback и закрепления. Вручную доступны все; порядок не задаёт приоритет. Рекомендация: выключено (все движки) или движки, тарифы которых должны тратить исполнители.",
  ),
  affinity: helpBlock(
    h(
      "Tie-breaker: which engine to lean to for a kind of work when candidates are otherwise equal. Never overrides pins, deny, the engine list, availability or quota.",
      "Решение при равенстве: к какому движку склоняться для вида работы, когда кандидаты в остальном равны. Не отменяет закрепления, запреты, список движков, доступность и лимиты.",
    ),
    [
      ["Affinity N work kind", h("implementation_backend, implementation_frontend, implementation_frontend_design, testing, review, design… or * for all.", "implementation_backend, implementation_frontend, implementation_frontend_design, testing, review, design… или * для всех.")],
      ["Affinity N engine", h("Preferred engine, or None for no lean.", "Предпочтительный движок или None — без предпочтения.")],
      ["Recommended", h(
        "off — built-in leaning (Codex for backend implementation, frontend implementation without design, and operations; Claude for frontend implementation with UI design, review, design, writing, planning, analysis) works well.",
        "выключено — встроенные предпочтения (Codex — backend-реализация, frontend-реализация без дизайна и операции; Claude — frontend-реализация с UI-дизайном, ревью, дизайн, тексты, планирование, анализ) работают хорошо.",
      )],
    ],
  ),

  // --- engines & models -----------------------------------------------------
  switchesLegend: helpBlock(
    h(
      "Only models switched on under Models are listed here. Connections and logins are never touched.",
      "Здесь показаны только модели, включённые в Models. Подключения и логины не затрагиваются.",
    ),
    [
      ["Use <engine> in Cortex", h(
        "Off hides every model of the engine from the selector, refuses it when chosen or pinned and removes it from automatic routing (saved as engine/*). Recommended: on for every engine you pay for.",
        "Выключено — все модели движка скрыты из выбора, отклоняются при ручном выборе и закреплении и исключены из автоматики (сохраняется как engine/*). Рекомендация: включено для каждого оплачиваемого движка.",
      )],
      ["Automatic routing may use <engine>", h(
        "Off keeps automatic worker routing, failover and pins away from the engine; its models stay selectable by hand. Recommended: on, so workers can use both plans.",
        "Выключено — автоматика, fallback и закрепления не используют движок; вручную его модели доступны. Рекомендация: включено, чтобы исполнители расходовали оба тарифа.",
      )],
      ["Use <model> for automatic ranking", h(
        "Adds the model to the engine's Automatic members list. Recommended: on for models you want workers to use.",
        "Добавляет модель в список Automatic members движка. Рекомендация: включено для моделей, которые должны брать исполнители.",
      )],
      ["Automatic tiers for <model>", h(
        "Which work levels the model serves (light, standard, deep, flagship); it runs at that tier's effort. No tier means it is never picked automatically.",
        "Какие уровни работ берёт модель (light, standard, deep, flagship); запускается с effort этого уровня. Без уровня автоматически не выбирается.",
      )],
      ["Preferred work for <model>", h(
        "Work kinds where this model is ranked first within its tier; availability, policy and plan usage still apply. Recommended: mark only clear strengths.",
        "Виды работ, где модель ставится первой внутри своего уровня; доступность, правила и расход тарифа всё равно учитываются. Рекомендация: отмечайте только явные сильные стороны.",
      )],
      ["Allow <model> in automatic routing", h(
        "Exact exception for a model left out of automatic routing by default; membership and a tier are still required.",
        "Точное исключение для модели, по умолчанию не участвующей в автоматике; членство и уровень всё равно нужны.",
      )],
      ["Now", h(
        "State under the rules Cortex uses at the moment. Quota and recorded unavailability are decided per dispatch and not shown.",
        "Состояние по правилам, которые Cortex использует сейчас. Лимиты и недоступность решаются при каждом запуске и здесь не показаны.",
      )],
    ],
  ),
  saveSwitchesTo: h(
    "Which routing file the switches are saved to. A project file, when it exists, replaces the user file for that project.",
    "В какой файл маршрутизации сохраняются переключатели. Проектный файл, если он есть, заменяет пользовательский для этого проекта.",
  ),

  // --- models ---------------------------------------------------------------
  modelsLegend: h(
    "Switch on the models Cortex may use. Only these appear in the model selector, under Engines & models and in automatic routing; a model switched off is refused when chosen or pinned. New models an engine adds stay off until you switch them on. Recommended: switch on only models you actually want to use and pay for.",
    "Включите модели, которые может использовать Cortex. Только они видны в выборе модели, в Engines & models и в автоматике; выключенная модель отклоняется при ручном выборе или закреплении. Новые модели движка остаются выключенными, пока вы их не включите. Рекомендация: включайте только модели, которые реально хотите использовать и оплачивать.",
  ),
  modelLists: h(
    "Cortex learns each engine's models from the engine itself. After an engine update, press Refresh model lists to see new models: engines are started without a prompt, so no paid call is made.",
    "Cortex узнаёт модели каждого движка у самого движка. После обновления движка нажмите Refresh model lists, чтобы увидеть новые модели: движки запускаются без запроса, платных вызовов нет.",
  ),
  filterModels: h(
    "Shows only models whose name or id contains these words; Switch on/off all applies to the models shown.",
    "Показывает только модели, в имени или id которых есть эти слова; кнопки включения/выключения всех действуют на показанные модели.",
  ),

  // --- MCP ------------------------------------------------------------------
  mcpIntro: h(
    "External MCP tool servers Cortex gives to its agents. Profiles store only the usage policy (enabled, roles, engines, tool and path grants); command, arguments and environment are shared by all profiles.",
    "Внешние MCP-серверы инструментов, которые Cortex даёт своим агентам. Профили хранят только правила использования (включение, роли, движки, доступ к инструментам и путям); команда, аргументы и переменные окружения общие для всех профилей.",
  ),
  mcpServer: helpBlock(null, [
    ["Server name", h("Unique name of the server in Cortex.", "Уникальное имя сервера в Cortex.")],
    ["Command / Arguments", h("Executable that starts the server and its arguments.", "Исполняемый файл, запускающий сервер, и его аргументы.")],
    ["Environment variables", h(
      "Variables passed to the server. Stored values are never shown. Recommended: no real secrets here; use the server's own secret source.",
      "Переменные для сервера. Сохранённые значения не показываются. Рекомендация: не храните здесь реальные секреты, используйте собственный источник секретов сервера.",
    )],
    ["Enabled", h("Default is on; Off keeps the entry but does not start it.", "По умолчанию включён; Off — запись остаётся, но сервер не запускается.")],
    ["Network access", h("Default is no network. Recommended: On only if the server must reach the internet.", "По умолчанию без сети. Рекомендация: On, только если серверу нужен интернет.")],
    ["Limit to roles", h("coordinator and/or worker; unset means both.", "coordinator и/или worker; не задано — оба.")],
    ["Limit to engines", h("Engines whose sessions receive the server; unset means all.", "Движки, в сессиях которых доступен сервер; не задано — все.")],
    ["Allowed tools", h("Exact tool names Cortex may call; nothing else is granted. Recommended: list only what you need.", "Точные имена инструментов, которые можно вызывать; остальные запрещены. Рекомендация: только нужные.")],
    ["Read-only tools", h("Subset of allowed tools that only read; read-only roles may call only these.", "Подмножество разрешённых инструментов, которые только читают; read-only роли вызывают только их.")],
    ["Coordinator tools", h("Subset of read-only tools the coordinator receives.", "Подмножество read-only инструментов, доступных координатору.")],
    ["Always-loaded tools", h(
      "Claude keeps these tool schemas in the prompt instead of searching for them; each costs context in every session. Recommended: only tools used in almost every task.",
      "Claude держит схемы этих инструментов в промпте, а не ищет их; каждая расходует контекст в каждой сессии. Рекомендация: только инструменты, нужные почти в каждой задаче.",
    )],
    ["Read paths / Write paths", h("Absolute paths the server may read or write.", "Абсолютные пути, которые сервер может читать или писать.")],
    ["Runtime-directory variables", h(
      "Variable names through which Cortex gives the server a private working directory.",
      "Имена переменных, через которые Cortex выдаёт серверу личный рабочий каталог.",
    )],
  ]),

  // --- connections ----------------------------------------------------------
  connections: helpBlock(
    h(
      "Registered engine accounts Cortex runs models through. Session limits belong to profiles; names, credentials, validation and removal do not.",
      "Зарегистрированные аккаунты движков, через которые Cortex запускает модели. Лимиты сессий входят в профили; имена, учётные данные, проверка и удаление — нет.",
    ),
    [
      ["Name", h("Display name; does not change routing.", "Отображаемое имя; на маршрутизацию не влияет.")],
      ["Max concurrent sessions", h(
        "How many model sessions may run on this account at once; extra work waits. Recommended: 3–4 for a subscription account; lower it if you hit rate limits.",
        "Сколько сессий моделей одновременно может работать на аккаунте; остальная работа ждёт. Рекомендация: 3–4 для подписки; уменьшайте при упоре в лимиты частоты.",
      )],
      ["Validate", h("Checks the account and login without calling a model.", "Проверяет аккаунт и логин без вызова модели.")],
      ["Remove", h("Unregisters the connection; its models disappear from Cortex.", "Удаляет подключение; его модели пропадают из Cortex.")],
    ],
  ),
  openCode: h(
    "Optional engine, opt-in only. Registering never changes routing: OpenCode routes stay out of automatic routing until you switch models on under Models and add them to automatic routing. Recommended: enable only if you need models available only through OpenCode.",
    "Необязательный движок, только по явному согласию. Регистрация не меняет маршрутизацию: маршруты OpenCode не участвуют в автоматике, пока вы не включите модели в Models и не добавите их в автоматический выбор. Рекомендация: включайте, только если нужны модели, доступные лишь через OpenCode.",
  ),
} as const satisfies Record<string, HelpText>;

/** What each work kind covers (keys are the runtime's work kinds, plus the legacy
 * `implementation` spelling a stored version-2 file may still use). */
export const WORK_KIND_HELP: Record<string, HelpText> = {
  implementation: h(
    "Legacy kind kept from an older routing file: it covers implementation_backend, implementation_frontend and implementation_frontend_design. Saving keeps it unchanged; turn it off and turn on the current kinds to choose them separately.",
    "Устаревший вид из старого файла маршрутизации: охватывает implementation_backend, implementation_frontend и implementation_frontend_design. При сохранении он остаётся как есть; выключите его и включите текущие виды, чтобы выбирать их по отдельности.",
  ),
  implementation_backend: h(
    "Backend implementation: APIs, business logic, persistence, data pipelines, server-side refactoring. Also every role the runtime does not list elsewhere.",
    "Бэкенд-реализация: API, бизнес-логика, хранение данных, обработка данных, серверный рефакторинг. Сюда же попадают роли, не отнесённые к другим видам.",
  ),
  implementation_frontend: h(
    "Frontend implementation that does not need visual design decisions: UI components, client state, styling to an existing design, accessibility fixes.",
    "Frontend-реализация без визуальных дизайн-решений: компоненты интерфейса, состояние клиента, стили по уже готовому дизайну, исправления доступности.",
  ),
  implementation_frontend_design: h(
    "Frontend implementation that also makes visual design choices: new screens, layout and visual design-system work without a prior design to follow.",
    "Frontend-реализация с собственными визуальными решениями: новые экраны, верстка и работа с визуальной дизайн-системой без готового дизайна.",
  ),
  operations: h(
    "Infrastructure work: CI/CD, containers, deployment, operational diagnostics.",
    "Инфраструктура: CI/CD, контейнеры, развёртывание, эксплуатационная диагностика.",
  ),
  debugging: h(
    "Finding and fixing the cause of failures and slowness: reproduce, prove the root cause, fix, verify.",
    "Поиск и устранение причин сбоев и медленной работы: воспроизвести, доказать причину, исправить, проверить.",
  ),
  testing: h(
    "Writing and running tests, checking builds and acceptance criteria.",
    "Написание и запуск тестов, проверка сборки и критериев приёмки.",
  ),
  investigation: h(
    "Read-only exploration of the codebase: where things are, how they are called, what depends on what.",
    "Исследование кода без изменений: где что находится, как вызывается, что от чего зависит.",
  ),
  review: h(
    "Independent checking of finished work, including the mandatory review of every task, security and accessibility audits.",
    "Независимая проверка готовой работы, включая обязательное ревью каждой задачи, аудит безопасности и доступности.",
  ),
  design: h(
    "Architecture and design decisions: system structure, database schemas, UX, senior consultation.",
    "Архитектурные и проектные решения: структура системы, схемы баз данных, UX, консультации старшего уровня.",
  ),
  writing: h(
    "Documentation: project knowledge, feature docs, decision records.",
    "Документация: знания о проекте, описания функций, записи решений.",
  ),
  planning: h(
    "Breaking a task into steps with dependencies, risks and checks.",
    "Разбиение задачи на шаги с зависимостями, рисками и проверками.",
  ),
  analysis: h(
    "Bounded research or analysis when no specialist fits.",
    "Ограниченное исследование или анализ, когда не подходит ни один специалист.",
  ),
};

/** What each worker role does (keys are the runtime's role ids). */
export const ROLE_HELP: Record<string, HelpText> = {
  "worker-accessibility-auditor": h("audits WCAG conformance, keyboard navigation and screen-reader risks", "проверяет соответствие WCAG, работу с клавиатуры и экранными дикторами"),
  "worker-accessibility-fixer": h("fixes accessibility: semantics, keyboard, focus, contrast", "исправляет доступность: семантика, клавиатура, фокус, контраст"),
  "worker-architect": h("designs component boundaries, data flow and API contracts", "проектирует границы компонентов, потоки данных и контракты API"),
  "worker-backend-dev": h("implements APIs, business logic, persistence and server tests", "реализует API, бизнес-логику, хранение данных и серверные тесты"),
  "worker-build-verification": h("runs builds and tests and proves their exact results", "запускает сборку и тесты и подтверждает их точные результаты"),
  "worker-code-reviewer": h("reviews changes for bugs, regressions, security and missing tests; does the mandatory task review", "проверяет изменения на ошибки, регрессии, безопасность и нехватку тестов; выполняет обязательное ревью задач"),
  "worker-data-engineer": h("builds ETL, data migrations and integrity checks", "делает ETL, миграции данных и проверки их целостности"),
  "worker-database-architect": h("designs schemas, indexes, migrations and query plans", "проектирует схемы, индексы, миграции и планы запросов"),
  "worker-debugger": h("reproduces failures, proves the root cause, fixes and verifies", "воспроизводит сбои, доказывает причину, исправляет и проверяет"),
  "worker-devops-engineer": h("works on CI/CD, containers and deployment safety", "работает с CI/CD, контейнерами и безопасностью развёртывания"),
  "worker-explorer": h("maps execution paths, dependencies and relevant files without changing them", "изучает пути выполнения, зависимости и нужные файлы, ничего не меняя"),
  "worker-frontend-dev": h("implements UI components, state, styling and UI tests", "реализует компоненты интерфейса, состояние, стили и UI-тесты"),
  "worker-fullstack-dev": h("makes coordinated backend and frontend changes", "делает согласованные изменения бэкенда и фронтенда"),
  "worker-general": h("general agent for research, analysis or small implementation", "универсальный агент для исследования, анализа или небольших правок"),
  "worker-mobile-dev": h("implements iOS, Android, React Native or Flutter changes", "реализует изменения для iOS, Android, React Native или Flutter"),
  "worker-performance-engineer": h("finds bottlenecks and makes safe optimizations", "находит узкие места и делает безопасные оптимизации"),
  "worker-planner": h("breaks work into steps, dependencies, risks and checks", "разбивает работу на шаги, зависимости, риски и проверки"),
  "worker-qa-engineer": h("writes automated tests and reproduction scenarios", "пишет автотесты и сценарии воспроизведения"),
  "worker-refactorer": h("restructures code without changing behavior", "перестраивает код без изменения поведения"),
  "worker-security-auditor": h("audits auth, secrets, cryptography, dependencies and data protection", "проверяет авторизацию, секреты, криптографию, зависимости и защиту данных"),
  "worker-senior-consultant": h("gives senior decisions on the coordinator's request from published reports", "даёт решения старшего уровня по запросу координатора на основе опубликованных отчётов"),
  "worker-technical-writer": h("writes project and feature documentation", "пишет документацию проекта и функций"),
  "worker-ux-designer": h("designs layouts, interaction flows and responsive behavior", "проектирует макеты, сценарии взаимодействия и адаптивность"),
};

/** Help under one "Preferred work" switch: what the kind is and which agents it covers. */
export function workKindHelp(kind: string, roles: readonly string[]): HelpText {
  const base = WORK_KIND_HELP[kind] ?? h(`Work of kind ${kind}.`, `Работа вида ${kind}.`);
  const agents = (lang: "en" | "ru") => roles.length === 0
    ? (lang === "en" ? "No worker role uses this kind directly." : "Ни одна роль исполнителя не использует этот вид напрямую.")
    : `${lang === "en" ? "Agents" : "Агенты"}:\n${roles.map((role) => `  • ${role}${ROLE_HELP[role] ? ` — ${ROLE_HELP[role][lang]}` : ""}`).join("\n")}`;
  return { en: `${base.en}\n${agents("en")}`, ru: `${base.ru}\n${agents("ru")}` };
}

export const PREFERRED_WHEN_HELP = h(
  "Optional condition in your own words, for example \"frontend changes without design work\". The coordinator judges each task step against it and names this model for the steps that meet it; Cortex then ranks the model first for them. The switches above, if any, limit the condition to those kinds of work. Steps that do not meet it treat the model as an ordinary candidate. Leave empty for a plain preference by kind of work.",
  "Необязательное условие своими словами, например «для фронтенда без дизайна». Координатор сверяет с ним каждый шаг задачи и для подходящих шагов указывает эту модель; Cortex ставит её для них первой. Включённые выше виды работ, если есть, ограничивают условие этими видами. Для неподходящих шагов модель остаётся обычным кандидатом. Оставьте пустым для обычного предпочтения по видам работ.",
);

export const PREFERRED_WORK_HELP = h(
  "Turn a work kind on to rank this model first, within its tier, when Cortex picks a worker for that kind. It affects only automatic worker selection (not the coordinator or models you pick by hand), never overrides availability, policy or plan usage, and needs the model selected for automatic ranking. Recommended: mark only the kinds where this model is clearly strongest; leave the rest off.",
  "Включите вид работы, чтобы эта модель ставилась первой внутри своего уровня, когда Cortex выбирает исполнителя для такой работы. Влияет только на автоматический выбор исполнителей (не на координатора и не на ручной выбор), не отменяет доступность, правила и расход тарифа и требует включённого Use … for automatic ranking. Рекомендация: отмечайте только виды, где модель явно сильнее; остальные оставьте выключенными.",
);
