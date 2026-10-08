# claude-agent-acp outgoing adapter

## Current support: 0.88.0 (2026-10-08)

Cortex now has an exact instruction-delivery registration for the installed
`@agentclientprotocol/claude-agent-acp` **0.88.0**, bundled Claude Agent SDK
**0.3.293** and ACP SDK **1.7.0**, still speaking ACP v1. It needs no SDK
override build. The independent 0.78.0 and 0.87.0 registrations remain available.

Read-only inspection of the installed distribution established these paths:

| Contract | 0.88.0 source |
| --- | --- |
| ACP v1 | `dist/acp-agent.js:1368` |
| String override / preset append → query options, separate from conversation | `dist/acp-agent.js:7091-7107,7221-7225,7381-7384` |
| Load/resume resupply session-defining metadata when recreating queries | `dist/acp-agent.js:1468,1499-1523,6968-6976` |
| SDK string/preset conversion and initialize control fields | SDK `sdk.mjs:142` (`fT`), `sdk.mjs:138` |
| Strict MCP/options, explicit disallowed tools and tools forwarding | `dist/acp-agent.js:7140-7145,7225,7267-7268`; SDK `sdk.mjs:136` |
| Model / effort / mode select shapes | `dist/session-model.js:179-192`, `dist/session-effort.js:64-100`, `dist/session-mode.js:68-75` |
| Usage model metadata (`_claude/model`), not full execution identity | `dist/acp-agent.js:2614-2625,3565,4377,4835,5206` |
| Reactive non-AIR compaction marker | `dist/tool-calls/reporters/compaction.js:17-65` |

The inspected `dist/acp-agent.js` SHA-256 is
`790992f7c7387d78a5324fcfcbf9eb79a811ebd68eb3d6161a93de34b28327a0`.
Tests also initialize the real adapter with an empty account directory and no
prompt, and inspect its option forwarding without calling a model. Static
source qualification and this protocol check do **not** prove paid execution,
effective native-tool enforcement, rendered GUI behavior or release readiness.

The previous failure was a runtime version-selection mismatch: the updater
installed 0.88.0, discovery preferred it, but only 0.78.0/0.87.0 had exact
instruction-delivery registrations. Unregistered versions could also miss the
runtime's native-tool metadata branch. The adapter's `AskUserQuestion`-only
denial and `--tools default` are its defaults, **not** Cortex's policy.
Dispatch now uses the exact registry for that metadata branch, supplying the
existing Cortex native-subagent/gateway-alternative denial, strict MCP,
empty plugins and disabled hooks to all registered Claude releases.

Discovery orders valid candidates by their **declared package version's**
exact instruction-delivery registration first, then by version/build. Future
unregistered installs cannot displace the newest registered install; pruning
retains registered versions and their SDK refresh builds. With no registered
install, discovery still reports the actual unsupported version for diagnostics,
but critical-role dispatch refuses it before launch.

A non-retryable `instruction_delivery_unqualified` failure is retained as
route unavailability, not a model-performance failure. Critical-role dispatch
excludes that engine/version across connections and models for the task's
dispatch lifetime; authorized alternate routes remain subject to existing
routing policy, and pins/user-selected identities are never silently replaced.
A different qualified version on the same connection does not inherit the
refusal. Paid live check not requested by the owner.

<a id="current-support-0870-2026-10-07"></a>

## Previous exact support: 0.87.0 (2026-10-07)

On 2026-10-07 the fallback and exact instruction-delivery registration were
`@agentclientprotocol/claude-agent-acp` **0.87.0**. The 0.78.0 registration
is retained independently; unregistered installed Claude versions are refused
rather than borrowing the fallback's qualification. The owner's engine update
path is `connections.engine_update.update_engine("claude-agent-acp")`;
it installs beside existing versions under Cortex's owned engine directory,
not the production Paseo installation. An initial invocation on 2026-10-07 found the upstream 0.87.0 install current.
A subsequent no-paid investigation found that its exact SDK dependency
0.3.287 predates Haiku 5.5. Cortex now installs a separately inspected SDK
**0.3.293** override at `0.87.0+sdk.0.3.293`, beside the unchanged upstream
install. Discovery prefers this build while reporting the real adapter version
0.87.0. The updater pins this exact SDK for this exact adapter, not SDK latest;
future adapter versions do not inherit the override.

Evidence was read from the [published npm tarball](https://registry.npmjs.org/@agentclientprotocol/claude-agent-acp/-/claude-agent-acp-0.87.0.tgz)
and the installed package, not inferred from a version-string change:

| Contract | 0.87.0 source |
| --- | --- |
| ACP v1; SDK dependencies Claude 0.3.287, ACP 1.7.0 | `package.json`; `dist/acp-agent.js:1342` |
| String override / preset-append system instructions | `dist/acp-agent.js:7047-7063` |
| SDK options forwarded; separate prompt and options into query | `dist/acp-agent.js:7079,7177-7181,7223,7337-7340` |
| Load/resume preserve supplied session metadata | `dist/acp-agent.js:1442,6928` |
| SDK turns system text into separate initialize-control fields | SDK `sdk.mjs`, `BR`, `systemPrompt` / `appendSystemPrompt` |
| Exact allowlisted model IDs retained; labels versioned from independent SDK metadata | `dist/session-model.js:272-325`, `versionedModelDisplayName` |
| Effort choices come from the current SDK model; recommended presentation can omit `default` | `dist/session-effort.js:buildEffortConfigOption` |
| Non-AIR reactive compaction marker | `dist/tool-calls/reporters/compaction.js`, `_meta.claudeCode.toolName="compact"` |

The [upstream 0.78→0.87 changelog](https://raw.githubusercontent.com/agentclientprotocol/claude-agent-acp/v0.87.0/CHANGELOG.md)
also records SDK updates, resume-query recreation, bypass-policy and
cancellation fixes, AIR tool-contract changes, optional session notices, and
restored main-thread agent selection. Cortex continues using ACP v1 and does
not expose the agent picker as runtime delegation. Native subagent/async tools
remain disallowed. Model metadata on newer usage updates does not bind a
complete model/effort/mode execution identity; Cortex's version-neutral
execution-verification status remains unavailable.

The instructions and native-tool restriction payloads are unchanged:
`strictMcpConfig`, empty plugins, disabled hooks, `disallowedTools`, and
unified string/preset-append system prompts still follow their inspected SDK
paths. Static qualification does **not** establish a paid model turn, effective
tool enforcement, rendered GUI behavior, or release readiness.

Unlike 0.78.0, 0.87.0 reserves detailed compaction metadata for AIR clients.
Cortex does not advertise AIR or standardized compaction support. It now
recognizes the explicit Claude `toolName="compact"` marker on tool-call start
and completion, retaining one correlated evidence identity, rather than
inferring compaction from the display title. The tier remains reactive
`automatic`, never a pre-compaction hook.

### Haiku 5.5 catalog and effort evidence

No-prompt initialization on 2026-10-07 distinguished two native runtimes:

| Native runtime | SDK catalog value | Resolved model / display name | Efforts |
| --- | --- | --- | --- |
| Upstream SDK 0.3.287 / CLI 2.1.287 | `haiku` | `claude-haiku-4-5-20251001` / Haiku 4.5 | No effort support fields |
| Refreshed SDK 0.3.293 / CLI 2.1.293 | `haiku` | `claude-haiku-5-5` / Haiku 5.5 | low, medium, high, xhigh, max |
| Refreshed SDK 0.3.293 / CLI 2.1.293 | `claude-haiku-4-5-20251001` | Same exact ID / Haiku 4.5 | No effort support fields |

The owner's independently installed CLI 2.1.293 returned the same two SDK
rows before installation. Its bundled model table explicitly records
`claude-haiku-5-5`, Haiku 5.5 and effort/max/xhigh capabilities. The npm
adapter's latest remained 0.87.0 while SDK latest/next was 0.3.293; updating
only the adapter version could not reveal the new model.

Using the installed refreshed build, ACP `session/new` advertised `haiku`
as **Haiku 5.5**. With no prompt sent, `session/set_config_option` selected
that alias and independently accepted high, xhigh and max with exact returned
current values. ACP exposed default/low/medium/high/xhigh/max effort choices.
The SDK lists old Haiku separately; the observed ACP picker only exposed
the new alias. This is configuration/control evidence, not inference from
the generic unknown-model effort fallback and not a paid inference result.

The public preset and the owner's active `default` profile enable `haiku`
with the same light tier, testing stretch, high→xhigh→max ladder, maximum,
preferences and request-name policy as `codex-acp/gpt-6-luna`.
`seven_day_haiku` names that exact alias. Only the old exact
`claude-agent-acp/claude-haiku-4-5-20251001` is denied; Cortex deny matching
does not infer families. Existing owner settings are not silently replaced
by installation; this production adoption was explicitly authorized and
backed up. New sessions must use the refreshed install, not the original SDK.

SDK 0.3.293's `sdk.mjs` `fT` still separates system/preset-append
instructions from conversation input, and its initialize control payload
carries `initConfig.systemPrompt`/`appendSystemPrompt`. CLI argument
construction still forwards strict MCP and disallowed tools. The adapter
source and compaction marker are unchanged. These independently inspected
paths extend the exact 0.87.0 registration; they do not qualify unknown SDK
versions or prove native-tool enforcement.

Native Claude login was checked without printing identity or credentials:
logged in, `claude.ai`, first-party provider. No prompt or paid model call
was sent; all owned discovery processes exited. A new owner-requested live
check is still required for turn-level qualification; paid live check not
requested by the owner.

### Catalog refresh lifetime and deployed runtimes

A prompt-free Claude refresh observes every model's own effort selector with
one `session/set_config_option` each; on 2026-10-08 eleven models took about
12 s (11.5 s of it in that observation). Paseo reads models from a Cortex ACP
process it stops about a second after `session/new`. While the refresh started
after `session/new` ran on a thread of that process, it was killed every time:
the isolated dev Paseo's Refresh saved Codex and OpenCode catalogs but never a
Claude one. It now runs in its own process session
(`src/cortex_runtime/catalog/detached_refresh.py`, one refresh per connection
at a time) and saves its snapshot after the requesting process has exited.

The picker also shows only models the effective routing enables. On the same
date the dev profile's `routing.json` still carried the pre-Haiku policy (bare
`haiku` denied, not enabled), so a catalog that already held Haiku 5.5 still
produced the six-model picker. It was updated to the current preset through
`management.update_routing` after a private backup.

A Cortex runtime deployed before the `<version>+sdk.<sdk>` build-name ordering
in `connections/discovery.py` sorts `0.87.0+sdk.0.3.293` below `0.87.0`. Each
start of such a runtime repoints the Claude connection back to the upstream
install, whose SDK 0.3.287 resolves `haiku` to Haiku 4.5 with no effort
choices, even though routing names `haiku` at Luna parity. The owner's
production Paseo runtime was in that state on 2026-10-08; production sessions
get Haiku 5.5 only after the current runtime is deployed there.

## Historical 0.78.0 implementation evidence

The sections below retain the original T026/0.78.0 evidence and historical
qualification limits. Their exact-version-only statements describe that
historical release, not the current 0.87.0 registration above.


Cortex is an outgoing ACP v1 client of a pinned, operator-verified
`@agentclientprotocol/claude-agent-acp` install (Node.js entry point over
the Claude Agent SDK) — the second, non-Codex engine proving the portable
runtime is genuinely engine-independent (see T002's ADR-001/ADR-001a/
ADR-001c selection record). This page covers the claude-agent-acp-specific
pieces added in T026:
`src/cortex_runtime/engine/claude_install.py` (pinned
install identity/integrity) and
`src/cortex_runtime/engine/claude_adapter_acp.py`
(launch mapping, configuration-option mapping, evidence).

Pinned release basis: `@agentclientprotocol/claude-agent-acp` **0.78.0**
(bundles `@anthropic-ai/claude-agent-sdk` 0.3.270, ACP SDK 1.4.0), per the
static evidence in `.cortex/acp-migration/T002/CAPABILITY_MATRIX.json` and
`.cortex/acp-migration/evidence/T002/static-capability-findings.md`, and
the fresh, lead- and agent-re-verified dist file:line citations below (read
2026-09-17 directly against the real installed copy at
`~/.local/share/zed/external_agents/registry/npx/claude-acp/node_modules/
@agentclientprotocol/claude-agent-acp/`, never executed):

| Fact | Evidence (file:line) |
| --- | --- |
| `package.json` `"version"`: `0.78.0`; `"bin"`: `{"claude-agent-acp": "dist/index.js"}` | `package.json` (sha256 `e23a25d93b0acb17bbf282a682a38081c9dc4ee4aa73984b1bc163724d4934e2`) |
| `dist/index.js` entry (shebang `#!/usr/bin/env node`, imports `acp-agent.js`'s `runAcp`) | `dist/index.js:1-5` (sha256 `9d73d1f0f121fb96cc8badb28c22d5bff02d8582eb2e40360a81c189e1b9422a`) |
| `protocolVersion: 1` | `dist/acp-agent.js:915` |
| `agentCapabilities.loadSession: true`; `sessionCapabilities` (close/fork/list/resume) | `dist/acp-agent.js:905-943` |
| `authMethods`: `"claude"` (terminal login), `"gateway"`, `"gateway-bedrock"` | `dist/acp-agent.js:824,950,1050` |
| `session/new` method registration | `dist/acp-agent.js:7754` |
| `session/prompt` -> `runPromptWithCancellation`; `async prompt(params)` content parsing | `dist/acp-agent.js:7768`, `dist/acp-agent.js:1383` |
| `session/set_config_option` / `unstable_` model application via `query.setModel` | `dist/acp-agent.js:4805-4879` |
| `CLAUDE_CONFIG_DIR` default `~/.claude` | `dist/acp-agent.js:43` |
| `requestPermission`/`readTextFile` (client-side fs/permission calls the agent makes) | `dist/acp-agent.js:559-565, 5124-5170` |
| `usage_update` incl. `total_cost_usd` | `dist/acp-agent.js:2756,3504-3513`; `dist/usage-markdown.js:22,147` |
| `session/new`'s `params._meta.systemPrompt` (string override or `{append, excludeDynamicSections}` object, merged with the SDK's `{"type":"preset","preset":"claude_code"}` default) | `dist/acp-agent.js:5887-5901` |
| `session/new`'s `params._meta.claudeCode.options.disallowedTools` is merged into the SDK query's `disallowedTools` before launch | `dist/acp-agent.js:5910-5918,6065`; bundled SDK 0.3.270 `sdk.d.ts:1477-1482` and `sdk.mjs` forwarding to CLI `--disallowedTools` |
| `session/new`'s `params._meta.claudeCode.options.strictMcpConfig` reaches the SDK query, whose CLI transport passes `--strict-mcp-config` | `dist/acp-agent.js:5910-5918,6021-6036`; bundled SDK 0.3.270 `sdk.d.ts:2139-2145` and `sdk.mjs` forwarding the flag |
| `_session/steering` (a distinct, non-system-prompt continuation/idle-behavior extension — never treated here as an instructions channel) | `dist/acp-agent.js:160-163,955-959,1626-1737` |
| Context compaction lifecycle | `dist/context-compaction.js` (see `.cortex/acp-migration/evidence/T002/static-capability-findings.md:38`) |

Nothing here claims a main-branch/newer-release capability, and nothing
above was exercised live.

## Current runtime gateway policy

All roles receive `strictMcpConfig: true`, empty plugins, disabled hooks,
and explicit native subagent plus gateway-alternative denial through installed
SDK options. Only runtime-brokered MCP tools are supplied for Cortex-governed
code/file/command work. A coordinator receives only an owner-registry
`coordinator_tools` subset of external MCP tools; specialists receive their
explicit grants. A code-index coordinator grant must include `list_projects`,
`trace_path`, `get_code_snippet` and `check_index_coverage` as well as search
tools before its documented index procedure is actually available. The
session prompt now derives that procedure from the effective role grants,
not merely the attached server name; an incomplete grant yields a limited
capability note instead. Index creation remains worker-authorized.

External MCP catalogues are filtered by exact administrator grants, role and engine.
Each permitted call rechecks the current channel, runs in its contained server and
retains an idempotency receipt. Partial frames, revocation and ambiguous outcomes
fail closed. Tool titles do not establish identity or authorization. Actual tool
and display updates are forwarded, including titleless lifecycle updates.

The installed 0.78.0/SDK 0.3.270 option paths are verified locally. That inspection
and scripted protocol tests do not establish real-model or GUI qualification for
this rewrite; see [current status](runtime-rewrite-status.md).

## Pinned install requirements (manual recipe placeholder)

This task implements **no installer** (see T031). An operator who has
already installed claude-agent-acp by some other means (for example
following Zed's own npx-based Custom Agent registration, or a manual `npm
install @agentclientprotocol/claude-agent-acp`) records the resulting
install as an `engine.claude_install.ClaudeAcpInstall`:

```python
from cortex_runtime.engine.claude_install import ClaudeAcpInstall, verify_install

install = ClaudeAcpInstall(
    node_binary="/usr/bin/node",                              # absolute, no symlink component
    adapter_entry="/opt/claude-agent-acp/dist/index.js",       # absolute, no symlink component
    package_json="/opt/claude-agent-acp/package.json",
    package_version="0.78.0",                                  # must equal package.json's own "version"
    package_json_sha256="<sha256 of package.json>",
    adapter_entry_sha256="<sha256 of dist/index.js>",
)
verify_install(install)  # re-hashes/re-checks everything fresh; raises on any drift/untrust
```

Unlike `CodexAcpInstall`, there is no `bundled_codex_binary`-equivalent
field: the pinned release bundles no separate spawned binary (the Claude
Agent SDK is a pure JS/TS library loaded into the same Node process as
`dist/index.js`). The same untrusted-location list codex-acp's own install
uses applies verbatim (`$HOME/.local/share/zed/external_agents`,
`$HOME/.local/share/app.codeg`, `$HOME/.config/app.codeg`) — both engines
are observed installed under the identical Zed registry layout.

`claude_install.launch_command_for(verified_install)` is always exactly
`(node_binary, adapter_entry)`.

## Environment variable mapping

`engine.claude_adapter_acp.build_claude_launch(connection_record, install,
environ)` returns `(launch_command, pinned_engine_vars)`, where
`pinned_engine_vars` is **always the empty dict** — a deliberate difference
from codex-acp's own `{"CODEX_PATH", "NO_BROWSER"}`: this pinned release
bundles no separate binary to pin an env var to. The connection's
`CLAUDE_CONFIG_DIR` is already set generically for every `claude-agent-acp`
connection by `connections.environment.build_process_env`'s
`_ISOLATION_ENV_VAR` mapping (T006) — no per-launch escape hatch is
required or accepted. Discovered connections reference the user's existing
`~/.claude` directory; explicitly created native-account connections can
still use a Cortex-owned directory. `connections.environment.build_process_env`'s
`pinned_engine_vars` parameter remains validated as accepted **only** for
`record.engine == "codex-acp"`; a caller must therefore never pass this
function's `pinned_engine_vars` return value there for a claude-agent-acp
launch (passing `None`, the default, is always correct).

## Configuration-option mapping

`engine.claude_adapter_acp.claude_option_mapping(revision,
observed_config_options)` maps a `ConfigurationRevision` to
`session/set_config_option` pairs, pre-checked against this session's own
observed `configOptions` (see `tests/fixtures/catalog/
claude_agent_acp_config_options.json`'s `_fixture_note` for the exact dist
file:line citations behind each id/category/value below):

| Cortex parameter | claude-agent-acp option id | Category | Observed values |
| --- | --- | --- | --- |
| `route.model` (always required) | `model` | `model` | catalog-discovered per connection (never invented) |
| `thought_level` / `effort` | `effort` | `thought_level` | `default`, `medium` |
| `mode` | `mode` | `mode` | `default`, `acceptEdits`, `plan`, `auto`, `bypassPermissions` |
| *(never mapped)* | `fast` | `model_config` | boolean — same excluded category as codex-acp's own `fast-mode` |

The `effort` option depends on the session's current model. `session/new`
describes the default model, which offers `effort`. Setting the model to
`haiku` removes both `effort` and `fast` from the returned `configOptions`,
and a later `effort` set is rejected (`-32603`). `mode` stays offered.
This was observed on 2026-09-23 against the installed 0.78.0 without sending
any prompt. A worker session therefore sets its model on its own first. It
then picks its effort from what the switched session offers: the requested
effort if offered, otherwise the nearest lower one, otherwise the nearest
higher one. When the session offers no comparable effort, no effort is sent.
Next, it confirms the `default` mode floor on the wire. That step is still
confirm-or-fail: if the switched session no longer offers `mode`, the worker
is refused. See `delegation.live_adapters
.EngineSessionManagerAdapter._apply_worker_mode_floor` and
`tests/test_delegation_worker_mode_floor.py`.

## Evidence status

`engine.claude_adapter_acp.build_claude_evidence_status` returns a
`ClaudeEvidenceStatus` with three distinct fields — `selected`,
`configuration_applied`, `execution_verified` — mirroring
`CodexEvidenceStatus` exactly. `execution_verified` is always
`"unavailable_in_claude_agent_acp_0_78_0"`: nothing in the pinned release's
static surface confirms a specific turn actually executed under a
specific confirmed model/effort/mode (a `usage_update` reports token
counts/cost, never a confirmed model identity).

## Instruction delivery: QUALIFIED via `session/new _meta.systemPrompt` (0.78.0 only)

The installed Claude SDK can defer opening the Cortex MCP stdio relay until
the first `session/prompt`: a no-model-call, real `session/new` diagnostic on
2026-09-20 created a session but observed no relay connection. The broker
therefore records the actual relay's connection and successful `tools/list`,
or its child PID and exit status if it closes first. A premature close during
the prompt cancels that turn and reports `cortex_mcp_connection_failed` with
a bounded reason. A server that already answered `tools/list` and later exits
nonzero on its own is reported the same way as `mcp_server_crashed` (see
[failures and cancellation](failures-and-cancellation.md)). An absent
pre-prompt connection never counts as failure or success. The relay token now has a fixed leading letter because an unprefixed
URL-safe token could begin with `-` and be parsed as an option by the
installed Python `argparse` relay launcher. This explains a reproducible
no-paid startup-close regression; the transient live failure's discarded
token bytes were not retained, so that individual case cannot be proved
byte-for-byte after the fact.

Every Cortex-created session at this pinned version, coordinator or worker,
sends `strictMcpConfig: true`, empty `plugins`,
`managedSettings.disableAllHooks`, and disallows native subagents plus `Bash`,
`Read`, `Edit`, `Write`, `Grep`, `Glob`, `Skill` and `NotebookEdit` through the
Claude-specific SDK `disallowedTools` option. MCP tools are not disallowed.
The installed 0.78.0 and 0.81.2 option-forwarding paths are statically checked,
and fake-wire tests check the emitted payload. The isolated CLI diagnosis
observed a 0.81.2 session *before* this restriction with full MCP schemas and
native Bash/Read/Edit still available. A new owner-requested live canary is
needed to establish effective 0.81.2 enforcement. A session with role instructions
sends the [unified system prompt](unified-system-prompt.md) and the role text as
one plain-string `systemPrompt`, replacing the `claude_code` preset, with
`skills: []` and a `settings` layer that empties commit/PR attribution and turns
off git instructions; a session without role instructions keeps the preset but
still has native skills disabled. `settingSources` is not sent, so the adapter default (`user`,
`project`, `local`) applies and Claude Code loads `CLAUDE.md`/`AGENTS.md`. The process runs in Cortex's engine jail, which bounds
native tools to the assignment scope (see
[containment](permissions-and-containment.md)). A 2026-09-22 no-paid check started
the installed adapter inside the jail and created a session; no model turn has
yet been observed there.

`TaskCreate` and related plan tools are separate. This is a static and
fake-wire-supported prevention mechanism, not a completed real-client
observation; the existing post-event refusal remains a defense if an engine
ever emits a native subagent call despite the restriction. This change is
independent of the observed Codex worker native-subagent attempt.

`engine.instruction_delivery_registry.ClaudeAgentAcp0_78Delivery` is
**QUALIFIED** for exactly `("claude-agent-acp", "0.78.0")`, channel
`session/new _meta.systemPrompt`. This reclassifies an earlier T026 pass
that had kept it UNSUPPORTED (a since-corrected, overly conservative
reading of the same evidence); the change was directed by the session
lead after the dist evidence below was produced and independently
re-verified — this document does not otherwise justify itself.

Evidence chain (every step read directly against the real installed dist,
lead- and agent-verified 2026-09-17):

1. `createSession(params, ...)` reads `params._meta.systemPrompt`
   (`acp-agent.js:5887-5901`): a string fully overrides the SDK's own
   `{"type":"preset","preset":"claude_code"}` default; an object form
   merges `append`/`excludeDynamicSections`, locked to `type: "preset"`.
   Cortex sends the object form with `append`, preserving the engine's
   `claude_code` preset and adding its role instructions.
2. The result is assigned to local `systemPrompt`, placed into
   `options.systemPrompt` (`acp-agent.js:6019`), and that same `options`
   is passed as `query({prompt: input, options})`
   (`acp-agent.js:6179-6182`) — `systemPrompt` and `prompt` (the
   conversation content) are two distinct fields, never merged.
3. The installed `@anthropic-ai/claude-agent-sdk` 0.3.270's own `query()`
   (`sdk.mjs`, function `t0`) destructures `systemPrompt` separately from
   the prompt input and forwards it as its own named field
   (`systemPrompt`/`appendSystemPrompt`/`systemPromptSnapshot`) in the
   spawn-config sent to the Claude Code CLI subprocess's own control
   protocol at query creation — never text concatenated into the
   conversation stream.
4. `resumeSession`/`loadSession` both funnel through
   `getOrCreateSession(params, ...)` (`acp-agent.js:993,1004`), which
   forwards `_meta: params._meta` into the same `createSession()`
   (`acp-agent.js:5776-5782`): a resupplied `_meta.systemPrompt` on
   resume/load is honored exactly as on a fresh `session/new`, not only
   once.

Wiring path (registry → live_adapters → session_manager → wire `_meta`,
a real runtime effect, never a paper-only registry entry):
`ClaudeAgentAcp0_78Delivery.plan()` builds the payload via
`engine.claude_adapter_acp.build_claude_session_new_meta`;
`delegation.live_adapters._session_new_meta_for`/`EngineSessionManagerAdapter.
open()` resolve it per connection and pass it to
`engine.session_manager.EngineSessionManager.create_session`'s additive
`session_new_meta` parameter, which sends it as that `session/new` call's
own `_meta` field on the wire.

**Practical consequence**: a claude-agent-acp coordinator/worker role with
non-negotiable `critical_rules` is admitted at the pinned 0.78.0 version
with no `allow_unqualified_delivery` escape hatch needed at all. codex-acp
1.12.0's own tagged channel is QUALIFIED too as of T035 live evidence (see
`docs/acp/codex-acp-adapter.md`), so it no longer needs that escape hatch
either; the flag remains available for any future engine whose channel is
still UNQUALIFIED. Any *other*/unrecognized claude-agent-acp version remains UNSUPPORTED
(`ClaudeAgentAcpUnrecognizedVersionDelivery`) and is still refused exactly
as before — the evidence chain above was only confirmed against 0.78.0's
own dist. `tests/test_engine_claude_coordinator_live_session.py` and
`tests/test_engine_claude_coordinator_delegation_cycle.py` exercise and
assert this real production path
(`OrchestratingRuntime`/`EngineSessionManagerAdapter`) actually succeeding
end to end (coordinator session negotiate/create with the real
`_meta.systemPrompt` observed on the wire; delegate/worker-report/decide),
and `tests/test_instruction_delivery_qualification.py` (T015, updated by
this same change) asserts the qualification directly, plus that an
unrecognized version/unknown engine still refuses a critical-rules role.

**Still not live-executed**: no real turn has run through this channel. What a live
run would still need to show: (a) the spawned Claude Code CLI actually
honors `systemPrompt` as delivered — a real turn whose observable behavior
demonstrably follows the injected instructions, not merely that the field
was sent; (b) exact-version drift re-check against whatever an operator's
live install actually reports; (c) `native_tool_containment` remains
`unknown` — a `systemPrompt` channel existing is not evidence of any
sandbox around what it instructs.

## Capability matrix vs codex-acp (static only; see `CAPABILITY_MATRIX.json` for the full, machine-readable table)

| Capability | codex-acp 1.12.0 | claude-agent-acp 0.78.0 |
| --- | --- | --- |
| protocol_version | unqualified (observed) | unqualified (observed) |
| session_creation | unqualified (observed) | unqualified (observed) |
| configuration_options | unqualified (observed) | unqualified (observed) |
| model_application_confirmation | unqualified (observed) | unqualified (observed) |
| reasoning_levels | unqualified (observed) | unqualified (observed, effort↔thinking budget) |
| auth_modes | unqualified (documented) | unqualified (observed: terminal/gateway/gateway-bedrock) |
| credential_config_isolation | unknown | unqualified (observed: `CLAUDE_CONFIG_DIR`) |
| runtime_bridge (MCP) | unqualified (http only) | unqualified (http + sse) |
| role_instruction_delivery | UNQUALIFIED (leading prompt block) | **QUALIFIED** at 0.78.0 (`session/new _meta.systemPrompt`, see above); UNSUPPORTED at any other version |
| compaction_observation/request | unqualified (UNSTABLE SDK schema) | unqualified (UNSTABLE SDK schema, gated on unadvertised client capability) |
| usage / cost | unqualified / unknown | unqualified / unqualified (`total_cost_usd` observed) |
| native_subagents_optional | unqualified (AIR `_meta`, non-canonical) | unqualified (AIR `_meta`, non-canonical) |
| summarizer_selection | unsupported (engine-owned, T024) | unsupported (engine-owned, T024) |

## Unavailable native features (honest statement)

- **Native summarizer/compaction selection**: not exposed by either pinned
  adapter's static `configOptions` surface (T024,
  `recovery.summarizer_policy.ADAPTER_SUMMARIZER_SUPPORT`); Cortex relies
  on the deterministic T021 recovery capsule instead.
- **Compaction tier**: `AUTOMATIC` per `delegation.generations` (both
  0.78.0 and 0.77.0 registered explicitly, plus a version-agnostic
  wildcard) — a reactive, never-preemptive tool_call/tool_call_update
  observation, not a controllable request.
- **Role instruction delivery**: QUALIFIED at the pinned 0.78.0 only (see
  above); any other version is UNSUPPORTED — never a silent
  degrade/upgrade across versions.
- **Sandbox/containment claims**: none. `native_tool_containment`,
  `filesystem_execution_location`, and `terminals` are `unknown` in
  `CAPABILITY_MATRIX.json` for claude-agent-acp — a `requestPermission`/
  `fs.readTextFile` method existing in the dist is not evidence of an
  actual sandbox (per `INVARIANTS.md`); no containment test was performed.
