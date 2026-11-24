# Unified system prompt

> Status: current. Every statement below was checked against the Rust source
> paths it cites. Earlier environment switches, prompt editions and adapter
> metadata fields no longer exist in the Rust runtime and are not described.

Every native engine session (coordinator, writing worker, read-only worker,
senior consultant) is given one Cortex-owned instruction text. The host assembles
the text when the session starts and hands it to the engine through an
engine-specific channel: Claude and Codex receive it as their system or base
instructions, while OpenCode task turns receive it as a prefix of the user prompt
(Cortex does not set or prove replacement of OpenCode's own base prompt). Per-turn material
(recovery capsule, assignment brief, attachments, project skills) travels as user
input, not as system instructions.

## Parts, in order

The final text for a session is built in
`crates/workspace-service/src/native_sessions.rs` (the admission step calls
`native_instructions::compose`, then the session start appends the route text).

1. **Role body, inlined skills and situational catalogue** (generated data, one
   entry per profile). The profile `body` is the role's `SKILL.md` with its
   frontmatter and `cortex:native` blocks removed, followed by
   `## Skills already included` (the always-included skills in full) and
   `## Skills, and when to read one` (an `id -- trigger` list of situational
   skills read on demand with Cortex MCP `read_skill`). Always included:
   coordinator — `tool-discipline`, `content-safety`, `coordinator-communication`;
   worker — `tool-discipline`, `tool-discipline-worker`, `content-safety`,
   `report-publication`, `code-and-evidence`; the consultant skips the ones in
   `CONSULTANT_EXCLUDED_NAMED`. Source: `crates/cortex-xtask/src/instructions.rs`
   (`composed_instructions`, `INLINED_COORDINATOR`, `INLINED_WORKER`,
   `SKILL_TRIGGERS`).
2. **Claude specialist narration rule** (`compose` in `native_instructions.rs`):
   the session prompt tells delegated execution workers and reviewers to work
   through tools without interim assistant text. If a closing response is needed
   after the final tool call, it is limited to one short sentence and cannot
   replace the structured result. The coordinator, senior consultant, Codex
   sessions and OpenCode sessions do not receive this rule. It is prompt guidance,
   not enforcement or output filtering. If Claude generates interim assistant
   text anyway, it is still recorded in worker activity.
3. **Native capability boundary** (`compose` in `native_instructions.rs`): a fixed
   paragraph saying only the tools advertised on the channel are available, a
   paragraph that names the Cortex discovery tool and says how to treat external
   MCP tools, and a one-line hint about splitting large edits after a JSON parse
   error. The discovery tool is named `mcp__cortex__tools_discover` for Claude and
   `tools_discover` for the other engines.
4. **Codex tool cheat sheet** (Codex only): `# Cortex tool cheat sheet (code
   mode)`, rendered mechanically from the channel's served tool schemas
   (`schema::catalogue` in `crates/cortex-engine/src/schema.rs`, filtered by
   `native_instructions::supports`). Required and optional fields, enums and the
   first schema example come from the schema; priority tools come first; the
   section is capped at 7,850 bytes, after which a line points to
   `tools_discover`. Claude sessions never receive it.
5. **Mode additions** (`native_sessions.rs`): a coordinator always gets
   `lifecycle::automatic::COORDINATOR_NOTE`, plus a sentence about project file
   tools when the task is Automatic; an Automatic execution worker gets the
   "Automatic sprint" note about attributable write receipts.
6. **Route text** (`route.instructions`): the route's instruction string (in
   `native_routes.rs`) plus the owner-language paragraph added in
   `native_runtime.rs`. The coordinator is told to write what the owner reads in
   the owner's language, including final-result prose and descriptions of
   verification limits, and to keep worker-facing text in English. The
   coordinator-only communication skill reinforces this after plan submission
   and English tool output: skip the plan recap and keep direct replies,
   questions and owner-facing report prose in the owner's language. Workers get
   `user_question_language` and are told to write reports and every other
   message in English. The runtime resolves Automatic using the original
   request; it does not translate model text.

The Rust host does **not** add an attached-MCP-servers section, a
worker-capability summary or an environment section to the system text. External
MCP servers are attached at session start (`native_mcp.rs`) and the engine
discovers their tools through `tools/list`.

## Sent as user input instead

`SessionDriver::prompt` (`native_sessions.rs`) builds each turn as
`<project skills section>\n\n<recovery capsule>\n\n<assignment brief or user text>`:

- **Project skills section** (`## Project skills`, `native_skills.rs::section`),
  only for channels served `project_read` (not the consultant); see
  [Project skills](#project-skills).
- **Recovery capsule** (`cortex_core::recovery_context::delivery_tx`): sent in
  full until delivery is confirmed in the current context generation (a
  `delivery_confirmed` or `continuation_observed` receipt). Only after that
  confirmation is a short `read_capsule` reference used; a capsule that was merely
  transmitted, for example in an interrupted first turn, is sent in full again.
  See [context generations](../context-generations/index.md#delivery-ledger).
- **Assignment brief or user text.** Stored attachments are not embedded in the
  brief: `native_attachments.rs` appends separate prompt blocks or markers whose
  form (body or marker) depends on the engine's attachment capabilities and on
  prior delivery. Coordinator turns may also carry sprint dependency context and a
  skill-proposal planning instruction (`native_runtime.rs`).

The project skill list is captured once at session start
(`native_sessions.rs`) and reused for the turns of that session.

Each prompt records digests, never text, of the instruction bundle, capsule,
brief and served tool catalogue (`recovery_context::record_delivery_tx`).

## Delivery per engine

| Engine | Channel | Source |
| --- | --- | --- |
| Claude | `systemPrompt` in the session `_meta`, a plain string that replaces the Claude Code preset, next to `native_engine_policy::claude_session_meta()` | `native_sessions.rs` |
| Codex | a private `model-instructions.md` (mode 0600, created without following links in the broker directory) referenced by `model_instructions_file` inside `CODEX_CONFIG`; replaces Codex base instructions | `cortex-engine/src/broker.rs` (`write_model_instructions`), `cortex-engine/src/engine.rs` (`launch_with_model_instructions`), checked in `native_codex_runtime.rs` |
| OpenCode | on the native task path no system channel is set: the instruction text is prepended to the user prompt of every turn. Cortex does not install an agent prompt there, so it neither replaces nor proves replacement of OpenCode's base prompt | `native_sessions.rs` (`SessionDriver::prompt`), `native_routes.rs` |

OpenCode also has a launch-time agent prompt, but it belongs to the separate
catalog-read process (`machine/config_opencode.rs`), not to task sessions.

Short-text generation sessions (`short_text_engine.rs`) are separate: they use a
one-sentence text-generation instruction through the same two channels (Claude
`systemPrompt`, Codex instruction file) and receive no role instructions.

## Roles

The role and stage come from the authenticated channel, not from an engine
request. `compose(name, stage, engine)` picks the profile (`orchestrator` for the
coordinator, a `worker-*` id otherwise) and filters the served tool list.

- **Coordinator** — `orchestrator` body and coordinator inlined skills.
- **Writing worker** — worker body and the worker inlined skills
  (`INLINED_WORKER`). The mutation-only skills `skill-authoring` and
  `knowledge-harvest` are not inlined: eligible writing profiles see their
  triggers in the situational list and call `read_skill` when applicable.
- **Read-only worker** — the same inlined skills; the read-only filter removes the
  mutation-only skills from the available and advertised set, so `read_skill`
  refuses them for these profiles (the `READ_ONLY` list in `instructions.rs`).
- **Senior consultant** (`worker-senior-consultant`) — a worker variant limited
  to the supplied packet; no project-source or code-index tools and no project
  skills section.

The tools a role may call, and so the cheat-sheet content, come from
`schema::catalogue(role, profile, stage)`; `result_submit` is served only for the
execution and review stages.

## Project skills

The project skill directories are `.agents/skills`, `.claude/skills` and
`.codex/skills` below the project root, looked up in that order
(`PROJECT_SKILL_DIRECTORIES`). A project skill is a regular, single-link
`<dir>/<name>/SKILL.md` of at most 65,536 bytes of UTF-8 text. Cortex never
follows symbolic links, and user-level skill directories are neither listed nor
read.

The section lists up to 64 skills by name, path and description and is marked as
project content that grants no tool access, scope or permission. The first
directory wins for a duplicated name. A skill whose name equals a bundled Cortex
id is listed by path only. The description is the top-level `description`
frontmatter value (plain, quoted or block form), read from the first 16 KiB and
capped at 300 characters; this is not a YAML parser. The section is omitted when
there are no project skills.

Cortex MCP `read_skill` reads a bundled Cortex id allowed for the channel's
profile, or a project skill by name, directory (`.agents/skills/<name>`) or file
(`.agents/skills/<name>/SKILL.md`), relative to the project or as an absolute
path inside it. Cortex ids win over project names. Project skill paths follow the
same rules as `project_read`; a reply carries `source: "project"`, the path,
SHA-256 and size. An unreadable skill is an ordinary tool error with a `code`:
`skill_unknown`, `skill_not_for_role`, `project_skill_path_refused`,
`project_skill_outside_project`, `project_skill_location_unrecognised`,
`project_skill_missing`, `project_skill_symlink_refused`,
`project_skill_not_regular`, `project_skill_too_large`,
`project_skill_unreadable`, `project_skill_not_text`,
`project_skills_unavailable` or `invalid_arguments`. Source:
`crates/workspace-service/src/native_skills.rs`. See also
[skill management](../skill-management/index.md).

## Where each source lives

| What | Path |
| --- | --- |
| Coordinator and shared skill sources | `skills/<id>/SKILL.md`; coordinator references in `skills/orchestrator/references/` |
| Worker skill sources (generated) | `skills/worker-*/SKILL.md` and `skills/worker-general/references/`, rendered from `agent-sources/*.md` by `crates/cortex-xtask/src/profiles.rs` |
| Generated instruction data | `crates/workspace-service/schema/instructions.json` (`version`, `profiles`, `skills`), embedded with `include_str!` |
| Generator | `crates/cortex-xtask/src/instructions.rs` |
| Runtime assembly and bundled `read_skill` | `crates/workspace-service/src/native_instructions.rs` |
| Tool schemas and descriptions (edit here) | `crates/cortex-engine/schema/tools.json`; `schema.rs` embeds it and builds `schema::catalogue` |
| Worker profile inputs | `agent-sources/profiles.json`, the protocol and verification fragments (`agent-sources/worker-protocol.md`, `consultant-protocol.md`, `verification-*.md`) and `agent-sources/references/` |
| Session start, delivery, per-turn prompt | `crates/workspace-service/src/native_sessions.rs`, `native_runtime.rs` |
| Codex instruction file | `crates/cortex-engine/src/broker.rs`, `engine.rs`; `native_codex_runtime.rs` |
| Short-text sessions | `crates/workspace-service/src/short_text_engine.rs` |
| Project skills | `crates/workspace-service/src/native_skills.rs` |

## How to change it

1. Edit the source, never the generated data: role text in `skills/<id>/SKILL.md`
   (worker roles in `agent-sources/*.md`); which skills are inlined, situational
   or role-restricted in the constants of
   `crates/cortex-xtask/src/instructions.rs`; runtime-appended text in
   `native_instructions.rs`, `native_sessions.rs` or `native_runtime.rs`; tool
   schemas and descriptions in `crates/cortex-engine/schema/tools.json` (the
   Rust `schema.rs` only loads and filters it). Worker skills are rendered from
   `agent-sources/profiles.json`, the protocol and verification fragments and
   `agent-sources/references/`, not from the `skills/worker-*` output.
2. Regenerate: `scripts/cortex-xtask.sh profiles write` (worker skills), then
   `scripts/cortex-xtask.sh generate-instructions write` (`instructions.json`).
   `generate-contract` is a separate command for the Workspace wire contract and
   is not part of the prompt text. `profiles` and `generate-instructions` take one
   positional action, `check` or `write`; verify with the bare
   `scripts/cortex-xtask.sh check-generated`, which takes no action and runs the
   contract, instruction and profile checks together. Instruction data is embedded
   at build time, so a runtime built before regeneration keeps the old text until
   it is rebuilt.
3. After a package payload edit run `scripts/cortex-xtask.sh package stamp`, then
   `scripts/cortex-xtask.sh package check`.
4. Tests: `cargo test -p workspace-service --lib native_instructions` (every
   profile embedded and scoped, only served tools named, cheat sheet matches the
   served tools and is Codex-only), `cargo test -p cortex-engine` for schemas, and
   the delivery tests in `crates/workspace-service/tests/worker_roles.rs` and
   `native_sessions.rs`. Run commands under `umask 022` and GNU `timeout`; see
   [verification](../../project/verification.md).

The Codex cheat sheet is derived output: it is rendered mechanically from the
live schemas at session start. Never hand-edit contract text into it; change the
schema or its descriptions instead. Tool argument contracts belong in schemas,
not in skills or profile text.

## Boundaries

The prompt is guidance, not authority: permissions, scope and completion are
decided by the runtime. The writing-worker text says that a missing, incomplete or
stale file-read precondition is fixed by a fresh read and a rebuilt edit under
existing authority, while real access denials stay final (see
[read-precondition recovery](../permissions-and-containment/index.md#read-precondition-recovery)).
The runtime does not enforce situational-skill triggers; compliance is a model
behavior checked only by review.
