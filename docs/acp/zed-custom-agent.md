# Zed 1.20.1 Custom Agent setup for Cortex

Applies to the qualified client version **Zed 1.20.1** (see
`.cortex/acp-migration/T002/ADR-001-integration-selection.md`). Zed's
Custom Agent mechanism (`zed.dev/docs/ai/external-agents`) launches an
arbitrary local command speaking ACP over stdio and lists it as a
separate agent in the Agent panel — no Registry approval is required.

## What this does, and does not, do

- Registers Cortex as **one additional, separately-listed agent** named
  `Cortex` in Zed's Agent panel. It does **not** change Zed's built-in
  provider configuration, and selecting a built-in Zed provider never
  configures or activates Cortex (see `INVARIANTS.md`/T011 non-negotiable
  boundary: "Do not imply built-in Zed provider configuration automatically
  configures Cortex").
- Cortex only starts when the user explicitly picks the `Cortex` agent in
  Zed and opens/starts a session with it. Nothing about this repository
  being named "Cortex", or `.cortex` existing in a project Zed has open,
  causes any activation on its own (see `AGENTS.md`).
- Zed launches the process with its own environment; Cortex's registered
  `env` is an **empty object** — Cortex never inherits or requires any Zed
  provider API key or credential (`INVARIANTS.md`: "No undocumented
  Zed-credential inheritance"). Cortex's own model/provider connections are
  configured separately, through Cortex's own connection onboarding (see
  `/cortex-setup`), never through Zed.

## Manual JSON snippet (absolute paths — placeholders shown, use your own)

Add this inside the top-level `"agent_servers"` object in Zed's
`settings.json` (`~/.config/zed/settings.json` on Linux):

```json
{
  "agent_servers": {
    "Cortex": {
      "type": "custom",
      "command": "<ABSOLUTE_PATH_TO_python3>",
      "args": ["-B", "<ABSOLUTE_PATH_TO_REPO>/plugins/cortex/scripts/cortex_acp.py"],
      "env": {}
    }
  }
}
```

Every other key in the file, and any other entry already inside
`agent_servers`, must be left exactly as it was.

## Helper usage (default is a dry run — nothing is written)

```sh
# Print the exact entry to add (no file touched):
python3 -B scripts/cortex_acp_client_config.py

# Preview against a real settings file (still no write): prints only the
# Cortex entry itself plus a one-line summary ("would add"/"would replace"/
# "already identical") -- never a full-file diff, so no other agent's
# value (including anything secret-looking) is ever echoed:
python3 -B scripts/cortex_acp_client_config.py --settings ~/.config/zed/settings.json

# Actually write it (explicit consent required; a timestamped backup is
# written first; the write is atomic; the file mode is preserved; every
# other key/entry is preserved):
python3 -B scripts/cortex_acp_client_config.py \
  --settings ~/.config/zed/settings.json \
  --apply --i-consent-to-write

# If a *different* "Cortex" entry already exists, --apply refuses unless
# --replace is also given:
python3 -B scripts/cortex_acp_client_config.py \
  --settings ~/.config/zed/settings.json \
  --apply --i-consent-to-write --replace
```

If the settings file is JSONC (comments/trailing commas — Zed accepts this,
but the helper deliberately refuses to parse-and-rewrite it, to avoid
silently dropping comments or reformatting unrelated content), the helper
prints the manual JSON snippet above with instructions instead of writing
anything.

## Expected UI once registered

- Zed's Agent panel lists **Cortex** as its own agent, distinct from any
  built-in provider or other custom agent.
- Opening a new Cortex session negotiates ACP v1 capabilities: text-only
  prompts (no image/audio/embedded-context), `loadSession: false` (no
  session resume/fork/list/close in this release — see
  `cortex_runtime/acp/protocol.py`).
- With at least one configured Cortex connection, Zed's session config UI
  shows a **Model** selector and, when the selected route reports one, a
  **Thought level** selector (see `session/new`'s `configOptions` and
  `session/set_config_option`, T010). With no connection configured yet,
  neither selector is shown at all — never an empty/fake selector.
- `/cortex-status`, `/cortex-model`, `/cortex-effort`, `/cortex-setup` are
  available as slash commands (Zed's `available_commands_update`
  fallback path) regardless of whether Zed's own config-option UI renders.
- Sending any ordinary prompt before a connection is configured returns
  the agent's real current state: `orchestration_unavailable`
  (`-32001`, `retryable: false`) — Cortex reports this truthfully rather
  than pretending to have completed a turn. This is not a bug to hide; it
  reflects that the production entry point runs no orchestration engine
  until a real one is wired in a later task.

## Uninstall

Remove the `"Cortex"` entry from `agent_servers` in Zed's `settings.json`
(manually, or restore one of the helper's own timestamped
`settings.json.bak.<timestamp>` backups). This does not touch any Cortex
connection/credential state, which lives entirely under
`$XDG_CONFIG_HOME/cortex` and `$XDG_STATE_HOME/cortex`, never inside Zed's
own configuration.
