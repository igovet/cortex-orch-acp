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

## What Cortex is

Cortex is an ACP agent: run it from an editor like Zed or Paseo over stdio. It receives your work requests, runs other coding agents (codex-acp, claude-agent-acp) as isolated workers in the exact project you have open, and records everything.

The runtime owns the plan lifecycle, worker scheduling, review, verification, and completion. Workers publish immutable Markdown reports and results. Each project stores its own tasks, events, and reports in a local `.cortex/` directory.

## Installation

### In Paseo

Cortex runs in Paseo 0.9.1+ as a plugin. Install directly from GitHub or from a local checkout.

**GitHub URL (recommended)**

Install the plugin from GitHub using the CLI:

```bash
paseo plugin install github:igovet/cortex-orch-acp:integrations/paseo
```

The Git installation's declared preparation steps create Cortex's private
configuration and launch record automatically. It does not log in to Codex or
Claude; add a connection after installation if you have not configured one yet.

**Local checkout**

Clone the repository and run the local setup, which writes the private launch
record and installs the plugin from this checkout:

```bash
git clone https://github.com/igovet/cortex-orch-acp.git
cd cortex-orch-acp
python3 -B scripts/cortex_acp_setup.py install-paseo-plugin
```

See [Paseo plugin installation](docs/acp/packaging-and-onboarding.md) for detailed setup and troubleshooting.

### In other ACP clients

Cortex runs as a standard ACP agent. Register it with any ACP-compatible client using:

```
python3 -B /absolute/path/to/cortex-orch-acp/src/cortex_acp.py
```

Refer to your client's documentation for the specific registration mechanism (e.g., configuration file or settings UI). See [Packaging and onboarding](docs/acp/packaging-and-onboarding.md) for detailed setup guidance for your ACP client.

## Quick configuration

Cortex stores configuration in `~/.config/cortex/`. You'll need to:

1. **Add engine connections** – Register your Codex or Claude account:

```bash
python3 -B scripts/cortex_acp_setup.py add-connection \
    --name work --engine codex-acp --provider openai \
    --auth-mode api_key --key-kind openai_api_key \
    --api-key-file /path/to/key.txt \
    --install-record /path/to/codex-install-record.json
```

There is no installer for either engine: you build this install record yourself for an engine you already have installed by some other means, and Cortex re-verifies its file hashes on every use. See [engine install records](docs/configuration.md#engine-install-records) for the exact recipe.

2. **Set defaults** – Choose a model and effort level; they'll be saved when you select them in the client.

3. **(Optional) Configure MCP servers** – Add tools for knowledge, APIs, or custom operations. See [MCP configuration](docs/configuration.md#mcp-servers-tools-and-capabilities).

4. **(Optional) Override routing** – Create `~/.config/cortex/routing.json` to control which engine/model handles specific work. See [routing configuration](docs/configuration.md#routing-controlling-which-engine-handles-work).

For comprehensive configuration documentation, see [Cortex Configuration Guide](docs/configuration.md).

## Key features

- **Multi-provider coordination** – Run Codex and Claude workers in the same task
- **Automatic routing** – Cortex selects engines and models based on request complexity
- **Fallback routing** – If a worker fails, Cortex can route to a backup provider with your consent
- **Verification and review** – Every change is reviewed before acceptance
- **Structured reporting** – Task history, worker output, and decisions are stored as Markdown and SQLite

The Paseo Usage pill reads native plan windows for Codex and Claude; it states when a provider supplied no current window. See [token and plan usage](docs/acp/token-and-plan-usage.md) for token totals and availability rules.

## Starting a task

Open your project in Cortex and describe what you want done. Cortex will propose delegated work, coordinate the agents, verify the results, and report what it accomplished.

## Further documentation

- [Packaging and onboarding](docs/acp/packaging-and-onboarding.md) – Install, first-run setup, diagnostics
- [Configuration guide](docs/configuration.md) – Detailed configuration for connections, routing, and MCP
- [Codex adapter](docs/acp/codex-acp-adapter.md) – How Cortex works with Codex
- [Claude adapter](docs/acp/claude-agent-acp-adapter.md) – How Cortex works with Claude
- [Routing and fallback](docs/acp/routing-and-fallback.md) – Worker routing, multi-provider fallback
- [ACP documentation index](docs/acp/index.md) – Full reference
- [Project storage and recovery](docs/project/storage.md) – Task storage, format upgrades, recovery
