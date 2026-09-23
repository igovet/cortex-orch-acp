# codex-acp outgoing adapter (T013)

Cortex is an outgoing ACP v1 client of a pinned, operator-verified
`@agentclientprotocol/codex-acp` install (Node.js entry point over the Codex
App Server) — the exact same outgoing-client role documented for
`engine.session_manager.EngineSessionManager` (see T012). This page covers
the codex-acp-specific pieces added in T013:
`src/cortex_runtime/engine/codex_install.py` (pinned
install identity/integrity) and
`src/cortex_runtime/engine/codex_adapter_acp.py`
(launch/env mapping, configuration-option mapping, evidence, instruction
delivery).

Pinned release basis: `@agentclientprotocol/codex-acp` **1.12.0**, per the
static evidence in `.cortex/acp-migration/T002/CAPABILITY_MATRIX.json` and
`.cortex/acp-migration/T002/ADR-001b-codex-acp-version-drift.md`, and the
`dist/index.js` line citations recorded in
`tests/fixtures/catalog/codex_acp_config_options.json`'s `_fixture_note`.
Nothing here claims a main-branch/newer-release capability (see
`ADR-001b`'s own version-drift note: the lead cited 1.11.0; the actually
pinned/verified copy reports 1.12.0).

That pin is enforced at engine selection, not only at launch.
`connections.discovery.QUALIFIED_ADAPTER_VERSIONS` names the qualified
codex-acp versions; `discovery.discover_engine` prefers a qualified install
over a newer unqualified one, and `connections.engine_update` never prunes a
pinned version. Before this, `connections.engine_update`'s background
install of 1.13.0 made discovery repoint the connection at it, and every
Codex session and every catalog refresh then failed closed with
`codex_manual_review_adapter_unsupported` — Codex was entirely unavailable
and no new model could reach the catalogue (live, 2026-09-22).

Qualified versions: **1.12.0** and **1.13.0**. What was verified for 1.13.0,
against the installed bundle rather than assumed:

- All 15 patched call sites match; exactly one differs from 1.12.0
  (`_CODEX_SEND_PROMPT_SOURCE`: `runTurn`'s `input` became `input: input2`),
  and that variant is derived from the 1.12.0 literal in
  `engine.process._CODEX_SEND_PROMPT_BY_VERSION` so the two cannot drift
  apart silently.
- The `_AgentMode` table, the `CODEX_PATH app-server` invocation and the
  `session/new` → `thread/start.developerInstructions` delivery call sites
  are byte-identical in both bundles.
- `tests/test_codex_developer_instructions.py`'s no-paid installed-bundle
  trace (fake App Server, no model turn) passes against each bundle:
  `CORTEX_CODEX_ACP_TEST_PACKAGE=<engines>/codex-acp/<version>/node_modules/@agentclientprotocol/codex-acp`,
  12 passed for 1.12.0 and for 1.13.0 (2026-09-22).
- Compaction stays `automatic`: `createCompletedContextCompactionUpdate`
  still carries `_meta: CONTEXT_COMPACTION_META`, the signal
  `engine.session_manager` detects. One real difference, recorded rather
  than glossed: 1.13.0 gates the *in_progress* start update behind
  `clientSupportsCompaction(capabilities) = capabilities?.session?.compaction
  != null`, and Cortex advertises no such capability, so Cortex sees only the
  completed signal where 1.12.0 sent a start/complete pair.

1.13.0 bundles Codex CLI 0.155.1 instead of 0.154.0, which is what makes that
CLI's `gpt-6-sol` / `gpt-6-luna` models reachable: `catalog.discovery` only
records models a live adapter session actually advertised.

Still unverified for both versions, as before: the behaviour of native
developer instructions after Codex internal compaction has never been
observed in a real client.

## Current runtime gateway policy

The verified private codex-acp 1.12.0 copy and Codex CLI 0.154.0/0.155.1 wrapper disable
native shell, patch, agents, hooks, plugins, apps, browsing and other native action
tools. Every turn requests `readOnly` with network access disabled. A private model
catalogue preserves configured model routes while disabling native shell/patch
capabilities. Authenticated Cortex MCP operations provide scoped project access.
Account configuration and credentials remain in the production native account.

The wrapper verifies installed shapes and checks that account MCP servers are
disabled for this child before starting App Server. Unsupported versions or changed
patch sites fail closed. The installed no-paid dispatch test observes a successful
MCP call and rejection of a forced native patch. It uses a local fake provider,
so it does not qualify a real paid model or client UI. See
[containment](permissions-and-containment.md) and [status](runtime-rewrite-status.md).

An operator who has already installed codex-acp by some other means (for
example following Zed's own npx-based Custom Agent registration, or a manual
`npm install @agentclientprotocol/codex-acp`) records the resulting install as a
`engine.codex_install.CodexAcpInstall`:

```python
from cortex_runtime.engine.codex_install import CodexAcpInstall, verify_install

install = CodexAcpInstall(
    node_binary="/usr/bin/node",                      # absolute, no symlink component
    adapter_entry="/opt/codex-acp/dist/index.js",      # absolute, no symlink component
    package_json="/opt/codex-acp/package.json",
    package_version="1.12.0",                          # must equal package.json's own "version"
    package_json_sha256="<sha256 of package.json>",
    adapter_entry_sha256="<sha256 of dist/index.js>",
    bundled_codex_version="<observed @openai/codex version>",
    bundled_codex_binary="/opt/codex-acp/node_modules/@openai/codex-linux-x64/vendor/.../bin/codex",
    bundled_codex_binary_sha256="<sha256 of that binary>",
)
verify_install(install)  # re-hashes/re-checks everything fresh; raises on any drift/untrust
```

See `tests/fixtures/engine/codex/install-record.template.json` for the
`--install-record` JSON shape `scripts/cortex_codex_probe.py` expects (a
shape-only template, never real hashes).

**This requires explicit operator consent**: recording an install means
trusting a specific set of files as the thing Cortex will actually execute.
`verify_install()` re-verifies every pinned file fresh on every use (never
trusts a previously-recorded hash without re-checking it) and refuses:

- any path that is not absolute, or that resolves differently than given
  (a relative component or a symlink anywhere in the path) —
  `engine_install_untrusted_location`;
- paths under `$HOME/.local/share/app.codeg` or `$HOME/.config/app.codeg`
  (codeg's own private data directories) —
  `engine_install_untrusted_location`. A Zed
  `$HOME/.local/share/zed/external_agents` checkout is allowed when the
  operator deliberately records that exact install: D-T035-02 treats the
  client-managed registry as the configured engine source, while
  `verify_install()` still requires canonical paths, the pinned package
  version, and fresh hashes for every executable and package file.
- a hash mismatch on `package.json`, `dist/index.js` (`adapter_entry`), or
  the bundled `codex` binary, or a `package.json` `"version"` field that
  disagrees with the recorded `package_version` — `engine_install_drifted`.

`codex_install.launch_command_for(verified_install)` is always exactly
`(node_binary, adapter_entry)` — the only two elements a
`ConnectionRecord.launch_command` for a codex-acp connection may ever
contain when an install is supplied (see `engine.process.EngineProcess`,
which now accepts an optional `install=` and refuses to launch if
`record.launch_command` does not match this exactly).

## Environment variable mapping

`engine.codex_adapter_acp.build_codex_launch(connection_record, install,
environ)` returns `(launch_command, pinned_engine_vars)`, where
`pinned_engine_vars` is always exactly:

| Name | Value | Source |
| --- | --- | --- |
| `CODEX_PATH` | the verified install's `bundled_codex_binary` (absolute) | pins codex-acp to launching the exact vendored `codex` binary this install verified, never a `$PATH`-resolved one — see `dist/index.js`'s `CODEX_PATH` read (~line 35150) |
| `NO_BROWSER` | `"1"` | suppresses ChatGPT-login browser flow at startup (`dist/index.js` ~line 26961) so a headless launch never blocks on it |

`connections.environment.build_process_env`'s new `pinned_engine_vars`
parameter is the only place these two names may ever be set: it is
accepted only for `record.engine == "codex-acp"`, only as exactly
`{"CODEX_PATH", "NO_BROWSER"}`, and is the only explicit, documented bypass
of the module's own credential-shaped denylist. Nothing in this adapter
ever sets `OPENAI_API_KEY`, `CODEX_CONFIG`, `MODEL_PROVIDER`,
`DEFAULT_AUTH_REQUEST`, `INITIAL_AGENT_MODE`, or `APP_SERVER_LOGS` — those
are either a different auth-precedence variable Cortex never uses (see
`connections/model.py`: Cortex only ever sets `CODEX_API_KEY`, never
`OPENAI_API_KEY`) or native mechanisms this task does not wire up.

## Configuration-option mapping

`engine.codex_adapter_acp.codex_option_mapping(revision, observed_config_options)`
maps a `ConfigurationRevision` to `session/set_config_option` pairs, using
the pinned release's observed `configOptions` (from `session/new`, per
`tests/fixtures/catalog/codex_acp_config_options.json`):

- `model` — always required, always taken from `revision.route.model`.
- `reasoning_effort` — mapped only when the revision's
  `supported_parameters` includes a `thought_level` or `effort` entry
  (T007/T008 naming); the pinned release's only real observed values are
  `"none"`/`"medium"` (see the fixture's own note — no other effort string
  appears literally in the pinned adapter).
- `mode` — mapped only when `supported_parameters` includes a `mode`
  entry, and only when it is one of the pinned release's observed `mode`
  values (`read-only`/`agent`/`agent-full-access`).
- `collaboration_mode` and `fast-mode` (`model_config` category) are
  **never** mapped, regardless of what a revision's parameters contain —
  a distinct, unsupported category by design.

Every pair is checked against `observed_config_options` **before**
`EngineSessionManager.configure()` sends its first wire call for this
revision: an unsupported model, effort, or mode known up front raises
`ConfigurationRejected` immediately, before touching the engine at all.

That upfront check alone cannot guarantee a partial application never
happens, though: the pinned release's own option set may narrow again
*mid-call*, as a direct effect of an earlier option in the same
`configure()` call actually being applied (`reasoning_effort`'s offered
values depend on the selected `model`). `EngineSessionManager.configure()`
re-checks every subsequent option against the *latest* observed
`configOptions` before sending it, and raises
`engine.session_manager.EngineConfigurationPartiallyApplied` — a distinct,
plain exception, never `ConfigurationRejected` — if an earlier option in
the same call already succeeded before a later one is rejected, mismatched,
errored, or found no-longer-offered (F3, T013 review). `configuration.
service.ConfigurationService.apply()` maps `ConfigurationRejected` to the
revision's terminal `failed` state (nothing was ever applied) but maps
`EngineConfigurationPartiallyApplied` — like any other plain exception —
to `indeterminate` (the engine may be left in a mixed state; a fresh
`reconcile()` using real engine evidence is required), never a false
`failed` that would understate what the engine actually did.

## Client-facing selection surface: provider, model, effort (T035)

For Zed, Cortex offers three independent `SessionConfigOption`s:
`provider` (category `provider`, `catalog.labels
.build_connection_label`), `model` (category `model`, scoped to the
selected provider's own `ExecutionRoute.model` values), and
`thought_level` (category `thought_level`, scoped to the selected model —
unchanged from the mapping described above). Paseo's installed ACP picker
only displays the `model` category, so Cortex supplies it a combined
model list with exact route IDs across configured providers. Selecting
one updates the concrete provider and model together. Selecting provider+model
resolves to one concrete `ExecutionRoute`/`ConfigurationService` revision
through the same T008 flow every other selection already used; an
unsupported value is refused by `configuration.resolution
.validate_dependent_settings` / `engine.codex_adapter_acp
._require_supported_option`, never silently accepted. Changing `provider`
necessarily changes engine and so may change the valid model/effort sets:
the same model *name* is kept across the switch only if the new provider
happens to offer it too, otherwise the new provider's own first model is
used explicitly and `thought_level` is always reset — never a silently
kept invalid model, never a silently dropped choice. See
`acp.config_options.build_provider_option`/`build_model_option` and
`acp.agent.Agent._apply_config_selection`.

Independently of the route, every client also receives the user's access
mode: Zed as the `access_mode` config option, Paseo as ACP `modes` (its mode
control) — see
[permissions](permissions-and-containment.md#access-mode-guarded-and-auto).
`session/set_mode` sets only that mode.

The `/cortex-model <route label or route id>` slash command still selects
a whole route (any provider) at once, for a host with no
`SessionConfigOption` support — see `commands.py`'s own docstring.

### Model catalog: real observation, static seed, never invented

`catalog.discovery.selectable_entries_from_snapshots` normally reads a
connection's most recently **observed** catalog snapshot (a real, parsed
`session/new` `configOptions` payload — `DISCOVERY` provenance). A
connection that has never yet had a real session observed for it falls
back to a small, static, per-engine **seed** list
(`catalog.model_seed.SEED_MODELS`, `SEED` provenance, never `DISCOVERY`)
so a first-time user still gets a usable `model` select instead of an
empty one. A seeded value is always rendered "seeded — not yet confirmed
by an engine" (`acp.config_options._evidence_note`) — never as
observed/confirmed — and is unconditionally superseded the moment a real
observation is captured for that connection (the seed is never itself
persisted to a snapshot file, so the very next real snapshot simply wins).
`catalog.model_seed`'s own module docstring records exactly which local
source (the installed `codex` CLI's own `codex debug models`, or this
machine's `~/.config/zed/settings.json`/the pinned `claude-agent-acp`
build) each seeded value came from — never an invented/recalled vendor
name. codex's effort levels are genuinely per-model (e.g. `gpt-5.5` lacks
`max`/`ultra` that other models have); the seed intentionally does not
attempt to reflect that (`ExecutionRoute.supported_parameters` today is
one flat set per catalog snapshot, not per model — a real architecture
change, out of scope here), so a seeded model's `thought_level` stays
unavailable until a live session actually observes it.

A real engine session opened by `delegation.live_adapters
.EngineSessionManagerAdapter.open()` now automatically captures its own
observed `configOptions` into a real `DISCOVERY`-provenance snapshot
(`EngineSessionManagerAdapter._capture_catalog_observation`, using
`EngineSessionManager.observed_config_options`) — best-effort and
non-blocking, never itself sending a prompt or failing the engine session.
`scripts/cortex_acp_setup.py record-observation` remains the manual
fallback for capturing an observation from outside a real task run.

## Evidence levels

`engine.codex_adapter_acp.build_codex_evidence_status(revision_id,
evidence_ref)` returns a `CodexEvidenceStatus` with three genuinely
distinct fields:

- `selected` — the `ConfigurationRevision.id` (never itself proof of
  application).
- `configuration_applied` — the `evidence_ref` string
  `EngineSessionManager.configure()` returned (engine instance id, wire
  session id, and the exact confirmed `option_id=value` pairs) — proof the
  *configuration call* round-tripped and the engine echoed back the
  requested values.
- `execution_verified` — always exactly
  `"unavailable_in_codex_acp_1_12_0"`. **This is a deliberate, explicit
  "not available" status for this pinned release, never inferred from a
  `usage_update` event or a model's own self-report.** The pinned
  release's wire surface gives no statically-verifiable, non-model-authored
  signal that a specific turn actually executed under a specific confirmed
  model/effort/mode; a `usage_update` reports token counts only (see
  `EngineEventKind.USAGE_UPDATE`'s `UsageBasis.ENGINE_REPORTED`, which is
  basis for the usage figures only, never a model-identity confirmation).
  A real per-turn execution-verification mechanism, if any exists for a
  later codex-acp release, is out of this task's scope — see T035.

`usage_update` events observed through this adapter are always labeled
unqualified (no claim this pinned release's usage figures were ever
qualified against a real bill). Compaction (`COMPACTION_OBSERVATION`)
events are only ever surfaced when the negotiated capability set actually
includes it (see `EngineSessionManager.negotiate`'s capability grants);
otherwise none are produced — this adapter never invents a compaction
signal.

## Instruction delivery (private adapter developer channel)

The stock, pinned codex-acp 1.12.0 bundle reads `_meta` for additional
directories but drops `_meta.systemPrompt` on `session/new`. Cortex's
owner-private, exact-source-checked copy forwards Cortex-supplied
`_meta.systemPrompt` into App Server `thread/start.developerInstructions`.
This is an ACP extension owned by this pair of implementations, not a standard
ACP system-prompt field. The copy preserves effective native developer
instructions and refuses a missing or malformed config response. The original
installed bundle and native account are unchanged. The same private copy also
selects `user` instead of `auto_review` as the `agent` reviewer; both exact call
sites must match the verified pinned source before launch.

The no-paid adapter trace establishes the outgoing `thread/start` payload only;
it does not prove model visibility or compliance. Native Codex compaction
persistence remains unqualified. Cortex does not use a separate user-prompt
role block for delivery.

The same automatically generated private adapter normalizes Codex App Server
`rawResponseItem/completed` records for `custom_tool_call` and
`custom_tool_call_output`. It emits one ACP tool lifecycle for each exact
`call_id`, maps `exec`, `apply_patch`, and `view_image` to the corresponding
portable operation kinds, and preserves the observed input, output, exit code,
and failure status. The mapping also handles nested `responseItem`/
`rawResponseItem` envelopes and historical replay events. This is a display
and evidence-preservation bridge: it does not invent a tool result or turn
missing output into success. The no-paid installed-bundle trace covers the
serialized transformation and replay shapes; live Paseo visibility for a
real project task remains unverified until a fresh retained client run.

## Terminal-output deltas

Codex ACP 1.12.0 may place terminal stdout/stderr in
`_meta.terminal_output_delta` update metadata and leave the final
`rawOutput.formatted_output` empty. `EngineSessionManager` correlates each
delta by the exact wire session and tool-call ID, accumulates it only for that
call, and merges it into the portable terminal observation when the tool
completes. The accumulator is bounded at 4 MiB. If that bound is reached, the
merged output includes an explicit truncation marker and
`output_truncated=true`; it is never silently presented as complete.

This is output preservation, not verification. A terminal completion with no
observed output remains unavailable, and truncated output cannot satisfy a
check that requires complete output. Paseo replay receives the same observed
or truncated distinction through the durable tool event rather than inferring
content from a model report.

## MCP bridge

`engine.mcp_bridge.build_mcp_servers` always puts Cortex's authenticated
server first. Additional stdio servers come from the Cortex-owned registry
(`$XDG_CONFIG_HOME/cortex/mcp-servers.json`) and, when supplied, the ACP client's
`session/new` `mcpServers` list. Paseo currently sends an empty list, so the
registry is the portable configuration path for tools such as
`codebase-memory-mcp`. The effective list is loaded on `session/new` and again
for `session/load`, then passed to the coordinator and worker engine sessions.
An owner-registry server may set `"roles": ["coordinator"]` or
`"roles": ["worker"]` (or both) to expose it only to those engine sessions.
Omitting `roles` retains visibility for both. Cortex validates this field and
removes it before the ACP `session/new` request. The optional `engines` array
can select `"codex-acp"` and/or `"claude-agent-acp"`; it is matched against
the verified route connection's engine and likewise never sent on the ACP
wire. For example, a browser MCP server intended only for Claude workers uses
`"roles": ["worker"], "engines": ["claude-agent-acp"]`.
The session log records only
the selected server names and count, bounded to 64 names with an explicit
truncation flag; it does not claim to have observed those servers' tools.
This controls server visibility, not individual tools within a server.
Zed's forwarded entries remain supported as explicit session-scoped additions;
the global registry wins on duplicate names. Registry files are owner-only and
their environment values are never persisted in the project store or
diagnostics. A project-local registry is intentionally ignored. A live
Codex/Claude run is still required to qualify that each configured server is
actually started and used by the pinned engine.

Inherited Codex account MCP entries are a separate source from these explicit
session servers. The Cortex-owned child launcher invokes the verified CLI with
`features.plugins=false`, checks the installed feature inventory, reads `codex
mcp list --json`, and constructs per-name `mcp_servers.<name>.enabled=false`
overrides. It verifies that the inherited native entries are disabled before
executing App Server. It reads names and enabled booleans, not credential
fields; the inventory and child stderr are neither printed nor retained. A
malformed inventory, duplicate name, or name outside `[A-Za-z0-9_-]+` fails
closed because it cannot safely form an override. The private adapter forces
`plugins: false` in the `thread/start` session features for new, resumed and
forked sessions. It also retains requested session `mcpServers` if one has the
same name as a disabled native config entry; otherwise the upstream duplicate
filter would omit that explicit server. These checks and adapter behavior
have offline coverage, but effective isolation and configured-server use in a
real client remain unverified pending a retained live observation.

The accepted owner registry shape is:

```json
{
  "version": 1,
  "servers": {
    "codebase-memory-mcp": {
      "command": "/absolute/path/to/codebase-memory-mcp",
      "args": [],
      "roles": ["worker"],
      "enabled": true
    }
  }
}
```

`env` is optional and should contain only variables required by the configured
server; the installed `codebase-memory-mcp` registration does not require an
extra environment variable. `mcpServers` is also accepted as the top-level key for compatibility with an
ACP client payload, but the owner file should use `servers`. The reserved
`cortex` name is never accepted from either source because Cortex owns that
authenticated channel.

Installed Codex CLI 0.155.1 reports native `tool_search` as removed, so Cortex
cannot rely on dynamic, names-first schema retrieval through the current Codex
ACP adapter. As additional MCP servers are configured, keep the Codex-visible
set bounded to those required for the assignment. Cortex filters whole servers
by role and engine, but neither filters tools within a server nor observes the
aggregate `tools/list` count. The inventory's 64-name bound truncates only
diagnostics, not the servers forwarded to an engine. One large server or many
small ones can therefore still exhaust the model-visible tool catalogue; use
each server's own service/tool filter and qualify its actual `tools/list`
before enabling it. Claude's SDK supports deferred MCP loading when tool
search is enabled, but use through this ACP route still requires a retained
live observation before claiming it worked for a specific Cortex task.

## No native fallback

**Scope of this claim (F4, T013 review):** the statement below is checked,
and true, for `engine.codex_install`/`engine.codex_adapter_acp` — the two
modules this task (T013) adds — and, by the same automated check, for
every other `cortex_runtime/engine/*.py` module as it exists today
(`client.py`, `correlation.py`, `logs.py`, `mcp_bridge.py`, `process.py`,
`session_manager.py`, `stub_handlers.py`). It is **not** a standing
guarantee about a future module added to that package: any later task
adding a new `cortex_runtime/engine/*.py` file must keep this boundary
itself (the import-boundary test below scans the whole directory by glob,
so a new module is automatically covered on its next run, but the
boundary is still something an author must not violate, not something
enforced at review time only).

`engine.codex_install`/`engine.codex_adapter_acp` never import the legacy
native Codex `_meta` adapter (`cortex_runtime.codex_adapter` — an unrelated
module name, deliberately different to avoid confusion, and deleted
outright at T033 along with `server.py`'s unchanneled dispatch branch that
used it), `hooks.py`, `host_source.py`, `marketplace_bootstrap.py` (also
all deleted at T033), or anything under a `.codex-plugin` path (see
`tests/test_engine_codex_no_native_fallback.py`, an AST-level
import/string-literal boundary check that scans every
`cortex_runtime/engine/*.py` module, not only this task's own two). A
`ConfigurationRejected` or engine `PortableError` raised anywhere in this
adapter never triggered the legacy native path, silently or otherwise:
there was no code path from this module back into it, and now there is no
such path at all in the repository — the whole Codex plugin harness was
removed at T033 by explicit product decision. There is no legacy
comparison/rollback operation.

## Probe usage (requires explicit approval)

`scripts/cortex_codex_probe.py` runs exactly one bounded real assignment
through a verified pinned install. **It is never run with authorization
flags as part of this task or its automated tests** — see
`tests/test_engine_codex_probe.py`, which only exercises the refusal and
`--plan` paths.

```sh
# Always safe: refuses before loading cortex_runtime at all.
python3 -B scripts/cortex_codex_probe.py
# exit 2

# Always safe: prints the steps it would take, executes nothing.
python3 -B scripts/cortex_codex_probe.py --plan

# NEVER run this without genuine, out-of-band human authorization:
# python3 -B scripts/cortex_codex_probe.py \
#   --i-authorize-paid-call --approval-ref "<real approval reference>" \
#   --connection-id cn_... --route-id xr_... \
#   --install-record /path/to/install-record.json --output /path/to/result.json
```

Live qualification against a real Codex assignment with a verified
selected model is out of this task's scope; see T035's own live-run
boundary and `.cortex/acp-migration/templates/QUALIFICATION_RUN.json`.

Codex worker turns allow network access so local HTTP servers and browser
verification can run. The role is bound to the exact newly created engine
session; coordinator and unbound sessions retain disabled network access.
This permits worker networking generally, not a loopback-only network policy.
Workspace and temporary-directory write restrictions remain in force.
A 2026-09-21 live worker exposed the former blanket network ban by failing
`http.server` at socket creation; the revised worker policy needs a fresh
live serving observation before that check can be counted as passing.
