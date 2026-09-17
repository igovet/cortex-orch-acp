# AionUI 2.2.2 Custom Agent recipe (partly unverified)

The second real ACP client for qualification is **AionUI 2.2.2**
(`/usr/bin/AionUi`, deb-installed app under `/opt/AionUi`; supersedes codeg
per the ADR-001a amendment history — see
`.cortex/acp-migration/T002/ADR-001a-second-client-codeg.md` for the
now-historical codeg record). This page records what is actually known
about AionUI's Custom Agent support and, explicitly, what is not.

## Registration flow (verified from the AionUI wiki)

Per the upstream wiki
(`https://github.com/iOfficeAI/AionUi/wiki/ACP-Setup`) and strings observed
in the installed `/opt/AionUi/resources/app.asar`, AionUI registers a
Custom Agent through its own GUI, not a user-editable settings file:

1. Open AionUI → Settings → Agent Management → Custom Agents → Add Custom
   Agent.
2. The form accepts these JSON-shaped fields:
   - `name` — display name, e.g. `"Cortex"`.
   - `defaultCliPath` — the launch command (an absolute path, e.g. the
     `python3` interpreter).
   - `acpArgs` — an array of arguments, e.g. `["-B", "<abs>/cortex_acp.py"]`.
   - `env` — an object of environment variables (empty for Cortex, same
     rationale as the Zed entry: no credential is read from process
     environment).
   - `enabled` — boolean.
3. Save. AionUI stores the entry in its own backend database (not a file
   this project can inspect or write); there is no Registry/JSONC file
   equivalent to Zed's `settings.json` for this client.

`scripts/cortex_acp_client_config.py --print-aionui-recipe` prints these
same manual steps and the JSON field values for convenience; it never
attempts to write to AionUI's backend database, and this task does not
open or modify anything under `~/.config/AionUi` (AionUI's own private
data directory).

## Explicitly UNKNOWN

The following are not verified against a live AionUI session and must be
determined by an actual GUI check in T035 (see the runbook), not guessed:

- The exact `clientCapabilities` (fs/terminal) AionUI advertises at
  `initialize`.
- Whether/how AionUI uses `session/load`.
- How AionUI renders `configOptions` (model/thought-level selector
  equivalent).
- Whether AionUI supports slash commands (`/cortex-status`, etc.).
- AionUI's permission-request UI shape for `session/request_permission`.
- Whether AionUI renders embedded resources from `agent_message_chunk`.
- Whether AionUI injects its own `mcpServers` into `session/new` params
  (unlike codeg's confirmed `codeg-mcp` injection, this has not been
  observed for AionUI). If it does, the same `strip_mcp_servers` handling
  (`cortex_runtime/acp/protocol.py`) applies unconditionally to any
  `mcpServers` entry regardless of source, so no AionUI-specific change is
  needed even once this is confirmed.

## Launch protocol (verified)

AionUI launches a Custom Agent as a stdio subprocess speaking ACP
JSON-RPC/NDJSON, protocol version 1, and calls `session/new` with a `cwd`
— the same launch shape Zed uses and the same one
`scripts/cortex_acp_smoke.py` exercises. No AionUI-specific protocol
divergence is currently known.

## This task's scope

This task prepares the recipe and the shared no-paid smoke harness
(client-agnostic: `scripts/cortex_acp_smoke.py` exercises the same
`cortex_acp.py` entry point AionUI or Zed would launch). It does **not**
claim AionUI custom-agent registration has been performed or verified —
see [real-client-runbook.md](real-client-runbook.md), whose AionUI rows
are all `NOT RUN`, owner T035.
