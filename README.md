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

> **Runtime rewrite in progress.** The coordinator API and control loop have changed.
> This working tree is not release qualified; see [status and remaining work](docs/acp/runtime-rewrite-status.md).

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

The runtime owns the plan lifecycle, worker scheduling, mandatory independent
review, automatic verification and completion. Models provide proposals and
structured results. Existing accounts and ACP engine sessions are retained;
role profiles remain generated from the 23-role catalogue.

Workers publish immutable Markdown reports and submit results through the current
advertised API. Report prose and final chat messages cannot complete a task.

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
current ACP session and task. The Workers and Pipeline pills appear only on
Cortex sessions. Pipeline opens a tab in the right-hand explorer sidebar that
shows the task from top to bottom and refreshes as it runs: the request and
follow-ups, the coordinator's requirements, each delegated worker with its
status, reported checks and the coordinator's accept/revise verdict, then the
final acceptance or the reason the task is waiting. The chat's context
indicator shows the coordinator engine's context fill, forwarded from the
engine's ACP `usage_update` (`used`/`size`). Each worker's latest context fill
and its engine-reported tokens (`↑` input, `↓` output, cached reads) appear on
its chat card, in the Workers popover and on its Workers tab; the Pipeline
header totals the task's tokens and the coordinator's share. Tokens come from
each ACP `session/prompt` response's `usage` and are recorded in
`usage_records`; codex-acp reports only the turn's last model request there,
so Codex totals are a lower bound. The Workers pill remains available with no current
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
When a reconnect finds a worker card whose recorded process identity is
provably gone, `session/load` corrects that existing card to failed without
starting a replacement engine or making a model call; the next prompt is the
recovery boundary. A normal ACP EOF also shuts down the daemon's owned engine
processes before exit.
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
Paseo shows coordinator and worker messages and meaningful tool calls with readable
labels. Read and Read skill are nonexpandable rows with their target. Raw data is
never rendered; other calls expand only for readable output, errors or file diffs. Thinking and protocol bookkeeping (including
wait) are hidden in both chat and worker panels, including replay; durable redacted
evidence is retained, except that file/skill read bodies are omitted from new display
history while the model still receives the complete result. Old reads are projected
as metadata on replay. Long worker titles use two lines and a fixed-size icon.
Workers communicate in English; the coordinator uses the language selected for
the current message. Simple informational questions can be answered after relevant
read-only inspection without creating workers, review or verification jobs. Planned
work still requires independent review and verification.

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
| `/cortex-stop` | stop the active task, close its bound ACP host sessions, and terminate its owned coordinator and worker attempts, including workers with terminal events still awaiting process exit |
| `/cortex-answer <n> <text>` | answer one open coordinator question |

### Decide who may use what — `route-policy.json`

The coordinator's own route is your choice in the editor. Which routes a
*worker* may use is the policy file:

`~/.config/cortex/route-policy.json`

Cortex writes it on its first run with both engines permitted; after that it
is yours to edit by hand. Cortex reads, in precedence order:

1. `<project>/.cortex/route-policy.json` — this project only
2. `$CORTEX_ROUTE_POLICY` — a path you set in your editor's `env` for Cortex
3. `~/.config/cortex/route-policy.json` — the default location
4. `~/.cortex/route-policy.json` — older user configuration location

A policy can only ever **narrow**. It cannot grant a route or an effort that
the permission ceiling forbids — otherwise editing a file would be a way
around the ceiling, which is the one thing a ceiling is for. A malformed file
fails closed with `invalid_route_policy`; it is never read as "no restrictions".

### MCP servers

Cortex supplies its authenticated `cortex` broker to both engines. Native account
MCP servers, native action tools, plugins and hooks are disabled for each child;
account files are unchanged. External servers use the runtime gateway, with
current-channel authorization before each call and retained request/result receipts.
The coordinator receives no external MCP tools. Read-only worker roles receive
only the administrator's explicit `read_only_tools` subset.

The owner-only registry is `$XDG_CONFIG_HOME/cortex/mcp-servers.json`, defaulting
to `~/.config/cortex/mcp-servers.json`. Project-local executable registries are
not supported. Each enabled stdio server requires an absolute `command`, optional
`args` and `env`, and a nonempty `allowed_tools` list naming exact tool names.
`read_paths` grants additional absolute read-only runtime/dependency directories;
project ancestors, project paths and private stores cannot be granted this way.
`network` defaults to false. Optional `roles` and `engines` narrow availability.
The reserved name `cortex`, malformed grants and uncontained transports fail closed.
Client-forwarded servers require the same explicit gateway grants; raw forwarding
is not a compatibility path. Keep secrets out of repository configuration.

External processes run in a Linux namespace with a read-only project, explicitly
granted dependencies, private temporary storage, and only configured environment
variables. Network-enabled gateways also mount public system DNS/host configuration
and the system CA bundle, so name resolution and HTTPS work inside containment.
Calls have deadlines; partial frames, uncertain outcomes and revoked
channels cannot authorize retries. The broker retains actual `tools/list` evidence;
a missing discovery signal remains unobserved.

`python3 -B scripts/cortex_install_playwright_mcp.py` installs the pinned
`@playwright/mcp@0.0.81`, checks its actual MCP catalogue, and registers exact tools,
dependency directories and a network grant for workers. It requires an installed
Google Chrome or the pinned Playwright Chromium, passes its executable path into
the sandbox, and excludes `browser_run_code_unsafe`. Existing role/engine filters
are preserved. It uses `--headless --isolated`; browser tools are available through
the gateway for the configured engines.
An installation handshake is not a live browser-workflow qualification.

The Paseo updater validates this registry before changing the daemon and reloads
an already-running plugin. Old entries without `allowed_tools` must be repaired
with exact installed tool names and executable/dependency read grants; the updater
never invents permissions from a server catalogue. See the
[production repair evidence](docs/acp/evidence/paseo-2026-09-21/production-mcp-repair.md).

## Checking it works

```bash
python3 -B scripts/cortex_diagnose.py --format text          # offline, no model call
python3 -B scripts/cortex_acp_smoke.py --report /tmp/s.json  # launches the real entry point, no paid call
```

`cortex_diagnose.py` checks storage, schema and dependency digests, connection
availability and adapter qualification; `--project <path>` adds that project's
own ceiling. Neither ever makes a model call.

On Linux, Cortex requires `bubblewrap` with usable namespaces for gateway commands,
external MCP processes and verification. Missing containment fails closed. Native
Codex/Claude action tools are disabled; project access uses the channel-bound
Cortex gateway. Coordinator and consultant cannot run commands or edit source.
Workers can write only their runtime-assigned paths. The scheduler serializes
overlapping scopes and reserves at most four workers per project store.

Reports are filled through `draft_write` using an owned draft ID and published as
immutable Markdown. There is no native-file exception for pipeline/report edits.
The runtime owns all pipeline editions. Verification sees a read-only project,
private temporary storage and no runtime store, user home or inherited environment.

`scope_request`, `replan_request` and `escalation_request` are proposals. They
suspend old assignments and return to planning; they never directly increase
permissions or select a model. `deny-write: database`, `do not change database`,
and `Не меняй database` are supported explicit path denials. Other user steering
is retained and delivered after revoking the previous plan's worker grants.

See [permissions and containment](docs/acp/permissions-and-containment.md) and
[the security boundary](SECURITY.md). The complete rewrite remains under test;
[the status page](docs/acp/runtime-rewrite-status.md) distinguishes automated
checks from real-client qualification.

## Removing it

```bash
python3 -B scripts/cortex_uninstall.py --project /path/to/project
```

Dry-run by default, and it deletes only what Cortex created. Add
`--i-confirm-removal` to act, `--remove-user-scope` and
`--remove-project-store` to include the user-level configuration and a
project's task store.

## How the work actually flows

1. Runtime saves the original goal and enters PLANNING.
2. Coordinator submits a structured plan; runtime validates the DAG and scopes.
3. Scheduler reserves ready nodes, resolves configured routes and runs workers.
4. Workers publish immutable reports and structured results.
5. Runtime schedules independent review and a bounded fix/review loop when needed.
6. Runtime executes verification commands and retains complete observations.
7. Coordinator submits synthesis; runtime checks completion invariants.

See [the current architecture](docs/acp/orchestration.md) and
[unfinished qualification work](docs/acp/runtime-rewrite-status.md).

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

Result submissions validate every passing automated check against the worker’s
retained command receipt before publication. A malformed passing claim returns
a field-specific correction; failed or unresolved checks still block completion.
Paseo renders gateway reads, edits and commands from the observed payload while
retaining execution evidence for replay. Read/Read skill display history retains
only targets and status, and raw protocol objects are never shown.

## Documentation

- [ACP integration hub](docs/acp/index.md) — every ACP subsystem, in order
- [Zed setup](docs/acp/zed-custom-agent.md)
- [Security boundaries](SECURITY.md) — what is enforced, and what is not
