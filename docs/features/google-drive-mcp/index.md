# Google Drive and Docs MCP for Cortex

> Status: migrated from the former ACP reference. Statements were not re-verified line by line against source in this pass; claims without a source path are unverified until reviewed.


The Google Drive plugin installed in Codex is a Codex app connector. Its
`.app.json` names a hosted connector; it does not install a local MCP command
or make that connector's authorization available to Cortex. Cortex's current
additional-server registry accepts stdio commands only. Google's own Drive MCP
endpoint is [remote HTTP and in Developer Preview](https://developers.google.com/workspace/drive/api/reference/mcp),
so it is not a direct registry entry either.

For a local Drive and Docs server, [Workspace MCP](https://github.com/taylorwilsdon/google_workspace_mcp)
documents stdio transport, service selection and cumulative tool tiers. The
following is a **disabled template** for the owner-only
`$XDG_CONFIG_HOME/cortex/mcp-servers.json` registry. Merge it with existing
entries rather than replacing the file. Disabled entries are skipped before
Cortex attempts to launch a command, so the template is safe to retain before
installing or authorizing the server.

```json
{
  "version": 1,
  "servers": {
    "google-workspace": {
      "command": "/home/igovet/.local/share/cortex/mcp/workspace-mcp-1.27.1/bin/workspace-mcp",
      "args": [
        "--tool-tier", "core", "--tools", "drive", "docs"
      ],
      "roles": ["coordinator"],
      "enabled": false
    }
  }
}
```

On the current host, `workspace-mcp==1.27.1` is installed at that versioned
user-local path and the disabled entry is present in the owner-only registry.
The versioned executable avoids package installation or network resolution
when an ACP session starts.

Activation requires a separate Google Cloud OAuth client of type **Desktop
application**, the Drive and Docs APIs enabled for that project, and the
account's consent in Workspace MCP's local OAuth flow. The Codex plugin's
connection does not supply those credentials. Keep the OAuth client secret
and resulting tokens outside this repository. Follow the server's
[current OAuth and stdio instructions](https://workspacemcp.com/quick-start)
for credential placement and authorization. The server accepts
`GOOGLE_OAUTH_CLIENT_ID` and `GOOGLE_OAUTH_CLIENT_SECRET`; supply them only in
the owner-private environment or registry, never in a project file. Complete
the separate Google consent flow and a read-only Drive/Docs probe before
setting `enabled` to `true` in the owner-only registry. After enabling, use
diagnostics and an actual `tools/list` observation to confirm the pinned
command starts and exposes only the selected services. A live Cortex session
must separately verify that the engine can use the tools before the connection
is treated as qualified for work.

Codex ACP currently does not provide deferred `tool_search`. The pinned
command above returned **12 tools** in a local, unauthenticated stdio
`initialize` and `tools/list` check on 2026-09-21. Keep this server at no more
than 20 advertised tools; use Workspace MCP's `--disabled-tools` filter or a
narrower permission set if later versions exceed that budget. This check proves
only startup and catalogue shape, not Google authorization or tool execution.
Do not claim authenticated access until the corresponding authorization and tool-use signals have been observed.
