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
| Engine connections | `~/.config/cortex/connections.json` | Cortex, on first run; `scripts/cortex_acp_setup.py` for the rest |
| Connection secrets (API-key connections only) | `~/.config/cortex/secrets/<connection id>` | setup tooling only |
| Engine credentials | your own native directories, `~/.codex` and `~/.claude` | the engine's own CLI (`codex`, `claude`) |
| Engine installs | wherever your editor put them (Zed: `~/.local/share/zed/external_agents/`) | your editor |
| Route + effort policy | `~/.config/cortex/route-policy.json` | you, by hand |
| Your defaults | `~/.config/cortex/defaults.json` | the model/effort selectors in the editor |
| One project's tasks | `<project>/.cortex/` | Cortex |

A project's `.cortex/` holds `cortex.sqlite3` (tasks, events, audit),
`pipeline-drafts/` and `draft-reports/` (Markdown being written), and
`workspaces/` (each worker's isolated copy). It is Cortex's own storage, not
your source: keep it out of version control if you do not want task history in
your repository.

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
checkout. `env` stays empty: Cortex never takes Zed's own model credentials.

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
5. The coordinator reads the report and accepts or sends the worker back. It
   cannot accept work that is not in your project: if that integration
   conflicted and wrote nothing, acceptance is refused until the conflict is
   resolved and the change actually lands.

When something cannot be done, it is said rather than waited out: a worker
whose engine refuses to start (an expired login, for instance) ends as a named
failure carrying the engine's own words, not as a worker that never appears.

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
