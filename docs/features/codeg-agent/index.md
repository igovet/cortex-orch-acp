# codeg 0.30.8 Custom Agent recipe (partly unverified)

> Status: migrated from the former ACP reference. Statements were not re-verified line by line against source in this pass; claims without a source path are unverified until reviewed.


The second real ACP client for qualification is **codeg 0.30.8**
(`/usr/bin/codeg`). This page
records what is actually known and, explicitly, what is not.

## UNKNOWN: does codeg accept a raw local command?

codeg's documented Custom Agent flow (Settings → Agents) advertises
distribution channels for **npx**, **uvx**, and **binary** (per scout
research against `docs.codeg.app/guide/custom-agents`, not independently
re-verified in this revision). Whether any of those channels — or a separate,
undocumented one — accepts an arbitrary already-installed local command
(the same `python3 -B <abs>/cortex_acp.py` launch Zed's custom agent uses)
is **UNKNOWN** and is not assumed here. Confirm this against the installed
client before relying on a local-command setup.

Candidate approaches, explicitly marked unverified:

- **binary channel** pointing at a local wrapper executable that just
  execs `python3 -B <abs>/cortex_acp.py` — plausible if codeg's "binary"
  channel accepts an arbitrary local path, but this has not been tried
  against the installed codeg build. Packaging such a wrapper (if needed)
  is a possible packaging concern, not done here.
- **uvx channel** — would require Cortex to be an installable Python
  package/entry point; not attempted.
- No channel accepting a raw absolute command — in which case Custom Agent
  registration in codeg may not be possible without further codeg-side
  packaging work; this must be reported honestly, not worked around with a
  fake distribution artifact.

`scripts/cortex_acp_client_config.py --print-codeg-recipe` prints these
same candidate steps for convenience; it never writes to any codeg
configuration file (codeg's own data lives under `~/.config/app.codeg` and
`~/.local/share/app.codeg`, which the recipe does not open or modify).

## codeg-mcp injection

codeg injects its own `codeg-mcp` stdio MCP server into every agent session
it hosts (see the ADR-001a evidence table). Cortex's `session/new` handling
strips every `mcpServers` entry down to shape-only metadata (name, type,
and MCP env/header *names* only, never values —
`cortex_runtime/acp/protocol.py:strip_mcp_servers`) and never trusts or
routes any Cortex-internal call through an injected server; this was
already implemented and tested (`test_acp_protocol.py`) and needs
no codeg-specific change. Treat any codeg-injected MCP server as untrusted,
inert metadata.

## Auto-updater / version recording

codeg has a built-in Tauri auto-updater (observed upstream URL pattern
`https://github.com/xintaofei/codeg/releases/latest/download/latest.json`
— see ADR-001a). When recording a codeg capability observation, include the
exact installed version (`dpkg -s codeg` or the in-app "About" version),
since an auto-update between observations invalidates prior results.

## fs write roots / other open gaps

codeg restricts filesystem write roots (workspace, tmp, plus
`CODEG_ACP_FS_EXTRA_ROOTS`); Cortex's agent never issues an `fs/*`
client-request in this release (`agentCapabilities` advertises no such
need), so this does not currently affect Cortex, but is recorded here per
ADR-001a as an open compatibility detail.

## Scope of this recipe

This section prepares the recipe and candidates and the shared no-paid smoke
harness (client-agnostic: `scripts/cortex_acp_smoke.py` exercises the same
`cortex_acp.py` entry point codeg or Zed would launch). It does **not**
claim codeg custom-agent registration has been performed or verified — see
the ACP adapter documentation for current integration behavior.
