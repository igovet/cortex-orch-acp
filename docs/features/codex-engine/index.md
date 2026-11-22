# codex-acp outgoing adapter

> Status: migrated from the former ACP reference. Statements were not re-verified line by line against source in this pass; claims without a source path are unverified until reviewed. The former Python runtime was removed on 2026-11-16: references to its modules and tests were dropped, and statements that cited them describe behavior carried from that implementation that has not been re-verified against the Rust runtime.


Cortex is an outgoing ACP v1 client of a pinned, operator-verified
`@agentclientprotocol/codex-acp` install (Node.js entry point over the Codex
App Server) — the exact same outgoing-client role documented for
the engine session layer (`crates/cortex-engine`). This page covers
the codex-acp-specific pieces: pinned install identity/integrity and the
launch/env mapping, configuration-option mapping, evidence and instruction
delivery.

For the current Rust workspace session path, Cortex writes the composed
instructions to a session-private `model-instructions.md` and passes it through
`CODEX_CONFIG.model_instructions_file`. The native Codex session therefore does
not receive Cortex instructions in the user turn. The adapter compatibility
notes below retain the `_meta.systemPrompt` and `thread/start` terminology for
the historical ACP adapter contract. Model-facing Cortex discovery is always
`tools_discover`; ACP `tools/list` is only transport bootstrap.

## Schema discovery and evidence batches

Native Codex Code Mode is supplied by the engine, not Cortex.
Cortex's additive [discovery/read API](../tool-discovery-and-read-batch/index.md)
recovers exact channel-offered schemas when a native renderer shows only names.
The Codex cheat sheet remains mechanically derived from the live catalogue.
The batch's native-arguments object is deliberately dynamic; discover its inner
tool schema rather than guessing. Individual tools and normal bootstrap loading
remain unchanged. Native exec can already combine independent calls in one
model turn: no additional turn, token or cost saving is established there.

Workspace Debian packages include this adapter (2.6.0), Codex 0.166.0 and its Linux binary, a locked npm dependency tree and Node 24.14.1. Discovery prefers the current channel's read-only `/usr/lib/<package>/engines` tree with its own Node; the private patch remains in connection state. No first-run npm/download is required. See [offline packaging](../../project/packaging.md#offline-workspace-engines) and the [licence inventory](../../project/bundled-engine-licenses.md).

## Approved installed adapter selection

Incoming ACP startup and approved engine installations advance existing connections
to newer approved owned installs (or relocate the same version to the verified Workspace bundle), preserving accounts, routes and model
settings. Unregistered versions never displace a registered install; launch-time
digest/private-patch checks remain mandatory. There is no per-connection pin;
`CORTEX_ENGINE_FOLLOW=off` disables following for rollback. See
[selection, private backups and rollback](../../configuration.md#approved-adapter-following-and-rollback).
The source path is `follow_approved_adapters`.

## Current support: 2.6.0 (2026-11-16)

<a id="current-support-260-2026-11-16"></a>

Exact **2.6.0** is registered alongside **2.3.0**, **2.1.1**, **2.0.1**,
**1.13.0** and **1.12.0**; unknown releases still fail closed (2.4.0, 2.5.0 and
anything after 2.6.0 are not registered). The inspected distribution's
`dist/index.js` SHA-256 is
`733b7521c2a437a34ed016d45a6f46bebff2bac123a172467574ee827db98b99`. The
adapter declares `@openai/codex` `^0.166.0`, so the bundle installs Codex
**0.166.0**, not the newer 0.169.0 (outside the declared range and not
inspected). It bundles ACP SDK 2.4.0 inside `dist/index.js` (no runtime import
of the nested `@agentclientprotocol/sdk`).

Cortex still negotiates **ACP v1**: the real 2.6.0 adapter answered a v1
`initialize` with `protocolVersion: 1`, the same `api-key`/`chat-gpt` auth
methods, `auth.logout`, and one added `sessionCapabilities.title`. Cortex's
native login probes are CLI commands, not adapter calls: `codex login status`,
and the wrapper's startup checks (`features list`, `mcp list --json` with the
nine `-c` overrides) behave identically on 0.166.0 as on 0.164.0.

The private patch re-derives four of the 14 guarded sites for the 2.6.0
bundle; ten are byte-identical to 2.3.0 and every site still must match
exactly once. Behavior is unchanged by design:

- `newSession` now destructures `{ config, skippedMcpServers,
  defaultPermissionProfileId, ...permissionProfileMetadata }`. The role,
  `_meta.systemPrompt`, base-instruction and native-context-flag handling is
  the 2.3.0 logic against that variable.
- The session config now reads the effective Codex config (permission
  profile, default approval policy and reviewer) before MCP conversion, and
  the converter is `toCodexMcpServerConfig(mcp.server)`. The patch still
  keeps every requested server (no native-config de-duplication) and adds
  `enabled`, `default_tools_approval_mode: "approve"`, the one-year tool
  timeout and the 32 KiB input-schema budget.
- Upstream adds permission profiles: a profile id makes `sendPrompt` send
  `permissions` and `runtimeWorkspaceRoots` instead of `sandboxPolicy`. The
  patched turn start removes that branch, so Cortex keeps sending only
  `approvalPolicy: "untrusted"`, `approvalsReviewer: "user"` and
  `sandboxPolicy: { type: "dangerFullAccess" }`; the 0.166.0 App Server schema
  still accepts `sandboxPolicy` on `turn/start`, but no turn was run here.
- Upstream rejects an empty `systemPrompt` content list; Cortex always sends
  non-empty text.

No-paid observation with the real 0.166.0 install, an isolated project and the
owner's existing ChatGPT login (`initialize`, `session/new`, and set-config
calls only, no prompt): the same four modes (`read-only`, `workspace-write`,
`agent`, `agent-full-access`), the same `model`, `reasoning_effort` (low to
max), `collaboration_mode` and `fast-mode` options and the same model list as
2.3.0 with 0.164.0 (`gpt-6.1-sol`, `gpt-6-astra`, `gpt-6.1-terra`, `gpt-6-sol`,
`gpt-6-luna`, `gpt-5.6-sol`, `gpt-5.6-terra`, `gpt-5.6-luna`). Model choices
remain the account's native `model/list`; the adapter version does not add
models. The patched 2.6.0 bundle also created a session with a worker role,
instruction text and `cortexNativeContext: false`. This is not a paid model
turn, native-tool enforcement or rendered client evidence.

## Previous support: 2.3.0 (2026-10-27)

<a id="current-support-230-2026-10-27"></a>

Exact **2.3.0** is registered alongside **2.1.1**, **2.0.1**, **1.13.0** and
**1.12.0**; unknown releases still fail closed. The inspected distribution's
`dist/index.js` SHA-256 is
`d584cb7df74847661c3cf69d34de6ad3a24f6bc69787faeaff49a1fd9a3d8870`.
This installed copy resolves Codex **0.164.0** and ACP SDK **2.0.2**.

Cortex still negotiates **ACP v1**, not the adapter's new v2 transport.
The private adapter translates Cortex's existing `_meta.systemPrompt` into
the new append-style session instruction channel. Upstream
`resolveSessionInstructions` preserves the effective native developer
instructions before the role text. The exact private patch now handles the
structured session config, renamed MCP converter, turn-start interrupt state
and optional `clientUserMessageId`, while retaining the manual reviewer,
native-context controls, scoped MCP schema budget and custom-tool/compaction
reporting. Both the original pinned install digest and every patch site are
checked before launch; the original package is never edited.

Model choices remain native `model/list` results. The new gateway filtering
only applies after an explicit ACP `providers/set`; Cortex does not send
that request. No static model list or v2/subagent capability is introduced.

No-paid checks initialize the real adapter with an empty isolated account
directory, and run the installed private bundle against a fake App Server:
instruction preservation/replacement, MCP configuration, model/effort/mode
options, custom tools and compaction. The same trace still passes on 2.1.1.
These are not paid model turns, native-tool enforcement or rendered client
evidence. Existing owner/dev launch commands are not rewritten by source
qualification; activation and isolated CLI qualification are separate.

## Previous support: 2.1.1 (2026-10-07)

<a id="current-support-211-2026-10-07"></a>

The npm latest release observed on 2026-10-07 was **2.1.1**. Its exact registration is separate from
the historical 1.x/2.0.1 evidence below; unknown releases remain unsupported.
All 14 guarded private-adapter patch sites are byte-identical to 2.0.1
(`dist/index.js` SHA-256
`4b76310393d756a0f111687cd9df899720f36f7c59b1eb1d86429484034ff91b`).
ACP remains v1, with unchanged `model`, `reasoning_effort` and `mode`
option IDs and model/list-derived choices. No new option mapping or static
model allowlist is needed. `collaboration_mode`/`fast-mode` remain unmapped;
the existing execution-verification unavailable sentinel is not promoted.

[Upstream releases](https://github.com/agentclientprotocol/codex-acp/releases)
since 2.0.1 fix imported attachment history, readable service errors, an
active-writer resume error, and AIR custom answers. Cortex does not advertise
AIR or enable native question tools. The session/new instruction extension,
MCP schema budget, private reviewer, per-turn config, custom-tool reporting,
typed-history patch and restored compaction metadata keep their existing
exact-source guards.

The owner's engine directory already contained 2.1.1. This checkout's
discovery selection changed **2.0.1 → 2.1.1** after qualification; the approved
`connections.engine_update.update_engine("codex-acp")` operation returned
no update because the installation was already current. The fallback is now
2.1.1. No production Paseo installation or plugin was changed, and a running
older runtime/session is not hot-upgraded by editing this checkout.

This installed 2.1.1 copy resolves **Codex CLI 0.159.3** (the npm dependency is
`^0.159.1`, not a fixed binary version). Sanitized native login status reports
ChatGPT authentication. Its offline `debug models` catalog includes
`gpt-6.1-sol`; offline catalog presence does not establish a paid route or
grant model-policy authorization.

No-paid checks: 13 installed-bundle fake App Server tests pass for 2.1.1
and retained 2.0.1, including instruction/native-context delivery, explicit
MCP configuration, custom-tool lifecycle, option surface and compaction.
Three installed 0.159.3 wrapper/jail tests pass with a loopback fake provider,
including native patch containment and the advertised tool inventory.
These do not qualify paid turns, rendered client behavior, retained raw
custom-tool history, or instruction survival after real internal compaction.
Paid live check not requested by the owner.

Each qualified codex-acp version is pinned at engine selection, not only at
launch: `QUALIFIED_ADAPTER_VERSIONS` names the qualified
versions, `discovery.discover_engine` prefers a qualified install over a newer
unqualified one, and `engine_update` never prunes a pinned
version. An install outside this qualified set fails closed with
`codex_manual_review_adapter_unsupported` rather than running unverified
patch sites.

Previously qualified versions: **1.12.0**, **1.13.0**, **2.0.1** and **2.1.1**. What was verified for 1.13.0,
against the installed bundle rather than assumed:

- All 15 patched call sites match; exactly one differs from 1.12.0
  (`_CODEX_SEND_PROMPT_SOURCE`: `runTurn`'s `input` became `input: input2`),
  and that variant is derived from the 1.12.0 literal in
  `_CODEX_SEND_PROMPT_BY_VERSION` so the two cannot drift
  apart silently.
- The `_AgentMode` table, the `CODEX_PATH app-server` invocation and the
  `session/new` → `thread/start.developerInstructions` delivery call sites
  are byte-identical in both bundles.
- The former Python no-paid installed-bundle trace (fake App Server, no model turn) passed against each bundle (12 passed for 1.12.0 and for 1.13.0, 2026-09-22). That test was removed on 2026-11-16 and not ported.
- Compaction stays `automatic`: `createCompletedContextCompactionUpdate`
  still carries `_meta: CONTEXT_COMPACTION_META`, the signal
  `session_manager` detects. One real difference, recorded rather
  than glossed: 1.13.0 gates the *in_progress* start update behind
  `clientSupportsCompaction(capabilities) = capabilities?.session?.compaction
  != null`, and Cortex advertises no such capability, so Cortex sees only the
  completed signal where 1.12.0 sent a start/complete pair.

1.13.0 bundles Codex CLI 0.155.1 instead of 0.154.0, which is what makes that
CLI's `gpt-6-sol` / `gpt-6-luna` models reachable: the runtime only
records models a live adapter session actually advertised.

### 2.0.1 compatibility

codex-acp 2.0.1 changes the agent mode label to `Auto review` and replaces the
1.x response-item and history helpers with typed reporters. Its exact-source-
checked patch preserves the manual reviewer, developer instructions,
native-agent restrictions, explicit MCP configuration (plus whole tool
schemas; see [MCP bridge](#mcp-bridge)), per-turn permissions and custom-tool
lifecycle. The renderer restores `_meta.contextCompaction` for ordinary ACP
clients as well as AIR clients. Other Codex versions remain unsupported until
their compatibility is verified the same way.

The no-paid installed-bundle trace uses a fake App Server, including a
completed compaction event; the installed CLI/jail check uses a loopback fake
provider. Real paid turns and rendered client behavior are not run: paid live
check not requested by the owner. Raw custom-tool recovery from retained
2.0.1 history has not been qualified; custom items returned directly by its
typed history path are translated, without recreating the removed 1.x
rollout reader. The behavior of native developer instructions after Codex
internal compaction has never been observed in a real client, for either
2.0.1 or 2.1.1.

## Current runtime gateway policy

The native Codex launcher and codex-acp's thread config disable native
shell/view-image tools, agents, hooks, plugins, apps and native question tools
through supported config, verified with `codex features list` before App Server
starts. Native `apply_patch` remains available; Codex has no supported switch for
it. Cortex does not sandbox the entire Codex process. It requires codex-acp's
native `read-only` mode for every managed Codex session, regardless of role and
access mode. If the adapter does not offer or confirm that mode, the runtime
refuses the session (`restricted_mode_unavailable`), with no writable fallback.
The runtime refuses native permission requests, including those for
`apply_patch`; only channel-bound MCP requests are eligible to pass. Writing
workers edit through Cortex file tools, authorized against their current effective
scope. Brokered commands can write only in contained scratch, not the live
checkout; `outside_command`, `outside_write`, live `project_git` and external
MCP servers are unavailable. See [Managed
Codex write authority](../permissions-and-containment/index.md#managed-codex-write-authority).
Codex has no native read tool here, so the read
rows of the table do not arise. Codex's
`unified_exec` feature stays listed as enabled, but the launcher's `shell_tool`
switch keeps both native shell tool declarations out of the local CLI inventory; see
[Codex shell tool inventory and `unified_exec`](#codex-shell-tool-inventory-and-unified_exec).
Authenticated Cortex MCP operations
provide scoped project access. Account configuration and credentials remain in the
production native account.

### Codex shell tool inventory and `unified_exec`

Codex's native command path is two model-visible tools: `exec_command` (start a
command in a PTY) and `write_stdin` (continue a running session). Both belong to
the `shell_tool` feature. What is and is not established:

- **Feature flag.** In Codex CLI 0.166.0 `codex features list` keeps
  `unified_exec` as `true` whether it is disabled by
  `-c features.unified_exec=false`, a `config.toml` entry or `--disable
  unified_exec`. The launcher therefore does not list it among the features it
  verifies as off (`native_codex_runtime.rs`); adding it would make every launch
  fail on the listing. The listed flag is not the exposure: the tool
  declarations are.
- **Local CLI inventory.** With the launcher's own process flags (`BASE` plus the
  `trust_override` layer, which include `features.shell_tool=false`), `codex debug
  prompt-input` renders a model-visible inventory with neither `exec_command` nor
  `write_stdin`. With only `-c features.shell_tool=true` added, the inventory gains
  exactly those two declarations. This is an observation of the installed CLI
  renderer, not proof for every connection.
- **Offline regression.** `codex_launcher_offline_tool_inventory`
  (`crates/workspace-service/src/native_codex_runtime/tool_inventory.rs`) asserts
  both facts. It is `#[ignore]`d by default and compiled only on Linux x86-64, so
  the default suite needs no Codex binary. An explicit run needs a trusted, locally
  installed Codex CLI 0.166.0 at an absolute `CORTEX_TEST_CODEX_CLI` path, Linux
  seccomp, the pinned Rust toolchain and GNU `timeout`:

  ```bash
  umask 022; CORTEX_TEST_CODEX_CLI=/absolute/path/to/codex \
    timeout --kill-after=5 300 ~/.cargo/bin/cargo test --locked --offline \
    -p workspace-service --lib codex_launcher_offline_tool_inventory -- --ignored --nocapture
  ```

  Each child runs with a cleared environment, private mode-0700 scratch (HOME,
  CODEX_HOME, XDG, temp, project), no MCP servers, and seccomp denying socket,
  connect and io_uring setup. No login, credential, network, model turn, live
  session or tool invocation is used; the test only reads the inventory. The
  **positive control** (the `shell_tool=true` run) separates hidden tools from a
  broken parser: it must show exactly the baseline plus both declarations. Empty,
  truncated, duplicate or unsupported-shape output, a non-zero exit, a timeout or
  another CLI version fails an explicitly selected run rather than passing it. The
  default `native_codex_runtime` unit tests cover malformed-inventory rejection and
  the agreement of process and thread settings.
- **Process vs thread controls.** The launcher flags apply to the Codex process;
  codex-acp's `thread/start` config carries the same switches (`CODEX_CONFIG`
  must match `CODEX_DISABLED_FEATURES`). The test covers the process flags only,
  not the thread config, and Codex may omit deferred nested tools from the
  description, so absence is strong evidence but not conclusive.
- **Custom argv exception.** A managed Codex launch through a custom command
  bypasses the pinned launcher and its flags, so the inventory observation does
  not apply to it. Short-text sessions (titles, commit and pull request texts) are
  stricter: they refuse a Codex route without the launcher, answer every
  permission request with reject and discard the answer on any tool call.
- **Brokered commands vs native rows.** Commands the model runs through Cortex
  tools are authorized by the runtime and leave broker receipts. A native Codex
  command, if one ever started or continued, would reach Cortex only as an ACP
  tool-call update (display) or a permission request. Native permission callbacks
  that are not channel-bound MCP calls are refused, but that covers only requests
  the engine sends; an action it performs without one is unobservable. A timeline
  row for a native command is therefore display only: neither an authorization nor
  a receipt of execution.
- **Not observed.** A live managed session's real tool inventory, the start and
  continuation behavior of either tool, and the rendered timeline. No paid live
  check was requested.

The wrapper verifies installed shapes and checks that account MCP servers are
disabled for this child before starting App Server. Unsupported versions or changed
patch sites fail closed. The earlier installed-wrapper observations above used
a loopback fake provider. They do not qualify the current managed-mode policy
against an installed Codex, or exercise a paid model or rendered client UI. See
[containment](../permissions-and-containment/index.md).

Settings > Connections > Download engine installs the manifest-pinned codex-acp into Cortex's owned engine root and needs no install record (see [engine install records](../../configuration.md#engine-install-records)). The former Python API for recording an install made by another route (for
example Zed's own npx-based Custom Agent registration, or a manual
`npm install @agentclientprotocol/codex-acp`) as a `CodexAcpInstall` was removed; the Rust runtime verifies the manifest-pinned or bundled install natively. The record carried:

```text
node_binary                  absolute, no symlink component
adapter_entry                absolute, no symlink component
package_json                 absolute path
package_version              must equal package.json's own "version"
package_json_sha256          sha256 of package.json
adapter_entry_sha256         sha256 of dist/index.js
bundled_codex_version        observed @openai/codex version
bundled_codex_binary         absolute path of the bundled codex binary
bundled_codex_binary_sha256  sha256 of that binary
```

**This requires explicit operator consent**: recording an install means
trusting a specific set of files as the thing Cortex will actually execute.
Install verification re-verifies every pinned file fresh on every use (never
trusts a previously-recorded hash without re-checking it) and refuses:

- any path that is not absolute, or that resolves differently than given
  (a relative component or a symlink anywhere in the path) —
  `engine_install_untrusted_location`;
- paths under `$HOME/.local/share/app.codeg` or `$HOME/.config/app.codeg`
  (codeg's own private data directories) —
  `engine_install_untrusted_location`. A Zed
  `$HOME/.local/share/zed/external_agents` checkout is allowed when the
  operator deliberately records that exact install: the install record treats the
  client-managed registry as the configured engine source, while
  install verification still requires canonical paths, the pinned package
  version, and fresh hashes for every executable and package file.
- a hash mismatch on `package.json`, `dist/index.js` (`adapter_entry`), or
  the bundled `codex` binary, or a `package.json` `"version"` field that
  disagrees with the recorded `package_version` — `engine_install_drifted`.

`codex_install.launch_command_for(verified_install)` is always exactly
`(node_binary, adapter_entry)` — the only two elements a
`ConnectionRecord.launch_command` for a codex-acp connection may ever
contain when an install is supplied (see `EngineProcess`,
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

`build_process_env`'s new `pinned_engine_vars`
parameter is the only place these two names may ever be set: it is
accepted only for `record.engine == "codex-acp"`, only as exactly
`{"CODEX_PATH", "NO_BROWSER"}`, and is the only explicit, documented bypass
of the module's own credential-shaped denylist. Nothing in this adapter
ever sets `OPENAI_API_KEY`, `CODEX_CONFIG`, `MODEL_PROVIDER`,
`DEFAULT_AUTH_REQUEST`, `INITIAL_AGENT_MODE`, or `APP_SERVER_LOGS` — those
are either a different auth-precedence variable Cortex never uses or native mechanisms that Cortex does not wire up.

## Configuration-option mapping

`engine.codex_adapter_acp.codex_option_mapping(revision, observed_config_options)`
maps a `ConfigurationRevision` to `session/set_config_option` pairs, using
the pinned release's observed `configOptions` (from `session/new`, per
`tests/fixtures/catalog/codex_acp_config_options.json`):

- `model` — always required, always taken from `revision.route.model`.
- `reasoning_effort` — mapped only when the revision's
  `supported_parameters` includes a `thought_level` or `effort` entry; the pinned release's only real observed values are
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
`EngineConfigurationPartiallyApplied` — a distinct,
plain exception, never `ConfigurationRejected` — if an earlier option in
the same call already succeeded before a later one is rejected, mismatched,
errored, or found no-longer-offered. `configuration.
service.ConfigurationService.apply()` maps `ConfigurationRejected` to the
revision's terminal `failed` state (nothing was ever applied) but maps
`EngineConfigurationPartiallyApplied` — like any other plain exception —
to `indeterminate` (the engine may be left in a mixed state; a fresh
`reconcile()` using real engine evidence is required), never a false
`failed` that would understate what the engine actually did.

## Client-facing selection surface: provider, model, effort

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
through the same flow every other selection already used; an
unsupported value is refused by `configuration.resolution
.validate_dependent_settings` / `engine.codex_adapter_acp
._require_supported_option`, never silently accepted. Changing `provider`
necessarily changes engine and so may change the valid model/effort sets:
the same model *name* is kept across the switch only if the new provider
happens to offer it too, otherwise the new provider's own first model is
used explicitly and `thought_level` is always reset — never a silently
kept invalid model, never a silently dropped choice. See
`build_provider_option`/`build_model_option` and
`_apply_config_selection`.

Independently of the route, every client also receives the user's access
mode: Zed as the `access_mode` config option, Paseo as ACP `modes` (its mode
control) — see
[permissions](../permissions-and-containment/index.md#access-mode-guarded-and-auto).
`session/set_mode` sets only that mode. Fresh ACP sessions initialize Development
for brokered writing-worker host commands; Guarded and explicit Auto remain
selectable. No access mode changes the Codex session mode or native-tool policy; see the permission page for Docker/alias limits.

The `/cortex-model <route label or route id>` slash command still selects
a whole route (any provider) at once, for a host with no
`SessionConfigOption` support — see the ACP command module.

### Model catalog: real observation, static seed, never invented

`selectable_entries_from_snapshots` normally reads a
connection's most recently **observed** catalog snapshot (a real, parsed
`session/new` `configOptions` payload — `DISCOVERY` provenance). A
connection that has never yet had a real session observed for it falls
back to a small, static, per-engine **seed** list
(`SEED_MODELS`, `SEED` provenance, never `DISCOVERY`)
so a first-time user still gets a usable `model` select instead of an
empty one. A seeded value is always rendered "seeded — not yet confirmed
by an engine" — never as
observed/confirmed — and is unconditionally superseded the moment a real
observation is captured for that connection (the seed is never itself
persisted to a snapshot file, so the very next real snapshot simply wins).
`model_seed`'s own module docstring records exactly which local
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
`EngineSessionManager.observed_config_options`) — best-effort, never itself
sending a prompt or failing the engine session. It runs before the session's
first prompt, so a process that already saved an identical observation (same
connection, install and `configOptions` apart from current selections, within
six hours) skips it: the per-model probe cost seconds per worker session.
The former manual `record-observation` setup command was removed; use `refresh-catalog` (`scripts/cortex-paseo-tools.sh refresh-catalog`).

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
  later codex-acp release, is not covered by the automated suite.

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

Two more Cortex-owned `_meta` fields remove what Codex adds on its own. The
runtime sends both with the [unified system prompt](../unified-system-prompt/index.md)
(`build_codex_session_new_meta(..., base_instructions=...)`). A
string `cortexBaseInstructions` becomes `thread/start.baseInstructions` and
replaces the model's base prompt; `""` removes it. `cortexNativeContext: false`
sets `include_permissions_instructions`, `include_apps_instructions`,
`include_collaboration_mode_instructions`, `include_environment_context` and
`skills.include_instructions` to `false` in that thread's config. A request
with both fields sent to Codex CLI 0.155.1 against a local stub endpoint
carried only the Cortex developer message, the tool list and the user
message. Model behavior without the native prompt has not been qualified.

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

Managed Codex sessions receive only Cortex's authenticated MCP server.
The runtime withholds matching external servers, including registry entries
that select `codex-acp`, and rejects external MCP calls on that route.
See [current runtime gateway policy](#current-runtime-gateway-policy).

For eligible non-Codex routes, additional stdio servers can come from the
Cortex-owned registry (`$XDG_CONFIG_HOME/cortex/mcp-servers.json`) and, where
supported, the ACP client's `session/new` `mcpServers` list. Paseo sends an
empty list, so the registry is the portable configuration path. The runtime
resolves the effective list on `session/new` and again on `session/load`.
An owner-registry server may set `"roles": ["coordinator"]` or
`"roles": ["worker"]` (or both); omitting `roles` retains visibility for both.
Cortex validates this field and removes it from the ACP session request. The
optional `engines` array matches the verified route's engine and is likewise
not sent on the ACP wire. These filters narrow eligibility; they cannot
override the managed Codex rule. For example, a browser MCP server intended
only for Claude workers uses `"roles": ["worker"], "engines": ["claude-agent-acp"]`.
The session log records only the selected server names and count, bounded to
64 names with an explicit truncation flag; it does not claim to have observed
those servers' tools. This controls server visibility, not individual tools
within a server. Where client forwarding is supported, the global registry
wins on duplicate names. Registry files are owner-only; their environment
values are never persisted in the project store or diagnostics. A
project-local registry is intentionally ignored. Qualifying a configured
external server requires observing it start and serve a call on an eligible
route, not a managed Codex run.

Inherited Codex account MCP entries are a separate source from these explicit
session servers. The Cortex-owned child launcher invokes the verified CLI with
`features.plugins=false`, checks the installed feature inventory, reads `codex
mcp list --json`, and constructs per-name `mcp_servers.<name>.enabled=false`
overrides. It verifies that the inherited native entries are disabled before
executing App Server. It reads names and enabled booleans, not credential
fields; the inventory and child stderr are neither printed nor retained. A
malformed inventory, duplicate name, or name outside `[A-Za-z0-9_-]+` fails
closed because it cannot safely form an override. These checks have offline
coverage, but effective isolation in a real client remains unverified
pending a retained live observation.

The `-c features.*` layers alone do not keep features off in a session thread.
codex-acp 2.6.0 sends a `features` table in every session's `thread/start`
config (at least `cwd_relative_turn_diffs = false`), and Codex 0.166.0 lets that
table replace the whole `features` layer of the App Server's `-c` flags; the
owner's `config.toml` then applies underneath. With only those layers, each
Cortex-launched session therefore started the MCP servers of the owner's
installed plugins (observed live: the computer-use REPL `cua_repl`, which starts
`node_repl` and a sandbox probe, and the Codex app tools `server.mjs` run by the
node of `~/.cache/codex-runtimes/codex-primary-runtime`), and the other launcher
feature switches did not hold for the thread either. codex-acp spreads the
`features` of its own session config, the JSON object in its `CODEX_CONFIG`
variable, into that table, and Codex itself never reads the variable. Every
launcher-backed Codex launch therefore sets `CODEX_CONFIG` to exactly
`{"features":{...}}` with `multi_agent`, `multi_agent_v2`, `plugins`, `hooks`,
`apps`, `shell_tool` and `view_image` all `false` (`CODEX_DISABLED_FEATURES` in
`crates/cortex-engine/src/environment.rs`, the same features as the launcher's
`-c` layers). The launcher inherits that variable from codex-acp and refuses a
session launch (one that hands over relays) whose `CODEX_CONFIG` is missing or
has any other shape; another launch may omit it but never carry a different
one. Observed offline with the bundled codex-acp 2.6.0 and Codex 0.166.0, a
fixture plugin and owner server, a fake API key and no network: without the
variable the plugin's server starts at `session/new`; with it only the
session's relay starts. The catalogue observation sets `CODEX_PATH` itself and
does not set `CODEX_CONFIG` yet, so its short-lived observation session still
starts the owner's plugin MCP servers.

Codex 0.166.0 lets a thread's request config replace the whole `mcp_servers`
layer that carries those overrides: codex-acp sends a session's ACP
`mcpServers` as one `mcp_servers` table in `thread/start`, a later
`mcp_servers` table replaces earlier `mcp_servers.<name>` keys of the same
layer, and the owner's `config.toml` merges back in underneath, so every
owner-configured server started in Cortex sessions although the launcher had
turned it off (observed offline with the bundled codex-acp 2.6.0 and Codex
0.166.0, fake owner servers, a fake API key and no network). A session started
through the launcher therefore sends no ACP `mcpServers`. For managed Codex,
the service hands only the authenticated Cortex channel's secret-free relay
entry to the launcher in `CORTEX_CODEX_MCP_SERVERS`; external server relays are
not included. The launcher defines the relay as an inline
`-c mcp_servers.<name>={command,args,env}` layer, checks with
`codex mcp list --json` that every owner server is off and the relay is on,
removes the variable and executes App Server. A relay with the same name as an
owner-configured server (whose other fields would merge into it) or a malformed
hand-over refuses the launch. A managed Codex launch without the launcher
(a custom command) still receives the Cortex channel relay via ACP, not
external server relays. For launcher-backed sessions, codex-acp does not
report MCP startup failures for the relay because it did not request it;
App Server starts it at `thread/start`. The plan-limits read
(`account/rateLimits/read`) starts its App Server with the same feature
layers, so plugin marketplaces with a remote source are no longer synced
(`git ls-remote`) on every limits read.

The accepted owner registry shape for eligible routes is:

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
ACP adapter. Every model in its catalogue (`gpt-5.6-luna`/`terra`/`sol`,
`gpt-6-astra`, `gpt-6.2-sol`, `gpt-6.2-astra`) is `tool_mode: code_mode_only`, and
`tool_search_always_defer_mcp_tools` is fixed on. The model therefore sees one
JavaScript `exec` tool; the `cortex` MCP tools are nested behind it as
`tools.mcp__cortex__<name>(...)` and listed in `ALL_TOOLS` as
`{ name, description }`. A live 2026-09-23 worker read only the names there and
guessed every tool's first arguments (`command`/`cwd`, `patch`, `id`), so each
first call was refused with `invalid_arguments` and retried. Cortex does not
turn code mode off: Codex warns that these models degrade without it. Instead
the worker protocol tells a worker to read a tool's full listed description
before its first call, and every `invalid_arguments` refusal names all missing
and unadvertised fields together with the complete required/optional field
list, so one corrected retry succeeds. A failed `project_read`/`read_skill`
keeps that value-free reason (`error`, `field`, `expected`, `correction`, never
`received` or the body) in the worker's display history. Whether a live worker
now reads descriptions first has not been observed yet.

The Rust host puts Cortex role and route instructions in a private per-session file selected by Codex's `model_instructions_file` thread configuration. A bounded Codex supplement renders signatures from the served schemas, with required and optional fields, numeric and length bounds, array item types and schema examples where available. The full contract and the catalogue for the current channel are available through `tools_discover` on both engines.

What code mode shows of a nested tool is a TypeScript declaration Codex derives
from the MCP `inputSchema` (Codex 0.159.1 `code-mode-protocol`
`json_schema_types.rs`), and it loses contract text in two ways; both exist in
0.155.1 as well. First, Codex keeps only `type`, `description`, `enum`,
`items`, `properties`, `required`, `additionalProperties`, the combinators and
`$ref`/`$defs`; bounds, `pattern`, `not` and `if`/`then`/`else` never reach the
model, and a schema whose normalized JSON exceeds the server's
`tool_input_schema_max_bytes` (default 5,000) is compacted, the first pass
stripping every description. Live 2026-09-30 sessions received `plan_submit`
and `result_submit` with no property descriptions, so the coordinator sent an
explorer task `scope: []`. For codex-acp 2.0.1 the private patch sets
`tool_input_schema_max_bytes` to 32,768 (`CODEX_MCP_TOOL_SCHEMA_MAX_BYTES`)
on every forwarded MCP server; codex-acp 1.x has no such key, so its large
schemas stay stripped. Second, a combinator is rendered instead of the
object's own properties: the root `oneOf` of `result_submit` produced
`{ [key: string]: unknown; } | { [key: string]: unknown; }` and the `allOf`
on each `checks` item produced `Array<unknown>`. Cortex therefore expresses
those presence rules as `if`/`then`/`else`, which Codex drops while JSON
Schema validators and the runtime still enforce them. The former Python
test suite ported the 0.159.1 normalization, compaction and rendering and
required every advertised Cortex field and description to survive it. The port matched the installed 0.159.1
binary's declarations exactly in a no-paid loopback-provider trace through the
patched 2.0.1 adapter. A forwarded server's schema passes through the external
gateway unchanged except for arguments it leaves undescribed and Cortex knows: codebase-memory-mcp
0.10.8 describes none of `search_graph`'s `file_pattern` (a glob on the
project-relative path, converted to SQL `LIKE`), `name_pattern` or
`qn_pattern` (case-insensitive regular expressions), and a model sent a regex
`file_pattern` that silently matched nothing. The gateway adds those three
descriptions; a description the server supplies itself is never replaced.

As additional MCP servers are configured, keep the Codex-visible
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

The Rust engine layer never routes through the legacy native Codex `_meta` adapter, the lifecycle hook dispatcher, the host-source or marketplace bootstrap modules, or anything under a `.codex-plugin` path: the whole Codex plugin harness was removed by explicit product decision, and the Python runtime that once guarded this boundary with an import-boundary test was removed as well. A configuration or engine error never falls back to a native path, silently or otherwise. There is no legacy comparison/rollback operation.

## Probe usage (requires explicit approval)

`scripts/cortex-paseo-tools.sh probe-codex` runs exactly one bounded real prompt (a single reply word) through an explicitly named adapter executable. **It is never run with authorization flags in the automated tests**; only the refusal and `--plan` paths are exercised.

```sh
# Always safe: refuses before any file read, workspace creation or process start.
scripts/cortex-paseo-tools.sh probe-codex
# exit 2

# Always safe: prints the steps it would take, executes nothing.
scripts/cortex-paseo-tools.sh probe-codex --plan

# NEVER run this without genuine, out-of-band human authorization:
# scripts/cortex-paseo-tools.sh probe-codex \
#   --i-authorize-paid-call --approval-ref "<real approval reference>" \
#   --adapter /absolute/path/to/codex-acp [--model <model>] --output /path/to/result.json
```

Live qualification against a real Codex assignment with a verified
selected model is not covered by the automated suite.

Codex worker turns allow network access so local HTTP servers and browser
verification can run. The role is bound to the exact newly created engine
session; coordinator and unbound sessions retain disabled network access.
This permits worker networking generally, not a loopback-only network policy.
Workspace and temporary-directory write restrictions remain in force.
A 2026-09-21 live worker exposed the former blanket network ban by failing
`http.server` at socket creation; the revised worker policy needs a fresh
live serving observation before that check can be counted as passing.
