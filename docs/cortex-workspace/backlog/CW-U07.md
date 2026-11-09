# CW-U07: Per-machine orchestration settings

**Status:** Planned; design gate passed 2026-11-05; implementation waits for its dependencies · **Phase:** 3+ · **Size:** L

## Goal
Per-machine settings. They hold the orchestration settings, the machine-level defaults for new tasks (main model, effort, access mode and language, as set out in [CW-U10](CW-U10.md)), the voice-input provider choice ([CW-U11](CW-U11.md)) and the Worktrees page that sets the machine's worktree root.

The orchestration settings reach field-level parity with the Cortex configuration that Paseo exposes, in a more convenient form. MCP servers can be edited there, and the tools a connected server lists show their access configuration in the same place ([CW-R41](../requirements.md)).

## Requirements
CW-R19, CW-R23, CW-R25, CW-R26, CW-R30, CW-R31, CW-R41; see [requirements](../requirements.md).

## Inputs
[Domain model](../domain-model.md), [testing and acceptance](../testing-and-acceptance.md), [desktop rework](../design/desktop-rework-proposal.ru.md) and dependencies below. Owner decisions supersede conflicting historical proposal text.

## Deliverables
Provide detailed per-machine orchestration settings and explicit save/conflict states. Include the machine defaults for model, effort, access mode and language, the default send mode of the task composer ([CW-U12](CW-U12.md)), the voice-input provider, model and language ([CW-U15](CW-U15.md)), and archive retention per machine (see [CW-C10](CW-C10.md)). Worktrees page: the worktree root with its default (`~/.local/share/cortex-workspace/worktrees`), a folder picker or typed path, "Reset to default", and the validation states in [SCREENS](../SCREENS.md#worktree-settings-desktop) (invalid, not writable, inside a project checkout, not saved). The saved root is the input to [CW-G06](CW-G06.md). Split before implementation if effort exceeds L. With the field checklist below, effort is expected to exceed L: split into orchestration settings, Tools and MCP, and Profiles and Files items.

The orchestration settings pages and shared rules are in [SCREENS](../SCREENS.md#orchestration-settings-desktop). The pages are Overview, Coordinator, Models, Routing, Route preview, Limits and fallback, Tools and MCP, Profiles, and Files and presets; Connections keeps its own page. The settings surfaces `user_defaults`, `connections`, `routing`, `project_ceiling` and `mcp_servers` each keep their own revision and draft. Models and Routing share the `routing` draft. Tool access is edited as a matrix in which every saved list is a subset of the list above it. New tools get no access by default. Consultant access cells are fixed.

## Field inventory checklist (CW-R41)

Each item must be reachable on an orchestration settings page, as Simple, Advanced or read-only as stated. Scope: M machine, P project. Source references name runtime modules: `configuration/management.py`, `configuration/mcp_servers.py`, `routing/auto_route.py` and `routing/model_policy.py`.

Coordinator and defaults (`user_defaults`):
- [ ] `coordinator.engine`, `coordinator.model`: Coordinator, Simple; M; the model must be enabled.
- [ ] `coordinator.parameters.thought_level` as Effort: Coordinator, Simple; the levels offered are the selected model's observed levels.
- [ ] Other `coordinator.parameters` (name and value rows): Coordinator, Advanced.
- [ ] `route_id`, `route_connection_id`, `route_model`, `parameters` (exact default route): Coordinator, Advanced; setting it while the coordinator is portable is refused with "Clear the exact route first".
- [ ] `language` (Automatic, English, Russian): Coordinator, Simple.
- [ ] Access mode (Development, Guarded, Auto): Coordinator, Simple; Workspace-owned, not a configuration key.
- [ ] Default checkout (worktree or main): Coordinator, Simple; Workspace-owned.
- [ ] `coordinator_resolution`: Coordinator, read-only; shows bound, or the error with a fix link.
- [ ] Preserved keys the app does not manage: Files, read-only.

Engines and connections (`connections`):
- [ ] `name`, `max_concurrent_sessions` (1 to 64): Connections, Simple.
- [ ] Rename, set max concurrent sessions, validate, remove: Connections.
- [ ] Engine status (not downloaded, downloaded without login, ready to connect, connected): read-only, refreshed every 3 seconds.
- [ ] `automatic_setup`, `automatic_setup_reason`, `auto_connect_disabled`: read-only.
- [ ] Download engine (Codex and Claude, pinned adapter): Connections.
- [ ] Add connection (native login only): Connections.
- [ ] OpenCode discover, enable and name (explicit opt-in, off by default): Connections.
- [ ] API-key connection: Connections, with a note that setup is CLI-only.
- [ ] Generated fields (id, engine, provider, auth mode, key kind, endpoint, launch command, credential kind, state, qualified, timestamps): Connections, read-only under Advanced.
- [ ] `CORTEX_ENGINE_UPDATE`, `CORTEX_ENGINE_FOLLOW`, `CORTEX_OPENCODE_BINARY`: Files, read-only environment overrides.
- [ ] `ssh-askpass.json` `enabled`: Files, read-only.

Routing (`routing`; the first existing file wins and files are never merged):
- [ ] `model_policy_version` (must be 2): Files and Overview, read-only; a migration banner when it is not 2.
- [ ] `enabled.<engine>`: Models, Simple.
- [ ] `providers` (engines for automatic routing; at least one): Models, Simple.
- [ ] `deny` tokens (model, `engine/model`, `engine/*`): Models engine switch (Simple) and Routing > Rules (Advanced).
- [ ] `models.<engine>.auto`: Models, Simple; requires a tier.
- [ ] `models.<engine>.tiers.<model>` (flagship, deep, standard, light): Models, Simple.
- [ ] `policy.effort_floor`, `policy.max_effort`: Models detail, Simple.
- [ ] `policy.effort_ladder`: Models detail, Advanced; shown for light models.
- [ ] `policy.also_standard_for`: Models detail, Advanced; shown for light models.
- [ ] `policy.preferred_for`: Models detail, Simple; also edited from Work kinds.
- [ ] `policy.preferred_when` (up to 500 characters): Models detail, Advanced.
- [ ] `policy.request_names`: Models detail, Advanced.
- [ ] `pin.<role or *>` (engine, model, effort; strict): Routing > Roles, Simple.
- [ ] `engine_affinity.<work kind or *>` (engine lean, preferred models, stretch): Routing > Work kinds, Simple.
- [ ] `quota_windows.<engine>[].window` and `.models`: Routing > Quota, Advanced.
- [ ] `auto_allow`: Routing > Rules, Advanced; shown only when non-empty, with the note that it has no effect under current routing.
- [ ] Top-level `tiers` (legacy): Routing > Rules, read-only with a migrate link; shown only when present.
- [ ] Deny and other tokens that match no model: Routing > Rules, Advanced, removable.
- [ ] Refresh model lists (20-second bound): Models.
- [ ] Per-connection model list (observed time, refresh outcome, models not offered now, models not yet confirmed by the engine): Models, read-only except removal of unoffered models.
- [ ] Model routes per connection (route id, state, automatic verdict and reason, matching deny tokens): Model detail, read-only.
- [ ] Built in, read-only and marked with a lock: role base tier, complexity shift, role work kind, built-in engine affinity, the affinity-bound kind `implementation_frontend_design`, tier fallback order, effort by tier, hot band (95%), rank order, the flagship rule, the role tier table, and the twelve work kinds. Routing.

Route preview (read-only):
- [ ] Role, work kind, complexity, attempt, and saved or draft rules: Route preview.
- [ ] Tier path, effort, ranked routes with their decision, excluded routes with reasons, and the all-roles grid: Route preview.

Limits and fallback (`ceiling` in `defaults.json`, and the project ceiling):
- [ ] `allowed_routes`: Limits, Advanced; machine and project.
- [ ] `allowed_connections`: Limits, Simple; machine and project.
- [ ] `allowed_data_recipients`: Limits, Simple; machine and project.
- [ ] `limits.attempts`: Limits, Simple; blank means no limit.
- [ ] `fallback_policy.allowed_failure_classes` (`engine_failure`, `capacity_unavailable`): Limits, Simple.
- [ ] `fallback_policy.allowed_target_routes`: Limits, Advanced.
- [ ] `fallback_policy.budget`: Limits, Simple.
- [ ] `fallback_policy.data_recipient_consent`: Limits, Simple; labelled "Allow work to move to another provider"; required for any fallback.
- [ ] Fixed note on hard denials (cancellation, refusal, authorization loss, provider restriction): Limits, read-only.
- [ ] `permission_level`, `limits.calls`, `limits.tokens`, `limits.concurrency`, `spend_limit`, `spend_limit_enforceable`: Limits, read-only and marked "Stored, not applied"; shown only when set.
- [ ] Project ceiling state (not set, violation, no longer narrows) and its removal with confirmation: Limits, project scope; a project ceiling may only narrow the machine limit.

Tools and MCP (`mcp_servers`; machine scope only; stdio only):
- [ ] `name`: Tools; key matching `^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$`, not `cortex`, unique; read-only after creation.
- [ ] `command`: Connection.
- [ ] `args`: Connection; ordered; "Paste a command line" splits it; redacted values keep their stored value.
- [ ] `env`: Connection; names with secure values; keep, replace or remove; values never shown after saving.
- [ ] `enabled`: Tools list; new servers start disabled.
- [ ] `roles` (coordinator, worker): Tools, "Who gets this server"; unset means both.
- [ ] `engines`: Tools, engine checkboxes; all engines when unset; at least one engine.
- [ ] `allowed_tools`: Tools matrix, Granted; required and non-empty.
- [ ] `read_only_tools`: Tools matrix, Read-only column.
- [ ] `coordinator_tools`: Tools matrix, Coordinator column; "Same as read-only" or an explicit list.
- [ ] `always_load_tools`: Tools matrix, Always load column; Claude only; needs Writing.
- [ ] `read_paths`, `write_paths`: Sandbox; absolute, existing, not the project or its parent, no symlink ancestors.
- [ ] `network`: Sandbox; off by default.
- [ ] `runtime_dir_env`: Sandbox, Advanced; uppercase; `PATH`, `HOME`, `TMPDIR` and `XDG_RUNTIME_DIR` are reserved.
- [ ] Derived status (`valid`, `problem`, `redacted`, `unsupported_keys`): Tools list and server, read-only; unsupported keys allow only turn on, off or remove.
- [ ] Per-tool state (Granted, New, Not offered, Not checked): Tools matrix.
- [ ] Server actions (Check tools, which shows what is new or no longer offered and changes nothing else; Duplicate; Remove): Tools.
- [ ] Consultant access: fixed "Never" on every tool, with its reason; not a setting.

Profiles, presets and retention:
- [ ] Profile name (`^[a-z0-9][a-z0-9-]{0,39}$`) and description (up to 256 characters): Profiles, Simple.
- [ ] Actions (create, save, update, delete, activate, deactivate, discard modifications): Profiles and context bar; delete removes only that snapshot.
- [ ] Profile state (none, in sync, modified, interrupted, unavailable): context bar, read-only.
- [ ] Differences, modified sections, pending, problems, matches profile, shadowed by: Profiles, read-only; section and key names only.
- [ ] Captured sections (routing, defaults and ceiling, MCP usage policy, connection session limits): Profiles; a partial profile says which sections it stores.
- [ ] Never captured (project settings, MCP `command`, `args`, `env`, credentials): Profiles, note.
- [ ] Default preset apply (absent files only) and reset (replaces routing and defaults, keeps private backups, needs a literal confirmation): Files.
- [ ] Model-policy migration (preview, then apply with explicit consent): Files.
- [ ] Storage cleanup (0 to 36500 days; explicit owner action only; no automatic configuration key): Storage page. Archive retention per machine is Workspace-owned and belongs to [CW-C10](CW-C10.md).
- [ ] Help language (English, Russian, both; default both): App > Appearance; a client setting, not a configuration key.

Sources and overview:
- [ ] Routing source precedence, path, existence, validity and revision: Files and Overview.
- [ ] `$CORTEX_ROUTING`: read-only; all routing controls are off with that reason.
- [ ] Shadowing (a higher source wins; saved but shadowed): Models and Routing, project scope.
- [ ] Project routing file, seeded from machine rules on first save: Models and Routing, project scope note.
- [ ] Invalid routing file (every dispatch refused; link to Files): Overview, danger strip.
- [ ] Overview needs-attention list and summary rows: Overview.

## Acceptance
- Positive: Settings identify the selected machine and persist only through supported contracts. A new task on that machine starts with the saved defaults, and the task's header shows the values the runtime confirmed.
- Negative: Offline, stale or unsupported saves are not shown as successful. A saved default that the machine's route no longer supports is shown as unavailable, not silently replaced.
- A saved speech-service key is never displayed again after it is saved; the settings show only whether a key is set.
- A worktree root that cannot be saved (offline, stale or unsupported) is shown as not saved and is not used by new tasks.
- Positive (orchestration settings): every item in the field checklist is reachable on an orchestration page, and a test walks the list.
- Positive: a connected server's listed tools show their access per role class. A new tool is shown as New with no access. A granted tool the server no longer lists is shown as Not offered and can be removed.
- Positive: Check as shows the same access result that the runtime uses for the chosen role class and engine.
- Positive: Models and Routing share one draft and one save; Coordinator and Limits share one draft and one save.
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

For the orchestration settings, correct the current configuration docs and the settings help text where they disagree with the source, after verifying each against the source: an engine omitted from `enabled` has no models enabled; `auto_allow` has no effect under model policy version 2; top-level `tiers` is refused under version 2; `allowed_tools` is required and non-empty, also for a disabled server; `engines` accepts all three engines; profile names do not allow underscores; `runtime_dir_env` names must be uppercase; MCP transport is stdio only. The [SCREENS](../SCREENS.md#orchestration-settings-desktop) section is the planned page contract.
