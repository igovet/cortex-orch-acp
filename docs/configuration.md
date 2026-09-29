# Cortex configuration

This reference describes configuration the current ACP runtime actually reads. The source of truth is the loaders named in each section. Private files are local operator state; do not commit credentials or real connection registries.

## Where configuration lives

| Data | Default path | Scope and precedence |
|---|---|---|
| Connection registry | `$XDG_CONFIG_HOME/cortex/connections.json` or `~/.config/cortex/connections.json` | User-wide. Managed by `cortex_acp_setup.py`; it has no project override. |
| Connection secrets | `$XDG_CONFIG_HOME/cortex/secrets/<connection-id>` | User-wide private files, written by setup. Native-account credentials can instead be referenced from the existing user engine directory. |
| User defaults and policy ceiling | `$XDG_CONFIG_HOME/cortex/defaults.json` | User-wide base configuration. |
| Routing rules | `<project>/.cortex/routing.json`, then `$CORTEX_ROUTING`, then `$XDG_CONFIG_HOME/cortex/routing.json` (or `~/.config/cortex/routing.json`) | First existing file wins; files are not merged. Setup writes the user file's default `{"deny": ["gpt-5.5", "haiku"]}`. |
| Cortex-owned external MCP servers | `$XDG_CONFIG_HOME/cortex/mcp-servers.json` | User-wide only. Project-local MCP registries are intentionally ignored. |
| Engine-native state and catalog snapshots | `$XDG_STATE_HOME/cortex/connections/<id>/...` or `~/.local/state/cortex/...` | User-wide mutable state, separate from configuration. |
| Paseo launch record | `~/.config/cortex/paseo-plugin-launch.json` | Owner-private non-secret command, environment and shim metadata written by the setup helper. |
| ssh password dialog (optional, off) | `$XDG_CONFIG_HOME/cortex/ssh-askpass.json` | User-wide, written by hand. See [ssh from workers](#ssh-from-workers). |
| Task and report store | `<project>/.cortex/cortex.sqlite3` and sibling published Markdown | Project data, not operator configuration. |

`HOME`, `XDG_CONFIG_HOME` and `XDG_STATE_HOME` must resolve to absolute paths. The user roots cannot be placed inside a project workspace. Cortex creates managed directories owner-only and validates existing files before trusting them. See `connections.registry.config_root/state_root`, `configuration.store`, `routing.auto_route._routing_path`, `configuration.mcp_servers._global_path`, and `scripts/cortex_acp_setup.py::_write_paseo_launch_config`.

## Connections

Use the setup command to add or change connections; do not hand-edit `connections.json`. Setup validates the engine install record, creates IDs and canonical credential references, and writes the registry atomically. Credential values are read from a file or stdin, never from command-line text.

The supported engines are `codex-acp` and `claude-agent-acp`. Authentication modes are `native_account` and `api_key`; `gateway` is represented in the type enum but has no implemented launch path and is not usable. Codex API-key kind is `openai_api_key`; Claude accepts `anthropic_api_key` or `claude_oauth_token`. Native account connections may reference the user's existing `~/.codex` or `~/.claude` directory, or use Cortex-managed isolated engine state. The supported combinations come from `connections.model.SUPPORT_MATRIX`; onboarding is in `connections.onboarding`.

Typical setup:

```bash
python3 -B scripts/cortex_acp_setup.py status
python3 -B scripts/cortex_acp_setup.py add-connection --help
```

An API key should be supplied with `--api-key-file PATH` or `--api-key-stdin`, never inline in arguments. For native login, check the engine's native login state before depending on it. The CLI's `status` output reports IDs and credential kind, not credential values.

The registry JSON is intentionally an implementation format, not a hand-authored config API. Its currently accepted root keys are `schema_version` (integer, exactly `1`), `connections` (object keyed by connection ID), and optional `override_history` (object). Each generated connection record has the fields below; records are written by setup and should be treated as read-only:

- `id`, `name`, `engine`, `provider`: strings; `id` is generated and matches its object key.
- `auth_mode`: `native_account`, `api_key`, or the unsupported `gateway` enum value.
- `key_kind`: null for native account; one supported key kind for API-key mode.
- `endpoint`, `launch_command`: nullable endpoint string and nullable string array; setup derives/approves launch details.
- `credential_ref`: generated tagged object (`engine_native_dir`, `secret_file`, or `os_keyring`); it identifies storage and never embeds the secret.
- `approved_fingerprint`: generated string binding the approved install/connection configuration.
- `state`: `configured`, `authorization_required`, `unavailable`, or `invalid`.
- `qualified`: boolean registration capability marker; it is not proof of live client qualification.
- `created_at`, `updated_at`: ISO timestamp strings.
- `max_concurrent_sessions`: integer from 1 to 64; defaults to 4 when absent in an older record.

The closed fields and defaults are enforced by `connections.registry._parse_registry_bytes/_record_from_dict` and `connections.model.ConnectionRecord`. Unknown fields fail closed.

### Engine install records

There is no installer for either engine. `add-connection --install-record FILE` takes an operator-recorded install of an engine you already installed by other means (for example `npm install @agentclientprotocol/codex-acp`). It holds absolute paths without symlink components and the SHA-256 of each pinned file. `verify_install()` re-hashes every file before launch. The JSON shape follows `engine.codex_install.CodexAcpInstall` or `engine.claude_install.ClaudeAcpInstall`: `node_binary`, `adapter_entry`, `package_json`, `package_version` (must equal the package's own `"version"`) and their digests, plus the bundled Codex binary and version for Codex only. `tests/fixtures/engine/codex/install-record.template.json` shows the shape without real hashes. The field-by-field recipes are in the [Codex adapter](acp/codex-acp-adapter.md#current-runtime-gateway-policy) and [Claude adapter](acp/claude-agent-acp-adapter.md#pinned-install-requirements-manual-recipe-placeholder) pages.

Accepted adapter versions (`connections.discovery.QUALIFIED_ADAPTER_VERSIONS`): codex-acp exactly `1.12.0` or `1.13.0` (an allowlist, not a floor; any other version is refused). claude-agent-acp has no allowlist, and `pinned-dependencies.json` records the last tested version.

## Defaults and policy ceiling

`defaults.json` is created under the user config root. The supported top-level keys are `defaults` and optional `ceiling`; when missing, defaults are empty and the ceiling is unrestricted. This file is persisted by the route-selection/configuration flow; manual edits should be cautious because the route identifiers are derived from observed engine catalogs.

A route default has this shape:

```json
{
  "defaults": {
    "route_id": "xr_<observed-route-id>",
    "route_connection_id": "cn_<configured-connection-id>",
    "route_model": "gpt-5.6-luna",
    "parameters": {"thought_level": "medium"}
  }
}
```

The model, connection ID and parameters must correspond to an observed selectable route. Parameter values such as `thought_level`, `effort`, `mode`, `model_config` and `summarizer` are accepted only when the selected route's observed catalog advertises that exact name/value pair. Do not copy a made-up route ID or assume all engines accept the same parameters. Configuration selection is revalidated by `configuration.resolution` and `configuration.service.ConfigurationService`.

The optional `ceiling` object may only narrow permissions and route access. Its exact fields are:

```json
{
  "allowed_routes": null,
  "allowed_connections": ["cn_<configured-connection-id>"],
  "allowed_data_recipients": null,
  "permission_level": "STANDARD",
  "limits": {"attempts": 8},
  "spend_limit": null,
  "spend_limit_enforceable": false,
  "fallback_policy": {
    "allowed_target_routes": null,
    "allowed_failure_classes": ["engine_failure"],
    "budget": 0,
    "data_recipient_consent": false
  }
}
```

- Route/connection/recipient fields are arrays of strings or null. Null means unrestricted at that policy layer.
- `permission_level` is null or `READ_ONLY`, `STANDARD`, or `ELEVATED`.
- `limits` is an object of named integer limits. Only `attempts` is enforced for routing attempts currently; `calls` and `tokens` are not metered by this code.
- `spend_limit` is a number or null; `spend_limit_enforceable` is a boolean.
- Fallback route/failure lists are arrays of strings or null; `budget` is an integer and `data_recipient_consent` is boolean. Fallback remains disabled without the required policy and any required per-recipient consent record.

The accepted ceiling model is `configuration.policy.PolicyCeiling`; serialization/loading is `configuration.store.serialize_ceiling/deserialize_ceiling/read_user_defaults`. A project ceiling is stored in the project's SQLite configuration table and may further restrict the user ceiling. There is no second project JSON file for defaults. Task and assignment configuration can narrow the effective ceiling but cannot grant more.

Access mode is a separate task-scoped user choice, not a JSON option: new tasks default to Guarded, and the user may select Guarded or Auto in the client. It is stored as task events in the project database (`configuration.access_mode`).

## Routing rules

The routing loader reads six keys: `pin`, `deny`, `auto_allow`, `tiers`, `providers`, and `engine_affinity`; additional JSON keys are currently ignored. Its validation and selection logic are in `routing.auto_route`. This is the complete supported shape:

```json
{
  "pin": {
    "worker-security-auditor": "claude-agent-acp/opus:high",
    "*": "codex-acp/gpt-5.6-luna"
  },
  "deny": ["gpt-5.5", "haiku"],
  "auto_allow": ["claude-agent-acp/haiku"],
  "tiers": {
    "gpt-5.6-luna": "light",
    "gpt-5.6-sol": "standard"
  },
  "providers": ["codex-acp", "claude-agent-acp"],
  "engine_affinity": {
    "implementation": "claude-agent-acp",
    "testing": null,
    "*": "codex-acp"
  }
}
```

- `pin`: object mapping worker role names or `*` to `engine/model` or `engine/model:effort`. A pin is strict: unavailable or disallowed pins fail instead of silently switching routes.
- `deny`: array of model tokens, each optionally prefixed `engine/`. A token is an exact model id; a versioned id, which also covers that version's variants (`gpt-5.5` covers `gpt-5.5[1m]`, not `gpt-5.6-luna`); or a bare family name, which covers every version and alias of that family (`haiku` covers `haiku`, `claude-haiku-4-5` and `claude-haiku-4-5-20251001`). A token with a `[...]` variant is exact only. Explicit deny applies to automatic routing, pins, and the model selector, and always wins.
- `auto_allow`: tokens of the same form that may enter automatic selection despite the built-in Haiku exclusion. It does not override `deny`, `providers`, catalog availability or quota limits.
- `tiers`: object mapping model names to `flagship`, `deep`, `standard`, or `light`. Built-in known families/models are classified automatically; this key overrides that classification. A Luna model moved out of `light` routes like any model of its new tier; only the Luna effort floor (`high`) and `max` still apply.
- `providers`: array restricted to `codex-acp` and/or `claude-agent-acp`. When absent, every configured provider is considered. A disallowed pin is refused.
- `engine_affinity`: object mapping a work kind or `*` to `codex-acp`, `claude-agent-acp`, or `null` (no lean). Work kinds are `implementation`, `operations`, `debugging`, `testing`, `investigation`, `review`, `design`, `writing`, `planning`, and `analysis` (each worker role's kind is `routing.auto_route.ROLE_WORK_KINDS`; an unlisted role is `implementation`). A kind named here wins, then `*`, then the built-in default: `implementation` and `operations` lean to `codex-acp`; `investigation`, `review`, `design`, `writing`, `planning` and `analysis` lean to `claude-agent-acp`; `debugging` and `testing` are neutral. Affinity breaks ties only after plan usage and retry diversification (a plan at least 10 points less used wins first), and it is itself broken by the 85% quota band only as a late tie breaker; it never overrides a pin, `deny`, `providers`, availability or the 100% quota exclusion. An unknown kind or engine makes the file invalid.

Precedence is first existing routing file: project `<project>/.cortex/routing.json`, then `CORTEX_ROUTING`, then user `routing.json`. The files do not merge, so a project file that should keep the defaults must list them itself. An existing malformed file fails closed. If no file exists, built-in automatic routing applies.

Default user file: `scripts/cortex_acp_setup.py init-storage`, `apply-routing-defaults`, `install-paseo-plugin` and the Paseo plugin's Git install hook (`bootstrap-paseo-git-install`) create `$XDG_CONFIG_HOME/cortex/routing.json` as exactly `{"deny": ["gpt-5.5", "haiku"]}` when it is absent. When the file already exists, setup keeps every setting and only appends the default `deny` tokens it lacks. It reports an invalid file and leaves it untouched. The same step removes every route whose model the global `deny` names from a leftover `$XDG_CONFIG_HOME/cortex/route-policy.json`, the per-role policy file that automatic routing replaced and the runtime no longer reads. It keeps a private `route-policy.json.cortex-backup-<ns>` copy first, preserves all other content, and removes a role left with no route. Neither the plugin nor the package ships a `route-policy.json`. Project files and `$CORTEX_ROUTING` are never modified. See `routing.defaults`.

Latest version only: automatic routing uses only the newest engine-confirmed version of each model family on an engine. Codex `gpt-6-luna` supersedes `gpt-5.6-luna`; Claude `opus[1m]` ("Opus 5.5") supersedes `claude-opus-5[1m]` and `claude-opus-4-8`, and `claude-sonnet-5` supersedes `claude-sonnet-4-6`. Family and version are parsed from the model id, or for a versionless alias from the engine's own display name for the same family, so a new release takes over without configuration. `deny` is applied first, so if it removes the newest version, the newest non-denied version is routed. A model whose version cannot be parsed, such as a bare `sonnet` alias, is never treated as older. A superseded version stays in the model selector. It is reported with `auto_eligible: false` and `superseded_by` in `list_route_options`, and under `superseded` in the route decision. A plan hint naming it is recorded as `ignored:superseded`. An explicit `pin` of an older version is still honoured. Supersession does not depend on availability: an older version does not take over while the newest is exhausted or recorded unavailable.

Legacy `route-policy.json`: `~/.config/cortex/route-policy.json` (`$XDG_CONFIG_HOME/cortex/route-policy.json`) is a legacy file. The runtime no longer reads it: `routing.json` plus automatic routing replaced it. Its model lists, whether or not they name Luna, have no effect. Setup only prunes denied routes from it and never deletes it. Configure routing in `routing.json`; the available models and their tiers are in `list_route_options`.

Luna policy: the Codex Luna line (`gpt-6-luna`, and its newest non-denied successor) is never run below `high` effort, even by a pin. Its effort follows the work: `high` for light work, `xhigh` for ordinary standard-tier testing, and `max` for a standard-tier testing retry or fix after requested changes. In the standard tier it is a candidate only for testing work (`qa-engineer`, `build-verification`), where it is ranked alongside the tier's own models; all other standard-tier work (implementation, operations, debugging, design, review and the rest) never runs on Luna. A normal standard-tier testing retry or review fix retains this candidate pool instead of automatically moving to deep; other standard-tier retries escalate normally. In the retained pool Luna targets max, while another selected model raises effort one step (normally medium to high). Retry diversification, plan usage and affinity still decide; hints are ignored on escalation. If this pool has no usable route, the usual tier fallback applies. It is never used for `complexity: high`, deep or flagship work, or deep roles, not even as a fallback. The rule is built in; `routing.json` has no per-work-kind or per-complexity setting for it. See [routing and fallback](acp/routing-and-fallback.md#codex-luna).

Haiku policy: the default user `routing.json` denies `haiku`, which removes every Claude Haiku model from automatic routing, pins and the selector. Independently of any file, Claude Haiku is also excluded from automatic worker model selection (`routing.auto_route.DEFAULT_AUTO_DENY_FAMILIES`). The light tier then has only Codex Luna, so light work runs on Luna: a task is never moved to a stronger tier (such as Claude Sonnet) because another engine lacks a model in its tier. To let Claude take light work, add Haiku to `auto_allow` (and remove it from `deny`). Only when no engine has a usable light model does light work fall back to the nearest other tier; if no route is usable, dispatch refuses. A matching explicit pin may select Haiku despite the default automatic exclusion. An explicit `deny` entry blocks it everywhere, including pins. Routing decisions are made by the runtime; models do not choose their own models.

Automatic routing uses engine-observed catalog routes, excludes exhausted/unavailable routes and chooses among tiers based on role, task complexity, prior retry/escalation, provider constraints and current plan usage. Within the tier, the plan with clearly more remaining quota (at least 10 points less used) takes the work; otherwise a retry's engine diversification, then the work kind's engine affinity decide. Only once none of those separate the candidates does the 85% quota band act, as a late tie breaker (an account at or above it ranks just behind one below it); crossing 85% alone no longer overrides plan usage, diversification or affinity. Finally exact used percent, then engine order decide. How often an engine was chosen before is not an input. Tier names do not guarantee that a specific model is installed or selectable. See [routing and fallback](acp/routing-and-fallback.md).

Each new plan node includes a `model` recommendation: `auto`, an engine ID, or an exact `engine/model` label returned by `list_route_options`. A non-`auto` recommendation requires `model_reason`. It does not choose effort or tier and cannot bypass a pin, deny rule, provider rule, availability, or quota limit. The one tier exception is a flagship route (Astra, Fable) the user explicitly asked for in the task: the runtime reads that request only from the user's own recorded messages and answers, never from the hint or `model_reason`, and records it in the route audit; `routing.json` has no setting for it, and `deny`, `providers` and `pin` still win. See [routing and fallback](acp/routing-and-fallback.md#user-requested-flagship-route). The route menu is a read-only snapshot; plan submission and dispatch validate current policy. Older stored plans without `model` keep their provider hint.

## MCP servers for Cortex and workers

Cortex may pass session MCP entries supplied by the ACP client, and it can load owner-managed stdio servers from `$XDG_CONFIG_HOME/cortex/mcp-servers.json`. A project-local MCP registry is deliberately unsupported: changing project contents must not install a persistent executable capability. The loader is `configuration.mcp_servers.load_mcp_servers`; setup and engine sessions validate explicit tool grants and containment.

Example (replace executable, tool names and paths with values appropriate to your installed server):

```json
{
  "version": 1,
  "servers": {
    "example-index": {
      "command": "/opt/example-index/bin/server",
      "args": ["--stdio"],
      "env": {"INDEX_CACHE": "/home/alex/.cache/example-index"},
      "allowed_tools": ["search", "read_result"],
      "read_only_tools": ["search", "read_result"],
      "coordinator_tools": ["search"],
      "read_paths": ["/home/alex/projects"],
      "write_paths": ["/home/alex/.cache/example-index"],
      "network": false,
      "runtime_dir_env": ["EXAMPLE_RUNTIME_DIR"],
      "roles": ["coordinator", "worker"],
      "engines": ["codex-acp", "claude-agent-acp"]
    }
  }
}
```

Fields:

- Root `version`: integer, exactly `1`; `servers`: object keyed by server name. The loader also accepts `mcpServers` as an alias for the server map.
- Required `command`: non-empty executable string; `args`: optional string array, default `[]`.
- Optional `enabled`: boolean; `false` skips this registry entry. When omitted it defaults to true.
- `env`: optional environment object or array of `{"name":"KEY","value":"VALUE"}` objects; names and values must be strings without NUL. Reserved launch variables cannot be overridden. Do not put real credentials in this file; protect this owner-only registry and prefer a server's supported external secret source.
- Required `allowed_tools`: non-empty list of exact tool names granted to Cortex. Unknown or omitted tools are not implicitly granted.
- `read_only_tools`: optional array, default `[]`, subset of `allowed_tools`.
- `coordinator_tools`: optional array, subset of `read_only_tools`; use this to limit which read-only tools the coordinator receives.
- `read_paths`, `write_paths`: optional arrays of absolute paths, default `[]`. Writes are limited to explicit grants and the engine jail's scope.
- `network`: optional boolean, default `false`.
- `runtime_dir_env`: optional unique array of environment-variable names. The runtime provides a private per-server directory through these names. `PATH`, `HOME`, `TMPDIR`, and `XDG_RUNTIME_DIR` are reserved.
- `always_load_tools`: optional array, subset of `allowed_tools`. The gateway marks these tools with MCP `_meta` `"anthropic/alwaysLoad": true` in `tools/list`, so Claude keeps their schemas in the prompt instead of deferring them behind `ToolSearch`. Codex ignores the mark. Each listed schema costs prompt context in every Claude session that receives the tool.
- `roles`: optional non-empty unique subset of `coordinator`, `worker`.
- `engines`: optional non-empty unique subset of `codex-acp`, `claude-agent-acp`.

Cortex-owned entries take precedence over client-supplied MCP entries with the same name; other client entries pass through for that session. MCP server values are loaded into memory for engine sessions, not copied into task reports. External tools still pass the gateway's channel, role, tool, path and network checks. Claude defers MCP tool schemas behind its native `ToolSearch`. Cortex marks every tool of its own `cortex` server channel catalogue as always loaded, and external tools only when listed in `always_load_tools`. Every engine session that receives external servers also gets an "Attached MCP servers" section after its role instructions naming them (see [unified system prompt](acp/unified-system-prompt.md)). The full `codebase-memory-mcp` procedure appears only when that role is granted `list_projects`, `search_graph`, `trace_path` and `get_code_snippet`; a partial grant yields a limited-capability note instead. For coordinators, include those tools and `index_status`/`check_index_coverage` in `coordinator_tools` if the full lookup procedure is intended; keep `index_repository` worker-only. List read-only index tools in `always_load_tools` so Claude need not discover their schemas through `ToolSearch`. Read [permissions and containment](acp/permissions-and-containment.md) for the enforcement boundary.

## ssh from workers

ssh in a worker's native shell and in `command_run` behaves as it does in your terminal. HOME is your own, read-only, so `~/.ssh/config`, `known_hosts` and host aliases apply. Cortex passes your running ssh-agent (`SSH_AUTH_SOCK` from the environment Cortex was started with) to every engine process and command, provided it names a socket you own. Keys unlocked in that agent, such as a desktop keyring's, work without a passphrase prompt. Nothing needs configuring. Without an agent, a key with a passphrase needs the optional password dialog below.

For prompts the agent cannot answer (a key passphrase, a password, a new host-key confirmation), `{"enabled": true}` in `$XDG_CONFIG_HOME/cortex/ssh-askpass.json` shows ssh's own prompt in a desktop dialog. See [SECURITY.md](../SECURITY.md#owners-ssh-agent) and `orchestration.ssh_askpass`. Both apply to engine processes started after the change, so open a new session.

## Verify configuration

Check the current user registry and engine discovery without printing secret values:

```bash
python3 -B scripts/cortex_acp_setup.py status
```

For a broader offline check (storage layout and permissions, pinned-dependency digests, connection and route availability, adapter qualification and, with `--project`, that project's policy ceiling), run the non-paid diagnostics. They make no model call. JSON is the default output; use `--format text` for a summary. Exit code `0` means ok, `1` degraded (a repairable gap) and `2` failed:

```bash
python3 -B scripts/cortex_diagnose.py --format text
python3 -B scripts/cortex_diagnose.py --project /path/to/project
```

To validate the documented loader examples without reading the real user registry, set isolated `HOME`, `XDG_CONFIG_HOME`, and `XDG_STATE_HOME` outside every repository and call the project loaders. For example:

```bash
env PYTHONPATH=src HOME=/tmp/cortex-config-check/home XDG_CONFIG_HOME=/tmp/cortex-config-check/config XDG_STATE_HOME=/tmp/cortex-config-check/state python3 -B -c 'from cortex_runtime.configuration.store import read_user_defaults; from cortex_runtime.configuration.mcp_servers import load_mcp_servers; print(read_user_defaults()); print(load_mcp_servers("/tmp/cortex-config-check/project"))'
```

Create owner-only JSON files under `$XDG_CONFIG_HOME/cortex` before running that command. For routing, set `CORTEX_ROUTING` to a JSON file containing the documented routing shape and call `routing.auto_route.load_overrides(project_root)`. The loader creates neither example registry files nor the project directory. Invalid existing registries fail closed; routing rejects malformed values in supported keys and ignores unknown routing keys.

Relevant source: `connections.registry`, `connections.model`, `connections.onboarding`, `configuration.store`, `configuration.resolution`, `configuration.policy`, `configuration.mcp_servers`, and `routing.auto_route`.
