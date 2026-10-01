# Cortex configuration

> **Current contract:** automatic model identity, tier, effort, request labels and eligibility are owner data in routing version 2. Every model name and routing JSON example in the older material below is a legacy illustration, not a current setup default; its built-in/family inference rules are removed. See [settings-only model policy](#settings-only-model-policy).

This reference describes configuration the current ACP runtime actually reads. The source of truth is the loaders named in each section. Private files are local operator state; do not commit credentials or real connection registries.

## Where configuration lives

| Data | Default path | Scope and precedence |
|---|---|---|
| Connection registry | `$XDG_CONFIG_HOME/cortex/connections.json` or `~/.config/cortex/connections.json` | User-wide. Managed by `cortex_acp_setup.py`; it has no project override. |
| Connection secrets | `$XDG_CONFIG_HOME/cortex/secrets/<connection-id>` | User-wide private files, written by setup. Native-account credentials can instead be referenced from the existing user engine directory. |
| User defaults and policy ceiling | `$XDG_CONFIG_HOME/cortex/defaults.json` | User-wide base configuration. |
| Routing rules | `<project>/.cortex/routing.json`, then `$CORTEX_ROUTING`, then `$XDG_CONFIG_HOME/cortex/routing.json` (or `~/.config/cortex/routing.json`) | First existing file wins; files are not merged. Setup installs the validated public version-2 settings template only when the file is absent; it never overwrites existing owner files. |
| User routing profiles | `$XDG_CONFIG_HOME/cortex/routing-profiles.json` | Saved owner settings: routing, defaults/ceiling, MCP usage policy and connection limits. Activation materializes these user files; project settings are not captured. |
| Cortex-owned external MCP servers | `$XDG_CONFIG_HOME/cortex/mcp-servers.json` | User-wide only. Project-local MCP registries are intentionally ignored. |
| Engine-native state and catalog snapshots | `$XDG_STATE_HOME/cortex/connections/<id>/...` or `~/.local/state/cortex/...` | User-wide mutable state, separate from configuration. |
| Paseo launch record | `~/.config/cortex/paseo-plugin-launch.json` | Owner-private non-secret command, environment and shim metadata written by the setup helper. |
| ssh password dialog (optional, off) | `$XDG_CONFIG_HOME/cortex/ssh-askpass.json` | User-wide, written by hand. See [ssh from workers](#ssh-from-workers). |
| Task and report store | `<project>/.cortex/cortex.sqlite3` and sibling published Markdown | Project data, not operator configuration. |

### Manage settings in Paseo

The plugin implements a **Cortex Settings** screen for supported Cortex configuration; this is not Paseo's plugin-settings document. However, independent desktop checks on installed Paseo 0.10.0 found the plugin enabled and running but did not find its screen in Settings or its command in the command palette. Earlier implementation/test evidence describes the screen, while live access in the tested host is inconsistent and remains unverified; do not assume an owner can reliably reach it. The screen has eight sections: user defaults, Models, Engines & models, project ceiling, routing, user routing profiles, MCP servers, and connections. **Models** switches individual models on or off for Cortex as a whole (routing `enabled` and exact `deny` entries, below) and has a **Refresh model lists** button that re-reads every connection's model list from its engine; **Engines & models** lists only the models switched on there. Editable fields use typed controls (such as text, number, selectors, switches, and lists), not JSON text editors; values the form cannot safely represent are read-only rather than rewritten. Select a workspace for project-scoped settings. Every field or group of similar fields (rows, per-model controls) has help that says what it does and recommends a setting; the **Help language / Язык подсказок** selector at the top shows it in English, Russian or both (default both, remembered by the client when local storage is available). The help texts live in `integrations/paseo/client/settings-help.ts` and name no model. This is a bounded management interface, not a control for every runtime or task setting. Use the documented setup CLI as a fallback where a supported command exists; for OpenCode onboarding, see [the OpenCode adapter](acp/opencode-adapter.md#onboarding). See [verification guidance](project/verification.md) for the desktop findings and untested flows.

In **Engines & models**, the engine switch adds/removes a `<engine>/*` token in `deny`: it disables model selection and dispatch for that engine but does not remove its connection or credentials. This is a routing rule, not a connection-registration toggle: use the Connections section or setup command to register a connection; removing one is a separate operation. The separate automatic-routing switch edits `providers`, which restricts automatic routing, failover, and pins but does not hide models from manual selection. Per-model switches edit `deny` (including selector visibility and dispatch) or `auto_allow` (lifting only a built-in automatic exclusion for that exact model). `deny` takes precedence; provider policy, catalog and connection readiness, and other runtime eligibility checks still apply. The screen shows effective status and matching rules. Project routing takes precedence over `$CORTEX_ROUTING`, then user routing; the first existing file wins and files do not merge. Review shadowing warnings before saving. A newly created project file is seeded from effective rules, not created empty.

The `routing` surface carries the `models` key (per-engine automatic-routing selection, tiers and per-model policy). In **Engines & models**, each discovered model has an automatic-ranking switch, a multi-select of light, standard, deep and flagship tiers, and work-kind preference switches using the runtime's work-kind labels. A preference can be selected only for a currently auto-eligible model or one explicitly selected in `models.<engine>.auto`; excluded models require that explicit selection first. Saving writes the selected engine/model values and `preferred_for` lists through the revision-checked routing API, preserving other per-model policy attributes. The read-only `route_catalog` reports each route's `tiers` (every tier it is a candidate of, strongest first), `tier_source`, engine `selection` mode, `auto_selected` membership, `configured_tier`, effective `policy`, and `not_selected` reason, plus every engine's `selected_models` and `unmatched_selected` ids. Each route also reports `enabled` (routing `enabled` or its per-engine default) and each engine its `enabled_models`, `enabled_default`, `unmatched_enabled` and, per connection, `catalog_observed_at` (when the engine last reported its model list; `null` when never).

User profiles snapshot current owner settings, not just routing. Edit working settings through their own sections, then explicitly save into the selected profile or create a new profile. Ordinary settings saves do not overwrite the saved profile. Activation restores its captured user sections; project/environment routing still takes precedence. See [User routing profiles](#user-routing-profiles) for states, revision checks and restore behavior. Rendered access remains subject to the host limitation above.

The owner API is version 1: `cortex_runtime.configuration.management`. The typed plugin RPCs are `cortex.config.read`, `cortex.config.update`, `cortex.config.opencode.discover`, `cortex.config.opencode.onboard`, and `cortex.config.catalog.refresh` (the management action `refresh` on the read-only `route_catalog` surface: a prompt-free re-read of every connection's model list, returning the fresh catalog with one outcome per connection). The plugin runs this API through its selected Cortex launch, using that launch's HOME/XDG configuration. Projects are resolved from the selected Paseo workspace; requests do not supply paths, commands, or binaries. Paseo RPC invocation is gated by its daemon authorization, and writes require the read's current revision. A stale revision returns a conflict; reload the latest value before retrying rather than overwriting it. A timed-out write can have an unknown outcome: re-read before retry.

The API is intentionally closed rather than a general JSON editor. It patches supported user defaults and ceiling fields; replaces supported routing keys while preserving unknown keys; uses the existing project SQLite row for the project ceiling; and validates MCP and connection updates with runtime loaders. It never writes the project task/report database except that existing SQLite configuration row. Routing precedence remains project `.cortex/routing.json`, then read-only `$CORTEX_ROUTING`, then user `routing.json`; sources do not merge, and a UI write to a shadowed file is surfaced as such. MCP environment values and connection credential values/paths are redacted. When editing MCP env, use `{"keep": true}` to preserve an existing secret; the API does not return it.

OpenCode v2 discovery is read-only. Onboarding requires an explicit opt-in and registers the existing owner login only; it does not copy credentials or enable automatic routing. Routing remains excluded unless the owner separately configures an exact OpenCode route under the supported routing rules. See [OpenCode adapter](acp/opencode-adapter.md#onboarding).

For API details and current restrictions, see `src/cortex_runtime/configuration/management.py` and `integrations/paseo/shared/config.ts`. Changes are consumed by the next Cortex session; they do not retroactively reconfigure active tasks. Saving settings does not restart active Cortex sessions. Conversely, a plugin code update/reload is not needed for a settings edit, but it restarts the plugin process and can close open provider connections; apply plugin updates between active tasks until recovery has been qualified on the deployed Paseo version.

`HOME`, `XDG_CONFIG_HOME` and `XDG_STATE_HOME` must resolve to absolute paths. The user roots cannot be placed inside a project workspace. Cortex creates managed directories owner-only and validates existing files before trusting them. See `connections.registry.config_root/state_root`, `configuration.store`, `routing.auto_route._routing_path`, `configuration.mcp_servers._global_path`, and `scripts/cortex_acp_setup.py::_write_paseo_launch_config`.

## Connections

Use the setup command to add or change connections; do not hand-edit `connections.json`. Setup validates the engine install record, creates IDs and canonical credential references, and writes the registry atomically. Credential values are read from a file or stdin, never from command-line text.

The supported engines are `codex-acp`, `claude-agent-acp` and the optional `opencode` (OpenCode v2, see [OpenCode adapter](acp/opencode-adapter.md)). Authentication modes are `native_account` and `api_key`; `gateway` is represented in the type enum but has no implemented launch path and is not usable. Codex API-key kind is `openai_api_key`; Claude accepts `anthropic_api_key` or `claude_oauth_token`. Native account connections may reference the user's existing `~/.codex` or `~/.claude` directory, or use Cortex-managed isolated engine state. OpenCode supports only `native_account`, referencing the login `opencode auth login` wrote to `~/.local/share/opencode`, and is added only with `add-opencode-connection` (below). The supported combinations come from `connections.model.SUPPORT_MATRIX`; onboarding is in `connections.onboarding`.

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

There is no installer for any engine. OpenCode is a standalone binary, not an npm package, and takes no install record: `python3 -B scripts/cortex_acp_setup.py add-opencode-connection [--binary PATH]` finds a trusted binary, reads its version in a network-less sandbox and registers the connection. `$CORTEX_OPENCODE_BINARY` names a binary for that search. For the npm engines: `add-connection --install-record FILE` takes an operator-recorded install of an engine you already installed by other means (for example `npm install @agentclientprotocol/codex-acp`). It holds absolute paths without symlink components and the SHA-256 of each pinned file. `verify_install()` re-hashes every file before launch. The JSON shape follows `engine.codex_install.CodexAcpInstall` or `engine.claude_install.ClaudeAcpInstall`: `node_binary`, `adapter_entry`, `package_json`, `package_version` (must equal the package's own `"version"`) and their digests, plus the bundled Codex binary and version for Codex only. `tests/fixtures/engine/codex/install-record.template.json` shows the shape without real hashes. The field-by-field recipes are in the [Codex adapter](acp/codex-acp-adapter.md#current-runtime-gateway-policy) and [Claude adapter](acp/claude-agent-acp-adapter.md#pinned-install-requirements-manual-recipe-placeholder) pages.

Accepted adapter versions (`connections.discovery.QUALIFIED_ADAPTER_VERSIONS`): codex-acp exactly `1.12.0` or `1.13.0` (an allowlist, not a floor; any other version is refused). claude-agent-acp has no allowlist, and `pinned-dependencies.json` records the last tested version. OpenCode accepts any `2.x.x` release, checked from the sandboxed `--version` output before launch and again from the engine's own `initialize` `agentInfo.version`; 1.x, 3.x and malformed versions are refused.

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

ACP's first pending/saved-default selection and later route/effort selections
read the owner defaults/ceiling and project SQLite layer. Only parameter maps
enter engine configuration; saved route identity, coordinator preference,
language and ceiling envelopes are not native parameters. The effective ceiling
is retained through selected-to-active application. A subsequent selection
intersects supplied layers with the prior task ceiling and retains an omitted
role profile, so omission or a route change cannot widen task grants.

Before a task starts, echoing the already selected exact model/provider is
idempotent: absent native parameters are not a request to clear them. It keeps
the saved portable coordinator and configured effort/mode unchanged. An actual
route change still resets dependent parameters; an explicit effort selection
still changes that field. No ACP reset signal or catalog-order inference is
introduced. See [bound inspection](acp/observability.md#bound-task-inspection)
for configuration states and engine confirmation references, not just UI values.

For automatic worker/reviewer routing, `allowed_routes`, `allowed_connections`
and `allowed_data_recipients` (connection provider IDs) filter the observed
candidates before ranking. Null/absent sets are unrestricted at that layer;
empty arrays permit nothing. A pin outside the intersection refuses without
substitution. Dispatch rechecks the chosen current route identity, not the
coordinator's route; an unavailable or ambiguous identity fails closed.

## Routing rules

The routing loader reads eight keys: `pin`, `deny`, `enabled`, `auto_allow`, `tiers`, `providers`, `engine_affinity` and `models`; additional JSON keys are currently ignored. Its validation and selection logic are in `routing.auto_route` and `routing.model_policy`. These are the eight supported routing keys (unknown keys are ignored):

```json
{
  "pin": {
    "worker-security-auditor": "claude-agent-acp/opus:high",
    "*": "codex-acp/gpt-5.6-luna"
  },
  "deny": ["gpt-5.5", "haiku"],
  "enabled": {"opencode": ["anthropic/claude-sonnet-4-5", "nvidia/nemotron-ultra"]},
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
  },
  "models": {
    "opencode": {
      "auto": ["anthropic/claude-sonnet-4-5"],
      "tiers": {"anthropic/claude-sonnet-4-5": "standard"},
      "policy": {"anthropic/claude-sonnet-4-5": {"preferred_for": ["review"]}}
    },
    "codex-acp": {
      "policy": {"gpt-6-sol": {"preferred_for": ["design"], "max_effort": "xhigh"}}
    }
  }
}
```

Routing validation diagnostics use generic `opencode/provider/model` (pin) and
`provider/model` (enabled model) syntax, not a fixed model recommendation. They
still require exact provider-qualified IDs and reject invalid settings before
writing configuration or dispatching work.

- `pin`: object mapping worker role names or `*` to `engine/model` or `engine/model:effort`. A pin is strict: unavailable or disallowed pins fail instead of silently switching routes.
- `deny`: array of model tokens, each optionally prefixed `engine/`. A token is an exact model id; a versioned id, which also covers that version's variants (`gpt-5.5` covers `gpt-5.5[1m]`, not `gpt-5.6-luna`); or a bare family name, which covers every version and alias of that family (`haiku` covers `haiku`, `claude-haiku-4-5` and `claude-haiku-4-5-20251001`). A token with a `[...]` variant is exact only. The special `<supported-engine>/*` token disables every model for that engine, including future models. Explicit deny applies to automatic routing, pins, dispatch, and the model selector, and always wins; it does not unregister the connection.
- `enabled`: object mapping a supported engine id to an array of exact model ids (OpenCode ids are `<provider>/<model>`). An engine it lists may use only those models in Cortex; every other model of that engine is treated exactly like a denied one (hidden from the selector, refused when chosen or pinned, never routed automatically), and `[]` switches all of its models off. An engine it does not list keeps the built-in default: every model is on, except the optional OpenCode engine, whose models stay off until listed here (OpenCode offers hundreds of provider models). `deny` still wins. Settings > Models writes this list for OpenCode and exact `deny` entries for the default-on engines.
- `auto_allow`: tokens of the same form that may enter automatic selection despite the built-in Haiku and OpenCode exclusions (on an engine without a `models` selection list; a list replaces these exclusions). OpenCode model IDs name their provider, so an OpenCode entry is the exact route, for example `"opencode/anthropic/claude-sonnet-4-5"`. It does not override `deny`, `providers`, catalog availability, connection readiness or quota limits.
- `tiers`: object mapping model names to `flagship`, `deep`, `standard`, or `light`, for every engine. Built-in known families/models are classified automatically; this key overrides that classification, and a per-engine `models` tier overrides this key. A Luna model moved out of `light` routes like any model of its new tier; only the Luna effort floor (`high`) and `max` still apply.
- `providers`: array restricted to `codex-acp`, `claude-agent-acp` and/or `opencode`. When absent, every configured provider is considered. It restricts automatic routing, failover and pins; it does not hide models from the selector or disable manual selection. A disallowed pin is refused.
- `engine_affinity`: object mapping a work kind or `*` to `codex-acp`, `claude-agent-acp`, `opencode`, or `null` (no lean). Work kinds are `implementation`, `operations`, `debugging`, `testing`, `investigation`, `review`, `design`, `writing`, `planning`, and `analysis` (each worker role's kind is `routing.auto_route.ROLE_WORK_KINDS`; an unlisted role is `implementation`). A kind named here wins, then `*`, then the built-in default: `implementation` and `operations` lean to `codex-acp`; `investigation`, `review`, `design`, `writing`, `planning` and `analysis` lean to `claude-agent-acp`; `debugging` and `testing` are neutral. Affinity breaks ties only after plan usage and retry diversification (a plan at least 10 points less used wins first), and it is itself broken by the 85% quota band only as a late tie breaker; it never overrides a pin, `deny`, `providers`, availability or the 100% quota exclusion. An unknown kind or engine makes the file invalid.
- `models`: object mapping a supported engine id to an object with any of `auto`, `tiers` and `policy`, each keyed by the exact model id the engine reports (OpenCode ids are `<provider>/<model>`). Validation (`routing.model_policy.validate_models`) reports every problem with its field path.
  - `auto`: array of unique model ids. Present, it is the complete list of that engine's models automatic routing may use (`[]`: none), replacing the built-in Haiku/optional-engine exclusions, `auto_allow` and newest-version-only routing for that engine; absent, the built-in rules apply. `deny` and `providers` still win; a `pin` or the model selector can still use any offered model.
  - `tiers`: model id to `flagship`, `deep`, `standard` or `light`, or to a non-empty list of them (for example `["standard", "deep"]`); it wins over the legacy top-level `tiers`, which wins over the built-in classification. A model given several tiers is an automatic-routing candidate of each (it is listed under each in `list_route_options`) and runs at the effort of the tier it was chosen for; a pin or explicit effort may use any effort one of its tiers permits, and it counts as flagship for a user's explicit flagship request when `flagship` is one of its tiers. Only this per-engine key takes a list; the legacy top-level `tiers` stays one tier per model. In Settings > Engines & models the tier control is a multi-select; clearing every tier removes the setting.
  - `policy`: model id to an object of optional attributes, each overriding the model's built-in value (`null` removes it): `effort_floor` and `max_effort` (efforts `low`..`max`; without `max_effort` a flagship-tier model stops at `high`), `effort_ladder` (1-3 strictly ascending efforts a light model runs at for light work, standard work, and standard work that fell short once), `also_standard_for` (work kinds for which a light model is also a standard-tier candidate, as the built-in Luna rule does for `testing`), `preferred_for` (work kinds this model is preferred for: rank version 9 puts it ahead of new plan hints, plan usage and retry diversification within its tier; it yields for quota only when at least 85% used and more than 10 points worse than a known usable alternative) and `request_names` (1-16 lowercase extra spellings, Cyrillic stems taking Russian endings, that count as the user naming this model's family in an explicit flagship request; ordinary request, work or negation words are refused).

  `preferred_for` uses the same ten work kinds as `engine_affinity`. The API's
  `routing.builtin` includes English `work_kind_labels`, the worker-role list
  `work_kind_roles`, and `builtin_affinity` for each kind. A model with a
  non-empty `preferred_for` must appear in its engine's explicit `models.auto`
  list when that list exists (`preferred_not_selected` otherwise). Without a
  list, built-in eligibility is dynamic: the route catalog marks a preference
  `preferred_inactive` when its route is not auto-eligible, and
  `list_route_options.preferred` includes only auto-eligible routes. The latter
  also returns `affinity_source` (`routing.json` or `builtin`) per kind;
  `affinity` is `null` when that engine has no auto-eligible route, with the
  withheld engine named in `affinity_not_eligible`.

  The coordinator sees the effective selection, tiers and policy only through `list_route_options`; no prompt or tool schema names a model. See [routing](acp/routing-and-fallback.md#owner-configured-models-selection-tiers-and-policy).

Precedence is first existing routing file: project `<project>/.cortex/routing.json`, then `CORTEX_ROUTING`, then user `routing.json`. The files do not merge, so a project file that should keep the defaults must list them itself. An existing malformed file fails closed. If no file exists, built-in automatic routing applies.

### User routing profiles

The `routing_profiles` management surface stores owner-only JSON at
`$XDG_CONFIG_HOME/cortex/routing-profiles.json`. Format 2 stores a profile's
`sections` and settings snapshot: the eight supported user-routing keys;
user defaults (route id, connection id and exact model id, named parameters,
language, and owner policy ceiling); each registered MCP server's usage
policy; and each registered connection's concurrent-session limit. The profile
file is a management snapshot, not a runtime routing source. Activation
materializes settings into the existing backing files; their loaders and
precedence remain unchanged.

Profiles exclude project routing and project ceilings, MCP server installation
definitions (`command`, `args`, `env`), connection credentials or credential
paths, engine login state and task state. Credentials remain in their existing
private stores and are neither copied nor exposed by profile reads. Names are
lowercase letters, digits or hyphens, 1–40 characters; at most 32 profiles.

Choose the current profile in the persistent profile bar. Apply the forms in
each settings section first, then use **Save to current profile** (`save`) to
replace that profile's captured settings, or **Save as new profile** (`create`)
to capture the current settings and select the new profile. A normal settings
save updates working settings only; it never writes through to the selected
profile. `update` changes profile name/description only. Activation restores
the saved snapshot to user backing files after validating all targets; a
journal is written before ordered file replacement, with in-process rollback
if a write fails. Switching away from modified settings offers save, discard
and switch, or cancel. Discarding requires the explicit
`discard_modifications: true`; `deactivate` detaches without changing working
settings. Re-activating an interrupted profile retries its pending activation;
deactivation clears the pending operation. The UI refreshes the mounted form
after activation.

Profile states are `none`, `in_sync`, `modified`, `interrupted` and
`unavailable`. `modified` means working values differ from the selected
snapshot; section names and key-only differences are reported without values.
`interrupted` indicates a pending journal remains after a failed restore;
`unavailable` means the profile or a required working section could not be
read safely. A partial legacy profile can be explicitly upgraded by saving a
complete snapshot. Every mutation requires the profile file's latest
`expected_revision`; create, save and activate also require
`expected_revisions` for all four sections from `read.current_revisions`.
Stale revisions refuse without writing. Reads do not write.

Format 1 is interpreted in memory as a routing-only profile. Its first
mutation creates a private verified backup under
`$XDG_CONFIG_HOME/cortex/.migration-backups/` and migrates the file to format
2; profile content remains routing-only until explicitly saved as a full
snapshot. Unknown formats fail closed.

Project routing or `$CORTEX_ROUTING` still wins over materialized user
routing. `shadowed_by` reports the winning source; selecting a profile cannot
force worker selection through a higher-precedence file or change an already
open worker's route. The route audit labels the profile only when its routing
matches the winning user file. A preference in an owner profile is ranking
input, not permission to dispatch.

#### Settings reference

“Level” identifies which configuration surface owns the setting. “Profile” says
whether the owner-profile snapshot captures it. Examples are illustrative values,
not a universal preset; exact model/connection identifiers must exist locally.

| Setting | What it changes | Level | Profile-bound? | Example |
|---|---|---|---|---|
| Default route (engine, connection, exact model) | Route used as the user default for new work when no more specific choice applies | User defaults | Yes | `codex-acp`, `cn_0123456789ab`, `gpt-5.6-luna` |
| Default language | Language preference for Cortex responses | User defaults | Yes | `en` |
| Named parameters | Default runtime parameters associated with work | User defaults | Yes | `effort=high` |
| Owner policy ceiling | Widest owner-level allowed routes, recipients, permission level, limits, spend/fallback policy | User policy | Yes | `permission_level: "STANDARD"` |
| Project policy ceiling | Narrows the owner ceiling for one project | Project | No | `permission_level: "READ_ONLY"` |
| Pin | Requires an eligible exact engine/model/effort route for a worker role | User or project routing | User: yes; project: no | `{"implementation":"codex-acp/gpt-5.6-luna"}` |
| Deny rules / whole-engine switch | Refuses routes and hides denied models; `<engine>/*` disables that engine in Cortex | User or project routing | User: yes; project: no | `["gpt-5.5"]` |
| Model enabled switches | Selects models available in Cortex's selector and dispatch; optional OpenCode models default off | User or project routing | User: yes; project: no | `{"opencode":["provider/model-id"]}` |
| Auto-allow | Opts an otherwise built-in-excluded exact route into automatic selection, subject to remaining policy | User or project routing | User: yes; project: no | `["claude-agent-acp/haiku"]` |
| Model tiers | Assigns automatic-routing candidates to light, standard, deep or flagship tiers | User or project routing | User: yes; project: no | `{"gpt-5.6-luna":"light"}` |
| Automatic-routing providers | Restricts providers considered for automatic routing, failover and pins | User or project routing | User: yes; project: no | `["opencode","claude-agent-acp"]` |
| Engine affinity | Breaks remaining routing ties toward an engine for a work kind | User or project routing | User: yes; project: no | `{"implementation":"opencode"}` |
| Per-model automatic selection | Defines the full set of models an engine may automatically route when `models.<engine>.auto` is present | User or project routing | User: yes; project: no | `{"opencode":{"auto":["provider/model-id"]}}` |
| Per-model policy and work-kind preference | Sets effort/tier behavior and preferred work kinds for an exact model | User or project routing | User: yes; project: no | `{"opencode":{"policy":{"provider/model-id":{"preferred_for":["review"]}}}}` |
| MCP enabled and usage limits | Controls whether a registered server and its tools, paths, networks, roles or engines may be used | User MCP policy | Yes | `enabled: true` |
| MCP command, arguments and environment | Defines how an MCP server is installed/launched and its environment | User MCP definition | No | `command: "python3"` |
| Connection session limit | Caps concurrent sessions on one registered connection | User connection policy | Yes | `max_concurrent_sessions: 2` |
| Connection name and onboarding | Changes display metadata or explicitly opts into connection setup | User connection metadata | No | `name: "Work"` |
| Connection credentials and login state | Authenticates to a provider; stored/referenced separately from profile data | User secret/engine state | No | Existing native login |

For routing, user values are profile-bound only when editing user routing; the
same keys written to project routing remain project-only. For MCP, the usage
policy is captured but server installation definitions are not. For connections,
only the concurrency limit is captured—not identity, name, credentials or login.
The screen's inline help describes each control's effect and scope; this table
summarizes the underlying settings rather than every repeated row in a dynamic
list.


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

Cortex-owned entries take precedence over client-supplied MCP entries with the same name; other client entries pass through for that session. MCP server values are loaded into memory for engine sessions, not copied into task reports. External tools still pass the gateway's channel, role, tool, path and network checks. The gateway forwards each external `tools/list` schema unchanged except that it adds a description to an argument the server leaves undescribed when Cortex knows its semantics (currently `codebase-memory-mcp` `search_graph` `file_pattern`, `name_pattern` and `qn_pattern`; see [Codex adapter](acp/codex-acp-adapter.md#mcp-bridge)); it never replaces a description the server supplies. Claude defers MCP tool schemas behind its native `ToolSearch`. Cortex marks every tool of its own `cortex` server channel catalogue as always loaded, and external tools only when listed in `always_load_tools`. Every engine session that receives external servers also gets an "Attached MCP servers" section after its role instructions naming them (see [unified system prompt](acp/unified-system-prompt.md)). The full `codebase-memory-mcp` procedure appears only when that role is granted `list_projects`, `search_graph`, `trace_path` and `get_code_snippet`; a partial grant yields a limited-capability note instead. For coordinators, include those tools and `index_status`/`check_index_coverage` in `coordinator_tools` if the full lookup procedure is intended; keep `index_repository` worker-only. List read-only index tools in `always_load_tools` so Claude need not discover their schemas through `ToolSearch`. Read [permissions and containment](acp/permissions-and-containment.md) for the enforcement boundary.

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

## Settings-only model policy

Explicit model requests match the complete, case-sensitive configured
`engine/model` identity, not a family, substring or normalized variant. Keep
the identity whitespace-delimited, or quote it with paired `"…"`, `'…'`,
backticks, `«…»` or `“…”`. A closing quote may be followed by one prose
punctuation character (`. , ! ? ; :`) before whitespace/end. Without quotes,
attached punctuation is opaque identity data, even a terminal dot; use
`Use "engine/unit".` rather than ambiguous `Use engine/unit.`. Longer/shorter
variants, punctuated prefixes and different provider paths do not authorize
each other. Optional owner-configured `request_names` remain separate labels
with the existing case-insensitive ending rules and ambiguity checks. Retained
user-source provenance and request/negation grammar still govern authorization;
a plan hint alone never promotes a flagship or overrides routing eligibility.
See [exact user-request boundaries](acp/routing-and-fallback.md#settings-only-routing).

The runtime and coordinator use only the effective first-existing routing file (project, then $CORTEX_ROUTING, then user); these files are not merged. Routing must declare model_policy_version: 2. The read-only `list_route_options` tool preserves `model_policy_migration_required`, `model_policy_version_unsupported` and `invalid_routing_overrides`, with the policy field and corrective action; it does not misclassify these refusals as `storage_error`. Missing, invalid, unsupported or legacy settings produce visible diagnostics; runtime does not overlay, infer or repair them from install data. The effective policy explicitly supplies exact enabled identities, per-engine automatic membership, tier placement and any non-neutral per-model effort/preference/request-label attributes. Omitted membership means none; new, renamed, unmatched and withdrawn identities do not inherit policy from spelling, family, catalog position or neighboring versions. Required fields/tiers and supported effort constraints are validated; unsupported policy refuses rather than silently weakening it. Coordinator route choice is a portable exact engine/model/parameter preference resolved only against one unique current selectable catalog match; automatic-worker `providers` does not restrict that binding, while enabled identities and deny rules still do; ambiguous/unavailable matches remain diagnostics, not substitutions.

The validated public asset policy/model-settings-default.json is installation/migration input only, not a runtime fallback or overlay. It contains a production-derived baseline candidate and portable coordinator preference, but no account/connection identity or credentials. Setup validates it completely and creates missing owner settings privately without replacement; existing, partial, invalid and legacy owner files and saved profiles are not silently changed. The asset cannot prove installed deployment or a live route. Migration is an explicit two-step operation: preview a candidate (non-mutating), then apply only with explicit consent, the exact preview source revision and candidate digest. Apply verifies a private backup and written bytes and rolls back on failure. Migrate each saved profile from a candidate derived from that profile itself; never fill an inactive profile from current settings or the install template. Retain backups and preview/apply evidence. See [model-policy migration](acp/packaging-and-onboarding.md#model-policy-installation-and-explicit-migration).


### Exact quota-window associations

Version-2 routing also accepts optional `quota_windows`: an engine id to an ordered list of `{ "window": <exact reported label>, "models": <null or list of exact model ids> }` rules. Configuration API version 1 accepts the same field under `routing.rules`; the public installation envelope stays schema version 1. For example (this policy fragment does not enable any models):

```json
{
  "model_policy_version": 2,
  "quota_windows": {
    "codex-acp": [
      { "window": "owner_scope", "models": ["EXACT@9"] },
      { "window": "primary", "models": null }
    ]
  }
}
```

- `window` is case-sensitive, not trimmed or interpreted as a model family. SDK window labels are used exactly as reported. The Claude local usage parser still normalizes printed scope labels to lowercase `seven_day_` labels with whitespace replaced by underscores; this parses evidence, not model membership.
- `models: null` explicitly marks a shared/account window. A nonempty list associates that label with those exact models on that engine; `[]` explicitly excludes the window for every model. Multiple different applicable windows contribute their maximum open reported percentage, not a sum. Duplicate exact window labels on one engine are ambiguous and refused; duplicate model ids, unsupported engines, unknown rule fields or malformed values are also refused before quota IO. Limits are 1000 rules per engine, 1000 unique model ids per rule, and 256 characters per nonempty window label (no controls). OpenCode ids must include the exact provider/model.
- Missing `quota_windows`, `{}`, an omitted engine, an empty engine list and any **unmapped** reported label all use conservative shared applicability. This can over-refuse an unrelated model when a newly observed scoped label is exhausted; it cannot hide that exhaustion as unknown/zero. Add an explicit association to distinguish such a label. An unavailable/unreported numeric value remains `None`/unknown, never confirmed zero. Expired windows no longer count; the existing latest-observation, CLI TTL/negative-cache and reset-time algorithms are unchanged. Paid overage windows remain ignored as before.
- These associations grant no eligibility: enabled/deny/providers/automatic-membership, consent and observed-catalog restrictions still apply. Withdrawn or renamed models do not inherit a scoped rule. The route menu exposes effective rules and `unmapped: conservative_shared` in `quota_window_policy`; automatic selection and failure-reset recording use the same winning first-existing project → environment → user settings. Raw status windows remain engine evidence, not a fabricated model-specific quota.
- Invalid mappings produce `invalid_routing_overrides` in runtime reads and `invalid_quota_windows` with a rule field in configuration updates, without a write or engine execution. An already reported engine refusal is still recorded with the existing bounded default expiry if invalid settings prevent resolving its reset.

**Editing and migration:** Routing → Quota window associations provides typed exact-label, shared/exact-model and model-list controls. Unknown/unrepresentable rule data makes the form read-only; it is not dropped on save. Revision conflicts require reload and explicit resubmission. CLI access remains independent of Paseo screen reachability: send configuration API v1 routing updates or use the explicit candidate migration script. For a file without mappings, copy its complete current settings to an owner candidate, add reviewed associations, preview, then apply against the preview's `expected_revision` and `candidate_sha256` with `--i-consent`. Existing decisions are preserved; the migration refuses changes to an existing association list, which instead requires a deliberate configuration update. Example preview:

```sh
python3 -B scripts/cortex_model_policy_migrate.py preview --candidate /absolute/owner-candidate.json
```

Saved profiles capture this field on explicit save/create and restore it on activation; older snapshots are not silently rewritten or filled from working settings/template data. Migrate a saved snapshot from its own candidate with `--profile`, or save the intended current working settings explicitly. Deleting a selected profile still only unlinks it, without resetting materialized settings.

The public install-only asset retains the previous seven-route tiers, efforts, preferences and denies, disabled OpenCode, and explicit canonical scoped-label associations preserving the former quota predicate for the configured identities. No runtime template overlay exists. Unknown/new provider labels remain conservative until configured; this is not evidence those labels are currently emitted or that production has adopted the candidate. Production application requires separate reviewed/verified adoption.
