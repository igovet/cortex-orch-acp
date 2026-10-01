# Zed Custom Agent setup for Cortex

The original qualification notes were written against **Zed 1.20.1** (see
`.cortex/acp-migration/T002/ADR-001-integration-selection.md`). The current
live observation used **Zed 1.20.2**; the registration and ACP capability
details below apply to that observed client as well. Zed's
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
      "args": ["-B", "<ABSOLUTE_PATH_TO_REPO>/src/cortex_acp.py"],
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
- Opening a new Cortex session negotiates ACP v1 capabilities: text, image,
  and embedded-resource prompt content (audio remains unsupported), and
  `loadSession: true`. `session/load` is implemented as a display-only replay;
  `session/resume`, `session/fork`, `session/list`, and `session/close`
  remain unsupported (see
  `cortex_runtime/acp/protocol.py` and `cortex_runtime/acp/replay_store.py`).
- With at least one configured Cortex connection, Zed's session config UI
  shows a **Model** selector and, when the selected route reports one, a
  **Thought level** selector (see `session/new`'s `configOptions` and
  `session/set_config_option`, T010). With no connection configured yet,
  neither selector is shown at all — never an empty/fake selector.
- An **Access** selector (config option `access_mode`, category `mode`)
  is always offered: **Guarded mode** (default for every new session) or
  **Auto mode** — see
  [permissions](permissions-and-containment.md#access-mode-guarded-and-auto).
  Its rendering in Zed has not been observed live yet.
- `/cortex-status`, `/cortex-model`, `/cortex-effort`, `/cortex-setup`,
  `/cortex-clear` are
  available as slash commands (Zed's `available_commands_update`
  fallback path) regardless of whether Zed's own config-option UI renders.
  `/cortex-status` reports direct native engine execution without making a
  model call. No host-policy repair
  is required for normal operation.
- `/cortex-clear` previews project task retention without a model call; trailing
  `confirm` deletes only unprotected tasks. Use this name, not host-reserved
  `/clear`; see [retention](packaging-and-onboarding.md#retention-maintenance).
  This implementation has no new live Zed rendering evidence.
- Sending any ordinary prompt before a connection is configured returns
  the agent's real current state: `orchestration_unavailable`
  (`-32001`, `retryable: false`) — Cortex reports this truthfully rather
  than pretending to have completed a turn. This is not a bug to hide; it
  reflects that the production entry point runs no orchestration engine
  until a real one is wired in a later task.

## Isolated Zed runs

For a throwaway live run, pass Zed an isolated `--user-data-dir`. Zed then
reads its settings from `<user-data-dir>/config/settings.json`; that path
takes precedence over `XDG_CONFIG_HOME/zed/settings.json`. Apply the Cortex
entry to the settings file under the selected user-data directory, and keep
the Zed process's user-data directory, `HOME`, and XDG state/cache directories
disposable. Set `agent_servers.Cortex.env` in that **test-only** settings file
to `{"HOME": "<REAL_USER_HOME>"}`. Only the Cortex child receives this override:
it discovers the existing Codex and Claude ACP installs and uses their normal
`~/.codex` and `~/.claude` accounts. Keep Cortex's inherited
`XDG_CONFIG_HOME` and `XDG_STATE_HOME` pointing at the disposable profile so
the connection registry and other Cortex configuration remain separate.
Do not run a separate Codex or Claude login under the disposable home.

## Tool confirmations

Cortex's downstream ACP permission bridge auto-approves recognized ordinary
worker actions and asks about deletion or unclassifiable actions. Zed has its
**own** tool-permission layer; Cortex's decision does not change Zed's setting.
The isolated live run on 2026-09-20 showed Zed prompting for ordinary
`Run Command` actions until the disposable Zed profile was configured with
`agent.tool_permissions`. With that test setting, ordinary read/test/edit
actions in the next run did not prompt. See [the observation](evidence/zed-2026-09-20/live-acp-regressions.md)
and [Zed's tool-permission documentation](https://zed.dev/docs/ai/tool-permissions).

Zed can use `"agent": {"tool_permissions": {"default": "allow"}}` to
auto-approve its own ordinary tool actions, with tool-specific confirmation
rules for `delete_path`, `move_path`, and destructive terminal commands.
These are **Zed-wide** settings, not a Cortex-only option, so the Cortex
registration helper does not silently change them. Regex checks for terminal
commands cannot prove that an arbitrary script will not delete a file; native
engine tools can also act without an ACP permission request. The exact
“everything except deletion” boundary is therefore not guaranteed by this
setting alone. Review a Zed-wide permission change explicitly for the
installed client version before using it outside an isolated profile.

## Uninstall

Remove the `"Cortex"` entry from `agent_servers` in Zed's `settings.json`
(manually, or restore one of the helper's own timestamped
`settings.json.bak.<timestamp>` backups). This does not touch any Cortex
connection/credential state, which lives entirely under
`$XDG_CONFIG_HOME/cortex` and `$XDG_STATE_HOME/cortex`, never inside Zed's
own configuration.
