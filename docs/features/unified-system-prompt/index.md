# Unified system prompt

> Status: migrated from the former ACP reference. Statements were not re-verified line by line against source in this pass; claims without a source path are unverified until reviewed. The former Python runtime was removed on 2026-11-16: references to its modules and tests were dropped, and statements that cited them describe behavior carried from that implementation that has not been re-verified against the Rust runtime.


## Current Rust delivery

The Rust host composes the role and route instructions once per native task session. Claude receives a plain string in session metadata's `systemPrompt`, which replaces the Claude Code preset. Codex reads a private per-session file through `CODEX_CONFIG.model_instructions_file`, which replaces its model base instructions. Each engine can add its own protocol or service messages. Project skill listings, recovery context, attachments and the assignment travel as user input. Both engines use `tools_discover` for the active Cortex catalogue; Claude's prompt identifies it by its native name, `mcp__cortex__tools_discover`. The prompt directs models to the engine's tool list or search for external tools, to use the advertised name and schema without replacing the server namespace, and to report unavailable tools rather than guess. Short-text sessions use a separate text-generation system instruction.

The remainder of this page is the historical ACP reference unless a section is
explicitly marked as current. In particular, its former environment switches,
edition builder and `_meta.cortexBaseInstructions` delivery table do not describe
the Rust session path above. The engine queries each MCP server's `tools/list`
to load its tool descriptions and schemas. `tools_discover` is a model-facing
tool on the Cortex server only.

## Historical ACP reference

The environment switches, editions and private adapter fields described below refer to the former implementation.

The writing-worker prompt in both editions distinguishes recognized
missing/incomplete/stale file-read preconditions from access refusals. It directs
a current same-channel read and a rebuilt valid edit under existing authority,
without an owner question or extra approval; actual channel, role, scope and
policy denials remain non-bypassable. Coordinator, read-only and consultant
prompts edit no files, so they omit that recovery sentence; the shared tool
discipline states it only for roles that edit files, and writer recovery
capsules carry the same distinction.
This is guidance, not deterministic model obedience or a permission grant. See
[read-precondition recovery](../permissions-and-containment/index.md#read-precondition-recovery).

The unified Cortex base prompt is enabled by default. Set
`CORTEX_UNIFIED_BASE_PROMPT=0` in the environment before starting Cortex to
disable it for newly created session metadata. This restores the engines'
pre-existing native base-prompt path; it does not roll back unrelated runtime
optimizations or routing changes. After sent session metadata is recorded, Cortex best-effort records a secret-free
`base_prompt` observability event; the diagnostics summary counts observed
`unified`, `native` and `unknown` modes, per engine as well as overall
(`counts`/`counts_by_engine` in `_base_prompt`). `native`
means the session's engine kept its own native system-prompt/instructions
channel and Cortex's text was appended alongside it; `unified` means that
native channel was replaced outright by Cortex's own base-prompt text; this is
the same per-engine classification `base_prompt_mode`
already derives from the sent session metadata, not a separate one. When the sent metadata carries the
unified prompt, the event also records which [edition](#editions) it was, read
back from the sent text (`sent_prompt_edition`), and the summary counts events
per edition; a native-mode session records no edition. Event recording can fail without
blocking session delivery, so absence is not evidence of a mode. It does not
record prompt text or the environment variable values. The switch changes prompt construction, not permissions or
runtime authority. Real-model behavior with the unified prompt has not been
compared in a paid live qualification, so any quality improvement remains
unverified.

The same event also carries, when non-empty, `unresolved_catalog_count` and
`unresolved_catalog_ids`: the situational/catalog `read_skill` entries this
session's role names that would currently fail to load, from
`catalog_resolution_gaps`. A situational skill is
never loaded eagerly -- a model reads one only once its own stated trigger
arises -- so a broken entry here never blocked and never blocks that session;
it is recorded only as a count and a list of catalog ids, never raw skill
text. This is the advisory counterpart to `portable.roles.composed_
instructions`'s required-inlined-skill guard, which does block session start
when one of the always-included skills fails to load or is empty. The same
resolution is also recorded on the underlying `InstructionBundle` itself (the
`unresolved_catalog_entries` field, populated by both `build_instruction_
bundle` and `capture_instruction_bundle`), alongside the already-existing role
id/kind, source manifest and assembled-text identity fields; it is excluded
from the bundle's `assembled_sha256`/identity the same way `observed_engine_
version` already is, since both describe live install-time state rather than
the intended instructions.

By default, every Cortex engine session, coordinator or worker, on Codex or
Claude, starts from one Cortex-owned base prompt instead of the engine's own.
When the switch above disables it, each adapter uses its documented native
base-prompt path. The unified text is part of the generated instruction data (`scripts/cortex-xtask.sh generate-instructions`),
and the session metadata builder selects the mode.

## Editions

Two editions of the unified prompt exist; both carry the same rules and the
same role-specific variants (coordinator, writing worker, read-only worker,
consultant).

- **Edition 1** is the default and the text described in the rest of this page.
- **Edition 2** is selected only when `CORTEX_UNIFIED_PROMPT_EDITION=2` is set
  in the environment before starting Cortex. Unset or any other value keeps
  edition 1. Like `CORTEX_UNIFIED_BASE_PROMPT`, it is process-scoped and
  affects newly created sessions only; it has no effect when the unified prompt
  is disabled. `cortex_system_prompt` also accepts an explicit `edition`
  argument.

Edition 2 keeps the role introduction, the project-skills list and the
environment section unchanged and reorders everything between them into:

1. **How to work**: carry the assignment to a complete result (the coordinator
   brings the task to a verified result through plans and the final synthesis,
   and the runtime decides completion); make routine choices yourself and record the assumption
   in the report; ask the user, request access or request a re-plan only when
   the goal cannot be reached without it. It also holds the runtime-ownership,
   project-rules-file and context-compaction rules.
2. **Checks**: for a writing worker, run the checks that exercise the change;
   on a failure, fix the cause and rerun the same command, so the final run is
   the result and the history goes in the report. Read-only workers, the
   coordinator and the consultant get their edition-1 evidence rules here.
3. **Writing for your reader**: lead with the outcome in plain words, then what
   it means and what is needed next; no internal ids, codes, route names or
   receipts in what the reader sees unless asked (a worker keeps exact commands
   and receipts in its report evidence and result fields); describe checks by
   their consequence. Workers stay silent between tool calls; coordinators keep
   progress updates to one short, plain line and lead final answers with the result.
4. **Hard limits**: one list covering scope, refused calls, destructive and
   outward-facing actions, other people's changes, unobserved results, secrets,
   untrusted content, subagents, memory and attribution, and the security-help
   policy. Its lead says that the runtime enforces its own permissions and
   refusals and that the other limits depend on the model.
5. **Tools and evidence**: the Cortex tool, schema, native-tool, command and
   `read_skill` mechanics, then the finding-code (or, for the consultant,
   working-material) subsection. Tool argument contracts stay in the advertised
   schemas.

Every edition-1 rule sentence is present verbatim in edition 2 except four
deliberate rewordings (listed and checked by the former Python test suite): the length rule ("at a length that fits the work" instead of
"concisely"), the outcome-first rule, the missing-information rule (ask only
when the gap would materially change the result, otherwise proceed on a stated
assumption) and the writing-worker check rule. Edition 2 duplicates some of the
substance of the inlined `tool-discipline`, `code-and-evidence` and
`report-publication` skills and the shared worker protocol; those are
unchanged.

Rendered on Linux with no working directory or project skills, edition 1 is
5,481 to 7,788 characters and edition 2 is 6,178 to 8,821, depending on role and
native-tool policy. The code-mode variants are 6,069 to 8,240 and 6,797 to 9,304
respectively (measured with the former Python prompt builder on 2026-10-23). Existing size ceilings are
unchanged: they were checked by the former Python test suite. Real-model
behavior with edition 2 has not been observed in a paid live check.

## Working economy and code mode

Both editions give every engine and role the same working-economy rules:

- act once you know enough for the next step; do not re-read a file, report
  page or skill already read in the session unless it may have changed, and do
  not repeat a call that already succeeded;
- a conditional skill's trigger matches when the next action is the one it
  names, not when the skill's topic merely looks related; reading a matching
  skill stays part of the work, once per session and again only after a
  compaction;
- compaction does not end the work: continue from the summarized state without
  redoing completed steps, and recover from the runtime only an exact
  assignment detail or evidence the summary lacks;
- no approval steps, warnings or checklists for hypothetical risks (the
  authorization rules themselves are unchanged);
- routine choices are made and recorded rather than asked about, and the
  coordinator orients only as far as the plan needs, reconciling worker reports
  instead of repeating their work;
- a writing worker widens testing only when new changes, failures or doubts
  justify it, and adds no tests for low-impact, reversible changes or tests that
  only restate the implementation.

Codex's own base prompt states these and Claude kept to them without being
told; once Cortex replaced the Codex prompt, Codex sessions did not. Retained
project-local stores read on 2026-10-22 showed, on the same seeded tasks,
1.9 times as many tool calls for Codex as for Claude (10.6 against 5.5 per
attempt), and over all current-prompt attempts 3.9 times the skill reads per
attempt, 121 `context-compaction` skill reads against 19 observed compactions,
and Codex coordinators reading 3.5 times the reports. The same Codex failed-call
rate (2.6% against Claude's 3.4%) shows argument guessing was not the main cost.
Whether these rules reduce live Codex actions has not been observed in a paid
live check.

Codex sessions render with `code_mode=True`, which swaps three tool lines and
changes nothing else. Code mode lists each nested tool by name and description,
and its `ALL_TOOLS` array returns only those two fields; in 150 retained Codex
rollouts 10% of `exec` calls only queried it, 57% carried at most one tool call
and 9% used `Promise.all`. The code-mode lines therefore say not to query
`ALL_TOOLS` for a listed tool, to take Cortex arguments from the
[tool cheat sheet](../codex-engine/index.md#mcp-bridge) and correct a failed
validation once from the error, and to put independent calls in one `exec` with
`await Promise.allSettled([...])`; the generic "read the full schema first" and
tool-search lines are omitted. Because the cheat sheet shows field names, types
and enums but not bounds or `if`/`then` conditions, the code-mode line adds that
a session fetches one tool's exact record with `tools_discover`, when offered,
only when such a constraint matters. The inlined `tool-discipline` skill states
the same code-mode rule, so it no longer tells Codex to read every schema first.
Claude and OpenCode prompts never contain these code-mode lines.

## Why

Each engine's native base prompt is written for its own product:

- Claude Code's preset covers terminal markdown output, native skills invoked
  as `/name` through its `Skill` tool, a file memory under the Claude config
  directory, commit and pull-request attribution, and preferring its own file
  tools over the shell.
- Codex's base prompt covers `commentary`/`final` channels, its own skill
  catalog read from `SKILL.md` files, visualizations, and "do not add or run
  tests unless the user asks".

Cortex sessions are neither product. With the native prompts in place, the same
role instructions produced different behavior: observed Codex workers read the
Cortex skills and used the code index, while Claude workers mostly did not, and
parts of both native prompts contradicted the runtime contract.

## What the prompt says

The unified base prompt composes four distinct role blocks over shared authority
and safety rules: coordinator, writing worker, read-only worker and consultant. Worker selection uses the runtime `READ_ONLY`
contract, tested for exact equality with `profiles.json` access classes, with
consultant aliases resolved separately. Coordinators receive no file-edit or
worker command-receipt obligations; read-only workers receive only non-mutating
check guidance; consultants evaluate the packet and retained reports.

The prompt names only the tools each role is offered. The attached-server section for
a read-only worker lists only the read-only tools that its external proxy will accept,
so it omits index-writing tools such as `index_repository`. A server whose configured
grant path is missing is not advertised at all; its absence is logged as an
`external_mcp_server_path_missing` diagnostic. The base prompt's worked example of the
`read_skill` convention is chosen per role, and names only a skill that role can load.
The "ask through the question tool" line is per role: the consultant has no question
tool, so it records a missing fact in its report as a limitation and asks the user
nothing.

Native-tool guidance is emitted only when the launch adapter enables those
tools: Claude's gateway-alternative policy disables native shell/file/search
and omits that guidance, while Codex and OpenCode retain it (`native_tools=True`
in `_session_new_meta_for`). For Codex this is still
accurate, not stale: `shell_tool` and `view_image` are now forced off for
every Cortex-launched Codex session (the per-process launcher and the
verified private adapter's session-config patch both set them, verified
against the installed Codex CLI 0.154.0's `features list`), but native
`apply_patch` remains Codex's own file-edit tool with no supported off switch,
so the generic native-tool caveat (local workspace aid, never a report or a
result) still applies to it. A separate `unified_exec` feature has no supported
off switch either (neither `-c features.unified_exec=false` nor `--disable
unified_exec` changes its reported `true` state), but with the launcher's flags
the local CLI inventory (0.166.0) declares neither `exec_command` nor
`write_stdin`; a custom launch command and live sessions are not covered (see
[Codex shell tool inventory](../codex-engine/index.md#codex-shell-tool-inventory-and-unified_exec)). This changes prompt construction, not tool authority, sandbox
permissions or routing. The consultant's prohibition on using remaining
native tools appears only where those tools remain enabled. Consultant route
selection follows normal policy; the same `apply_patch`/`unified_exec`
limitation above applies to a Codex consultant session. Defaults on the standalone renderer preserve
ordinary writing-worker callers; runtime launch seams supply the actual policy.

In edition 1: a role-specific introduction (coordinator or worker), then sections on
authority, tools, Cortex skills, finding code, changing the project, evidence,
safety, communication and context, then the project's own skills, and finally
a runtime-generated environment section (working directory, whether it is a Git
repository, platform, UTC date). The prompt names no engine or product. Tool
argument contracts stay in each tool's advertised schema; role duties stay in
the role instructions that follow the prompt. The authority section ranks project
rules files below runtime scope, permissions and lifecycle, and the same
precedence covers anything the host engine adds, whatever its framing (see
[What each engine still adds](#what-each-engine-still-adds)). The skills
section points to the [project skills](#project-skills) list only when that
list is rendered. Refusal routing is role-aware:
workers use `access_request` first for additional project paths and the question
tool for decisions reserved to the user; scope/replan requests are for changes
to the plan itself. Coordinators plan refused work for a worker, ask the user
when only the user can decide, or report a blocker. These prompt directions do
not change tool authority or make a refused operation available.

Both engines' native skill catalogs are switched off, so Cortex lists the
project skills either engine would have found by itself, for every engine (see
[Project skills](#project-skills); user-level directories are not listed). Where each
engine looks was observed on 2026-09-28 with a probe skill in every candidate
directory and each installed engine pointed at a local stub endpoint:

| Directory | Claude Code 0.81.2 | Codex CLI 0.155.1 |
|---|---|---|
| `<project>/.claude/skills` | read | not read |
| `<project>/.agents/skills` | not read | read |
| `<project>/.codex/skills` | not read | read |
| `~/.claude/skills` | read | not read |
| `~/.agents/skills` | not read | read |
| `$CODEX_HOME/skills` (default `~/.codex/skills`) | not read | read |
| `<project>/skills`, nested project directories | not read | not read |

### Project skills

The project skill directories are `.agents/skills`, `.claude/skills` and
`.codex/skills` below the project root, looked up in that order. A project skill
is a regular, single-link `<dir>/<name>/SKILL.md` of at most 65,536 bytes of
UTF-8 text. Hidden names are skipped, and Cortex never follows a symbolic link
(including the `.claude/skills/<name>` link that `project_skill_link` creates; the
real `.agents/skills/<name>` directory is found by name). User-level skill
directories in the owner's home are neither listed nor read.

At session start, every channel that is served `project_read` (the coordinator
and every specialist; not the senior consultant) receives a `## Project skills`
section after the role instructions listing up to 64 project skills by name,
path and description, marked as project content that grants no tool access,
scope or permission. The first directory wins for a duplicated name. A project
skill whose name equals a Cortex shared/reference id is listed by path only.
The description is the top-level `description` frontmatter value (plain, quoted
or `|`/`>` block form with indented continuation lines, whitespace collapsed),
read from the first 16 KiB and capped at 300 characters; anything else yields
no description. This is not a YAML parser. The section is omitted when the
project has no project skills.

Cortex MCP `read_skill` reads a Cortex shared/reference id for the channel's
role, or a project skill by its name, its directory (`.agents/skills/<name>`)
or its file (`.agents/skills/<name>/SKILL.md`), relative to the project or as
an absolute path inside it. The accepted forms are stated in the tool's
advertised schema. Cortex ids win: a Cortex id always returns the Cortex body and
keeps its role restriction, so a project file cannot replace or unlock a Cortex
protocol skill; a same-named project file is read only by its path. Path forms
are checked with the same rules as `project_read` (no empty, `.` or `..` parts,
no backslashes, no `.git`, `.cortex`, `.cortex-dev` or workspace owner files)
and are read by descriptor traversal without following links. A project skill
reply carries `source: "project"`, its path, SHA-256, size and a note that the
text is project content, not runtime instruction. Reading a project skill grants
nothing beyond reading that file, which the same channel can already do with
`project_read`; channels without `project_read` cannot read project skills.

A skill that cannot be read is an ordinary tool error (`isError`), not an
authority refusal. It carries a `code` (`skill_unknown`, `skill_not_for_role`,
`project_skill_path_refused`, `project_skill_outside_project`,
`project_skill_location_unrecognised`, `project_skill_missing`,
`project_skill_symlink_refused`, `project_skill_not_regular`,
`project_skill_too_large`, `project_skill_not_text`,
`project_skills_unavailable` or `invalid_arguments`), the requested value, and a
message that lists the accepted forms and this role's Cortex ids and, for
channels with project file access, points to `project_read` for other project
files. Channel authentication failures remain authority refusals.

When `codebase-memory-mcp` is attached, the prompt tells the session to call
`list_projects` once and take the project whose root path matches the working
directory. The runtime does not pass a pre-resolved project name
(`session_manager` sends none), so no project identity is guessed; the
inlined `code-and-evidence` reference gives the same first step. Only a role
granted `index_repository` is told to make one bootstrap attempt when no index
matches; other roles record the missing index as a limitation.

Writing-worker command guidance distinguishes ordinary Development host
capability from task authorization: deletion, production/shared mutations,
publication, credential changes and paid checks still require explicit authority.
The recovery capsule follows the effective task mode without broadening read-only
roles. Docker and arbitrary owner code can bypass filesystem masks; the prompt
never promises hostile-code isolation. See [permissions](../permissions-and-containment/index.md#access-mode-development-guarded-and-auto).

The senior-consultant prompt is a separate worker variant: it is limited to the
consultation packet and retained reports, and does not advertise code-index or
project-source investigation. No external MCP servers are attached to consultant
sessions; `project_read`, `project_git` and `command_run` are not offered, and
project-source calls through those gateway routes are refused with
`gateway_read_denied`. The consultant has no `ask_question`, `access_request`,
`replan_request`, `scope_request` or `escalation_request` route. Its role text
states English-only communication; its turns and brief name
`user_question_language` like every worker's, which has no effect without a
question tool.

Every worker prompt is the instruction bundle, the recovery capsule and the
assignment brief, in that order (`SessionDriver::prompt`). The capsule is complete
in the first prompt of each context generation and a one-line `read_capsule`
reference after that (see [context generations](../context-generations/index.md#delivery-ledger)). The brief lists the
objective, scope, acceptance (the plan's verification commands), the owner's
constraints, every predecessor report id with its role, title, size and page count,
the runtime-selected report skeleton and `user_question_language`; see
[roles and delegation](../roles-and-delegation/index.md). Each prompt is recorded
as a `NATIVE_ATTEMPT_DELIVERY` event with the sha256 and size of the bundle, the
capsule and the brief and the served tool catalogue's sha256; the attempt's first
delivery also fills `delegation_provisioning.instruction_delivery` and mirrors the
worker's `NATIVE_ROUTE_SELECTED` decision (engine, model, connection, parameters
including effort, decision basis) into `attempt_route_audit`. Only digests are
stored, never the text.
Claude's consultant profile disallows native file/search/edit tools. Codex native reads remain available and may run without a permission
request inside the read-only engine jail, so this is not a complete native-source
access boundary.

Workers are told to work without narration, including before the first tool and after tool results. The runtime also discards worker prose at tool boundaries from the public worker card and its replay history, retaining only the post-tool terminal response; underlying engine events remain available for evidence. Coordinator text still streams because it is the user's task view, except
that the runtime withholds the coordinator's text after its own accepted user
question in the same turn (see
[orchestration](../orchestration/index.md#plan-submission-and-coordinator-polling)).
The coordinator's one role-specific communication rule is to write the details
once before that question and write nothing after it. This is a presentation boundary, not a claim that either model stops generating interim text. A direct 2026-09-28 re-check counted ten between-tool Claude assistant text blocks and one Codex block; an earlier count of 83 Codex blocks conflated reasoning records with visible messages. Communication guidance also asks for one short progress line and a brief outcome-first final answer; unresolved checks go only in the final-answer tool's `not_verified` list, and successful checks and internal approvals stay out of the final text.

The attached-server section renders configured granted tool names under each
attached MCP server name; it does not prove engine discovery or invocation. Cortex MCP owns lifecycle, report reads/publication, conditional
skills, scoped project operations, command receipts and result submissions; it
does not provide code-search, graph or index operations. External MCP servers
own their own domain tools (for example `codebase-memory-mcp` owns its graph and
search operations). Use a tool only from the server that advertises it; do not
invent a Cortex search tool or assume a tool belongs to another server. The
prompt names the `mcp__<server>__<tool>` form when the engine exposes that
naming convention, and directs workers to use engine tool search for an attached
server tool absent from their available list. Codex code mode already lists
every attached tool, so its sessions do not receive that line.

For structural code questions (definitions, callers, dependencies, impact and
architecture), use an available code index first and inspect the pointed-to
source. For literal strings, configuration and documentation, use text search;
when the index search tool is granted, the prompt names its owning server, and
known-file reads use Cortex `project_read`. When no native shell/file search is
available, use the granted external `search_code` or a known-file
`project_read`, not an unavailable Cortex search operation. Follow the
code-index tool's own coverage and pagination guidance; a missing result is not
proof of absence. Tool argument contracts remain in live schemas, not this
prompt.

The role instructions that follow the prompt carry the everyday Cortex skills in
full (see [roles and delegation](../roles-and-delegation/index.md)); the prompt tells the
model to follow them from its first action and, before an action matching a
situational trigger, to call Cortex MCP `read_skill` with that id. The prompt
gives role-specific examples: documentation-impact planning and `worker-routing`
for the coordinator, affected documentation edits and interactive sessions for
writing workers, documentation-impact assessment and interactive sessions for
read-only workers, and `context-compaction` and `index` for the consultant. It
explicitly says not to re-read inlined skills or pre-load unmatched skills. The
same conditional catalogue is also in the role-scoped `read_skill` tool
description and `skill` selector description; this makes the trigger visible
at tool-selection time rather than only near the end of a long role prompt.
Neither prompt nor schema calls the tool on the model's behalf, so actual
Claude/Codex trigger compliance still needs a true-trigger live observation.
The native runtime observes neither skill reads nor per-attempt changes, so it enforces no situational trigger at `result_submit`; the trigger text above and the independent review are the only controls (see [roles and delegation](../roles-and-delegation/index.md)).
In the 2026-09-28 CLI run the Claude backend worker wrote code and tests, not
documentation. A Codex technical writer updated the README and read
`documentation-sync`. The Claude coordinator planned that writer; the earlier
"before editing" trigger did not cover planning. Zero Claude skill reads in
that run therefore do not demonstrate a missed Claude worker documentation
trigger. The catalogue now includes documentation-impact planning; delivery
tests do not establish live model compliance.

After the role instructions, a session that receives external MCP servers gets an
"Attached MCP servers" section built when the session starts
(`attached_servers_section`), the same for every engine. It names the attached
servers; only when the role's effective grants include `list_projects`,
`search_graph`, `trace_path` and `get_code_snippet` does it give that full
procedure. A narrower grant is named as such, without recommending ungranted
tools. Workers granted `index_repository` are told to make one authorized
bootstrap attempt when the exact workspace has no index; coordinators instead
ask an authorized worker to bootstrap. A conditional "prefer the
index when available" was not enough: on 2026-09-28 Claude sessions with the index
attached and its lookup tools loaded still searched source with the shell, and the
project lookup tool they would have needed first was deferred.

Coordinators also receive a separate
[configured-worker capability summary](../roles-and-delegation/index.md#capability-based-delegation),
including when they have no external attachments themselves. It lists only
bounded sanitized server/tool names by engine filter and access class, not
schemas or private launch data. Worker-only grants remain discovery hints, not
coordinator tool offers. Unavailable inventory and omissions remain explicit.
This uses instruction metadata for Codex/Claude and the launch prompt for
OpenCode, without changing channel authority or runtime-owned worker dispatch.
The same section now includes a bounded built-in Cortex discovery roster derived
from current role tool offers, independently of external attachments. Shared
ordinary-worker guidance distinguishes command namespaces from the host:
read-only/Guarded `/var/tmp` is private scratch, and missing there is not host
absence. Authorized host-only reads/searches use a granted external reader or
the advertised outside-command consent route (writing workers only), not remounts
or denial bypass. It also covers project-only path reservations, installed
host-history/export discovery, exact-session identity and explicit completeness;
the consultant remains packet-only.

## Historical delivery (former ACP adapter)

| | Claude (claude-agent-acp) | Codex (codex-acp, private adapter) |
|---|---|---|
| Base prompt | Plain-string `_meta.systemPrompt`: the unified prompt, a blank line, then the role instructions. Replaces the `claude_code` preset. | `_meta.cortexBaseInstructions` → `thread/start.baseInstructions`. Replaces the model's base prompt. |
| Role instructions | Same string, after the base prompt | `_meta.systemPrompt` → `thread/start.developerInstructions`, after any native `developer_instructions` from the user's Codex config |
| Native additions switched off | `skills: []` (native skill listing), `settings.attribution` with empty `commit`/`pr`, `settings.includeGitInstructions: false` | `cortexNativeContext: false`: `include_permissions_instructions`, `include_apps_instructions`, `include_collaboration_mode_instructions`, `include_environment_context` and `skills.include_instructions` set to `false` in that thread's config |

The base and role text remain identical across engines. Claude alone uses its
adapter's `disallowedTools` option to disallow native shell, file/search, notebook
edit and `Skill` alternatives, in addition to native subagents. Cortex MCP file,
command and skill tools and granted external MCP tools remain available. This
narrows tool choice; it does not prove that a model will follow every situational
skill or code-index instruction. Codex tool policy is unchanged.

Claude's `managedSettings` layer drops `attribution`; it works only through the
`settings` layer. See the [Codex adapter](../codex-engine/index.md) for the private
adapter fields.

The tested OpenCode 2.0.19 build ignores `session/new` `_meta.systemPrompt`; Cortex uses its launch-time agent prompt for all supported 2.x releases.
Its whole prompt is the prompt of the `cortex` agent Cortex configures at process
launch, a read-only file mounted into the jail. In order, it holds:

1. the unified base prompt;
2. a note that MCP tools, `cortex` included, are called inside its `execute` tool;
3. the role instructions;
4. the project's `AGENTS.md`/`CLAUDE.md`, which OpenCode stops loading once
   project configuration is off;
5. the attached-MCP-servers note.

Observed through a loopback fake provider, OpenCode appends an environment block
and its Code Mode tool catalog after that prompt. See the
[OpenCode adapter](../opencode-engine/index.md#role-instructions).

## What each engine still adds

Observed on 2026-09-28 by pointing each installed engine (claude-agent-acp
0.81.2, Codex CLI 0.155.1 through codex-acp 1.13.0) at a local stub endpoint
that recorded the model request and refused it, with an isolated home and no
model call:

- Claude: a billing header line, the single identity line "You are a Claude
  agent, built on Anthropic's Claude Agent SDK.", and a short environment
  message (working directory, platform, model, date). This observation predates
  the adapter's native-alternative restrictions; recheck the actual manifest
  before claiming what tools a newer live session receives.
- Codex: its native tool list (the code-mode `exec` tool and its nested tools).

Both engines still load the project's rules files (`AGENTS.md`, `CLAUDE.md`) as
before; Cortex still appends them for Claude when Claude does not trust the
folder. Under both engines the Cortex text reached the model byte-for-byte the
same. A real engine home can add the user's own rules files, native
`developer_instructions` and similar configuration, which this stub run did not
include.

A live claude-agent-acp 0.88.0 worker session on 2026-10-23, run with the
owner's real home, additionally received: the project's `AGENTS.md` wrapped in
Claude's own framing that it "OVERRIDE[s] any default behavior", the owner's
auto-memory index (`MEMORY.md`), a user-account context block, and a list of
deferred native tools with a web-search usage paragraph. No supported setting to
turn these off has been verified for that adapter version. The base prompt's
authority section therefore states that runtime scope, permissions and
lifecycle take precedence over anything the host engine adds, whatever its
framing; this is guidance, not enforcement.

## Not yet verified

One 2026-09-28 isolated CLI comparison did observe Claude using native Bash,
Read and Edit despite receiving the same Cortex prompt and full MCP schemas;
The effect of the
subsequent native-alternative restriction has not been observed in a real model
turn. That needs a paid live check, which runs only on an explicit owner request
naming `cli` or `desktop`.
