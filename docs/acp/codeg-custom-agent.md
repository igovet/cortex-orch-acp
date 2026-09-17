# codeg 0.30.8 Custom Agent recipe (partly unverified)

The second real ACP client for qualification is **codeg 0.30.8**
(`/usr/bin/codeg`; amends ADR-001 — see
`.cortex/acp-migration/T002/ADR-001a-second-client-codeg.md`). This page
records what is actually known and, explicitly, what is not.

## UNKNOWN: does codeg accept a raw local command?

codeg's documented Custom Agent flow (Settings → Agents) advertises
distribution channels for **npx**, **uvx**, and **binary** (per scout
research against `docs.codeg.app/guide/custom-agents`, not independently
re-verified by this task). Whether any of those channels — or a separate,
undocumented one — accepts an arbitrary already-installed local command
(the same `python3 -B <abs>/cortex_acp.py` launch Zed's custom agent uses)
is **UNKNOWN** and is not assumed here. This must be determined by an
actual GUI check in T035 (see the runbook), not guessed.

Candidate approaches, explicitly marked unverified:

- **binary channel** pointing at a local wrapper executable that just
  execs `python3 -B <abs>/cortex_acp.py` — plausible if codeg's "binary"
  channel accepts an arbitrary local path, but this has not been tried
  against the installed codeg build. Packaging such a wrapper (if needed)
  is a possible T031 concern, not done here.
- **uvx channel** — would require Cortex to be an installable Python
  package/entry point; out of scope for this task and not attempted.
- No channel accepting a raw absolute command — in which case Custom Agent
  registration in codeg may not be possible without further codeg-side
  packaging work; this must be reported honestly, not worked around with a
  fake distribution artifact.

`scripts/cortex_acp_client_config.py --print-codeg-recipe` prints these
same candidate steps for convenience; it never writes to any codeg
configuration file (codeg's own data lives under `~/.config/app.codeg` and
`~/.local/share/app.codeg`, which this task does not open or modify).

## codeg-mcp injection

codeg injects its own `codeg-mcp` stdio MCP server into every agent session
it hosts (see the ADR-001a evidence table). Cortex's `session/new` handling
strips every `mcpServers` entry down to shape-only metadata (name, type,
and MCP env/header *names* only, never values —
`cortex_runtime/acp/protocol.py:strip_mcp_servers`) and never trusts or
routes any Cortex-internal call through an injected server; this was
already implemented and tested in T009 (`test_acp_protocol.py`) and needs
no codeg-specific change. Treat any codeg-injected MCP server as untrusted,
inert metadata.

## Auto-updater / version recording

codeg has a built-in Tauri auto-updater (observed upstream URL pattern
`https://github.com/xintaofei/codeg/releases/latest/download/latest.json`
— see ADR-001a). Any real qualification run (T035) must record the exact
installed codeg version at the time of the run (`dpkg -s codeg` or the
in-app "About" version), since an auto-update between runs invalidates
prior capability observations.

## fs write roots / other open gaps

codeg restricts filesystem write roots (workspace, tmp, plus
`CODEG_ACP_FS_EXTRA_ROOTS`); Cortex's agent never issues an `fs/*`
client-request in this release (`agentCapabilities` advertises no such
need), so this does not currently affect Cortex, but is recorded here per
ADR-001a for T035's awareness.

## This task's scope

This task prepares the recipe/candidates and the shared no-paid smoke
harness (client-agnostic: `scripts/cortex_acp_smoke.py` exercises the same
`cortex_acp.py` entry point codeg or Zed would launch). It does **not**
claim codeg custom-agent registration has been performed or verified — see
[real-client-runbook.md](real-client-runbook.md), whose codeg rows are all
`NOT RUN`, owner T035.
