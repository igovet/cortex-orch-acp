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
        isolated workspaces, checks what they did, and integrates it. Tasks,
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

Cortex is an ACP agent: an editor (Zed today) launches it and talks to it over
stdio, exactly as it would any other coding agent. Cortex is also an ACP
*client* — it launches codex-acp and claude-agent-acp itself, as workers.

That double role is the whole design. The model you talk to coordinates; it
does not edit your project. Workers do the work, each in an isolated copy of
the project, and their changes reach your files only through an explicit,
conflict-checked integration step.

Nothing activates by itself. Cortex runs when you select it in your editor.

## Where everything lives

| What | Where | Edited by |
| --- | --- | --- |
| Agent entry point | `src/cortex_acp.py` in this repository | not edited; referenced from your editor |
| Editor registration | your editor's settings (`agent_servers` in Zed) | you |
| Engine connections | `~/.config/cortex/connections.json` | `scripts/cortex_acp_setup.py` |
| Connection secrets | `~/.config/cortex/secrets/<connection id>` | setup tooling only |
| Engine credential dirs | `~/.local/state/cortex/connections/<id>/` | setup tooling only |
| Installed engines | `~/.local/state/cortex/engines/<engine>/` | you, or your package manager |
| Route + effort policy | `~/.config/cortex/route-policy.json` | you, by hand |
| Your defaults | `~/.config/cortex/defaults.json` | the model/effort selectors in the editor |
| One project's tasks | `<project>/.cortex/` | Cortex |

A project's `.cortex/` holds `cortex.sqlite3` (tasks, events, audit),
`pipeline-drafts/` and `draft-reports/` (Markdown being written), and
`workspaces/` (each worker's isolated copy). It is Cortex's own storage, not
your source: keep it out of version control if you do not want task history in
your repository.

## Setting it up

### 1. Register Cortex in your editor

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

`scripts/cortex_acp_client_config.py` prints that entry for your checkout and,
with `--apply --i-consent-to-write` and an explicit settings path, writes it
for you — backed up and atomic. It writes nothing by default.

Select **Cortex** in the agent panel. It never activates on its own, and it
never uses your editor's own model credentials.

### 2. Connect an engine

Cortex does not install codex-acp or claude-agent-acp, and it does not take
your CLI's credentials by itself. Install the engine, then create a connection:

```bash
python3 -B scripts/cortex_acp_setup.py status
python3 -B scripts/cortex_acp_setup.py add-connection --help
```

Each step is separate and idempotent on purpose: consent for one is never
consent for another. With no connection, Cortex advertises no model and
refuses to orchestrate rather than silently reaching for something.

`/cortex-setup` in the editor prints the same guidance without changing
anything.

### 3. Choose the model for a task

The bar under the message box selects, in order: **engine and account** →
**model of that engine** → **effort of that model** → **language**. What you
pick there is the coordinator's own route.

Equivalent commands, for a client without those selectors:

| Command | Does |
| --- | --- |
| `/cortex-status` | current and pending model, effort, connection state |
| `/cortex-model <label or id>` | select a route (provider and model together) |
| `/cortex-effort <value>` | select the effort level |
| `/cortex-language <en\|ru\|auto>` | the language Cortex itself speaks |
| `/cortex-setup` | connection guidance, changes nothing |
| `/cortex-new-task` | start a separate task, leaving the current one alone |
| `/cortex-stop` | stop the active task and every worker it started |
| `/cortex-answer <n> <text>` | answer one open coordinator question |

### 4. Decide who may use what — `route-policy.json`

The coordinator's own route is your choice in the editor. Which routes a
*worker* may use is the policy file:

`~/.config/cortex/route-policy.json`

It is created with both engines permitted on first setup, and you edit it by
hand. Cortex reads, in precedence order:

1. `<project>/.cortex/route-policy.json` — this project only
2. `$CORTEX_ROUTE_POLICY` — a path you set in your editor's `env` for Cortex
3. `~/.cortex/route-policy.json`
4. `~/.config/cortex/route-policy.json` — the default location

A policy can only ever **narrow**. It cannot grant a route or an effort that
the permission ceiling forbids — otherwise editing a file would be a way
around the ceiling, which is the one thing a ceiling is for. A malformed file
is recorded and ignored, never read as "no restrictions".

### 5. MCP servers

Whatever MCP servers you configure in your editor are passed on to every
engine Cortex runs — the coordinator and every worker see the same tools you
do. There is nothing to configure twice.

Two entries are refused: one an engine could not launch, and one named
`cortex`, which would shadow Cortex's own server. Their `env` and `headers`
values are held in memory only and are never written to a task's storage.

## Checking it works

```bash
python3 -B scripts/cortex_diagnose.py --format text          # offline, no model call
python3 -B scripts/cortex_acp_smoke.py --report /tmp/s.json  # launches the real entry point, no paid call
```

`cortex_diagnose.py` checks storage, schema and dependency digests, connection
availability and adapter qualification; `--project <path>` adds that project's
own ceiling. Neither ever makes a model call.

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
2. The coordinator decides what to delegate and calls `delegate`. Provisioning
   starts immediately, while it is still talking.
3. A worker runs in its own copy of the project, with the role's instructions
   and the always-on shared skills already delivered. Its actions appear in
   your feed as they happen, attributed to it.
4. The worker asks to integrate. Changes are applied against a known base,
   conflict-checked, never last-write-wins — and never at all if your editor
   has unsaved changes to a file being written.
5. The coordinator reads the report and accepts or sends the worker back.

The coordinator cannot edit your project: its workspace is a read-only copy,
and a turn that changed the real project is refused. Its own store
(`.cortex/`) is writable — drafts and reports are written there with ordinary
file operations.

## Developing on Cortex

```bash
python3 -B -m pytest -q
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
- [Decisions](docs/project/decisions.md)
