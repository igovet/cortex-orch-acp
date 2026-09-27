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


## What Cortex is

Cortex is an ACP v1 stdio agent. Its runtime coordinates Codex and Claude ACP workers in the project you opened, and owns task scheduling, review, verification, and completion. Task records and Markdown reports are stored in that project’s `.cortex/` directory. Plan verification commands are literal argv from documented project commands or scripts; shell syntax is rejected before work starts. A worker check is marked passed only when its exact same-attempt `command_run` or user-confirmed `outside_command` receipt proves a complete, nontrivial command exited 0. Engine shell calls, reads and incomplete output cannot verify a check; older reports remain intact and weak historic evidence is marked separately when read.

ACP timeline updates use bounded display projections to avoid repeatedly sending growing worker transcripts. Worker cards keep a 20-action, 4 KiB tail for generic clients; per-update and field size limits and a fixed per-card event budget (forward-only status transitions plus at most 8 progress updates, one per 30 s, so at most 13 updates per worker whatever its size) bound host event volume. Coordinator cards send changed fields only, suppress no-op updates, omit an identical MCP text mirror, and reference repeated identical outputs; unchanged context polls return a compact receipt. Coordinator and engine tool cards also have a per-task total budget: the first 150 distinct calls (or 4 MiB of card updates) get their own card with at most 10 updates each; later calls are counted on one aggregate card, sent at most 20 times per task; and up to 50 failed calls past the budget keep their own cards. The budget persists across turns and restarts, and `session/load` replays older journals within the same bound. In the scale test, 5,000 coordinator calls over four turns and a restart sent 362 card updates and 194,079 bytes, against 312 updates for 500 calls. In the recorded scale test, repeated polls were 481–483 bytes, and each Paseo worker card sent 3 updates and 16,959 bytes for 10-, 200- and 1,000-tool workloads. These limits reduce emitted event volume; they do not cap durable task history or guarantee constant total daemon memory. See [ACP observability and resource limits](docs/acp/observability.md#bounded-acp-event-projections).

Worker routes and effort are selected from observed models by the task (role tier, complexity, work-kind engine affinity configurable with `engine_affinity` in `routing.json`, and the coordinator's hint) and each engine's subscription plan usage; how often an engine was used before is not a factor. Light work stays on light models and hard work gets stronger ones. Codex Luna runs at high to max effort and also competes for ordinary development work. A normal standard-tier development retry or review fix keeps the standard candidate pool and targets Luna at max; explicit high-complexity work and deep roles still escalate and exclude Luna. Plan model hints are constrained recommendations; explicit routing rules remain authoritative.

## Install

### Paseo

If rendering fails in a plugin panel, the composer popover, or a timeline card, Paseo shows a themed fallback for that surface with reopen guidance. If the thrown value cannot safely provide text, the fallback uses `unknown error`.

Paseo installs can prepare Cortex automatically: the plugin manifest’s supported `build` hook runs the setup command before activation.

Install the current GitHub version:

```bash
paseo plugin install github:igovet/cortex-orch-acp:integrations/paseo
```

Install from a branch, tag, or commit by adding `--ref`:

```bash
paseo plugin install github:igovet/cortex-orch-acp:integrations/paseo --ref dev
```


Apply plugin updates between active tasks until session recovery has been qualified for the version being deployed. A transport disconnect preserves task state and recovery reconciles saved Cortex state, but a host `SIGKILL` without `SIGTERM` cannot be recorded, and native engine actions do not have retained command receipts.

Update an installed Git plugin:

```bash
paseo plugin update cortex-acp --check
paseo plugin update cortex-acp --ref dev --yes
```

For a local checkout, install the plugin directory by its absolute path and reload it after editing:

```bash
paseo plugin install /absolute/path/to/cortex-orch-acp/integrations/paseo
paseo plugin reload cortex-acp
```

Git installs run their declared preparation hook. Local directory installs use the checkout directly; use the setup helper for a managed local Paseo installation when needed. See [Paseo onboarding and diagnostics](docs/acp/packaging-and-onboarding.md).

The Paseo plugin shows per-worker context and token usage on worker cards. It does not add a separate provider plan-usage button.

### Other ACP clients

Register Cortex as an ACP stdio agent with this command, adapting the absolute path to your checkout:

```text
python3 -B /absolute/path/to/cortex-orch-acp/src/cortex_acp.py
```

Use the client’s ACP agent registration settings. See [the ACP documentation index](docs/acp/index.md).

## Configuration

See the [configuration guide](docs/configuration.md) for JSON configuration files and their locations, MCP server setup, engine connections, routing rules, and the default Haiku exclusion with conditional Luna routing. ssh from workers uses your own ssh-agent and `~/.ssh/config`; see [ssh from workers](docs/configuration.md#ssh-from-workers).

## Further documentation

- [Packaging and onboarding](docs/acp/packaging-and-onboarding.md): install, first-run setup, diagnostics
- [Configuration guide](docs/configuration.md): connections, routing, MCP servers, ssh
- [Codex adapter](docs/acp/codex-acp-adapter.md) and [Claude adapter](docs/acp/claude-agent-acp-adapter.md): how Cortex runs each engine
- [Routing and fallback](docs/acp/routing-and-fallback.md): worker routing and multi-provider fallback
- [Project storage and recovery](docs/project/storage.md): task storage, format upgrades, recovery
- [Security](SECURITY.md): the sandbox and permission boundary
- [Verification guidance](docs/project/verification.md): check evidence rules and test commands
- [ACP documentation index](docs/acp/index.md): full reference
