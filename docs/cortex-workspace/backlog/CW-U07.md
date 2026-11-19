# CW-U07: Per-machine orchestration settings

**Status:** Implemented for the local machine with the eight-section structure that mirrors the Paseo settings panel (unreleased 1.0.6): per-machine orchestration settings, editable MCP servers and per-tool access ([screens](../SCREENS.md#orchestration-settings-desktop)); settings for remote machines do not exist; per-tool engine access and per-role access remain open decisions · **Phase:** 3+ · **Size:** L

## Goal
Per-machine settings. They hold the orchestration settings, the machine-level defaults for new tasks (main model, effort, access mode and language, as set out in [CW-U10](CW-U10.md)), the voice-input provider choice ([CW-U11](CW-U11.md)) and the Worktrees page that sets the machine's worktree root.

The orchestration settings reach field-level parity with the Cortex configuration that Paseo exposes, in a more convenient form. MCP servers can be edited there, and the tools a connected server lists show their access configuration in the same place ([CW-R41](../requirements.md)).

## Requirements
CW-R19, CW-R23, CW-R25, CW-R26, CW-R30, CW-R31, CW-R41; see [requirements](../requirements.md).

## Inputs
[Domain model](../domain-model.md), [testing and acceptance](../testing-and-acceptance.md), [desktop rework](../design/desktop-rework-proposal.ru.md) and dependencies below. Owner decisions supersede conflicting historical proposal text.

## Deliverables
Provide detailed per-machine orchestration settings and explicit save/conflict states. Include the machine defaults for model, effort, access mode and language, the default send mode of the task composer ([CW-U12](CW-U12.md)), the voice-input provider, model and language ([CW-U15](CW-U15.md)), and archive retention per machine (see [CW-C10](CW-C10.md)). Worktrees page: the worktree root with its default (`~/.local/share/cortex-workspace/worktrees`), a folder picker or typed path, "Reset to default", and the validation states in [SCREENS](../SCREENS.md#worktree-settings-desktop) (invalid, not writable, inside a project checkout, not saved). The saved root is the input to [CW-G06](CW-G06.md). Split before implementation if effort exceeds L. With the field checklist below, effort is expected to exceed L: split into orchestration settings, MCP servers, and Profiles and Files items.

The orchestration settings sections and shared rules are in [SCREENS](../SCREENS.md#orchestration-settings-desktop). The sections are Defaults, Models, Engines and models, Project ceiling, Routing, Profiles, MCP servers and Connections, in the order of the Paseo settings panel; Route preview and Files and sources are secondary pages. The settings surfaces `user_defaults`, `connections`, `routing`, `project_ceiling` and `mcp_servers` each keep their own revision and draft. Models, Engines and models, Routing and Route preview share the `routing` draft; Defaults holds the `user_defaults` draft, which includes the machine ceiling. The field checklist names the section for each field. Tool access is edited as a matrix in which every saved list is a subset of the list above it. New tools get no access by default. Consultant access cells are fixed.

## Field inventory checklist (CW-R41)

Each item must be reachable on an orchestration settings page, as Simple, Advanced or read-only as stated. Scope: M machine, P project. Source references name the former Python runtime modules (configuration management, MCP servers, automatic routing and model policy), now implemented in the native settings API.

Coordinator and defaults (`user_defaults`):
- [ ] `coordinator.engine`, `coordinator.model`: Defaults > Coordinator, Simple; M; the model must be enabled.
- [ ] `coordinator.parameters.thought_level` as Effort: Defaults > Coordinator, Simple; the levels offered are the selected model's observed levels.
- [ ] Other `coordinator.parameters` (name and value rows): Defaults > Coordinator > More coordinator parameters, Advanced.
- [ ] `route_id`, `route_connection_id`, `route_model`, `parameters` (exact default route): Defaults > Exact default route, Advanced; setting it while the coordinator is portable is refused with "Clear the exact route first".
- [ ] `language` (Automatic, English, Russian): Defaults, Simple.
- [ ] Access mode (Development, Guarded, Auto): Defaults > New tasks, Simple; Workspace-owned, not a configuration key.
- [ ] Default checkout (worktree or main): Defaults > New tasks, Simple; Workspace-owned.
- [ ] `coordinator_resolution`: Defaults > Coordinator, read-only; shows bound, or the error with a fix link to Models.
- [ ] Preserved keys the app does not manage: Defaults notice and Files and sources, read-only.

Engines and connections (`connections`):
- [ ] `name`, `max_concurrent_sessions` (1 to 64): Connections, Simple.
- [ ] Rename, set max concurrent sessions, validate, remove: Connections.
- [ ] Engine status (not downloaded, downloaded without login, ready to connect, connected): read-only, refreshed every 3 seconds.
- [ ] `automatic_setup`, `automatic_setup_reason`, `auto_connect_disabled`: read-only.
- [ ] Download engine (Codex and Claude, pinned adapter): Connections.
- [ ] Add connection (native login only): Connections.
- [ ] OpenCode discover, enable and name (explicit opt-in, off by default): Connections.
- [ ] API-key connection: Connections, with a note that setup is CLI-only.
- [ ] Generated fields (id, engine, provider, auth mode, key kind, endpoint, launch command, credential kind, state, qualified, timestamps): Connections, read-only under Details of <name>.
- [ ] `CORTEX_ENGINE_UPDATE`, `CORTEX_ENGINE_FOLLOW`, `CORTEX_OPENCODE_BINARY`: Files and sources, read-only environment overrides.
- [ ] `ssh-askpass.json` `enabled`: Files and sources, read-only.

Routing (`routing`; the first existing file wins and files are never merged):
- [ ] `model_policy_version` (must be 2): Routing > Routing files, read-only; a migration strip when it is not 2.
- [ ] `enabled.<engine>`: Models, Simple.
- [ ] `providers` (engines for automatic routing; at least one): Engines and models, Simple ("Automatic routing may use <Engine>").
- [ ] `deny` tokens (model, `engine/model`, `engine/*`): Engines and models engine switch for `engine/*` (Simple) and Routing > Deny list (Advanced).
- [ ] `models.<engine>.auto`: Engines and models > Automatic, Simple; requires a tier.
- [ ] `models.<engine>.tiers.<model>` (flagship, deep, standard, light): Engines and models > Tiers, Simple.
- [ ] `policy.effort_floor`, `policy.max_effort`: Routing > Model policy, Simple.
- [ ] `policy.effort_ladder`: Routing > Model policy, Advanced; shown for light models.
- [ ] `policy.also_standard_for`: Routing > Model policy, Advanced; shown for light models.
- [ ] `policy.preferred_for`: Engines and models > Preferred work, Simple; the same checklist is in Routing > Model policy.
- [ ] `policy.preferred_when` (up to 500 characters): Engines and models > Prefer when, Advanced.
- [ ] `policy.request_names`: Routing > Model policy, Advanced.
- [ ] `pin.<role or *>` (engine, model, effort; strict): Routing > Roles and pins, Simple.
- [ ] `engine_affinity.<work kind or *>` (engine lean, preferred models, stretch): Routing > Work kinds, Simple.
- [ ] `quota_windows.<engine>[].window` and `.models`: Routing > Quota windows, Advanced.
- [ ] `auto_allow`: Routing > Auto-allow, Advanced; shown only when non-empty; its effect is open (SCREENS open point 6).
- [ ] Top-level `tiers` (legacy): Routing > Legacy tiers, read-only with a migrate link; shown only when present.
- [ ] Deny and other tokens that match no model: Engines and models > Other entries in this file, and Routing > Deny list, Advanced, removable.
- [ ] Refresh model lists (20-second bound): Models.
- [ ] Per-connection model list (observed time, refresh outcome, models not offered now, models not yet confirmed by the engine): Models, read-only except removal of unoffered models.
- [ ] Model routes per connection (automatic verdict and reason, matching deny tokens; the route id is not shown, see SCREENS open point 8): Engines and models, the model row's Now line; read-only.
- [ ] Built in, read-only and marked with a lock: role base tier, complexity shift, role work kind, built-in engine affinity, the affinity-bound kind `implementation_frontend_design`, tier fallback order, effort by tier, hot band (95%), rank order, the flagship rule, the role tier table, and the twelve work kinds. Routing > How the router decides, Roles and pins, and Work kinds, locked.

Route preview (read-only):
- [ ] Role, work kind, complexity, attempt, and saved or draft rules: Route preview.
- [ ] Tier path, effort, ranked routes with their decision, excluded routes with reasons, and the all-roles grid: Route preview.

Limits and fallback (`ceiling` in `defaults.json`, and the project ceiling):
- [ ] `allowed_routes`: Defaults > Machine ceiling, Advanced; Project ceiling, Advanced; machine and project.
- [ ] `allowed_connections`: Defaults > Machine ceiling, Simple; Project ceiling; machine and project.
- [ ] `allowed_data_recipients`: Defaults > Machine ceiling, Simple; Project ceiling; machine and project.
- [ ] `limits.attempts`: Defaults > Machine ceiling, Simple; blank means no limit.
- [ ] `fallback_policy.allowed_failure_classes` (`engine_failure`, `capacity_unavailable`): Defaults > Machine ceiling > Fallback, Simple.
- [ ] `fallback_policy.allowed_target_routes`: Defaults > Machine ceiling > Fallback, Advanced.
- [ ] `fallback_policy.budget`: Defaults > Machine ceiling > Fallback, Simple.
- [ ] `fallback_policy.data_recipient_consent`: Defaults > Machine ceiling > Fallback, Simple; labelled "Allow work to move to another provider"; required for any fallback.
- [ ] Fixed note on hard denials (cancellation, refusal, authorization loss, provider restriction): Defaults > Machine ceiling > Fallback, read-only.
- [ ] `permission_level`, `limits.calls`, `limits.tokens`, `limits.concurrency`, `spend_limit`, `spend_limit_enforceable`: Defaults > Machine ceiling, read-only and marked "Stored, not applied"; shown only when set.
- [ ] Project ceiling state (not set, violation, no longer narrows) and its removal with confirmation: Project ceiling; a project ceiling may only narrow the machine limit.

MCP servers (`mcp_servers`; machine scope only; stdio only):
- [ ] `name`: MCP servers > Add server, Define step; key matching `^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$`, not `cortex`, unique; read-only after creation.
- [ ] `command`: MCP servers > edit page, Connection.
- [ ] `args`: MCP servers > edit page, Connection; ordered; "Paste a command line" splits it; redacted values keep their stored value.
- [ ] `env`: MCP servers > edit page, Connection; names with secure values; keep, replace or remove; values never shown after saving.
- [ ] `enabled`: MCP servers list; new servers start disabled.
- [ ] `roles` (coordinator, worker): MCP servers > edit page, "Limit to roles" ("Who gets this server"); unset means both.
- [ ] `engines`: MCP servers > edit page, "Limit to engines" checkboxes; all engines when unset; at least one engine.
- [ ] `allowed_tools`: MCP servers > tool matrix, Granted; required and non-empty.
- [ ] `read_only_tools`: MCP servers > tool matrix, Read-only column.
- [ ] `coordinator_tools`: MCP servers > tool matrix, Coordinator column; "Same as read-only" or an explicit list.
- [ ] `always_load_tools`: MCP servers > tool matrix, Always load column; Claude only; needs Writing.
- [ ] `read_paths`, `write_paths`: MCP servers > edit page, Sandbox; absolute, existing, not the project or its parent, no symlink ancestors.
- [ ] `network`: MCP servers > edit page, Sandbox; off by default.
- [ ] `runtime_dir_env`: MCP servers > edit page, Sandbox, Advanced; uppercase; `PATH`, `HOME`, `TMPDIR` and `XDG_RUNTIME_DIR` are reserved.
- [ ] Derived status (`valid`, `problem`, `redacted`, `unsupported_keys`): MCP servers list and server, read-only; unsupported keys allow only turn on, off or remove.
- [ ] Per-tool state (Granted, New, Not offered, Not checked): MCP servers > tool matrix.
- [ ] Server actions (Check tools, which shows what is new or no longer offered and changes nothing else; Duplicate; Remove): MCP servers list, More menu.
- [ ] Consultant access: fixed "Never" on every tool, with its reason; not a setting.

Profiles, presets and retention:
- [ ] Profile name (`^[a-z0-9][a-z0-9-]{0,39}$`) and description (up to 256 characters): Profiles, Simple.
- [ ] Actions (create, save, update, delete, activate, deactivate, discard modifications): Profiles and profile bar; delete removes only that snapshot.
- [ ] Profile state (none, in sync, modified, interrupted, unavailable): profile bar, read-only.
- [ ] Differences, modified sections, pending, problems, matches profile, shadowed by: Profiles, read-only; section and key names only.
- [ ] Captured sections (routing, defaults and ceiling, MCP usage policy, connection session limits): Profiles; a partial profile says which sections it stores.
- [ ] Never captured (project settings, MCP `command`, `args`, `env`, credentials): Profiles, note.
- [ ] Default preset apply (absent files only) and reset (replaces routing and defaults, keeps private backups, needs a literal confirmation): Defaults > Default preset.
- [ ] Model-policy migration (preview, then apply with explicit consent): Files and sources.
- [ ] Storage cleanup (0 to 36500 days; explicit owner action only; no automatic configuration key): Storage page. Archive retention per machine is Workspace-owned and belongs to [CW-C10](CW-C10.md).
- [ ] Help language (English, Russian, both; default both): App > Appearance; a client setting, not a configuration key. The settings help is English only today, and the choice is open (SCREENS, orchestration open point 1).

Sources and overview:
- [ ] Routing source precedence, path, existence, validity and revision: Routing > Routing files, and Files and sources.
- [ ] `$CORTEX_ROUTING`: read-only; all routing controls are off with that reason.
- [ ] Shadowing (a higher source wins; saved but shadowed): Models and Routing, project scope.
- [ ] Project routing file, seeded from machine rules on first save: Models and Routing, project scope note.
- [ ] Invalid routing file (every dispatch refused; link to Files and sources): Defaults needs-attention list, and the source strip on Models, Engines and models and Routing.
- [ ] Needs-attention list: Defaults, at the top. The former Overview summary rows are removed (SCREENS, Removed on purpose).

## Acceptance
- Positive: Settings identify the selected machine and persist only through supported contracts. A new task on that machine starts with the saved defaults, and the task's header shows the values the runtime confirmed.
- Negative: Offline, stale or unsupported saves are not shown as successful. A saved default that the machine's route no longer supports is shown as unavailable, not silently replaced.
- A saved speech-service key is never displayed again after it is saved; the settings show only whether a key is set.
- A worktree root that cannot be saved (offline, stale or unsupported) is shown as not saved and is not used by new tasks.
- Positive (orchestration settings): every item in the field checklist is reachable on an orchestration page, and a test walks the list.
- Positive: a connected server's listed tools show their access per role class. A new tool is shown as New with no access. A granted tool the server no longer lists is shown as Not offered and can be removed.
- Positive: Check as shows the same access result that the runtime uses for the chosen role class and engine.
- Positive: Models, Engines and models, Routing and Route preview share one routing draft and one save; Defaults (coordinator and machine ceiling) shares one draft and one save.
- Negative: an MCP server with no granted tool cannot be saved, even when it is disabled.
- Negative: the matrix never writes a list that is not a subset of its parent list. Consultant cells cannot be changed.
- Negative: tool listing from an unsaved draft calls no tool. Its failures show fixed reasons only, never raw server output or stored secrets. A failed check keeps the stored grants and the matrix editable.
- Negative: a save over a surface that changed elsewhere writes nothing until the owner has reviewed the changes.
- Negative: while the machine is offline, every orchestration write is disabled with its reason and nothing is queued.
- Negative: a stored value that the page does not offer (for example a model no longer offered, or a server key this app does not manage) stays visible as stored and is not removed by a save.

## Dependencies
CW-H05, CW-U01, CW-U10.

The orchestration pages that list MCP tools, run the route preview or read built-in facts need the runtime operations named in [CW-C01](CW-C01.md). Until they exist, those parts show an unavailable state, and tool names may be entered by hand and marked Not checked. Two owner decisions are open and do not block the mock: per-tool engine access, and access per worker role. Both would change the runtime schema.

## Validation
Write positive and negative domain or contract tests first where applicable; run focused tests and record command, environment, outcome and unrun cases.

For the orchestration settings: a test walks every checklist item and confirms that it is reachable; property tests confirm that every tool-access cascade yields a subset of the level above it; contract tests cover the revision conflicts of the five settings surfaces and the offline write block. The prototype uses the real preset and the three example servers, and it labels each simulated operation as simulated.

## Documentation impact
Update the relevant planned contracts and [roadmap](../roadmap.md) as behavior and qualification change; do not describe plans as shipped.

For the orchestration settings, correct the current configuration docs and the settings help text where they disagree with the source, after verifying each against the source: an engine omitted from `enabled` has no models enabled; `auto_allow` has no effect under model policy version 2 (contested: the runtime validator gives it an OpenCode effect, see SCREENS open point 6); top-level `tiers` is refused under version 2; `allowed_tools` is required and non-empty, also for a disabled server; `engines` accepts all three engines; profile names do not allow underscores; `runtime_dir_env` names must be uppercase; MCP transport is stdio only. The [SCREENS](../SCREENS.md#orchestration-settings-desktop) section is the planned page contract.
