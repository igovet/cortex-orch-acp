# Unified system prompt

Every Cortex engine session, coordinator or worker, on Codex or Claude, starts
from one Cortex-owned base prompt instead of the engine's own. The text lives in
`src/cortex_runtime/engine/unified_system_prompt.py` (`cortex_system_prompt`),
and `delegation.live_adapters._session_new_meta_for` builds it for both
engines at session creation.

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

A role-specific introduction (coordinator or worker), then sections on
authority, tools, Cortex skills, finding code, changing the project, evidence,
safety, communication and context, then the project's own skills, and finally
a runtime-generated environment section (working directory, whether it is a Git
repository, platform, UTC date). The prompt names no engine or product. Tool
argument contracts stay in each tool's advertised schema; role duties stay in
the role instructions that follow the prompt.

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

Cortex lists every `<dir>/<name>/SKILL.md` below all six read directories,
project first, with its frontmatter `name` (or the directory name) and
`description` (cut at the Agent Skills limit of 1024 characters) and its path:
relative for project skills, absolute for the owner's. There is no limit on the
number of skills. Hidden entries such as Codex's bundled `.system` set are
skipped. A symlinked directory or file counts only when it resolves inside the
same project, home or `$CODEX_HOME` directory, and one real `SKILL.md` is listed
once however many links reach it (a `.claude/skills` link to `.agents/skills`
appears once). The model reads a matching `SKILL.md` with its ordinary file read
tool; the engine jail keeps the host, including the owner's home, readable. The
listing is content placed in the system prompt, as each engine's own catalog was
before.

Workers are told to work without narration, including before the first tool and after tool results. The runtime also discards worker prose at tool boundaries from the public worker card and its replay history, retaining only the post-tool terminal response; underlying engine events remain available for evidence. Coordinator text still streams because it is the user's task view. This is a presentation boundary, not a claim that either model stops generating interim text. A direct 2026-09-28 re-check counted ten between-tool Claude assistant text blocks and one Codex block; an earlier count of 83 Codex blocks conflated reasoning records with visible messages.

The tool section distinguishes three sources: Cortex MCP owns retained reports,
conditional skills, scoped project operations, report publication, worker check
receipts and lifecycle submissions; attached external MCP servers such as
`codebase-memory-mcp` own their domain queries; engine-native workspace tools
cannot supply Cortex publication or passed-check receipts. Argument shapes stay
in the live tool schemas, not in this prompt.

The role instructions that follow the prompt carry the everyday Cortex skills in
full (see [roles and delegation](roles-and-delegation.md)); the prompt tells the
model to follow them from its first action and, before an action matching a
situational trigger, to call Cortex MCP `read_skill` with that id. The prompt
gives concrete examples for documentation-impact planning, affected documentation
edits and interactive sessions and
explicitly says not to re-read inlined skills or pre-load unmatched skills.
The same conditional catalogue is also in the role-scoped `read_skill` tool
description and `skill` selector description; this makes the trigger visible
at tool-selection time rather than only near the end of a long role prompt.
Neither prompt nor schema calls the tool on the model's behalf, so actual
Claude/Codex trigger compliance still needs a true-trigger live observation.
For triggers the runtime can observe, it no longer relies on the prompt alone:
a worker whose project changes pair documentation with behavior files and that
never read `documentation-sync` is refused at `result_submit` with
`situational_skill_required` until it reads the skill (see
[roles and delegation](roles-and-delegation.md)). The `result_submit` and
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
`settings` layer. See the [Codex adapter](codex-acp-adapter.md) for the private
adapter fields.

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
see the investigation report in task `t_388eb2ac3020`. The effect of the
subsequent native-alternative restriction has not been observed in a real model
turn. That needs a paid live check, which runs only on an explicit owner request
naming `cli` or `desktop`.
