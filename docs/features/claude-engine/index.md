# claude-agent-acp outgoing adapter

> Status: migrated from the former ACP reference. Statements were not re-verified line by line against source in this pass; claims without a source path are unverified until reviewed. The former Python runtime was removed on 2026-11-16: references to its modules and tests were dropped, and statements that cited them describe behavior carried from that implementation that has not been re-verified against the Rust runtime.


## Current support: 0.107.0 (2026-11-16)

<a id="current-support-01070-2026-11-16"></a>

Exact **0.107.0** is registered alongside **0.97.0**, **0.88.0**, **0.87.0**
and **0.78.0**. This installed adapter uses Claude Agent SDK **0.3.323** and
ACP SDK **2.6.0**; Cortex still negotiates **ACP v1** (the real adapter
answered `initialize` with `protocolVersion: 1`, no auth methods and added
`auth.status`, `options`, `status` and `title` capabilities). Later releases
0.107.1 and 0.107.2 exist, differ in several modules from 0.107.0 and are not
registered.

The package was reorganized (`dist/acp-agent.js` became `dist/v1/acp-agent.js`
with a separate `dist/v2/` tree, which Cortex does not enable). The
instruction channel is unchanged in behavior: `sessionSystemPrompt` in
`dist/client-system-prompt.js` uses the spec `systemPrompt` if supplied, else
the `_meta.systemPrompt` string override or preset-append object, else the
Claude Code preset, and the result is the SDK `options.systemPrompt`. The
options spread, disallowed tools, hooks and `_meta.claudeCode.toolName`
for compaction and tool calls are unchanged in meaning. `toolName` on a permission
request is still present only for MCP tools and subagent calls, as on 0.97.0.

In the Rust native task path, Cortex sends strict MCP configuration via
`_meta.claudeCode.options` and supplies separate named relays in
`session/new.mcpServers`. The system prompt identifies
`mcp__cortex__tools_discover` as Cortex-only. For external tools, use the exact
engine-advertised identity and the external server's description and schema.
An unavailable tool or server does not become a Cortex tool.

Behavior changes relevant to Cortex:

- With no `permissions.defaultMode` in the user's settings the session now
  starts in `auto` (when offered) instead of `default`. Cortex's native
  sessions configure `mode` explicitly before any prompt and require the
  confirmed value, so this is not relied on; observed on 0.107.0:
  `session/set_config_option` for `mode`, `effort` and `model` each returned the
  requested `currentValue`. A session that never configures `mode` would start
  in `auto`.
- Blob resources are now converted: text, PNG/JPEG/GIF/WebP images and PDFs are
  decoded, other MIME types are skipped. Cortex sends each retained attachment in
  the form the adapter reads (see
  [Attachment retention](../../project/native-incoming-acp.md#attachment-retention)).
- Authentication status is now reported by the adapter (`auth.status`);
  Cortex continues to use `claude auth status`, which is unchanged.

No-paid observation with the real adapter, an isolated project and the owner's
existing Claude login (`initialize`, `session/new` and set-config calls, no
prompt): the same model list as 0.97.0 (`default`, `opus[1m]`,
`claude-opus-5[1m]`, `claude-fable-5-1[1m]`, `claude-fable-5`,
`claude-sonnet-5`, `haiku`, `claude-opus-4-8`, `claude-opus-4-7`,
`claude-opus-4-6`, `claude-sonnet-4-6`), the same `effort` values (`default`,
`low` to `max`), the same `fast` and `ultracode` on/off options and the same five
modes. Model choices are the account's; the adapter version does not add models.

The adapter's `default` entry is an alias that names no concrete model. Cortex never offers, lists, routes to or sends it: a refreshed catalogue drops it, the bundled preset has no entry for it, and a saved routing file or profile that still names it loads and is reported as no longer offered. Every route is a concrete model. A session that was configured on it before this change shows the engine's own name recorded with the session (for example "Default (Opus 5.5)"), or "Default (engine alias)" when none was recorded.
This is not a paid model turn, tool-enforcement or rendered client evidence.

## Previous support: 0.97.0 (2026-10-27)

<a id="current-support-0970-2026-10-27"></a>

Exact **0.97.0** is registered alongside **0.88.0**, **0.87.0** and **0.78.0**.
This installed adapter uses Claude Agent SDK **0.3.310** and ACP SDK **2.0.2**;
Cortex still negotiates **ACP v1**. Experimental v2 is not enabled.
`dist/acp-agent.js` SHA-256:
`c569a914b2b9c75127193139a69bca3ca4ae6b2ebd6728de0330a16689fdc268`.

Read-only inspection confirmed the existing `_meta.systemPrompt` string
override / preset-append channel at `acp-agent.js:8714-8728`, separate
`options.systemPrompt` at 8846 and `query({prompt: input, options})` at 9057.
Resume/load forward metadata at 1498, 1549 and 8583. The new spec
`systemPrompt` takes precedence only if supplied; Cortex supplies no
conflicting field. The SDK separately forwards system/append instructions
through its initialize control request.

The options spread, explicit disallowed tools, strict MCP, empty plugins and
managed hook/memory restrictions retain their existing channel. Model,
effort and mode remain select options; the extra ultracode option is not
mapped or selected by Cortex. Root cancellation still awaits
`cancelTurns`/`query.interrupt()`; child-session cancellation and background
subagent behavior are not requested or qualified. Non-AIR compaction still
uses `_meta.claudeCode.toolName=compact`, not the display title.

No-paid tests inspect these exact installed sources and initialize the real
adapter with empty isolated account/config directories, without a prompt.
They do not prove model execution, effective native-tool enforcement,
cancellation during real inference or GUI behavior. Workspace discovery prefers its channel-local read-only bundle with Node 24.14.1,
this adapter, SDK 0.3.310 and native parts; no npm/download is required. See
[offline packaging](../../project/packaging.md#offline-workspace-engines) and the
[2026-11-06 owner distribution-risk decision](../../project/bundled-engine-licenses.md#public-distribution-owner-risk-decision-2026-11-06).
Public releases keep native-account login unchanged by owner choice, not recorded
Anthropic permission; the Agent SDK authentication interpretation remains unresolved.
Checkout discovery prefers the newest registered installation and retains all earlier registered versions
for rollback. Incoming ACP startup and approved engine installations now advance
existing connections to newer approved owned installs, or relocate the same version to the verified Workspace bundle, preserving accounts,
routes and model settings. The source path is
`follow_approved_adapters`. There is no per-connection
pin; `CORTEX_ENGINE_FOLLOW=off` disables following for rollback. See
[selection, private backups and rollback](../../configuration.md#approved-adapter-following-and-rollback).
Selection is not a substitute for isolated live qualification.

## Previous support: 0.88.0 (2026-10-08)

<a id="current-support-0880-2026-10-08"></a>

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
refusal.



<a id="current-support-0870-2026-10-07"></a>

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

The public preset enables `haiku` with the same light-only tier (no
`also_standard_for` testing override), high→xhigh→max ladder, maximum,
preferences and request-name policy as `codex-acp/gpt-6-luna`.
`seven_day_haiku` names that exact alias. Only the old exact
`claude-agent-acp/claude-haiku-4-5-20251001` is denied; Cortex deny matching
does not infer families. Existing owner settings are not silently replaced
by installation, so the preset does not establish the policy of an existing
profile. New sessions must use the refreshed install, not the original SDK.

SDK 0.3.293's `sdk.mjs` `fT` still separates system/preset-append
instructions from conversation input, and its initialize control payload
carries `initConfig.systemPrompt`/`appendSystemPrompt`. CLI argument
construction still forwards strict MCP and disallowed tools. The adapter
source and compaction marker are unchanged. These independently inspected
paths extend the exact 0.87.0 registration; they do not qualify unknown SDK
versions or prove native-tool enforcement.

Native Claude login was checked without printing identity or credentials:
logged in, `claude.ai`, first-party provider. No prompt or paid model call
was sent; all owned discovery processes exited. As of 2026-10-07, a new owner-requested live check was still required for turn-level qualification and none had been requested; the 0.88.0 Desktop check in the current section is separate.

### Catalog refresh lifetime and deployed runtimes

A prompt-free Claude refresh observes every model's own effort selector with
one `session/set_config_option` each; on 2026-10-08 eleven models took about
12 s (11.5 s of it in that observation). Paseo reads models from a Cortex ACP
process it stops about a second after `session/new`. While the refresh started
after `session/new` ran on a thread of that process, it was killed every time:
the isolated dev Paseo's Refresh saved Codex and OpenCode catalogs but never a
Claude one. It now runs in its own process session
 and saves its snapshot after the requesting process has exited.

The picker also shows only models the effective routing enables. On the same
date the dev profile's `routing.json` still carried the pre-Haiku policy (bare
`haiku` denied, not enabled), so a catalog that already held Haiku 5.5 still
produced the six-model picker. It was updated to the current preset through
`management.update_routing` after a private backup.

A Cortex runtime deployed before the `<version>+sdk.<sdk>` build-name ordering
in the connection discovery sorts `0.87.0+sdk.0.3.293` below `0.87.0`. Each
start of such a runtime repoints the Claude connection back to the upstream
install, whose SDK 0.3.287 resolves `haiku` to Haiku 4.5 with no effort
choices, even though routing names `haiku` at Luna parity. The owner's
production Paseo runtime was in that state on 2026-10-08; production sessions
get Haiku 5.5 only after the current runtime is deployed there.

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
the pinned version.

## Pinned install requirements (manual recipe placeholder)

Settings > Connections > Download engine installs the manifest-pinned claude-agent-acp into Cortex's owned engine root and needs no install record (see [engine install records](../../configuration.md#engine-install-records)). The record below describes an adapter installed outside Cortex (the former Python API for recording it was removed; the Rust runtime verifies the manifest-pinned or bundled install natively). An operator who has
already installed claude-agent-acp by some other means (for example
following Zed's own npx-based Custom Agent registration, or a manual `npm
install @agentclientprotocol/claude-agent-acp`) would have recorded the resulting
install as a `ClaudeAcpInstall`:

```text
node_binary          absolute, no symlink component
adapter_entry        absolute, no symlink component
package_json         absolute path
package_version      must equal package.json's own "version"
package_json_sha256  sha256 of package.json
adapter_entry_sha256 sha256 of dist/index.js
```

Unlike a Codex install record, there is no `bundled_codex_binary`-equivalent
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
connection by `build_process_env`'s
`_ISOLATION_ENV_VAR` mapping — no per-launch escape hatch is
required or accepted. Discovered connections reference the user's existing
`~/.claude` directory; explicitly created native-account connections can
still use a Cortex-owned directory. `build_process_env`'s
`pinned_engine_vars` parameter remains validated as accepted **only** for
`record.engine == "codex-acp"`; a caller must therefore never pass this
function's `pinned_engine_vars` return value there for a claude-agent-acp
launch (passing `None`, the default, is always correct).

## Configuration-option mapping

Claude Code confirms a selected model with its API and can reject the choice
with "Couldn't confirm model ... with the API. Try again" (observed in the dev
profile on 2026-10-10 and 2026-10-13, each time ending the coordinator's turn
as `orchestration_unavailable`). A rejected option applied nothing, so
`EngineSessionManager.configure` sends the same option again after 1 s and
then 2 s when the engine's error detail asks to try again
(`set_config_option_retry` in the engine log); any other rejection, or a third
one, is raised as before.

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
is refused. 

Fresh ACP sessions initialize Development for brokered writing-worker host
commands. No access mode changes the Claude session mode (`default`) or its
disallowed native tools; Auto only lets the runtime approve a writing worker's
remaining native requests without a card. See [access modes and Docker limits](../permissions-and-containment/index.md#access-mode-development-guarded-and-auto).

## Evidence status

`build_claude_evidence_status` returns a
`ClaudeEvidenceStatus` with three distinct fields — `selected`,
`configuration_applied`, `execution_verified` — mirroring
`CodexEvidenceStatus` exactly. `execution_verified` is always
`"unavailable_in_claude_agent_acp_0_78_0"`: nothing in the pinned release's
static surface confirms a specific turn actually executed under a
specific confirmed model/effort/mode (a `usage_update` reports token
counts/cost, never a confirmed model identity).

## Instruction delivery: QUALIFIED via `session/new _meta.systemPrompt` (0.78.0 basis)

The installed Claude SDK can defer opening the Cortex MCP stdio relay until
the first `session/prompt`: a no-model-call, real `session/new` diagnostic on
2026-09-20 created a session but observed no relay connection. The broker
therefore records the actual relay's connection and successful `tools/list`,
or its child PID and exit status if it closes first. A premature close during
the prompt cancels that turn and reports `cortex_mcp_connection_failed` with
a bounded reason. A server that already answered `tools/list` and later exits
nonzero on its own is reported the same way as `mcp_server_crashed` (see
[failures and cancellation](../failures-and-cancellation/index.md)). An absent
pre-prompt connection never counts as failure or success. The relay token now has a fixed leading letter because an unprefixed
URL-safe token could begin with `-` and be parsed as an option by the
installed Python `argparse` relay launcher. This explains a reproducible
no-paid startup-close regression; the transient live failure's discarded
token bytes were not retained, so that individual case cannot be proved
byte-for-byte after the fact.

Every Claude session Cortex starts (coordinator, every worker profile, reviewers,
sprint planning and short model texts) sends `session/new`
`_meta.claudeCode.options`, which claude-agent-acp 0.107.0 merges into its SDK
options (`userProvidedOptions`; its own `disallowedTools` are appended to ours):
`allowDangerouslySkipPermissions: false`, so `bypassPermissions` is unavailable;
`strictMcpConfig: true`, so only the MCP servers passed in `session/new` (the
`cortex` relay and the owner-registered external servers the runtime attaches)
start; empty `plugins` and `skills`; `disableAllHooks` and
`autoMemoryEnabled: false` in both `settings` and `managedSettings`; deny rules
`Read(./.cortex/**)`, `Read(./.cortex-dev/**)` and the two dev state files in
`settings.permissions.deny`, credential and key file `ask` rules in
`settings.permissions.ask`; and `disallowedTools` with native `Bash`,
`BashOutput`, `KillShell`, `KillBash`, `PowerShell`, `Edit`, `MultiEdit`, `Write`,
`NotebookEdit`, `Skill`, the subagent and scheduling tools (`Agent`, `Task`,
`Workflow`, `TeamCreate`, `TeamDelete`, `SendMessage`, `ScheduleWakeup`,
`Monitor`, `CronCreate`, `CronDelete`, `CronList`, `RemoteTrigger`) and the
lifecycle/outward tools (`EnterWorktree`, `ExitWorktree`, `PushNotification`,
`EnterPlanMode`, `ExitPlanMode`, `TaskStop`, `Artifact`, `SendFeedback`). Native
`Read`, `Grep`, `Glob`, `WebFetch` and `WebSearch` stay, and MCP tools are not
disallowed. The list is in `crates/workspace-service/src/native_engine_policy.rs`;
fake-wire tests (`native_sessions`, `worker_roles`) check the emitted payload. `settingSources` is
not sent, so the adapter default (`user`, `project`, `local`) applies and Claude
Code loads `CLAUDE.md`/`AGENTS.md`; allow rules in those settings still apply.
Role instructions reach the session in its prompt; no `systemPrompt` is sent.

Every session runs in `default` mode, in every access mode and role. Claude
Code runs native reads and searches inside the project without asking, except
that `settings.permissions.ask` rules make it ask before reading a credential or
key file name there (`.env*`, SSH key names, `*.pem`, `*.key`, token and secret
names), and asks for everything else. The runtime answers each request by the
access mode's decision table (see
[native engines](../permissions-and-containment/index.md#native-engines)): Cortex
private state is refused in every mode; Auto allows everything else and records
the rule; Development allows reads outside the project and asks the owner for
credential reads; Guarded asks the owner before reads outside the project,
credential reads and web fetches. Read-only channels get only reads. Claude has no
native write tool in a Cortex session, so the write rows apply to it only if it
ever sends such a request. The Claude process is not sandboxed. Whether a real
turn lists no native `Bash`, `Edit` or `Write`, keeps the attached MCP servers
reachable, asks before a read outside the project and honours the `ask` and deny
rules has not been observed in a live run.

`TaskCreate` and related plan tools are separate. This is a static and
fake-wire-supported prevention mechanism, not a completed real-client
observation; the existing post-event refusal remains a defense if an engine
ever emits a native subagent call despite the restriction. This change is
independent of the observed Codex worker native-subagent attempt.

`ClaudeAgentAcp0_78Delivery` is
**QUALIFIED** for exactly `("claude-agent-acp", "0.78.0")`, channel
`session/new _meta.systemPrompt`. This reclassifies an earlier pass
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
`build_claude_session_new_meta`;
`_session_new_meta_for`/`EngineSessionManagerAdapter.
open()` resolve it per connection and pass it to
`create_session`'s additive
`session_new_meta` parameter, which sends it as that `session/new` call's
own `_meta` field on the wire.

**Practical consequence**: a claude-agent-acp coordinator/worker role with
non-negotiable `critical_rules` is admitted at the pinned 0.78.0 version
with no `allow_unqualified_delivery` escape hatch needed at all. codex-acp
1.12.0's tagged channel is also qualified (see
`docs/features/codex-engine/index.md`), so it no longer needs that escape hatch
either; the flag remains available for any future engine whose channel is
still UNQUALIFIED. Any *other*/unrecognized claude-agent-acp version remains UNSUPPORTED
(`ClaudeAgentAcpUnrecognizedVersionDelivery`) and is still refused exactly
as before — the evidence chain above was only confirmed against 0.78.0's
own dist. The former Python coordinator live-session and delegation-cycle tests exercised and
asserted this real production path actually succeeding
end to end (coordinator session negotiate/create with the real
`_meta.systemPrompt` observed on the wire; delegate/worker-report/decide),
and the former Python qualification test asserted the qualification directly, plus that an
unrecognized version/unknown engine still refuses a critical-rules role.

**Still not live-executed**: no real turn has run through this channel. What a live
run would still need to show: (a) the spawned Claude Code CLI actually
honors `systemPrompt` as delivered — a real turn whose observable behavior
demonstrably follows the injected instructions, not merely that the field
was sent; (b) exact-version drift re-check against whatever an operator's
live install actually reports; (c) `native_tool_containment` remains
`unknown` — a `systemPrompt` channel existing is not evidence of any
sandbox around what it instructs.

## Capability matrix vs codex-acp (static only)

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
| summarizer_selection | unsupported (engine-owned) | unsupported (engine-owned) |

## Unavailable native features (honest statement)

- **Native summarizer/compaction selection**: not exposed by either pinned
  adapter's static `configOptions` surface; Cortex relies
  on the deterministic recovery capsule instead.
- **Compaction tier**: `AUTOMATIC` per the runtime (both
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
