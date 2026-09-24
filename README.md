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

> **Release qualification is incomplete.** 45 of 69 mandatory live-only cases remain; real-client observations are still marked NOT RUN. See [runtime status and remaining work](docs/acp/runtime-rewrite-status.md).

## What Cortex is

Cortex is an ACP v1 stdio agent. Its runtime coordinates Codex and Claude ACP workers in the project you opened, and owns task scheduling, review, verification, and completion. Task records and Markdown reports are stored in that project’s `.cortex/` directory.

Worker routes and effort are selected from observed models by task tier, quota and recent same-tier balance. Plan model hints are constrained recommendations; explicit routing rules remain authoritative.

## Install

### Paseo

Paseo installs can prepare Cortex automatically: the plugin manifest’s supported `build` hook runs the setup command before activation.

Install the current GitHub version:

```bash
paseo plugin install github:igovet/cortex-orch-acp:integrations/paseo
```

Install from a branch, tag, or commit by adding `--ref`:

```bash
paseo plugin install github:igovet/cortex-orch-acp:integrations/paseo --ref dev
```

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

### Other ACP clients

Register Cortex as an ACP stdio agent with this command, adapting the absolute path to your checkout:

```text
python3 -B /absolute/path/to/cortex-orch-acp/src/cortex_acp.py
```

Use the client’s ACP agent registration settings. See [the ACP documentation index](docs/acp/index.md).

## Configuration

See the [configuration guide](docs/configuration.md) for JSON configuration files and their locations, MCP server setup, engine connections, routing rules, and the default Haiku exclusion with conditional Luna routing.
