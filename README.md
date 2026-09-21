<table>
  <tr>
    <td width="190" align="center" valign="middle">
      <img src="./assets/logo.png" alt="Cortex logo" width="156" />
    </td>
    <td valign="middle">
      <h1>Cortex</h1>
      <p><strong>Multi-agent coordination for software work, as an ACP agent.</strong></p>
      <p>
        You talk to Cortex in your editor. Cortex decides what to delegate,
        runs other coding agents (codex-acp, claude-agent-acp) as workers in
        the exact project opened in your editor, serializes writers, checks
        what they did, and records it. Tasks,
        one Markdown pipeline per task, and reports are kept on disk next to
        your project.
      </p>
      <p>
        <img src="https://img.shields.io/badge/Cortex-1.0.0-7c3aed" alt="Cortex 1.0.0" />
        <img src="https://img.shields.io/badge/Python-3.11%2B-3776ab" alt="Python 3.11+" />
        <img src="https://img.shields.io/badge/Protocol-ACP%20v1-111827" alt="Agent Client Protocol v1" />
        <img src="https://img.shields.io/badge/Storage-Markdown%20%2B%20SQLite-0f766e" alt="Markdown files and SQLite metadata" />
      </p>
    </td>
  </tr>
</table>

## What this is

Cortex is an ACP agent: an editor such as Zed or Paseo launches it and talks to it over
stdio, exactly as it would any other coding agent. Cortex is also an ACP
*client* — it can launch pinned codex-acp and claude-agent-acp engines for
coordination and worker sessions.

For Codex, Cortex verifies the installed codex-acp 1.12.0 bundle and creates
an owner-private patched copy automatically when it launches the engine. The
copy forwards Cortex role instructions to Codex App Server's
`thread/start.developerInstructions` field, preserving effective native
developer instructions instead of putting the role into the user message. It
also preserves Codex custom-tool lifecycle events for the ACP feed. The
upstream install is left unchanged; no manual package patch or separate
account is needed. The same verified copy sets disabled native-subagent
options in each App Server session config, since its thread-level `features`
table otherwise overrides the process flags. Native Codex compaction persistence remains unqualified; see the
[Codex adapter](docs/acp/codex-acp-adapter.md) for the tested wire path and
the remaining live model-visibility limit.

That double role is the whole design. The model you talk to coordinates and
delegated workers do the implementation in the exact `project_root` opened by
the ACP client. Cortex serializes mutating worker turns, records writer ownership and
before/after revisions, and refuses stale or overlapping writes. There is no
hidden project copy treated as the destination of the user's work.
If a worker edits while the coordinator is still speaking, Cortex waits for
that worker's exact published revision before deciding whether the observed
project change is accounted for. An unresolved change remains blocking across
a paused or restarted turn.

Nothing activates by itself. Cortex runs when you select it in your editor.
An explicit `/harvest-refresh` rebuilds the project inventory and updates
`docs/project/` and `docs/features/` from current source. When behavior,
defaults or output changed, its writer inventories affected executable examples,
compares safe runs' exact exit status and stdout with documented results, and
records those it cannot verify. Visible UI and other nontrivial review work
defaults to an independent reviewer; a concrete recorded low-risk exception
may use one writer. Coordinator and implementer checks are not independent.
These are model instructions, not a runtime proof that every example is
correct.

## Where everything lives

| What | Where | Edited by |
| --- | --- | --- |
| Agent entry point | `src/cortex_acp.py` in this repository | not edited; referenced from your editor |
| Editor registration | your editor's settings (`agent_servers` in Zed) | you |
| Engine connections | `~/.config/cortex/connections.json` | Cortex, on first run; `scripts/cortex_acp_setup.py` for the rest |
| Connection secrets (API-key connections only) | `~/.config/cortex/secrets/<connection id>` | setup tooling only |
| Engine credentials | your own native directories, `~/.codex` and `~/.claude` | the engine's own CLI (`codex`, `claude`) |
| Engine installs | wherever your editor put them (Zed: `~/.local/share/zed/external_agents/`) | your editor |
| Route + effort policy | `~/.config/cortex/route-policy.json` | you, by hand |
| Your defaults | `~/.config/cortex/defaults.json` | the model/effort selectors in the editor |
| One project's tasks | `<project>/.cortex/` | Cortex |

A project's `.cortex/` holds `cortex.sqlite3` (tasks, events, audit),
`pipeline-drafts/` and `draft-reports/` (Markdown being written), and durable
writer/check evidence. It is Cortex's own storage, not your source: keep it
out of version control if you do not want task history in your repository.
When Cortex opens an existing format-11 store through its normal writable
runtime path, it automatically upgrades it to format 12 after creating and
verifying a private backup under `.migration-backups/` while holding the
exclusive storage lock. Inspection opens use `initialize=False` and remain
non-mutating, reporting that migration is required. Format 10 and unknown
formats remain fail-closed and require the explicit migration procedure.
For sessions recorded by the current runtime, clients supporting ACP `session/load` can reload text and
worker/tool cards from this store after an editor restart. In Paseo 0.8, the
agent-scoped composer Workers pill opens a bounded Workers popover for the
current ACP session and task; the pill remains available with no current
worker assignments and the popover shows an empty state. Selecting a worker records its assignment
reference before opening the Workers workspace tab, so the exact worker is
selected even when that tab was closed and must be created again. The panel
reads redacted `worker_activity` history from durable `task_events` with cursor
paging. Worker cards and the Workers view use the same role label as the
coordinator chat (for example, `explorer`); internal `delegate:` references
remain lookup keys rather than display names.
Sessions created
before structured update recording can reload their text only; see the
[replay behavior](docs/acp/input-history-and-replay.md).
When the native Paseo provider receives a tool update with status `failed` and
no output, its supported Paseo 0.8 transformer supplies a bounded non-null
display message while preserving `failed`. It does not fabricate a cause or
claim that the tool succeeded. The pre-fix Paseo history reproduction remains
retained evidence; the post-fix live agent check is still `NOT RUN` pending a
fresh disposable-client run.
During session reload, Paseo 0.8 can emit historical timeline items before
`session.opened`; the provider buffers them per session, forwards `opened`
first, then replays the items in order. The buffer is bounded at 4,096 items
and approximately 16 MiB of JSON; overflow fails closed rather than silently
dropping or partially accepting history.
Codex ACP may deliver terminal stdout/stderr as `_meta.terminal_output_delta`
updates while leaving the final tool `rawOutput.formatted_output` empty. Cortex
correlates those deltas by the exact ACP session and tool-call identity, merges
them into the durable terminal completion, and bounds the merged text at 4 MiB.
If the bound is reached, the completion is marked `output_truncated` and
contains an explicit marker. Missing or truncated output remains observable as
missing or truncated; it is never presented as a successful or complete check.
The adapter also forwards Codex App Server `custom_tool_call` and
`custom_tool_call_output` records, including nested and replayed
`rawResponseItem/completed` envelopes, once per exact call ID. Observed input,
output, exit status and failure are retained; missing output is not invented.
The no-paid trace covers this transformation, while live Paseo rendering of a
real nested call remains unverified until a retained client run.
Coordinator prose is held until its engine turn finishes. If it handed work
to an active worker or only used Cortex tools without a terminal decision,
procedural self-talk and intermediate narration are discarded from the feed; a later answer or user question uses an ACP `messageId` that
`session/load` reuses without attaching prose to a tool card.

## Setting it up in Zed

Three things have to be true. Zed already gives you the first two if you use
its external agents at all.

### 1. The engines

Cortex does not install, copy or pin an engine. It runs the ones **your editor
already installed**, from where your editor put them. In Zed that is the agent
registry under `~/.local/share/zed/external_agents/`, filled when you add
`codex-acp` or `claude-agent-acp` as external agents.

Sign in to each engine with its own CLI (`codex`, `claude`), as you would to
use it on its own. Cortex reads those native credential directories
(`~/.codex`, `~/.claude`) by reference — it never copies them and never asks
you for a token. One consequence worth knowing: your editor's own engine
process and Cortex's engine process then share one credential directory, so a
token refresh can race between them.

### 2. Cortex itself, registered as a custom agent

Zed reads `agent_servers` from `~/.config/zed/settings.json`:

```json
"agent_servers": {
  "Cortex": {
    "type": "custom",
    "command": "/usr/bin/python3",
    "args": ["-B", "/path/to/cortex-orch-acp/src/cortex_acp.py"],
    "env": {}
  }
}
```

Use absolute paths, and keep `-B` — it stops Python writing bytecode into the
checkout. In a normal Zed profile, `env` stays empty: Cortex never takes
Zed's own model credentials.
For a disposable Zed profile whose process has a temporary `HOME`, set
`agent_servers.Cortex.env.HOME` to your real home directory. This makes the
Cortex child use the Codex and Claude accounts already signed in under
`~/.codex` and `~/.claude`; keep the Zed process and Cortex XDG config/state
directories disposable. See the [Zed setup guide](docs/acp/zed-custom-agent.md).

`scripts/cortex_acp_client_config.py` prints exactly that entry for your
checkout; with `--apply --i-consent-to-write` and an explicit settings path it
writes it for you, backed up and atomic. It writes nothing by default.

Then pick **Cortex** in the agent panel. It never starts on its own.

### 3. Nothing else, usually

On its first start Cortex finds the engines your editor installed, gives each
one a connection pointing at your own credentials, and writes a default
`~/.config/cortex/route-policy.json` permitting both. None of that needs your
consent, because none of it reaches outside your own configuration.

You only need the setup tool for the cases that genuinely need you:

```bash
python3 -B scripts/cortex_acp_setup.py status          # what Cortex found, read-only
python3 -B scripts/cortex_acp_setup.py add-connection --help
```

— a second account for the same engine, an API-key connection instead of a CLI
login, or an engine installed somewhere your editor does not manage. With no
usable connection at all, Cortex advertises no model and refuses to
orchestrate rather than silently reaching for something.

`/cortex-setup` in the editor prints the same guidance and changes nothing.

### Checking the three in one command

```bash
python3 -B scripts/cortex_diagnose.py --format text
```

It reports the engines it can see, each connection's state, storage and
schema — and never makes a model call.

## Using it

### Choose the model for a task

In Zed, the selectors expose **engine and account** → **model of that
engine** → **effort of that model** → **language**. Paseo exposes one
model picker, so Cortex puts the engine-provided model name first and its
description, engine, account and catalog evidence below it. Paseo's mode
picker selects the coordinator language (Auto, English or Russian). The
literal engine aliases `default` are omitted from model and effort choices.
For Paseo, a unique model ID is also the selector value, so its `--model`
option can select it before the first prompt. If two accounts offer the same
model ID, their selector values are exact route IDs; choose one in the model
picker or use `/cortex-model` with the displayed route. A model merely shown
as the first available option has not been selected until the client sends a
selection or a previously saved user default applies.
A choice in either client resolves to the coordinator's concrete route;
the runtime verifies it again before launch. Cortex refreshes each engine's
advertised models in the background when a new ACP session opens and sends
an updated selector when discovery succeeds.

Equivalent commands, for a client without those selectors:

| Command | Does |
| --- | --- |
| `/cortex-status` | current and pending model, effort, connection state |
| `/cortex-model <label or id>` | select a route (provider and model together) |
| `/cortex-effort <value>` | select the effort level |
| `/cortex-language <en\|ru\|auto>` | the language Cortex itself speaks |
| `/cortex-setup` | connection guidance, changes nothing |
| `/cortex-new-task` | start a separate task, leaving the current one alone |
| `/cortex-stop` | stop the active task and terminate its owned coordinator and worker attempts, including workers with terminal events still awaiting process exit |
| `/cortex-answer <n> <text>` | answer one open coordinator question |

### Decide who may use what — `route-policy.json`

The coordinator's own route is your choice in the editor. Which routes a
*worker* may use is the policy file:

`~/.config/cortex/route-policy.json`

Cortex writes it on its first run with both engines permitted; after that it
is yours to edit by hand. Cortex reads, in precedence order:

1. `<project>/.cortex/route-policy.json` — this project only
2. `$CORTEX_ROUTE_POLICY` — a path you set in your editor's `env` for Cortex
3. `~/.cortex/route-policy.json`
4. `~/.config/cortex/route-policy.json` — the default location

A policy can only ever **narrow**. It cannot grant a route or an effort that
the permission ceiling forbids — otherwise editing a file would be a way
around the ceiling, which is the one thing a ceiling is for. A malformed file
is recorded and ignored, never read as "no restrictions".

### MCP servers

Cortex always supplies its authenticated `cortex` MCP server first. Additional
servers come from two sources: the owner-only global registry and the ACP
client's session-scoped `mcpServers` list. The global registry is the reliable
portable setup because Paseo currently sends an empty `mcpServers` list; Zed's
forwarded entries continue to work. The effective list is resolved for
`session/new` and again when a session is restored through `session/load`, so
coordinator and worker engine sessions receive the same current configuration.
The private Cortex relay uses a token beginning with a letter so its value
cannot be mistaken for a command-line option. If its MCP child exits before
advertising tools, the active turn reports `cortex_mcp_connection_failed`
with a bounded reason; a session alone cannot prove readiness because Claude
may connect to MCP only when its first prompt starts. The broker records names,
count and schema digests from an actual relayed `tools/list`; an observed list
missing base report tools fails closed, while deferred discovery remains
`not_observed`.

Create `$XDG_CONFIG_HOME/cortex/mcp-servers.json` (or
`~/.config/cortex/mcp-servers.json` when `XDG_CONFIG_HOME` is unset) with
owner-only permissions, for example:

```json
{
  "version": 1,
  "servers": {
    "codebase-memory-mcp": {
      "command": "/absolute/path/to/codebase-memory-mcp",
      "args": [],
      "enabled": true
    }
  }
}
```

The registry is a JSON object keyed by server name. Entries are stdio-only and
require an absolute executable command plus an optional string array of
arguments. An optional `env` object may provide variables required by a
particular server; it is empty for the installed `codebase-memory-mcp`
registration. That registration is global and is forwarded to both Codex and
Claude sessions.
Never put tokens or other secrets in repository files; registry values stay in
memory and are excluded from the project store and diagnostics. A project-local
registry is deliberately unsupported: project contents can be changed by a
worker and must not become a persistent executable authority for later sessions.
The reserved name `cortex` and malformed entries are rejected. Disabled entries
(`"enabled": false`) are skipped.

The native engine accounts may also contain their own MCP servers. For each
Codex child, Cortex disables the installed CLI's `plugins` feature, enumerates
the native config MCP names and starts App Server only after checking that each
one is disabled for that child. An unexpected inventory shape, duplicate or
unsupported name, or failed check stops that launch. Explicit Cortex registry
and ACP session `mcpServers` entries remain available, including a session
entry whose name overlaps a disabled native entry. For Claude, Cortex sends
`strictMcpConfig: true` on each new session so the installed SDK requests only
the explicitly supplied MCP servers. These controls leave the production
accounts in place; live engine enforcement of this MCP boundary has not yet
been verified.

To install the pinned Playwright MCP for browser work, run
`python3 -B scripts/cortex_install_playwright_mcp.py`. It installs
`@playwright/mcp@0.0.81` under the user's local Cortex data directory,
verifies its MCP `initialize` and `tools/list` replies, then updates the
owner-only registry without replacing other servers. The resulting entry uses
`--headless --isolated`, `"roles": ["worker"]`, and
`"engines": ["claude-agent-acp"]`. Its 26 advertised tools are therefore
available to Claude workers only; the current Codex ACP path does not offer
deferred MCP tool loading. A successful setup handshake does not qualify a
full ACP client session or a particular browser workflow.

## Checking it works

```bash
python3 -B scripts/cortex_diagnose.py --format text          # offline, no model call
python3 -B scripts/cortex_acp_smoke.py --report /tmp/s.json  # launches the real entry point, no paid call
```

`cortex_diagnose.py` checks storage, schema and dependency digests, connection
availability and adapter qualification; `--project <path>` adds that project's
own ceiling. Neither ever makes a model call.

On Linux, Cortex does not require a user-namespace setting, `bubblewrap`,
`systemd` sandboxing, or an AppArmor rule. Workers start directly in the exact
project opened in Zed, using the ordinary rights of the Zed-launched process.
There is no OS filesystem sandbox around the engine. `/cortex-status` reports
this execution mode without making a model call. See
[permissions and containment](docs/acp/permissions-and-containment.md).

The private Codex adapter requests a `workspaceWrite` policy with empty
`writableRoots`, disabled network access, and explicit exclusions for `/tmp`
and the environment-selected temporary directory. A clean real worker probe
observed the `/tmp` exclusion: its single sentinel attempt failed with
`Read-only file system` and the sentinel was absent. This qualifies that
observed `/tmp` outcome only; `/var/tmp`, `.cortex`, arbitrary same-user
paths, and OS-level isolation from other same-user processes remain
unverified. See the [retained boundary probe](docs/acp/evidence/paseo-2026-09-21/live-boundary-probe.md).

The verified Codex ACP adapter is launched from a Cortex-owned private copy
whose `agent` approval reviewer is the host user. The original installed
adapter and production Codex account remain untouched. Cortex auto-approves
ordinary permission requests; recognized `rm` requests go to the client for
confirmation. The [permission boundary](docs/acp/permissions-and-containment.md)
applies only when the engine actually emits an ACP permission request.
For a coordinator, Cortex also approves an engine-declared edit only when its
exact target is that coordinator thread's active, server-created pipeline draft.
That exact owner-bound native Claude draft edit is auto-approved after the
binding and path checks; host-confirmed deletion still requires the matching
`allow_once` receipt and observed completed action.
Coordinator requests for engine-private skill trees (`.codex`/`.claude`) or
named project checks are refused locally and routed back to the worker lane;
they are never forwarded as host permission prompts.
Native `PYTHONDONTWRITEBYTECODE=1 python3 -B - <<'PY'` is auto-approved only
for an AST-proven pure heredoc calculation; unknown or mutating scripts require
host confirmation.
Worker Bash classification now uses the observed command text for
non-destructive commands, including read-only heredocs such as `from math
import pow`. Observed Bash `>` redirects auto-approve when every target is
inside the bound workspace and outside `.cortex`; `/tmp`, private targets and
deletion remain host-confirmed. Dynamic or opaque interpreter effects cannot
be completely classified.
Coordinator ordinary non-delete Bash read operations are auto-approved when
the observed targets stay inside the bound workspace and outside `.cortex`;
coordinator source edits are refused locally with a notifier. Recognized
`/bin/bash -lc`, `bash -lc`, and `/bin/sh -c` wrappers
apply the same rule to literal nested non-delete commands; wrapped `rm` still
prompts. Malformed or opaque wrappers may prompt. Deletion, move, truncate and
outside/private targets go through a fresh one-shot host review when a live
requester is available. An explicit
denial, timeout, missing or stale requester, or malformed response remains
refused. Worker writes outside the bound workspace remain a hard deny and are
never forwarded to the user. This lets the
declared Bash commands use a broad default-allow policy under the same path
guards. Destructive, private, outside, symlink, or malformed commands still
require review or refusal; opaque effects remain unclassifiable.
The unified execute/other classifier permits `2>&1` when the surrounding
command remains safe; true truncation, deletion, private, and outside targets
still require review.
coordinator fill the required decision edition without silently writing project
source.
For an observed `rm` action, Cortex requires a matching task-and-attempt-bound
host `allow_once` response before accepting the result.

The local coordinator refusal has a deliberate ACP compatibility boundary.
Claude ACP exposes a `reject_once` outcome without a policy-reason field, so
Claude may first receive its generic native rejection text. Cortex records the
meaningful role event durably, withholds that misleading prose from the user
feed, and continues the same coordinator session with guidance that the
operation belongs to a worker. This applies to coordinator source edits,
engine-private skill access and coordinator-run project checks; none becomes a
Paseo permission card. The continuation is a runtime delivery path, not a
host-enforced system prompt, and live Claude confirmation of this behavior is
still pending.

## Removing it

```bash
python3 -B scripts/cortex_uninstall.py --project /path/to/project
```

Dry-run by default, and it deletes only what Cortex created. Add
`--i-confirm-removal` to act, `--remove-user-scope` and
`--remove-project-store` to include the user-level configuration and a
project's task store.

## How the work actually flows

1. You send a request. The session creates and binds exactly one task, and the
   runtime publishes the task's initial pipeline itself — no model turn is
   spent on bookkeeping.
   New tasks receive a bounded catalog of current source-backed pages from
   `docs/project/` and `docs/features/`; scan and omission limits are reported
   explicitly so the coordinator can request a refresh or bounded follow-up.
2. The coordinator decides what to delegate and calls `delegate`. Provisioning
   starts immediately. Once the current tool batch finishes, Cortex ends that
   coordinator engine turn and waits locally for a worker result, failure,
   question or user message; waiting does not consume model calls. The coordinator reads the
   current project root opened by the ACP client, including edits made by
   workers during its turn.
3. A worker runs against that exact project root, with the role's
   instructions and a catalogue of shared skills available through `read_skill`.
   Each new worker attempt also receives a bounded, versioned project-document
   catalog in its engine prompt. It can follow the relevant present index before
   broad source search; missing indexes do not stop the assignment.
   Its first-turn role instructions tell it to do assigned work itself and not
   start an engine-native subagent. For pinned claude-agent-acp 0.78.0,
   Cortex also sends the SDK's `disallowedTools` option for `Agent` and
   `Task` at session creation; live enforcement remains unverified.
   For pinned Codex CLI 0.154.0, its per-process launcher disables native
   agents and both multi-agent features, checking the feature rows before
   starting App Server;
   fresh real-client enforcement remains unverified.
    Cortex grants one mutating writer at a time, records the worker's action
    stream and filesystem revisions, and attributes actions in your feed.
    In ACP clients, each coordinator project or external MCP call gets its own
    top-level card with a human-readable title and complete visible inputs,
    subject only to credential/secret redaction; status updates coalesce within
    that call. Read, Skill and Cortex system operations are compact and
    non-expandable; Edit and Shell calls use the host's native expandable
    input/output fields. Coordinator and worker cards share semantic operation
    labels. The coordinator's `delegate` call is represented by the worker
    assignment card rather than a second top-level card. Worker calls stay bound
    to their assignment card and exact task/session history. Paseo's direct
    worker chat preserves and coalesces genuine worker message chunks and the
    published `worker_result`, and tells the user `Open the Workers pill for
    full history`; the agent-scoped composer Workers pill opens the exact worker
    in a workspace tab, including after a closed tab is recreated. The bounded
    popover is a flat list with separators only between worker rows, uses an
    inner scroll view, and refreshes active worker history about
    every 750 ms. Its loader is shown only for the first request, so cursor
    polling keeps the transcript mounted. Paseo 0.8 may still place the mobile
    composer pill close to the message footer; that host placement remains a
    known limitation. Exact tool lifecycle updates and adjacent message chunks
    are coalesced.
    Non-Paseo clients retain the full worker lifecycle card. In Paseo's
    coordinator and worker feeds, Cortex MCP calls and operations against the
    private `.cortex` store are hidden, including streamed and replayed
    presentation. Ordinary project operations and external MCP calls remain
    visible. The hidden operations remain in durable task/event evidence; this
    is a feed presentation rule, not deletion from the store. The cards stay
    attributable and survive `session/load`.
    Paseo project Read rows retain the structured project file path, and
    genuine project Edit rows can render a line-numbered diff. The installed
    Paseo plugin API has no public file/editor-open navigation, so paths and
    diffs are informative rather than clickable file-opening links. Protocol
    only and coordinator-generated worker events are filtered, while genuine
    worker message chunks, including `maturity`/`state` updates, are preserved
    and merged in order. The worker timeline row subtly pulses only while
    working and stops on terminal status, including after replay.
    A durable `worker_result` marks its graph attempt reported and takes
    precedence over stale running or lower-priority terminal snapshots. A
    compound Shell remains a Shell row with command/output; it is not shown as
    a Read or Edit.
    Procedural coordinator self-talk is suppressed during delegated work, but
    the substantive answer is still delivered after the worker result/report
    is available. Accepted final text still receives the pipeline digest guard.
    Paseo users can run
    `python3 -B scripts/cortex_acp_setup.py install-paseo-plugin` to install
    the repository-owned native `cortex-acp` provider, after which Paseo uses
    its native question choice UI for marked Cortex questions and no separate
    generic ACP registration is needed. See
    [question handling](docs/acp/steering-and-questions.md#questions-structured-elicitation-with-a-text-fallback)
    for the text fallback.
    Setup persists only the absolute HOME/XDG storage roots needed after a
    Paseo daemon restart; the record contains no credentials and preserves
    the native-account state selected by those roots.
   For structured worker context, Cortex uses the assignment's durable
   attempt and route audit: a mixed-team worker's engine, connection, model
   and effort come from that exact provisioned attempt, or are marked
   unavailable. The coordinator's active route and its `pipeline.md` prose
   cannot substitute for those facts. Selected reports, revision feedback and
   questions are carried as labelled evidence data; they do not become
   instructions merely because a model wrote their text.
   If a worker normally ends a turn before publishing a result, asking a
   question, or leaving an owned publication, Cortex sends one bounded
   corrective continuation into that same worker session and keeps the
   coordinator waiting as though the worker were still working. It does not
   create a second assignment or treat the worker's first text as evidence.
   A second no-result end becomes a durable worker failure; engine failures,
   refusals, and cancellation do not receive this continuation.
4. The worker publishes an immutable report and calls `publish_result`.
   The advertised `write_report.report_id` maps to
   `publish_result.report_ref`; an invalid first-call shape remains a tool
   error and is not silently accepted.
   `request_check` observes the exact project state associated with that
   attempt; `assess_check` records the coordinator's separate judgment against
   the observed revision. A stale revision, overlapping writer, or unknown
   editor-buffer state blocks acceptance. A missing or mismatched shared-root
   binding or revision observation also fails closed; report or pipeline prose
   cannot supply that identity. Python checks suppress bytecode
   writes so an ordinary unittest run does not alter its staged candidate.
   ACP engine processes also inherit this setting, preventing their ordinary
   Python tests from creating bytecode in the open project.
5. The coordinator reads the report and accepts or requests a revision.
   A report read is trusted only after a successful channel receipt bound to
   the exact task, attempt, report reference, and request key; a pending ACP
   observation alone is unverified.
   `decide(revise)` creates a new continuation assignment linked to the prior
   report; it does not silently replace that evidence. Task acceptance checks
   the retained result and applied project state. Accepting an assignment
   records only that assignment's decision; an overall task decision is
   required before Cortex reports completion. A separate task-level accept may
   follow an assignment accept in the same coordinator turn; its own reminder
   is superseded while new worker and user events still gate completion.
   Before task-level acceptance,
   the coordinator publishes a current `pipeline.md` edition with the reports,
   checks, remaining work and decision basis. Cortex refuses acceptance
   without a coordinator edition and binds the decision to its exact digest;
   a later edition makes that decision stale. Routine reports omit
   incident-only fields.
   A current candidate check can replace a worker's `not_run` or `unverified`
   rendered/UI check only when it is bound to the exact candidate digest,
   command, scope, environment and current requirement revisions, and the
   coordinator records an explicit semantic pass. An explicit worker failure
   is never replaced by that candidate pass. An exact previously assessed,
   still-applicable pass may be reused; changed requirements or any other
   binding invalidate reuse and require a new observation. For an explicit
   rendered/UI assignment, `publish_result` fails fast when `checks` is missing
   or empty, so report prose cannot stand in for structured evidence. The plan assignment ID returned by `update_plan` is
   distinct from the executable `assignment_ref` returned by `delegate`. An empty optional
   `plan_assignment_ref` is treated as omitted; a nonempty value must match this task's plan.
   If a task decision is refused because a new worker result or follow-up
   event has not yet reached a coordinator continuation, Cortex ends the
   downstream engine turn after two such observed refusals and delivers the
   events in a fresh continuation. The task remains active; the coordinator
   must make a new overall decision after delivery. This recognizes the
   Cortex MCP tool identity reported by both Codex ACP and Claude ACP.
   The coordinator is instructed to repair the refusal's missing durable
   evidence or state before retrying and never repeat a refused acceptance
   against unchanged state.
   That edition describes evidence and unresolved deliverable work without a
   momentary acceptance status; the durable task decision records the outcome.
   Before showing an accepted final answer, Cortex compares fingerprint-shaped
   strings in that answer with SHA-256 values in the current published
   `pipeline.md` edition. An unmatched value is withheld with a warning;
   the runtime does not infer that the rest of the prose is verified.
   After the accepted final text is delivered and the task state is saved,
   Cortex shuts down that task's coordinator engine and releases its
   connection capacity once process exit is confirmed. A later new task can
   open its own coordinator session.
   Queued workers recheck capacity after a confirmed release; coordinator-only
   saturation is determined from one consistent reservation snapshot.

For multi-step work, the coordinator can record versioned requirements and
planned assignments with `update_plan`, binding a dependent assignment to an
exact predecessor candidate. The model chooses the plan; the store preserves
its IDs, revisions and predecessor links.
Worker handoff prompts name the exact predecessor `report_ref` and require a
trusted `read_report` receipt before the coordinator relies on that report.

When something cannot be done, it is said rather than waited out: a worker
whose engine refuses to start (an expired login, for instance) ends as a named
failure carrying the engine's own words, not as a worker that never appears.

The coordinator remains advisory and does not perform implementation writes.
Delegated workers write only through the current attempt's authorized
`project_root` binding. A Cortex writer lock and ownership epoch serialize
mutating turns; the runtime rechecks the project revision immediately before
each write. Worker protocol prefers inline in-memory calculations, avoids
`/tmp` scratch scripts, and writes only assigned paths; a read-only QA script
outside that boundary still requires host permission.
and after each write. A worker publishing a result while a coordinator turn is
still running records the observed project revision before the result event;
the coordinator's turn can then recognize that exact result, while any later
unmatched change blocks the turn. The private `.cortex/` store is a separate Cortex data
area alongside the source files and holds reports, receipts and evidence. In
direct engine mode a native engine has the same user-level filesystem authority
as the Zed-launched process and can technically see that directory; the
runtime's dispatched store operations and path checks still protect their own
interfaces. A writer that cannot acquire
the binding, observes a stale revision, or loses project state fails closed; it
is never redirected to a hidden snapshot or arbitrary fallback directory.
The before/after digest reconciles the shared project tree with the
authorized attempt; it does not prove which same-user process wrote the
bytes. This coordinates Cortex writers but cannot prevent an unrelated
external process from changing the directory between observations. Real
Zed/Codex/Claude client and engine behavior remains subject to the live
runbook; offline checks cannot establish it.
Shared-root publication attribution also requires a durable pre-attempt
manifest and disjoint changed paths across concurrent attempts; missing or
overlapping evidence is reported as an explicit `shared_root_*` refusal.

## Developing on Cortex

```bash
PYTHONDONTWRITEBYTECODE=1 python3 -B -m pytest -q
```

Two requirements, both enforced: never run as root, and never run with a
`HOME` inside a git repository or a directory containing `.cortex` — the
suite must not touch your real configuration. `docs/acp/test-matrix-and-ci.md`
explains why and what a default run cannot do (it cannot start a paid
campaign).

After changing anything under `src/`, `skills/`, `agents/` or `agent-sources/`:

```bash
python3 -B scripts/generate_agent_profiles.py write   # worker profiles are generated
python3 -B scripts/cortex_package.py stamp            # package identity
```

## Documentation

- [ACP integration hub](docs/acp/index.md) — every ACP subsystem, in order
- [Zed setup](docs/acp/zed-custom-agent.md)
- [Security boundaries](SECURITY.md) — what is enforced, and what is not
