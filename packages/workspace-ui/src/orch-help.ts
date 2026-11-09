/**
 * Help for the orchestration settings, ported from the Paseo settings screen with its stale items corrected: what each field does and a
 * recommendation. Shown under a single field, or as one block for a group of
 * similar fields (rows, per-model controls). Static text only: no model id is
 * named here, every model comes from the owner's settings and engine catalogs.
 */

export type HelpText = string;

/** A block that explains several fields: one "• Field — text" line each. */
export function helpBlock(intro: HelpText | null, items: ReadonlyArray<[string, HelpText]>): HelpText {
  return [...(intro ? [intro] : []), ...items.map(([label, text]) => `• ${label} — ${text}`)].join("\n");
}

export const HELP = {
  // --- screen ---------------------------------------------------------------
  project: "Which project the Project ceiling and Project routing file act on. Owner only edits just your user-wide settings. Recommended: Owner only, unless one project needs its own rules.",
  section: "Which group of settings to show. Profiles stay visible above every section.",

  // --- profiles -------------------------------------------------------------
  profilesIntro: "A profile is a saved snapshot of user defaults, user routing (including engine and model switches), MCP usage policy and connection session limits. Save a section first; then save the applied settings into a profile. Project settings, credentials and MCP launch commands are never stored. Activating a profile rewrites those settings and discards unsaved form edits.",
  currentProfile: helpBlock(
    "Choose which saved profile is active.",
    [
      ["Current profile", "Switches all bound settings to that profile. No profile keeps the current settings without a link. Recommended: keep one profile active (for example default) and save changes into it, so you can always return to a known state."],
      ["Save to current profile", "Writes the currently applied settings into the active profile."],
      ["Revert to saved", "Throws away applied changes and restores the active profile's snapshot."],
      ["Reload profile state", "Re-reads profile state from disk, for example after editing files by hand."],
    ],
  ),
  newProfile: helpBlock(null, [
    ["New profile name", "Name of the new snapshot: lowercase letters, digits and dashes. Recommended: a short purpose name such as default or cheap-review."],
    ["New profile description", "Free note shown in the profile list; does not affect behavior."],
    ["Save as new profile", "Captures every applied setting into the new profile and makes it active."],
  ]),
  manageProfiles: helpBlock(null, [
    ["Saved profile", "Which saved profile to inspect or change."],
    ["Rename profile to / Profile description", "Change only the name and note; the stored settings stay the same."],
    ["Activate profile", "Same as choosing it under Current profile."],
    ["Delete profile", "Removes only the snapshot; current settings stay as they are."],
  ]),

  // --- user defaults --------------------------------------------------------
  defaultRoute: helpBlock(
    "The exact route preselected for new tasks (the coordinator), tied to one registered connection.",
    [
      ["Model", "The model this route runs."],
      ["Connection", "Registered connection that serves the route."],
      ["Recommended", "leave this off and use the portable coordinator instead: exact routes change when connections are registered again."],
    ],
  ),
  coordinator: helpBlock(
    "Which engine and model run the coordinator (the agent you talk to) in new sessions. Matched by exact engine/model against the current model list, so it survives re-registering a connection. Clear Default route before setting this.",
    [
      ["Coordinator engine", "Engine id: codex-acp, claude-agent-acp or opencode."],
      ["Coordinator model (exact id)", "Exact model id switched on under Models."],
      ["Coordinator parameter (name / value)", "Native options sent to the engine: thought_level (reasoning effort: low … max) and mode (agent). Only values the model advertises are accepted."],
      ["Recommended", "a strong general model with thought_level high and mode agent; the coordinator plans and checks work, so do not save on it."],
    ],
  ),
  language: "Language of replies, plans and reports. Automatic follows the language of your message. Recommended: Automatic, or Russian to always get Russian.",
  parameters: helpBlock(
    "Native engine parameters used together with Default route. Each row is a lowercase name and its value; only pairs the selected route advertises are accepted.",
    [
      ["Parameter N name / value", "For example thought_level = high."],
      ["Recommended", "leave empty and set coordinator parameters instead; remove every row to delete the setting."],
    ],
  ),
  ownerCeiling: "The widest limits any project or task may use; a project ceiling can only narrow them. Recommended: off unless you need a hard lock — everyday control is easier with Models and Engines & models.",

  // --- ceiling --------------------------------------------------------------
  allowedRoutes: "When on, work may run only on the listed models; anything else, including pins, is refused. An empty list allows nothing. Recommended: off — switch models on and off under Models instead.",
  allowedConnections: "When on, models run only through the listed connections: automatic routing skips other connections and a dispatch through any other connection is refused. An empty list allows nothing. Recommended: off; to stop using an engine, switch it off under Engines & models.",
  allowedRecipients: "Which providers (connection provider ids, for example openai or anthropic) may receive task data: other providers are skipped by automatic routing and refused at dispatch. An empty list allows none. Recommended: off, unless a project's code must not reach a particular provider.",
  limits: "Maximum model runs (workers, retries, fallbacks) one task may dispatch; the next one is refused. Blank means no limit. Recommended: blank, or about 8–12 to stop runaway retry loops.",
  fallback: helpBlock(
    "Fallback moves a failed worker to another route on its own. It stays off unless every condition below allows it.",
    [
      ["Restrict fallback target routes", "Routes a fallback may move to; off means any eligible route."],
      ["Restrict fallback failure classes", "Which failures trigger it: engine_failure, capacity_unavailable. authorization_loss, cancellation, provider_restriction and refusal never do."],
      ["Fallback budget", "Maximum fallback moves per task; blank means no separate cap."],
      ["Fallback data-recipient consent", "Allows a fallback to send task data to a different provider (a per-recipient consent record is still required)."],
      ["Recommended", "leave fallback off. If you want it: both failure classes, budget 1, target routes limited to models you trust, consent on only if another provider may see the code."],
    ],
  ),
  projectCeiling: "Narrows the owner ceiling for the selected project only; it is stored in that project and never in a profile. The fields mean the same as in the owner ceiling. Recommended: only for projects that must stay on specific routes or providers.",

  // --- routing --------------------------------------------------------------
  routingFile: "Which routing file to edit. Only the first file that exists is used — project, then $CORTEX_ROUTING, then user — and they never merge. Recommended: User routing file; create a project file only when one project needs different models.",
  modelPolicy: helpBlock(
    "Per-engine, per-model rules for automatic worker routing. Effort choices use that exact model's observed levels; no selector means engine default only. Unknown levels use a visibly marked global fallback. Stored unsupported values stay unchanged until you remove or replace them. Models are matched by exact id; nothing is guessed from names. A model without automatic membership and a tier is never picked automatically (it stays usable by hand).",
    [
      ["Policy engine / Policy model + Add model policy", "Add a rule block for an exact engine id and model id."],
      ["Automatic members for <engine>", "Complete list of that engine's models automatic routing may use; empty means none. Recommended: list every model you want workers to use."],
      ["Policy tiers", "Work level the model serves: light (simple, cheap tasks; effort low), standard (normal work; medium), deep (architecture, security, hard analysis; high), flagship (only on escalation or when you ask for that model; high). Several tiers are allowed. Recommended: cheap fast models light/standard, strong models deep, the most capable flagship."],
      ["effort_floor", "Lowest effort this model is ever run at. Recommended: set it for models that work badly at low effort."],
      ["max_effort", "Highest effort allowed; without it a flagship model stops at high. Recommended: unset, or xhigh/max only for a model you accept as expensive."],
      ["effort_ladder", "For a light model: 1–3 ascending efforts for light work, standard work, and a retry after falling short. Choose only this model's observed levels with Add, then reorder or remove entries. Clearing the list writes Neutral (unset)."],
      ["also_standard_for", "Work kinds where a light model also counts as a standard candidate (for example testing)."],
      ["preferred_for", "Work kinds this model is preferred for within its tier; pick them from the list. The model must be an automatic member. With preferred_when set, these kinds only narrow that condition (none: any kind)."],
      ["preferred_when", "Optional free-text condition, for example \"frontend changes without design work\". The runtime cannot judge text: the coordinator reads it, and for each task step that meets it names this model, which then ranks first as preferred. Steps that do not meet it treat the model as an ordinary candidate. Retries and fixes ignore the condition. Up to 500 characters."],
      ["request_names", "Extra lowercase names (Russian stems allowed) that count as you naming this model in a request such as \"do it with …\". Recommended: the short name you actually type."],
    ],
  ),
  quotaWindows: helpBlock(
    "Ties the plan usage windows an engine reports (weekly, per-model weekly…) to exact models, so an exhausted window blocks only the models it covers. Without it every applicable window counts for every model of that engine (safe but may block too much).",
    [
      ["<engine> quota windows", "Turns on explicit rules for that engine."],
      ["window N exact label", "The window name exactly as the engine reports it."],
      ["window N scope", "Shared / account: applies to all models. Exact models: only the listed ids; an empty list excludes the window from every model."],
      ["Recommended", "map a window only when the provider really has a separate limit for some models; otherwise leave it shared."],
    ],
  ),
  pins: helpBlock(
    "Pins force a worker role onto one exact route. A pin is strict: if the route is unavailable or not allowed, the work is refused rather than moved.",
    [
      ["Pin N role", "Worker role (for example worker-security-auditor) or * for all roles."],
      ["Pin N engine / model", "Exact engine and model id; OpenCode models are provider/model."],
      ["Pin N effort", "Fixed effort from this exact model’s observed levels, or Engine default. Unknown levels show a marked global fallback; saved unsupported values are preserved."],
      ["Recommended", "no pins — automatic routing balances cost, quota and quality. Pin only a role that must always use one model."],
    ],
  ),
  deny: "Models or engines never to use: hidden from the model selector, refused when chosen or pinned, never routed. Entry forms: exact model id, engine/model, or engine/* for a whole engine. Deny always wins. Recommended: let the Models and Engines & models switches write these entries.",
  autoAllow: "Stored, has no effect under current routing. Use Auto in Models.",
  tiers: "Legacy top-level tiers are refused by model policy version 2. Migrate them; tiers live per engine in Models.",
  providers: "Engines automatic worker routing, failover and pins may use. Every engine stays selectable by hand; the order does not rank them. Recommended: off (all engines), or the engines whose plans you want workers to spend.",
  affinity: helpBlock(
    "Tie-breaker: which engine to lean to for a kind of work when candidates are otherwise equal. Never overrides pins, deny, the engine list, availability or quota.",
    [
      ["Affinity N work kind", "implementation_backend, implementation_frontend, implementation_frontend_design, testing, review, design… or * for all."],
      ["Affinity N engine", "Preferred engine, or None for no lean."],
      ["Recommended", "off — built-in leaning (Codex for backend implementation, frontend implementation without design, and operations; Claude for frontend implementation with UI design, review, design, writing, planning, analysis) works well."],
    ],
  ),

  // --- engines & models -----------------------------------------------------
  switchesLegend: helpBlock(
    "Only models switched on under Models are listed here. Connections and logins are never touched.",
    [
      ["Use", "Off hides every model of the engine from the selector, refuses it when chosen or pinned and removes it from automatic routing (saved as engine/*). Recommended: on for every engine you pay for."],
      ["Automatic routing may use <engine>", "Off keeps automatic worker routing, failover and pins away from the engine; its models stay selectable by hand. Recommended: on, so workers can use both plans."],
      ["Use <model> for automatic ranking", "Adds the model to the engine's Automatic members list. Recommended: on for models you want workers to use."],
      ["Automatic tiers for <model>", "Which work levels the model serves (light, standard, deep, flagship); it runs at that tier's effort. No tier means it is never picked automatically."],
      ["Preferred work for <model>", "Work kinds where this model is ranked first within its tier; availability, policy and plan usage still apply. Recommended: mark only clear strengths."],
      ["Allow <model> in automatic routing", "Exact exception for a model left out of automatic routing by default; membership and a tier are still required."],
      ["Now", "State under the rules in use at the moment. Quota and recorded unavailability are decided per dispatch and not shown."],
    ],
  ),
  saveSwitchesTo: "Which routing file the switches are saved to. A project file, when it exists, replaces the user file for that project.",

  // --- models ---------------------------------------------------------------
  modelsLegend: "Switch on the models tasks may use. Only these appear in the model selector, under Engines & models and in automatic routing; a model switched off is refused when chosen or pinned. New models an engine adds stay off until you switch them on. Recommended: switch on only models you actually want to use and pay for.",
  modelLists: "Each engine reports its own models. After an engine update, press Refresh model lists to see new models: engines are started without a prompt, so no paid call is made.",
  filterModels: "Shows only models whose name or id contains these words; Switch on/off all applies to the models shown.",

  // --- MCP ------------------------------------------------------------------
  mcpIntro: "External MCP tool servers given to agents. Profiles store only the usage policy (enabled, roles, engines, tool and path grants); command, arguments and environment are shared by all profiles.",
  mcpServer: helpBlock(null, [
    ["Server name", "Unique name of the server."],
    ["Command / Arguments", "Executable that starts the server and its arguments."],
    ["Environment variables", "Variables passed to the server. Stored values are never shown. Recommended: no real secrets here; use the server's own secret source."],
    ["Enabled", "Default is on; Off keeps the entry but does not start it."],
    ["Network access", "Default is no network. Recommended: On only if the server must reach the internet."],
    ["Limit to roles", "coordinator and/or worker; unset means both."],
    ["Limit to engines", "Engines whose sessions receive the server; unset means all."],
    ["Allowed tools", "Exact tool names that may be called; nothing else is granted. Recommended: list only what you need."],
    ["Read-only tools", "Subset of allowed tools that only read; read-only roles may call only these."],
    ["Coordinator tools", "Subset of read-only tools the coordinator receives."],
    ["Always-loaded tools", "Claude keeps these tool schemas in the prompt instead of searching for them; each costs context in every session. Recommended: only tools used in almost every task."],
    ["Read paths / Write paths", "Absolute paths the server may read or write."],
    ["Runtime-directory variables", "Variable names that give the server a private working directory."],
  ]),

  // --- connections ----------------------------------------------------------
  connections: helpBlock(
    "Registered engine accounts that run models. Session limits belong to profiles; names, credentials, validation and removal do not.",
    [
      ["Name", "Display name; does not change routing."],
      ["Max concurrent sessions", "How many model sessions may run on this account at once; extra work waits. Recommended: 3–4 for a subscription account; lower it if you hit rate limits."],
      ["Validate", "Checks the account and login without calling a model."],
      ["Remove", "Unregisters the connection; its models disappear."],
    ],
  ),
  engineSetup: "The desktop package includes its pinned Codex and Claude adapters. It connects an existing native account login and loads model lists without a model prompt; checkout launches can download missing adapters in the background. Settings shows progress. Add connection supports API keys and a separate native-login terminal followed by Recheck. Removing a connection disables automatic registration for that engine; Add connection restores it explicitly. OpenCode is never downloaded or connected automatically.",
  defaultPreset: "The bundled default preset supplies routing.json, defaults.json and disabled MCP examples in mcp-servers.json. A fresh install selects one applied profile named default. Apply creates only absent files, including routing-profiles.json when working settings match the preset; it never replaces existing settings or profiles. Install MCP tools and replace /path/to/... examples before enabling them. Reset replaces only routing.json and defaults.json after private backups.",
  openCode: "Optional engine, opt-in only. Registering never changes routing: OpenCode routes stay out of automatic routing until you switch models on under Models and add them to automatic routing. Recommended: enable only if you need models available only through OpenCode.",
} as const satisfies Record<string, HelpText>;

/** What each work kind covers (keys are the runtime's work kinds, plus the legacy
 * `implementation` spelling a stored version-2 file may still use). */
export const WORK_KIND_HELP: Record<string, HelpText> = {
  implementation: "Legacy kind kept from an older routing file: it covers implementation_backend, implementation_frontend and implementation_frontend_design. Saving keeps it unchanged; turn it off and turn on the current kinds to choose them separately.",
  implementation_backend: "Backend implementation: APIs, business logic, persistence, data pipelines, server-side refactoring. Also every role the runtime does not list elsewhere.",
  implementation_frontend: "Frontend implementation that does not need visual design decisions: UI components, client state, styling to an existing design, accessibility fixes.",
  implementation_frontend_design: "Frontend implementation that also makes visual design choices: new screens, layout and visual design-system work without a prior design to follow.",
  operations: "Infrastructure work: CI/CD, containers, deployment, operational diagnostics.",
  debugging: "Finding and fixing the cause of failures and slowness: reproduce, prove the root cause, fix, verify.",
  testing: "Writing and running tests, checking builds and acceptance criteria.",
  investigation: "Read-only exploration of the codebase: where things are, how they are called, what depends on what.",
  review: "Independent checking of finished work, including the mandatory review of every task, security and accessibility audits.",
  design: "Architecture and design decisions: system structure, database schemas, UX, senior consultation.",
  writing: "Documentation: project knowledge, feature docs, decision records.",
  planning: "Breaking a task into steps with dependencies, risks and checks.",
  analysis: "Bounded research or analysis when no specialist fits.",
};

/** What each worker role does (keys are the runtime's role ids). */
export const ROLE_HELP: Record<string, HelpText> = {
  "worker-accessibility-auditor": "audits WCAG conformance, keyboard navigation and screen-reader risks",
  "worker-accessibility-fixer": "fixes accessibility: semantics, keyboard, focus, contrast",
  "worker-architect": "designs component boundaries, data flow and API contracts",
  "worker-backend-dev": "implements APIs, business logic, persistence and server tests",
  "worker-build-verification": "runs builds and tests and proves their exact results",
  "worker-code-reviewer": "reviews changes for bugs, regressions, security and missing tests; does the mandatory task review",
  "worker-data-engineer": "builds ETL, data migrations and integrity checks",
  "worker-database-architect": "designs schemas, indexes, migrations and query plans",
  "worker-debugger": "reproduces failures, proves the root cause, fixes and verifies",
  "worker-devops-engineer": "works on CI/CD, containers and deployment safety",
  "worker-explorer": "maps execution paths, dependencies and relevant files without changing them",
  "worker-frontend-dev": "implements UI components, state, styling and UI tests",
  "worker-fullstack-dev": "makes coordinated backend and frontend changes",
  "worker-general": "general agent for research, analysis or small implementation",
  "worker-mobile-dev": "implements iOS, Android, React Native or Flutter changes",
  "worker-performance-engineer": "finds bottlenecks and makes safe optimizations",
  "worker-planner": "breaks work into steps, dependencies, risks and checks",
  "worker-qa-engineer": "writes automated tests and reproduction scenarios",
  "worker-refactorer": "restructures code without changing behavior",
  "worker-security-auditor": "audits auth, secrets, cryptography, dependencies and data protection",
  "worker-senior-consultant": "gives senior decisions on the coordinator's request from published reports",
  "worker-technical-writer": "writes project and feature documentation",
  "worker-ux-designer": "designs layouts, interaction flows and responsive behavior",
};

/** Help under one "Preferred work" switch: what the kind is and which agents it covers. */
export function workKindHelp(kind: string, roles: readonly string[]): HelpText {
  const base = WORK_KIND_HELP[kind] ?? `Work of kind ${kind}.`;
  const agents = roles.length === 0 ? "No worker role uses this kind directly." : `Agents:\n${roles.map((role) => `  • ${role}${ROLE_HELP[role] ? ` — ${ROLE_HELP[role]}` : ""}`).join("\n")}`;
  return `${base}\n${agents}`;
}

export const PREFERRED_WHEN_HELP = "Optional condition in your own words, for example \"frontend changes without design work\". The coordinator judges each task step against it and names this model for the steps that meet it; the model then ranks first for them. The switches above, if any, limit the condition to those kinds of work. Steps that do not meet it treat the model as an ordinary candidate. Leave empty for a plain preference by kind of work.";

export const PREFERRED_WORK_HELP = "Turn a work kind on to rank this model first, within its tier, when a worker is picked for that kind. It affects only automatic worker selection (not the coordinator or models you pick by hand), never overrides availability, policy or plan usage, and needs the model selected for automatic ranking. Recommended: mark only the kinds where this model is clearly strongest; leave the rest off.";
