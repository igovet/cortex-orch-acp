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
| Engine installs | `~/.local/share/cortex/engines/<engine>/<version>/` | Cortex (updated at startup) |
| Worker routing overrides (optional) | `~/.config/cortex/routing.json` | you, by hand |
| Your defaults | `~/.config/cortex/defaults.json` | the model/effort selectors in the editor |
| ssh password dialog (optional, off) | `~/.config/cortex/ssh-askpass.json` | you, by hand (see [ssh passwords](#ssh-passwords)) |
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
worker/tool cards from this store after an editor restart. In Paseo 0.8,
the agent-scoped composer Workers pill opens a bounded Workers popover for the
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
no output, its supported Paseo 0.8+ transformer supplies a bounded non-null
display message while preserving `failed`. It does not fabricate a cause or
claim that the tool succeeded. The pre-fix Paseo history reproduction remains
retained evidence; the post-fix live agent check is still `NOT RUN` pending a
fresh disposable-client run.
During session reload, Paseo 0.9.1+ now supports long history through a new
recovery mechanism. The relay stages replay notifications in a temporary file,
validates each frame's UTF-8 and SHA-256, and replays in windowed batches of at
most 64 events or approximately 1 MiB per batch. Total history is bounded by
hard limits: 1,000,000 events, 512 MiB of serialized frame bodies, and 16 MiB
per frame. Overflow, malformed frames, corrupt staging, or incomplete replay
fails explicitly rather than silently dropping history. Prompts and
configuration changes are refused while restoration is incomplete; a reload
retry starts a fresh history with the same durable message and card identities.
Codex ACP may deliver terminal stdout/stderr as `_meta.terminal_output_delta`
updates while leaving the final tool `rawOutput.formatted_output` empty. Cortex
correlates those deltas by the exact ACP session and tool-call identity, merges
them into the matching command's final output, and uses the bounded result in
worker history.

## Paseo development checks

All Paseo and plugin checks use the persistent isolated `~/.paseo-dev` home,
the separate `~/.config/Paseo-dev` desktop profile, and a disposable project.
Run CLI checks through `python3 -B scripts/cortex_paseo_dev.py -- ...`; this
mode checks daemon and retained log output without model calls. CLI output is
not rendered-GUI evidence, and incomplete history remains unverified. See the
[Paseo development guide](docs/acp/dev-paseo-environment.md) for setup and
observed limits. The model-specific `gpt-5.6-sol` skill-editing rule is removed;
the orchestration runtime decides model routing for prompt and skill work.

## Starting a task

Open the project in your editor and choose Cortex. Describe the result you want;
Cortex will inspect the project, propose or route work, and report what it can
verify. It may ask a short question where the answer changes the implementation.
