# Unified system prompt

> Status: migrated from the former ACP reference. Statements were not re-verified line by line against source in this pass; claims without a source path are unverified until reviewed.


The unified Cortex base prompt is enabled by default. Set
`CORTEX_UNIFIED_BASE_PROMPT=0` in the environment before starting Cortex to
disable it for newly created session metadata. This restores the engines'
pre-existing native base-prompt path; it does not roll back unrelated runtime
optimizations or routing changes. After sent session metadata is recorded, Cortex best-effort records a secret-free
`base_prompt` observability event; the diagnostics summary counts observed
`unified`, `native` and `unknown` modes, per engine as well as overall
(`counts`/`counts_by_engine` in `observability.metrics._base_prompt`). `native`
means the session's engine kept its own native system-prompt/instructions
channel and Cortex's text was appended alongside it; `unified` means that
native channel was replaced outright by Cortex's own base-prompt text; this is
the same per-engine classification `delegation.live_adapters.base_prompt_mode`
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
`portable.instruction_bundle.catalog_resolution_gaps`. A situational skill is
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
base-prompt path. The unified text lives in
`src/cortex_runtime/engine/unified_system_prompt.py` (`cortex_system_prompt`),
and `delegation.live_adapters._session_new_meta_for` selects the mode when
building session metadata.

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

1. **How to work**: carry the assignment (or, for the coordinator, the task) to
   a complete result; make routine choices yourself and record the assumption
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
deliberate rewordings, which `tests/test_unified_system_prompt.py` lists and
checks: the length rule ("at a length that fits the work" instead of
"concisely"), the outcome-first rule, the missing-information rule (ask only
when the gap would materially change the result, otherwise proceed on a stated
assumption) and the writing-worker check rule. Edition 2 duplicates some of the
substance of the inlined `tool-discipline`, `code-and-evidence` and
`report-publication` skills and the shared worker protocol; those are
unchanged.

Rendered on one Linux host with no working directory and no project skills,
edition 1 is 6,151 to 7,703 characters and edition 2 is 7,185 to 9,568
characters, depending on role and native-tool policy. Size ceilings for edition 1 are in `tests/test_prompt_drift.py`; for
edition 2 they are in `tests/test_first_prompt_size_and_dedup.py`. Real-model
behavior with edition 2 has not been observed in a paid live check.

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
in `delegation.live_adapters._session_new_meta_for`). For Codex this is still
accurate, not stale: `shell_tool` and `view_image` are now forced off for
every Cortex-launched Codex session (the per-process launcher and the
verified private adapter's session-config patch both set them, verified
against the installed Codex CLI 0.154.0's `features list`), but native
`apply_patch` remains Codex's own file-edit tool with no supported off switch,
so the generic native-tool caveat (local workspace aid, never a report or a
result) still applies to it. A separate `unified_exec` feature was found to
have no supported off switch either (neither `-c features.unified_exec=false`
nor `--disable unified_exec` changed its reported `true` state against the
installed binary); whether it still offers Codex a residual native exec/read
path once `shell_tool` is off is not established without a live qualification
check. This changes prompt construction, not tool authority, sandbox
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
the role instructions that follow the prompt. Refusal routing is role-aware:
workers use `access_request` first for additional project paths and the question
tool for decisions reserved to the user; scope/replan requests are for changes
to the plan itself. Coordinators plan refused work for a worker, ask the user
when only the user can decide, or report a blocker. These prompt directions do
not change tool authority or make a refused operation available.

Both engines' native skill catalogs are switched off, so Cortex lists the
skills either engine would have found by itself, for every engine. Where each
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

The prompt catalog reads only bounded top-level `name` and `description`
frontmatter scalars; it is not a general YAML parser. The reader accepts plain,
single-quoted and JSON-compatible double-quoted scalars, including single- and
double-quoted scalars continued across indented physical lines, indented plain
continuations, and literal (`|`) or folded (`>`) block scalars with optional
`+`/`-` chomping and explicit indentation indicators. Continued quoted
scalars follow the reader's bounded folding rules; this is not general YAML
parsing. Frontmatter reads are limited to 16 KiB, the closing delimiter must be
an exact `---` line, and the rendered description is capped at 1024 characters.
Unsupported node types and malformed or incomplete frontmatter produce an
empty field rather than being interpreted as general YAML. No YAML package is
required.

Cortex lists bounded `<dir>/<name>/SKILL.md` files from the configured
project and user skill roots (project roots: `.agents/skills`, `.claude/skills`,
`.codex/skills`; user roots: `.agents/skills`, `.claude/skills`). Entries are
filtered to regular, single-link files inside an allowed root; hidden names,
duplicates and paths beyond the path-length limit are excluded. File size is
capped at 65,536 bytes, frontmatter inspection at 16 KiB and rendered
`description` at 1,024 characters. Project paths are relative and user paths are
absolute. The matching description and path appear in the prompt; Cortex does
not paste the full skill body there.

The read route is Cortex MCP `read_skill`, not an engine-native general file
read: a session may request only an exact listed skill path bound to that
session's sent prompt and attempt, or a supported shared skill/reference ID.
Project paths are relative and user paths absolute as listed. The runtime reads
the original canonical `SKILL.md` as a bounded UTF-8 regular file without
following symlinks, and fails closed if the listed target changes. It grants no
general file access, globs, sibling files or arbitrary paths; sessions without a
matching catalog snapshot cannot use the path form. Native catalogs are disabled
for the unified prompt, so Cortex's listing and this explicit read route form
the supported skill-discovery flow.

Channel issuance happens before the engine session exists. After session open,
`EngineSessionManagerAdapter._open_generation` snapshots the catalog from
observed sent metadata and binds the launch's exact channel/task/attempt to its
generation. Codex grants use `cortexBaseInstructions`, Claude uses
`systemPrompt`, and OpenCode uses the observed process launch prompt.
Reconstructed metadata is not sufficient to grant skill paths. Prompt listing
and snapshot enumeration use the same adapter environment, including `HOME`
and `CODEX_HOME`; a changed catalog/environment that no longer matches the
sent section yields no grant.

Reads select the current persisted channel binding, since an MCP server may
cache its channel context during `tools/list` before session open completes.
Only the newest generation of that bound session is eligible. Compaction
advances the channel's generation without re-enumerating the catalog or granting
late-added paths. Termination removes that session's snapshots even if process
exit is unconfirmed. A channel cannot be rebound to a different engine session;
worker route fallback and coordinator recovery issue fresh channels and receive
their own catalogs. Cross-task/attempt grants, revoked or expired channels and
superseded persisted generations fail closed.

When `codebase-memory-mcp` is attached and Cortex can resolve the project
root to a known indexed-project name, the prompt names that project so workers
can use it directly instead of repeating `list_projects`. If resolution fails or
the named project is unavailable, workers use the normal project-list fallback;
no project identity is guessed.

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
`gateway_read_denied`. The consultant has no `ask_question` or `access_request` route.
Claude's consultant profile disallows native file/search/edit tools. Codex native reads remain available and may run without a permission
request inside the read-only engine jail, so this is not a complete native-source
access boundary.

Workers are told to work without narration, including before the first tool and after tool results. The runtime also discards worker prose at tool boundaries from the public worker card and its replay history, retaining only the post-tool terminal response; underlying engine events remain available for evidence. Coordinator text still streams because it is the user's task view, except
that the runtime withholds the coordinator's text after its own accepted user
question in the same turn (see
[orchestration](../orchestration/index.md#plan-submission-and-coordinator-polling)).
The coordinator's one role-specific communication rule is to write the details
once before that question and write nothing after it. This is a presentation boundary, not a claim that either model stops generating interim text. A direct 2026-09-28 re-check counted ten between-tool Claude assistant text blocks and one Codex block; an earlier count of 83 Codex blocks conflated reasoning records with visible messages. Communication guidance also asks for one short progress line, a brief outcome-first final answer, and only unresolved user-relevant checks; successful checks and internal approvals stay out of the final text.

The attached-server section renders configured granted tool names under each
attached MCP server name; it does not prove engine discovery or invocation. Cortex MCP owns lifecycle, report reads/publication, conditional
skills, scoped project operations, command receipts and result submissions; it
does not provide code-search, graph or index operations. External MCP servers
own their own domain tools (for example `codebase-memory-mcp` owns its graph and
search operations). Use a tool only from the server that advertises it; do not
invent a Cortex search tool or assume a tool belongs to another server. The
prompt names the `mcp__<server>__<tool>` form when the engine exposes that
naming convention, and directs workers to use engine tool search for an attached
server tool absent from their available list.

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
gives concrete examples for documentation-impact planning, affected documentation
edits and interactive sessions and
explicitly says not to re-read inlined skills or pre-load unmatched skills. The
same conditional catalogue is also in the role-scoped `read_skill` tool
description and `skill` selector description; this makes the trigger visible
at tool-selection time rather than only near the end of a long role prompt.
Neither prompt nor schema calls the tool on the model's behalf, so actual
Claude/Codex trigger compliance still needs a true-trigger live observation.
For triggers the runtime can observe, it no longer relies on the prompt alone:
a worker whose project changes pair documentation with behavior files and that
never read `documentation-sync` is refused at `result_submit` with
`situational_skill_required` until it reads the skill (see
[roles and delegation](../roles-and-delegation/index.md)). The `result_submit` and
`read_skill` tool descriptions state this gate.
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

## Delivery

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

## Not yet verified

One 2026-09-28 isolated CLI comparison did observe Claude using native Bash,
Read and Edit despite receiving the same Cortex prompt and full MCP schemas;
The effect of the
subsequent native-alternative restriction has not been observed in a real model
turn. That needs a paid live check, which runs only on an explicit owner request
naming `cli` or `desktop`.
